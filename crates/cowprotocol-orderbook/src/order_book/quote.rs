//! Quote request and response types plus the projection that turns a
//! quote into the signable [`OrderData`].
//!
//! [`QuoteRequest`] is the `POST /api/v1/quote` body; [`OrderQuote`] and
//! [`OrderQuoteResponse`] model the reply. The request-binding guards
//! ([`OrderQuoteResponse::check_response_matches_request`]) are shared by
//! the orderbook client and the quote pipeline, so this module is ungated.

use alloy_primitives::{Address, U256, keccak256};
use serde::{Deserialize, Serialize};
use serde_with::{DisplayFromStr, serde_as};

use crate::app_data::AppDataHash;
use crate::error::{Error, Result};
use crate::order::{BuyTokenDestination, OrderData, OrderKind, SellTokenSource};
use crate::quote_amounts::{OrderCosts, ProtocolFeeBps, QuoteAmountsAndCosts, QuoteAmountsParams};
use crate::signing_scheme::SigningScheme;

use super::types::{PriceQuality, QuoteAppData};

/// `POST /api/v1/quote` request. Exactly one of `sell_amount_before_fee`,
/// `sell_amount_after_fee`, `buy_amount_after_fee` must be `Some`, and
/// must agree with [`Self::kind`]. Those four fields are private so the
/// invariant cannot be broken; build via the constructors below and read
/// the side via [`Self::kind`]. [`Self::validate`] enforces the invariant
/// for deserialised requests.
#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRequest {
    /// Token the owner is selling.
    pub sell_token: Address,
    /// Token the owner is buying.
    pub buy_token: Address,
    /// Order owner.
    pub from: Address,
    /// Defaults to `from` when omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receiver: Option<Address>,
    /// Sell-side vs buy-side fix. Crate-private so it cannot drift out of
    /// step with the set amount through the public API; read via
    /// [`Self::kind`], set via the constructors.
    pub(crate) kind: OrderKind,
    /// Sell amount before fee (sell-side quote). Crate-private so public
    /// callers cannot set more than one amount or one that disagrees with
    /// `kind`; use the constructors.
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sell_amount_before_fee: Option<U256>,
    /// Sell amount after fee (sell-side quote, fee already folded in).
    /// Crate-private; see [`Self::sell_amount_before_fee`].
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sell_amount_after_fee: Option<U256>,
    /// Buy amount after fee (buy-side quote). Crate-private; see
    /// [`Self::sell_amount_before_fee`].
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) buy_amount_after_fee: Option<U256>,
    /// Absolute expiry; orderbook applies a default when absent. This is
    /// the authoritative expiry: when pinned it is bound against the
    /// quote response, so a hostile orderbook cannot lengthen it. Prefer
    /// it over `valid_for` whenever the expiry is security-relevant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<u32>,
    /// Relative expiry (seconds from the *server* clock; wire `validFor`).
    /// Mutually exclusive with `valid_to` (setting both is rejected at the
    /// signing chokepoint). Advisory only: being server-relative it
    /// cannot be bound client-side, so the SDK signs whatever absolute
    /// `validTo` the orderbook derives from it. Use `valid_to` for a
    /// client-enforced expiry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_for: Option<u32>,
    /// Optional pin on the app-data digest or document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_data: Option<QuoteAppData>,
    /// Optional pin on partial-fill semantics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partially_fillable: Option<bool>,
    /// Optional pin on the sell-token source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sell_token_balance: Option<SellTokenSource>,
    /// Optional pin on the buy-token destination.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buy_token_balance: Option<BuyTokenDestination>,
    /// Optional pin on the signing scheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signing_scheme: Option<SigningScheme>,
    /// Gas budget for the on-chain `isValidSignature` callback on
    /// EIP-1271 quotes. Server default: 27_000.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_gas_limit: Option<u64>,
    /// `true` for orders placed on chain (EIP-1271 / PreSign).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onchain_order: Option<bool>,
    /// Price-quality hint. Omitted from the wire when `None`, in which
    /// case the server applies its default ([`PriceQuality::Verified`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_quality: Option<PriceQuality>,
}

