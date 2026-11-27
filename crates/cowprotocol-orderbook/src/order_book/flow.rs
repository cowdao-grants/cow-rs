//! The canonical quote, sign, and submit pipeline.
//!
//! [`OrderBookApi::quote_builder`] starts a type-state
//! [`QuoteRequestBuilder`] bound to the client; [`build`] sends the
//! quote and returns a [`QuotedOrder`] already cross-checked against
//! the request; [`sign`] (or [`sign_with`]) projects, signs, and
//! verifies the owner; [`submit`] pins the app-data document and posts
//! the order. The whole module is transport-generic, so the same
//! pipeline drives the native reqwest client and the wasm fetch
//! transport.
//!
//! ```no_run
//! # #[cfg(feature = "http-client")]
//! # async fn example() -> cowprotocol_orderbook::error::Result<()> {
//! use alloy_primitives::{U256, address};
//! use alloy_signer_local::PrivateKeySigner;
//! use cowprotocol_orderbook::{Chain, OrderBookApi};
//!
//! let wallet = PrivateKeySigner::random();
//! let uid = OrderBookApi::new(Chain::Gnosis)
//!     .quote_builder()
//!     .with_sell_token(address!("e91D153E0b41518A2Ce8Dd3D7944Fa863463a97d")) // WXDAI
//!     .with_buy_token(address!("9C58BAcC331c9aa871AFD802DB6379a98e80CEdb")) // GNO
//!     .with_from(wallet.address())
//!     .with_sell_amount(U256::from(10_000_000_000_000_000_000_u128)) // 10 WXDAI
//!     .build()
//!     .await?
//!     .sign(&wallet)?
//!     .submit()
//!     .await?;
//! println!("https://explorer.cow.fi/gnosis/orders/{uid}");
//! # Ok(()) }
//! ```
//!
//! # Reading the builder's type signature
//!
//! [`QuoteRequestBuilder`] encodes its four required fields in the type
//! system, so a half-configured builder cannot compile a call to
//! [`build`]. It carries one phantom marker per required field:
//! `SellToken`, `BuyToken`, `From`, and `Amount`, each either
//! [`Missing`](super::builder_state::Missing) (the default) or
//! [`Set`](super::builder_state::Set). Every `with_*` setter for a
//! required field flips its marker from `Missing` to `Set` and returns
//! the next state, so an IDE reports intermediate types such as
//! `QuoteRequestBuilder<T, Set, Set, Missing, Missing>`: sell and buy
//! tokens supplied, owner and amount still outstanding.
//!
//! Once all four are supplied the builder reaches the fully-set state,
//! named [`ReadyQuoteRequestBuilder<T>`], the only state in which
//! [`build`] and [`into_request`](QuoteRequestBuilder::into_request)
//! exist. Reach for that alias when writing a signature by hand rather
//! than spelling out `QuoteRequestBuilder<T, Set, Set, Set, Set>`.
//!
//! [`build`]: QuoteRequestBuilder::build
//! [`sign`]: QuotedOrder::sign
//! [`sign_with`]: QuotedOrder::sign_with
//! [`submit`]: OrderSubmission::submit

use core::marker::PhantomData;

use alloy_primitives::{Address, U256, keccak256};
use cowprotocol_signing::SignerSync;

use crate::app_data::{
    APP_DATA_SIZE_LIMIT, AppDataError, AppDataHash, EMPTY_APP_DATA_HASH, EMPTY_APP_DATA_JSON,
};
use crate::chain::Chain;
use crate::error::{Error, Result};
use crate::order::{BuyTokenDestination, OrderKind, OrderUid, SellTokenSource};
use crate::quote_amounts::{DEFAULT_SLIPPAGE_BPS, OrderCosts, ProtocolFeeBps};
use crate::signing_scheme::{EcdsaSigningScheme, SigningScheme};
use crate::transport::HttpTransport;

use super::api::OrderBookApi;
use super::builder_state::{Missing, Set};
use super::orders::OrderCreation;
use super::quote::{OrderQuoteResponse, QuoteRequest};
use super::types::{AppDataDocument, PriceQuality, QuoteAppData};