impl QuoteRequest {
    /// Sell-side quote with pre-fee input amount. Matches
    /// `cow-sdk`'s `getQuote` default.
    pub const fn sell_before_fee(
        sell_token: Address,
        buy_token: Address,
        from: Address,
        sell_amount: U256,
    ) -> Self {
        Self::new(sell_token, buy_token, from, OrderKind::Sell)
            .with_sell_amount_before_fee(sell_amount)
    }

    /// Sell-side quote with post-fee input amount.
    pub const fn sell_after_fee(
        sell_token: Address,
        buy_token: Address,
        from: Address,
        sell_amount: U256,
    ) -> Self {
        Self::new(sell_token, buy_token, from, OrderKind::Sell)
            .with_sell_amount_after_fee(sell_amount)
    }

    /// Buy-side quote.
    pub const fn buy_after_fee(
        sell_token: Address,
        buy_token: Address,
        from: Address,
        buy_amount: U256,
    ) -> Self {
        Self::new(sell_token, buy_token, from, OrderKind::Buy).with_buy_amount_after_fee(buy_amount)
    }

    const fn new(sell_token: Address, buy_token: Address, from: Address, kind: OrderKind) -> Self {
        Self {
            sell_token,
            buy_token,
            from,
            receiver: None,
            kind,
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
        }
    }

    const fn with_sell_amount_before_fee(mut self, a: U256) -> Self {
        self.sell_amount_before_fee = Some(a);
        self
    }
    const fn with_sell_amount_after_fee(mut self, a: U256) -> Self {
        self.sell_amount_after_fee = Some(a);
        self
    }
    const fn with_buy_amount_after_fee(mut self, a: U256) -> Self {
        self.buy_amount_after_fee = Some(a);
        self
    }

    /// Order side the request was built for. The amount fields are kept
    /// private and in step with this; read it here rather than off the
    /// (now private) field.
    pub const fn kind(&self) -> OrderKind {
        self.kind
    }

    /// Enforce the request-shape invariants the constructors already
    /// uphold, but which a deserialised or mutated request could break:
    /// exactly one amount is set, the set amount agrees with `kind`, and
    /// `valid_to` / `valid_for` are not both set. Called at the top of
    /// [`OrderBookApi::quote`] so an inconsistent request never reaches
    /// the orderbook or a signature.
    ///
    /// [`OrderBookApi::quote`]: super::OrderBookApi::quote
    pub fn validate(&self) -> Result<()> {
        // `valid_to` (absolute) and `valid_for` (server-relative) are
        // mutually exclusive: sending both is ambiguous, and only
        // `valid_to` can be bound client-side.
        if self.valid_to.is_some() && self.valid_for.is_some() {
            return Err(Error::QuoteRequestInvalid {
                field: "validTo/validFor",
                reason: "are mutually exclusive; set at most one",
            });
        }
        let count = u8::from(self.sell_amount_before_fee.is_some())
            + u8::from(self.sell_amount_after_fee.is_some())
            + u8::from(self.buy_amount_after_fee.is_some());
        if count != 1 {
            return Err(Error::QuoteRequestInvalid {
                field: "amount",
                reason: "exactly one of sellAmountBeforeFee, sellAmountAfterFee, \
                         buyAmountAfterFee must be set",
            });
        }
        // The set amount must agree with the side: a sell amount implies a
        // Sell order, a buy amount a Buy order. A disagreement would have
        // the orderbook price the wrong leg.
        let kind_for_amount = if self.buy_amount_after_fee.is_some() {
            OrderKind::Buy
        } else {
            OrderKind::Sell
        };
        if self.kind != kind_for_amount {
            return Err(Error::QuoteRequestInvalid {
                field: "kind",
                reason: "does not agree with the set amount; sell amounts \
                         require Sell, the buy amount requires Buy",
            });
        }
        Ok(())
    }
}