impl<T: HttpTransport + Clone> OrderBookApi<T> {
    /// Start a type-state quote builder bound to this client. Seeds
    /// [`DEFAULT_SLIPPAGE_BPS`] (50 bps) so the fluent path applies the
    /// recommended slippage protection rather than signing the raw
    /// quote; pass `.with_slippage_bps(0)` to opt out.
    pub fn quote_builder(&self) -> QuoteRequestBuilder<T> {
        QuoteRequestBuilder {
            api: self.clone(),
            parts: QuoteParts::new(OrderCosts {
                slippage_bps: DEFAULT_SLIPPAGE_BPS,
                ..OrderCosts::default()
            }),
            _state: PhantomData,
        }
    }
}

/// The 18 request fields plus the costs the pipeline threads from the
/// builder into [`QuotedOrder::sign`]. One payload struct so the
/// type-state cast is a move, not a per-field copy.
#[derive(Clone, Debug)]
struct QuoteParts {
    sell_token: Option<Address>,
    buy_token: Option<Address>,
    from: Option<Address>,
    receiver: Option<Address>,
    kind: Option<OrderKind>,
    sell_amount_before_fee: Option<U256>,
    sell_amount_after_fee: Option<U256>,
    buy_amount_after_fee: Option<U256>,
    valid_to: Option<u32>,
    valid_for: Option<u32>,
    app_data: Option<QuoteAppData>,
    partially_fillable: Option<bool>,
    sell_token_balance: Option<SellTokenSource>,
    buy_token_balance: Option<BuyTokenDestination>,
    signing_scheme: Option<SigningScheme>,
    verification_gas_limit: Option<u64>,
    onchain_order: Option<bool>,
    price_quality: Option<PriceQuality>,
    costs: OrderCosts,
}

impl QuoteParts {
    const fn new(costs: OrderCosts) -> Self {
        Self {
            sell_token: None,
            buy_token: None,
            from: None,
            receiver: None,
            kind: None,
            sell_amount_before_fee: None,
            sell_amount_after_fee: None,
            buy_amount_after_fee: None,
            valid_to: None,
            valid_for: None,
            app_data: None,
            partially_fillable: None,
            sell_token_balance: None,
            buy_token_balance: None,
            signing_scheme: None,
            verification_gas_limit: None,
            onchain_order: None,
            price_quality: None,
            costs,
        }
    }

    /// Project the parts into the wire [`QuoteRequest`]. Only called
    /// from the fully-`Set` type state, which guarantees the four
    /// required fields are present.
    fn into_request(self) -> QuoteRequest {
        let mut request = match self.kind.expect("amount typestate sets kind") {
            OrderKind::Sell => {
                if let Some(amount) = self.sell_amount_before_fee {
                    QuoteRequest::sell_before_fee(
                        self.sell_token.expect("sell token typestate is set"),
                        self.buy_token.expect("buy token typestate is set"),
                        self.from.expect("from typestate is set"),
                        amount,
                    )
                } else {
                    QuoteRequest::sell_after_fee(
                        self.sell_token.expect("sell token typestate is set"),
                        self.buy_token.expect("buy token typestate is set"),
                        self.from.expect("from typestate is set"),
                        self.sell_amount_after_fee
                            .expect("amount typestate set a sell amount"),
                    )
                }
            }
            OrderKind::Buy => QuoteRequest::buy_after_fee(
                self.sell_token.expect("sell token typestate is set"),
                self.buy_token.expect("buy token typestate is set"),
                self.from.expect("from typestate is set"),
                self.buy_amount_after_fee
                    .expect("amount typestate set the buy amount"),
            ),
        };
        request.receiver = self.receiver;
        request.valid_to = self.valid_to;
        request.valid_for = self.valid_for;
        request.app_data = self.app_data;
        request.partially_fillable = self.partially_fillable;
        request.sell_token_balance = self.sell_token_balance;
        request.buy_token_balance = self.buy_token_balance;
        request.signing_scheme = self.signing_scheme;
        request.verification_gas_limit = self.verification_gas_limit;
        request.onchain_order = self.onchain_order;
        request.price_quality = self.price_quality;
        request
    }
}