/// Quote response payload. The 12-field signed shape plus the
/// orderbook's expected signing scheme and price metadata. Use
/// [`OrderQuoteResponse::try_to_order_data`] to project into a
/// signable [`OrderData`] after binding the response to the
/// originating [`QuoteRequest`].
///
/// The openapi schema also carries `gasAmount` / `gasPrice` /
/// `sellTokenPrice`; those are not modelled here (cow-sdk drops them
/// too). Serde tolerates the extras, so callers needing the gas
/// breakdown can decode the raw JSON body themselves.
#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderQuote {
    /// Sold token, echoed from the request.
    pub sell_token: Address,
    /// Bought token, echoed from the request.
    pub buy_token: Address,
    /// Receiver, normalised from the request.
    #[serde(default)]
    pub receiver: Option<Address>,
    /// Sell amount the orderbook expects in the signed order.
    #[serde_as(as = "DisplayFromStr")]
    pub sell_amount: U256,
    /// Buy amount the orderbook expects in the signed order.
    #[serde_as(as = "DisplayFromStr")]
    pub buy_amount: U256,
    /// Quoted expiry, Unix seconds.
    pub valid_to: u32,
    /// 32-byte digest of the app-data document.
    pub app_data: AppDataHash,
    /// Orderbook fee in `sell_token` atomic units.
    #[serde_as(as = "DisplayFromStr")]
    pub fee_amount: U256,
    /// Sell-side vs buy-side fix.
    pub kind: OrderKind,
    /// Whether partial fills are allowed.
    pub partially_fillable: bool,
    /// Source the sell token is drawn from.
    #[serde(default)]
    pub sell_token_balance: SellTokenSource,
    /// Destination the buy token is paid to.
    #[serde(default)]
    pub buy_token_balance: BuyTokenDestination,
    /// Signing scheme the orderbook expects.
    pub signing_scheme: SigningScheme,
}

impl OrderQuoteResponse {
    /// Project the response into the [`OrderData`] the owner signs:
    /// THE single quote-to-order projection, routed through the
    /// parity-locked [`crate::quote_amounts::compute`].
    ///
    /// Cross-checks `sellToken`, `buyToken`, normalised `receiver`,
    /// `kind`, `from`, plus any caller-pinned `validTo` /
    /// `partiallyFillable` / balance / scheme / `appData`, against
    /// `request`; mismatches fail closed with
    /// [`Error::QuoteFieldMismatch`]. The signed amounts apply the
    /// partner-fee + protocol-fee + slippage composition in `costs`;
    /// `&OrderCosts::default()` reproduces the raw [step-3][step3]
    /// adjustment, folding `feeAmount` into the signed `sellAmount`
    /// for both SELL and BUY quotes (the orderbook reports BUY network
    /// costs outside `sellAmount`, matching the TS reference).
    /// `fee_amount` ships as `0` (solvers price gas at settlement).
    ///
    /// Fail-closed edge: `compute` rejects a degenerate quote whose
    /// `sellAmount` is zero with [`Error::QuoteSellAmountZero`], since
    /// the network-cost projection into the buy currency is undefined
    /// there. Such quotes never describe a settleable order.
    ///
    /// [step3]: https://docs.cow.fi/cow-protocol/howto/integrate/api#step-3-compute-the-amounts-to-sign
    pub fn try_to_order_data(
        &self,
        request: &QuoteRequest,
        app_data: AppDataHash,
        costs: &OrderCosts,
    ) -> Result<OrderData> {
        self.check_response_matches_request(request, app_data)?;
        let amounts = self.amounts_with_costs(costs)?;
        Ok(self.project_into_order_data(
            amounts.amounts_to_sign.sell_amount,
            amounts.amounts_to_sign.buy_amount,
            app_data,
        ))
    }