/// Type-state quote builder bound to an [`OrderBookApi`]: the entry
/// point of the canonical quote, sign, and submit pipeline.
///
/// [`build`](Self::build) (and [`into_request`](Self::into_request))
/// only exist once `sell_token`, `buy_token`, `from`, and exactly one
/// amount have been set; every optional request field and the cost
/// inputs have a `with_*` setter directly on the builder.
///
/// # Migrating from `TradingClient::post_swap_order`
///
/// The deleted `TradingClient` mapped 1:1 onto this pipeline:
///
/// - `TradingClient::new(chain)` becomes `OrderBookApi::new(chain)`;
/// - `TradingClient::from_orderbook(chain, api)` is no longer needed:
///   pass the chain to [`QuotedOrder::sign_with`], which performs the
///   same mismatch cross-check against the client's chain hint;
/// - `SwapOrder::eip712(request, &app_data)` becomes
///   `api.quote_builder().with_sell_token(..).with_buy_token(..)
///   .with_from(..).with_sell_amount(..).with_app_data(&app_data)`
///   (50 bps slippage is seeded, as `SwapOrder::eip712` did);
/// - `SwapOrder::with_partner_fee_bps` / `with_slippage_bps` and the
///   `protocol_fee_bps_override` field become
///   [`Self::with_partner_fee_bps`], [`Self::with_slippage_bps`], and
///   [`Self::with_protocol_fee_bps_override`];
/// - `post_swap_order(params, &signer)` becomes
///   `.build().await?.sign(&signer)?.submit().await?` (EIP-712; use
///   [`QuotedOrder::sign_with`] for EthSign or an explicit chain);
/// - `PostedSwapOrder`'s fields are reachable along the way: the
///   response via [`QuotedOrder::response`], the signed body via
///   [`OrderSubmission::order`], and the UID from
///   [`OrderSubmission::submit`].
#[derive(Clone, Debug)]
pub struct QuoteRequestBuilder<
    T,
    SellToken = Missing,
    BuyToken = Missing,
    From = Missing,
    Amount = Missing,
> {
    api: OrderBookApi<T>,
    parts: QuoteParts,
    _state: PhantomData<(SellToken, BuyToken, From, Amount)>,
}

/// A [`QuoteRequestBuilder`] with all four required fields
/// ([`Set`]): the state in which [`build`](QuoteRequestBuilder::build)
/// and [`into_request`](QuoteRequestBuilder::into_request) become
/// available. Spell out the phantom markers once here so callers naming
/// a ready builder in a signature or return type do not have to.
pub type ReadyQuoteRequestBuilder<T> = QuoteRequestBuilder<T, Set, Set, Set, Set>;

// Compile-time guard: pins the alias to the state in which the terminal
// methods exist. Calling `into_request` (available only on the buildable
// impl) fails to compile if a future edit desynchronises the alias
// markers from that impl. Never invoked; it exists to be type-checked.
#[allow(dead_code)]
fn ready_builder_is_buildable<T: HttpTransport + Clone>(
    builder: ReadyQuoteRequestBuilder<T>,
) -> QuoteRequest {
    builder.into_request()
}