    /// Project the response into [`OrderData`] with caller-supplied
    /// amounts and `app_data`; `fee_amount` is always `0` at signing
    /// time. Private because every public path must first run
    /// [`Self::check_response_matches_request`].
    const fn project_into_order_data(
        &self,
        sell_amount: U256,
        buy_amount: U256,
        app_data: AppDataHash,
    ) -> OrderData {
        let q = &self.quote;
        OrderData {
            sell_token: q.sell_token,
            buy_token: q.buy_token,
            receiver: q.receiver,
            sell_amount,
            buy_amount,
            valid_to: q.valid_to,
            app_data,
            fee_amount: U256::ZERO,
            kind: q.kind,
            partially_fillable: q.partially_fillable,
            sell_token_balance: q.sell_token_balance,
            buy_token_balance: q.buy_token_balance,
        }
    }

    /// Project the quote through `getQuoteAmountsAndCosts`-equivalent
    /// arithmetic ([`crate::quote_amounts::compute`]), exposing every
    /// fee-application stage. [`Self::try_to_order_data`] consumes the
    /// `amounts_to_sign` leg; call this directly to inspect the
    /// intermediate stages or the cost breakdown.
    /// `costs.protocol_fee_bps_override` pins the protocol fee; `None`
    /// falls back to [`Self::protocol_fee_bps`].
    pub fn amounts_with_costs(&self, costs: &OrderCosts) -> Result<QuoteAmountsAndCosts> {
        let q = &self.quote;
        crate::quote_amounts::compute(QuoteAmountsParams {
            kind: q.kind,
            sell_amount: q.sell_amount,
            buy_amount: q.buy_amount,
            fee_amount: q.fee_amount,
            partner_fee_bps: costs.partner_fee_bps,
            slippage_bps: costs.slippage_bps,
            protocol_fee_bps: costs.protocol_fee_bps_override.or(self.protocol_fee_bps),
        })
    }

    pub(crate) fn check_response_matches_request(
        &self,
        request: &QuoteRequest,
        app_data: AppDataHash,
    ) -> Result<()> {
        let q = &self.quote;
        ensure_eq("sellToken", request.sell_token, q.sell_token)?;
        ensure_eq("buyToken", request.buy_token, q.buy_token)?;
        // `from` lives on the response envelope, not OrderQuote. The
        // orderbook indexes the order under this address and the SDK
        // computes the UID from it; a mismatch silently swaps the
        // owner the order would settle for.
        ensure_eq("from", request.from, self.from)?;
        // `kind` is the most damaging swap: flipping Sell <-> Buy
        // reinterprets which side of the order is the fixed leg, so a
        // user-confirmed sell amount can come back as a quoted buy.
        ensure_eq("kind", request.kind, q.kind)?;
        // Treat `None`, `Some(ZERO)` and `Some(owner)` as "owner
        // receives" on both sides; the orderbook normalises the same
        // way. Comparing only when `request.receiver` is `Some` would
        // skip validation for the common default-receiver case and let
        // a hostile orderbook redirect proceeds to an attacker.
        let normalise = |owner: Address, receiver: Option<Address>| match receiver {
            Some(addr) if addr == Address::ZERO || addr == owner => None,
            other => other,
        };
        ensure_eq(
            "receiver",
            normalise(request.from, request.receiver),
            normalise(request.from, q.receiver),
        )?;
        // Conditional fields: only enforce when the request pinned
        // them, otherwise the orderbook is free to fill in defaults.
        if let Some(valid_to) = request.valid_to {
            ensure_eq("validTo", valid_to, q.valid_to)?;
        }
        if let Some(partially_fillable) = request.partially_fillable {
            ensure_eq(
                "partiallyFillable",
                partially_fillable,
                q.partially_fillable,
            )?;
        }
        if let Some(src) = request.sell_token_balance {
            ensure_eq("sellTokenBalance", src, q.sell_token_balance)?;
        }
        if let Some(dst) = request.buy_token_balance {
            ensure_eq("buyTokenBalance", dst, q.buy_token_balance)?;
        }
        if let Some(scheme) = request.signing_scheme {
            ensure_eq("signingScheme", scheme, q.signing_scheme)?;
        }
        if let Some(QuoteAppData::Hash(requested_hash)) = request.app_data.as_ref() {
            ensure_eq("appData", *requested_hash, app_data)?;
        }
        // `Full(json)` pins the document, not the digest: the orderbook
        // is expected to hash the bytes verbatim (matching
        // [`AppDataDocument::computed_hash`]) and return that digest on
        // the response. A mismatch means the server is signing the
        // caller against a different `app_data` than they handed in, so
        // refuse before the signature commits.
        //
        // [`AppDataDocument::computed_hash`]: super::AppDataDocument::computed_hash
        if let Some(QuoteAppData::Full(json)) = request.app_data.as_ref() {
            ensure_eq("appData", keccak256(json.as_bytes()), app_data)?;
        }
        // Bind the fixed leg the caller specified. Without this a hostile
        // orderbook can echo the right token pair and `kind` but inflate
        // the fixed amount, so the caller signs an order moving more (or
        // accepting less) than they requested. The variable leg (buy for
        // SELL, sell for BUY) is the quote itself and is deliberately not
        // bound: it is what the orderbook is being asked to price, and
        // slippage / fee composition adjust it downstream.
        if let Some(requested) = request.sell_amount_before_fee {
            // The signed `sellAmount` folds the fee back in, so the
            // pre-fee request equals `sellAmount + feeAmount`. The fold
            // shares `compute`'s fail-closed contract: overflow is
            // refused, never saturated.
            let signed_sell =
                q.sell_amount
                    .checked_add(q.fee_amount)
                    .ok_or(Error::QuoteFeeMathOverflow {
                        stage: "request_binding.sell_before_fee",
                    })?;
            ensure_eq("sellAmountBeforeFee", requested, signed_sell)?;
        }
        if let Some(requested) = request.sell_amount_after_fee {
            ensure_eq("sellAmountAfterFee", requested, q.sell_amount)?;
        }
        if let Some(requested) = request.buy_amount_after_fee {
            ensure_eq("buyAmountAfterFee", requested, q.buy_amount)?;
        }
        Ok(())
    }
}

/// Fail closed with a uniform [`Error::QuoteFieldMismatch`] when a
/// response field does not equal the value the request pinned. `field` is
/// the camelCase wire name; `requested` and `returned` are rendered with
/// `Debug` so addresses, amounts, enums and `Option`s all format
/// consistently in the error.
pub(crate) fn ensure_eq<T>(field: &'static str, requested: T, returned: T) -> Result<()>
where
    T: core::fmt::Debug + PartialEq,
{
    if requested == returned {
        return Ok(());
    }
    Err(Error::QuoteFieldMismatch {
        field,
        requested: format!("{requested:?}"),
        returned: format!("{returned:?}"),
    })
}

/// `POST /api/v1/quote` response body.
#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderQuoteResponse {
    /// Quoted [`OrderQuote`]; project via [`Self::try_to_order_data`].
    pub quote: OrderQuote,
    /// Order owner. Must equal `request.from`: this is the address the
    /// orderbook indexes the order under and the address the SDK packs
    /// into the UID, so a mismatch silently binds the signature to a
    /// different owner than intended. [`Self::try_to_order_data`]
    /// validates this for you; a hand-built [`OrderData`] that skips that
    /// path is the caller's responsibility to check.
    pub from: Address,
    /// ISO-8601 expiry of the quote, as the orderbook reports it.
    /// Informational only: the authoritative expiry the SDK signs into
    /// the EIP-712 hash is [`OrderQuote::valid_to`], not this string.
    /// Callers that need a typed timestamp for display should parse
    /// this field in their own layer; cow-rs deliberately does not pull
    /// in a date-time dependency for it.
    pub expiration: String,
    /// Server-assigned quote id; pass back when posting so the
    /// orderbook can reconcile fee/price.
    pub id: i64,
    /// `true` if the orderbook simulated against on-chain balances.
    pub verified: bool,
    /// Protocol fee, parsed from the wire's decimal string at decode
    /// time so a malformed value fails the whole response (fail-closed,
    /// and earlier than the signing path would catch it).
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    pub protocol_fee_bps: Option<ProtocolFeeBps>,
}