impl<T, SellToken, BuyToken, From, Amount>
    QuoteRequestBuilder<T, SellToken, BuyToken, From, Amount>
{
    /// Move the payload into the next type state.
    fn cast<S2, B2, F2, A2>(self) -> QuoteRequestBuilder<T, S2, B2, F2, A2> {
        QuoteRequestBuilder {
            api: self.api,
            parts: self.parts,
            _state: PhantomData,
        }
    }

    /// Set the token the owner sells.
    pub fn with_sell_token(
        self,
        sell_token: Address,
    ) -> QuoteRequestBuilder<T, Set, BuyToken, From, Amount> {
        let mut next = self.cast::<Set, BuyToken, From, Amount>();
        next.parts.sell_token = Some(sell_token);
        next
    }

    /// Set the token the owner buys.
    pub fn with_buy_token(
        self,
        buy_token: Address,
    ) -> QuoteRequestBuilder<T, SellToken, Set, From, Amount> {
        let mut next = self.cast::<SellToken, Set, From, Amount>();
        next.parts.buy_token = Some(buy_token);
        next
    }

    /// Set the order owner.
    pub fn with_from(
        self,
        from: Address,
    ) -> QuoteRequestBuilder<T, SellToken, BuyToken, Set, Amount> {
        let mut next = self.cast::<SellToken, BuyToken, Set, Amount>();
        next.parts.from = Some(from);
        next
    }

    /// Set a sell-side quote amount. Aliases
    /// [`Self::with_sell_amount_before_fee`].
    pub fn with_sell_amount(
        self,
        sell_amount: U256,
    ) -> QuoteRequestBuilder<T, SellToken, BuyToken, From, Set> {
        self.with_sell_amount_before_fee(sell_amount)
    }

    /// Set a sell-side quote amount before fee deduction.
    pub fn with_sell_amount_before_fee(
        self,
        sell_amount: U256,
    ) -> QuoteRequestBuilder<T, SellToken, BuyToken, From, Set> {
        let mut next = self.cast::<SellToken, BuyToken, From, Set>();
        next.parts.kind = Some(OrderKind::Sell);
        next.parts.sell_amount_before_fee = Some(sell_amount);
        next.parts.sell_amount_after_fee = None;
        next.parts.buy_amount_after_fee = None;
        next
    }

    /// Set a sell-side quote amount after fee deduction.
    pub fn with_sell_amount_after_fee(
        self,
        sell_amount: U256,
    ) -> QuoteRequestBuilder<T, SellToken, BuyToken, From, Set> {
        let mut next = self.cast::<SellToken, BuyToken, From, Set>();
        next.parts.kind = Some(OrderKind::Sell);
        next.parts.sell_amount_before_fee = None;
        next.parts.sell_amount_after_fee = Some(sell_amount);
        next.parts.buy_amount_after_fee = None;
        next
    }

    /// Set a buy-side quote amount after fee deduction.
    pub fn with_buy_amount_after_fee(
        self,
        buy_amount: U256,
    ) -> QuoteRequestBuilder<T, SellToken, BuyToken, From, Set> {
        let mut next = self.cast::<SellToken, BuyToken, From, Set>();
        next.parts.kind = Some(OrderKind::Buy);
        next.parts.sell_amount_before_fee = None;
        next.parts.sell_amount_after_fee = None;
        next.parts.buy_amount_after_fee = Some(buy_amount);
        next
    }

    /// Set an explicit receiver. Omit it to use the owner.
    pub const fn with_receiver(mut self, receiver: Address) -> Self {
        self.parts.receiver = Some(receiver);
        self
    }

    /// Pin the absolute order expiry returned by the orderbook.
    pub const fn with_valid_to(mut self, valid_to: u32) -> Self {
        self.parts.valid_to = Some(valid_to);
        self.parts.valid_for = None;
        self
    }

    /// Ask the orderbook for a server-relative expiry.
    pub const fn with_valid_for(mut self, valid_for: u32) -> Self {
        self.parts.valid_for = Some(valid_for);
        self.parts.valid_to = None;
        self
    }

    /// Pin app-data by digest, full canonical JSON, or document
    /// (`&AppDataDoc` converts via its canonical JSON).
    pub fn with_app_data(mut self, app_data: impl Into<QuoteAppData>) -> Self {
        self.parts.app_data = Some(app_data.into());
        self
    }

    /// Pin the partial-fill setting.
    pub const fn with_partially_fillable(mut self, partially_fillable: bool) -> Self {
        self.parts.partially_fillable = Some(partially_fillable);
        self
    }

    /// Pin the sell-token source.
    pub const fn with_sell_token_balance(mut self, balance: SellTokenSource) -> Self {
        self.parts.sell_token_balance = Some(balance);
        self
    }

    /// Pin the buy-token destination.
    pub const fn with_buy_token_balance(mut self, balance: BuyTokenDestination) -> Self {
        self.parts.buy_token_balance = Some(balance);
        self
    }

    /// Pin the signing scheme expected in the quote response.
    pub const fn with_signing_scheme(mut self, signing_scheme: SigningScheme) -> Self {
        self.parts.signing_scheme = Some(signing_scheme);
        self
    }

    /// Set the EIP-1271 verification gas limit hint.
    pub const fn with_verification_gas_limit(mut self, gas_limit: u64) -> Self {
        self.parts.verification_gas_limit = Some(gas_limit);
        self
    }

    /// Mark whether the order is placed on chain.
    pub const fn with_onchain_order(mut self, onchain_order: bool) -> Self {
        self.parts.onchain_order = Some(onchain_order);
        self
    }

    /// Set the price-quality hint.
    pub const fn with_price_quality(mut self, price_quality: PriceQuality) -> Self {
        self.parts.price_quality = Some(price_quality);
        self
    }

    /// Slippage tolerance in basis points, applied to the non-fixed side
    /// of the signed order (`buy_amount` for SELL, `sell_amount` for
    /// BUY). Defaults to [`DEFAULT_SLIPPAGE_BPS`]; pass `0` to sign the
    /// raw quote with no slippage.
    pub const fn with_slippage_bps(mut self, bps: u32) -> Self {
        self.parts.costs.slippage_bps = bps;
        self
    }

    /// Partner-fee tier in basis points, charged on the surplus side.
    /// Defaults to `0` (no partner fee).
    pub const fn with_partner_fee_bps(mut self, bps: u32) -> Self {
        self.parts.costs.partner_fee_bps = bps;
        self
    }

    /// Override the `protocolFeeBps` echoed by the quote response.
    /// Defaults to the value the quote reports.
    pub const fn with_protocol_fee_bps_override(mut self, value: ProtocolFeeBps) -> Self {
        self.parts.costs.protocol_fee_bps_override = Some(value);
        self
    }
}

impl<T: HttpTransport + Clone> QuoteRequestBuilder<T, Set, Set, Set, Set> {
    /// Consume the builder and return the wire [`QuoteRequest`] without
    /// sending it, for callers that drive [`OrderBookApi::quote`]
    /// directly. The cost inputs do not travel with the DTO.
    pub fn into_request(self) -> QuoteRequest {
        self.parts.into_request()
    }

    /// `POST /api/v1/quote` and bind the response to the request.
    ///
    /// Fails fast before any network round-trip when `from` is the zero
    /// address (the orderbook rejects every signing scheme with a zero
    /// owner, and the pipeline never infers the owner from the signer)
    /// or when a pinned full app-data document exceeds
    /// [`APP_DATA_SIZE_LIMIT`]. On success the response has already
    /// passed the request-binding cross-checks, so a hostile orderbook
    /// that swaps tokens, owner, kind, or amounts fails here, at quote
    /// time, not at signing time.
    pub async fn build(self) -> Result<QuotedOrder<T>> {
        let Self { api, parts, .. } = self;
        let costs = parts.costs;
        let request = parts.into_request();
        if request.from == Address::ZERO {
            return Err(Error::QuoteRequestInvalid {
                field: "from",
                reason: "must be the order owner, not the zero address; \
                         the pipeline does not infer it from the signer",
            });
        }
        if let Some(QuoteAppData::Full(json)) = request.app_data.as_ref()
            && json.len() > APP_DATA_SIZE_LIMIT
        {
            return Err(Error::AppData(AppDataError::DocumentTooLarge {
                len: json.len(),
                max: APP_DATA_SIZE_LIMIT,
            }));
        }
        let (app_data_hash, app_data_json) = app_data_for_submission(&request);
        let response = api.quote(&request).await?;
        response.check_response_matches_request(&request, app_data_hash)?;
        Ok(QuotedOrder {
            api,
            request,
            response,
            app_data_hash,
            app_data_json,
            costs,
        })
    }
}

/// Quote response plus the request context needed to sign and submit it
/// safely. Born response-bound: [`QuoteRequestBuilder::build`] has
/// already cross-checked the response against the request.
#[derive(Clone, Debug)]
pub struct QuotedOrder<T> {
    api: OrderBookApi<T>,
    request: QuoteRequest,
    response: OrderQuoteResponse,
    app_data_hash: AppDataHash,
    app_data_json: Option<String>,
    costs: OrderCosts,
}

impl<T: HttpTransport + Clone> QuotedOrder<T> {
    /// Request that produced this quote.
    pub const fn request(&self) -> &QuoteRequest {
        &self.request
    }

    /// Raw orderbook quote response.
    pub const fn response(&self) -> &OrderQuoteResponse {
        &self.response
    }

    /// Consume the context and return the raw orderbook quote response.
    pub fn into_response(self) -> OrderQuoteResponse {
        self.response
    }

    /// Sign with EIP-712 under the chain attached to the
    /// [`OrderBookApi`]. The signed amounts apply the builder's
    /// partner-fee, protocol-fee, and slippage composition. The signer
    /// is taken by value; `&wallet` works too, since `SignerSync` is
    /// implemented for references.
    pub fn sign<S: SignerSync>(&self, signer: S) -> Result<OrderSubmission<T>> {
        let chain = self.api.chain().ok_or(Error::OrderCreationInvalid {
            field: "chain",
            reason: "the signing domain can only be inferred when OrderBookApi was \
                     built with a chain; use sign_with",
        })?;
        self.sign_with(chain, EcdsaSigningScheme::Eip712, signer)
    }

    /// Sign with an explicit chain and ECDSA scheme, for clients built
    /// from custom URLs or for EthSign.
    ///
    /// Returns [`Error::ChainMismatch`] when the [`OrderBookApi`]
    /// carries a chain hint that disagrees with `chain`: signing under
    /// one chain and posting to another's orderbook produces an order
    /// the orderbook rejects. The assembled body is owner-verified
    /// ([`OrderCreation::verify_owner`]) before it is returned, so an
    /// [`OrderSubmission`] is only constructible from a body whose
    /// signature recovers to `from`.
    pub fn sign_with<S: SignerSync>(
        &self,
        chain: Chain,
        scheme: EcdsaSigningScheme,
        signer: S,
    ) -> Result<OrderSubmission<T>> {
        if let Some(api_chain) = self.api.chain()
            && api_chain != chain
        {
            return Err(Error::ChainMismatch {
                client: chain,
                api: api_chain,
            });
        }
        let order_data =
            self.response
                .try_to_order_data(&self.request, self.app_data_hash, &self.costs)?;
        let domain = chain.settlement_domain();
        let signature = order_data.sign(scheme, &domain, &signer)?;
        let app_data_json = self
            .app_data_json
            .clone()
            .ok_or(Error::OrderCreationInvalid {
                field: "app_data",
                reason: "full app-data JSON is required to submit a quote pinned \
                         by a non-empty hash",
            })?;
        let order = OrderCreation::new(
            &order_data,
            signature,
            self.response.from,
            app_data_json,
            Some(self.response.id),
        )?;
        order.verify_owner(&domain)?;
        Ok(OrderSubmission {
            api: self.api.clone(),
            order,
        })
    }
}

/// Owner-verified signed order plus the orderbook client that should
/// receive it. Only constructible through [`QuotedOrder::sign`] /
/// [`QuotedOrder::sign_with`], so the body's signature is known to
/// recover to its `from`.
#[derive(Clone, Debug)]
pub struct OrderSubmission<T> {
    api: OrderBookApi<T>,
    order: OrderCreation,
}

impl<T: HttpTransport + Clone> OrderSubmission<T> {
    /// Wire body that will be posted.
    pub const fn order(&self) -> &OrderCreation {
        &self.order
    }

    /// Pin the canonical app-data JSON, then `POST /api/v1/orders`.
    ///
    /// The `PUT /api/v1/app_data/{hash}` runs first so there is no
    /// window where the order index carries the digest but not the
    /// document. The PUT is skipped when the digest is
    /// [`EMPTY_APP_DATA_HASH`]: the orderbook universally knows the
    /// empty document, so pinning it is a wasted round-trip.
    pub async fn submit(&self) -> Result<OrderUid> {
        if self.order.app_data_hash != EMPTY_APP_DATA_HASH {
            self.api
                .put_app_data(
                    &self.order.app_data_hash,
                    &AppDataDocument {
                        full_app_data: self.order.app_data.clone(),
                    },
                )
                .await?;
        }
        self.api.post_order(&self.order).await
    }
}

/// Resolve the digest the quote is bound to and, where the document is
/// known, the canonical JSON [`OrderSubmission::submit`] will pin. A
/// quote pinned by a bare non-empty hash carries no document, so
/// signing such a quote fails until the caller supplies the JSON via
/// [`QuoteRequestBuilder::with_app_data`].
fn app_data_for_submission(request: &QuoteRequest) -> (AppDataHash, Option<String>) {
    match request.app_data.as_ref() {
        Some(QuoteAppData::Hash(hash)) if *hash == EMPTY_APP_DATA_HASH => {
            (*hash, Some(EMPTY_APP_DATA_JSON.to_owned()))
        }
        Some(QuoteAppData::Hash(hash)) => (*hash, None),
        Some(QuoteAppData::Full(json)) => (keccak256(json.as_bytes()), Some(json.clone())),
        None => (EMPTY_APP_DATA_HASH, Some(EMPTY_APP_DATA_JSON.to_owned())),
    }
}
