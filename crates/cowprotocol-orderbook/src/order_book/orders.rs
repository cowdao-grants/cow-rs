//! Order wire bodies: `OrderCreation`, the `POST /api/v1/orders` body
//! and its serde wire shape (carrying the owner's signature, the
//! canonical app-data JSON and the same amounts that were hashed for
//! EIP-712 signing), plus [`Order`] and [`OrderStatus`], the
//! `GET /api/v1/orders/{uid}` response model.

use alloy_primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};
use serde_with::{DisplayFromStr, serde_as};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{
    app_data::AppDataHash,
    error::{Error, Result},
    order::{BuyTokenDestination, OrderClass, OrderData, OrderKind, OrderUid, SellTokenSource},
    signature::Signature,
    signing_scheme::SigningScheme,
};

/// Default client-side upper bound for `OrderCreation::valid_to`.
///
/// Mirrors services' default `max-limit-order-validity-period` (1 year).
/// Market orders are usually capped more tightly server-side, but
/// `OrderCreation` does not carry order class, so the SDK uses the broader
/// limit-order default and lets callers configure a shorter local policy.
pub const DEFAULT_ORDER_VALID_TO_MAX_HORIZON_SECS: u64 = 31_536_000;

/// Server-side lifecycle status from `GET /api/v1/orders/{uid}`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OrderStatus {
    /// Awaiting on-chain pre-signature.
    PresignaturePending,
    /// Live; waiting for a solver to settle.
    #[default]
    Open,
    /// Fully matched on-chain.
    Fulfilled,
    /// Off-chain delete or on-chain pre-sign reversal.
    Cancelled,
    /// `validTo` passed before any fill.
    Expired,
}

/// Full order returned by `GET /api/v1/orders/{uid}`. Flattens the
/// 12 [`OrderData`] fields plus server-derived metadata; less-common
/// contextual objects stay as opaque JSON for forward-compat.
#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    /// The 12 signed fields ([`OrderData`]).
    #[serde(flatten)]
    pub data: OrderData,
    /// 56-byte order UID against the chain's settlement domain.
    pub uid: OrderUid,
    /// Owner that signed the order.
    pub owner: Address,
    /// Signing scheme used by the owner.
    pub signing_scheme: SigningScheme,
    /// Raw signature bytes, hex-encoded.
    pub signature: String,
    /// ISO-8601 timestamp the orderbook accepted the order.
    pub creation_date: String,
    /// Current server-side lifecycle status.
    pub status: OrderStatus,
    /// Server-side order classification.
    pub class: OrderClass,
    /// Cumulative buy-side fill, atomic units.
    #[serde_as(as = "DisplayFromStr")]
    pub executed_buy_amount: U256,
    /// Cumulative sell-side fill, atomic units.
    #[serde_as(as = "DisplayFromStr")]
    pub executed_sell_amount: U256,
    /// Executed fee in `executed_fee_token` atomic units.
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    pub executed_fee: Option<U256>,
    /// Token used to charge `executed_fee`.
    #[serde(default)]
    pub executed_fee_token: Option<Address>,
    /// `true` once the order is invalidated (cancelled / replaced).
    #[serde(default)]
    pub invalidated: bool,
    /// `true` if classified as a liquidity order.
    #[serde(default)]
    pub is_liquidity_order: bool,
    /// Full app-data document, when the orderbook stored it.
    #[serde(default)]
    pub full_app_data: Option<String>,
    /// Quote that produced the order, when one was supplied.
    #[serde(default)]
    pub quote: Option<serde_json::Value>,
    /// Pre/post settlement interactions from app-data hooks.
    #[serde(default)]
    pub interactions: Option<serde_json::Value>,
    /// EthFlow metadata for native-sell orders.
    #[serde(default)]
    pub ethflow_data: Option<serde_json::Value>,
    /// On-chain placement metadata for EthFlow orders.
    #[serde(default)]
    pub onchain_order_data: Option<serde_json::Value>,
    /// On-chain user (distinct from `owner` for proxy/relayer flows).
    #[serde(default)]
    pub onchain_user: Option<Address>,
    /// Settlement contract that processed the trade, when known.
    #[serde(default)]
    pub settlement_contract: Option<Address>,
}

/// Body of `POST /api/v1/orders`.
///
/// Differs from a raw [`OrderData`] in three load-bearing ways
/// (`cow-protocol/howto/integrate/api.mdx`):
///
/// - `fee_amount` here is what the user signed (which must be `0`); the
///   protocol fee is taken from surplus at settlement.
/// - `app_data` is the canonical JSON string of the metadata document;
///   `app_data_hash` is the `keccak256` digest of those exact bytes. The
///   signed [`OrderData::app_data`] field equals `app_data_hash`.
/// - `signing_scheme`, `signature` and `from` carry the owner's signature
///   along with the order.
///
/// Use [`Self::new`] to assemble the body once the owner has signed
/// [`crate::OrderQuoteResponse::try_to_order_data`], or [`Self::builder`]
/// when it helps to name each field explicitly (IDE completion, WASM
/// callers, or setting the optional `quote_id` conditionally). Both
/// enforce the same invariants and normalise the receiver identically.
#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", try_from = "OrderCreationWire")]
pub struct OrderCreation {
    /// Token the owner is selling.
    pub sell_token: Address,
    /// Token the owner is buying.
    pub buy_token: Address,
    /// Optional buy-token recipient.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receiver: Option<Address>,
    /// Sell amount in atomic units (must agree with the signed payload).
    #[serde_as(as = "DisplayFromStr")]
    pub sell_amount: U256,
    /// Buy amount in atomic units (must agree with the signed payload).
    #[serde_as(as = "DisplayFromStr")]
    pub buy_amount: U256,
    /// Order expiry in Unix seconds.
    pub valid_to: u32,
    /// Canonical JSON of the app-data document.
    pub app_data: String,
    /// `keccak256(app_data)`. Mirrors the signed payload's `app_data` field.
    pub app_data_hash: AppDataHash,
    /// User-signed fee amount. Must be `"0"` at submission.
    #[serde_as(as = "DisplayFromStr")]
    pub fee_amount: U256,
    /// Direction of the order.
    pub kind: OrderKind,
    /// Whether partial fills are allowed.
    pub partially_fillable: bool,
    /// Source the sell amount is drawn from.
    pub sell_token_balance: SellTokenSource,
    /// Destination the buy amount is paid to.
    pub buy_token_balance: BuyTokenDestination,
    /// Off-chain signing scheme used to authenticate the order.
    pub signing_scheme: SigningScheme,
    /// Signature bytes. Empty for [`SigningScheme::PreSign`].
    #[serde(serialize_with = "serialise_signature_bytes")]
    pub signature: Signature,
    /// Order owner. Required for `presign` / `eip1271`; recommended for
    /// ECDSA schemes so the server can reject malformed signatures early.
    pub from: Address,
    /// Identifier returned by `POST /api/v1/quote`. Optional but improves
    /// solver fee accounting when the order is matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_id: Option<i64>,
}

fn serialise_signature_bytes<S>(
    signature: &Signature,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    Bytes::from(signature.to_bytes()).serialize(serializer)
}

/// Deserialisation helper for [`OrderCreation`].
///
/// The wire format flattens `signature` to a hex string while
/// `signing_scheme` lives in a sibling field. Serde's per-field
/// `deserialize_with` cannot see siblings, so we shape the JSON into
/// this `Wire` form first (with `signature` as raw bytes) and then
/// reassemble the typed [`Signature`] enum in [`TryFrom`] using
/// [`Signature::from_bytes`].
#[serde_as]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrderCreationWire {
    sell_token: Address,
    buy_token: Address,
    #[serde(default)]
    receiver: Option<Address>,
    #[serde_as(as = "DisplayFromStr")]
    sell_amount: U256,
    #[serde_as(as = "DisplayFromStr")]
    buy_amount: U256,
    valid_to: u32,
    app_data: String,
    app_data_hash: AppDataHash,
    #[serde_as(as = "DisplayFromStr")]
    fee_amount: U256,
    kind: OrderKind,
    partially_fillable: bool,
    sell_token_balance: SellTokenSource,
    buy_token_balance: BuyTokenDestination,
    signing_scheme: SigningScheme,
    signature: Bytes,
    from: Address,
    #[serde(default)]
    quote_id: Option<i64>,
}

impl TryFrom<OrderCreationWire> for OrderCreation {
    type Error = crate::error::Error;

    /// Reassemble an [`OrderCreation`] from its wire form, applying the
    /// same invariants [`OrderCreation::new`] enforces on the construction
    /// path: the signature payload must parse for the
    /// declared scheme, `from` must be non-zero, and
    /// `keccak256(app_data) == app_data_hash`. Without the digest check, a
    /// hostile orderbook (or any intermediary) could hand the SDK a body
    /// whose JSON document disagrees with the hash the user signed.
    fn try_from(wire: OrderCreationWire) -> std::result::Result<Self, Self::Error> {
        let signature = Signature::from_bytes(wire.signing_scheme, &wire.signature)?;
        let order_data = OrderData {
            sell_token: wire.sell_token,
            buy_token: wire.buy_token,
            receiver: wire.receiver,
            sell_amount: wire.sell_amount,
            buy_amount: wire.buy_amount,
            valid_to: wire.valid_to,
            app_data: wire.app_data_hash,
            fee_amount: wire.fee_amount,
            kind: wire.kind,
            partially_fillable: wire.partially_fillable,
            sell_token_balance: wire.sell_token_balance,
            buy_token_balance: wire.buy_token_balance,
        };
        Self::new(
            &order_data,
            signature,
            wire.from,
            wire.app_data,
            wire.quote_id,
        )
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

#[cfg(target_arch = "wasm32")]
fn current_epoch_seconds() -> u64 {
    let millis = js_sys::Date::now();
    if millis.is_finite() && millis > 0.0 {
        (millis / 1_000.0) as u64
    } else {
        0
    }
}

#[cfg(test)]
pub(in crate::order_book) fn valid_to_after_for_tests(seconds: u64) -> u32 {
    current_epoch_seconds()
        .saturating_add(seconds)
        .min(u64::from(u32::MAX)) as u32
}

fn validate_valid_to_max_horizon(
    valid_to: u32,
    signing_scheme: SigningScheme,
    max_horizon_secs: u64,
) -> Result<()> {
    if signing_scheme == SigningScheme::PreSign {
        return Ok(());
    }

    let max_valid_to = current_epoch_seconds()
        .saturating_add(max_horizon_secs)
        .min(u64::from(u32::MAX));
    if u64::from(valid_to) > max_valid_to {
        return Err(Error::OrderCreationInvalid {
            field: "valid_to",
            reason: "valid_to exceeds the configured maximum horizon",
        });
    }
    Ok(())
}

impl OrderCreation {
    /// Project the 12 signed fields back out of an [`OrderCreation`] as
    /// the [`OrderData`] the EIP-712 hash and UID were computed against.
    /// Useful for re-hashing the order during owner verification.
    pub const fn order_data(&self) -> OrderData {
        OrderData {
            sell_token: self.sell_token,
            buy_token: self.buy_token,
            receiver: self.receiver,
            sell_amount: self.sell_amount,
            buy_amount: self.buy_amount,
            valid_to: self.valid_to,
            app_data: self.app_data_hash,
            fee_amount: self.fee_amount,
            kind: self.kind,
            partially_fillable: self.partially_fillable,
            sell_token_balance: self.sell_token_balance,
            buy_token_balance: self.buy_token_balance,
        }
    }

    /// Recover the signer of this order from its embedded signature and
    /// assert it matches `self.from`. Returns `self.from` on success.
    ///
    /// - [`SigningScheme::Eip712`] and [`SigningScheme::EthSign`]:
    ///   recovers via ECDSA and compares against `self.from`.
    /// - [`SigningScheme::Eip1271`] and [`SigningScheme::PreSign`]:
    ///   the signature does not carry a recoverable owner; the call
    ///   short-circuits to `Ok(self.from)` because the orderbook (or
    ///   `GPv2Signing.setPreSignature`) will validate the owner
    ///   on-chain. Callers that need to verify the EIP-1271 path
    ///   pre-submission must call the contract's `isValidSignature`
    ///   themselves.
    ///
    /// Recommended belt-and-suspenders call site:
    /// `creation.verify_owner(&settlement_domain(chain.id(), chain.settlement()))?;`
    /// before `OrderBookApi::post_order` to catch signing-key /
    /// `from`-address divergence client-side.
    pub fn verify_owner(
        &self,
        domain: &crate::domain::DomainSeparator,
    ) -> std::result::Result<Address, crate::error::VerifyOwnerError> {
        match self.order_data().recover_signer(domain, &self.signature)? {
            Some(recovered) if recovered.signer == self.from => Ok(self.from),
            Some(recovered) => Err(crate::error::VerifyOwnerError::SignerMismatch {
                declared: self.from,
                recovered: recovered.signer,
            }),
            // EIP-1271 / PreSign: the signature does not carry a
            // recoverable owner, but a synthesised `OrderCreation` (e.g.
            // round-tripped through JSON) could still set `from = ZERO`.
            // Reject that case explicitly so callers do not treat the
            // `Ok` arm as a positive owner assertion. The orderbook (or
            // `GPv2Signing.setPreSignature`) still validates the owner
            // on-chain in the non-zero case.
            None if self.from == Address::ZERO => {
                Err(crate::error::VerifyOwnerError::SignerMismatch {
                    declared: Address::ZERO,
                    recovered: Address::ZERO,
                })
            }
            None => Ok(self.from),
        }
    }

    /// Canonical constructor: assemble a submission body from a signed
    /// [`OrderData`] plus the metadata required by the orderbook (`from`,
    /// signature, app-data document, optional quote id). Use
    /// [`Self::builder`] for the argument-collecting builder equivalent.
    ///
    /// Validates that `from` is non-zero (the orderbook rejects every
    /// scheme with `from = Address::ZERO`, and the contract-signed schemes
    /// `Eip1271` / `PreSign` carry the owner explicitly there) and that
    /// `keccak256(app_data_json)` equals the signed `order_data.app_data`
    /// digest. Callers who want to additionally cross-check that `from`
    /// matches the recovered signer of an ECDSA signature can call
    /// [`Self::verify_owner`] on the assembled body.
    pub fn new(
        order_data: &OrderData,
        signature: Signature,
        from: Address,
        app_data_json: String,
        quote_id: Option<i64>,
    ) -> Result<Self> {
        Self::new_with_valid_to_max_horizon_secs(
            order_data,
            signature,
            from,
            app_data_json,
            quote_id,
            DEFAULT_ORDER_VALID_TO_MAX_HORIZON_SECS,
        )
    }

    /// Same as [`Self::new`], but with a caller-supplied maximum
    /// `valid_to` horizon in seconds.
    ///
    /// Passing `u64::MAX` effectively disables the client-side upper
    /// bound. Pre-sign orders are exempt, matching the services
    /// validation policy for signature-collection workflows.
    pub fn new_with_valid_to_max_horizon_secs(
        order_data: &OrderData,
        signature: Signature,
        from: Address,
        app_data_json: String,
        quote_id: Option<i64>,
        max_horizon_secs: u64,
    ) -> Result<Self> {
        if from == Address::ZERO {
            return Err(Error::OrderCreationInvalid {
                field: "from",
                reason: "owner address must be non-zero",
            });
        }
        let signing_scheme = signature.scheme();
        validate_valid_to_max_horizon(order_data.valid_to, signing_scheme, max_horizon_secs)?;
        // The JSON document MUST hash to the digest the order was signed
        // against. Otherwise a wrapper layer can bind the user's
        // signature to bytes the orderbook never sees, while pinning a
        // different document under the same hash via `put_app_data`.
        let json_digest = alloy_primitives::keccak256(app_data_json.as_bytes());
        if json_digest != order_data.app_data {
            return Err(Error::OrderCreationInvalid {
                field: "app_data",
                reason: "JSON digest does not match signed app_data hash",
            });
        }
        // `Some(Address::ZERO)` and `None` mean the same thing (use owner)
        // but cow-sdk and cow-py emit `None` on the wire. Normalise so the
        // wire payload, signed hash and contract decoding always agree.
        let receiver = match order_data.receiver {
            Some(addr) if addr == Address::ZERO => None,
            other => other,
        };
        Ok(Self {
            sell_token: order_data.sell_token,
            buy_token: order_data.buy_token,
            receiver,
            sell_amount: order_data.sell_amount,
            buy_amount: order_data.buy_amount,
            valid_to: order_data.valid_to,
            app_data: app_data_json,
            app_data_hash: order_data.app_data,
            fee_amount: order_data.fee_amount,
            kind: order_data.kind,
            partially_fillable: order_data.partially_fillable,
            sell_token_balance: order_data.sell_token_balance,
            buy_token_balance: order_data.buy_token_balance,
            signing_scheme,
            signature,
            from,
            quote_id,
        })
    }

    /// Start an [`OrderCreationBuilder`] from the required fields. The
    /// optional `quote_id` is supplied through
    /// [`OrderCreationBuilder::with_quote_id`], and
    /// [`OrderCreationBuilder::build`] runs the same validation as
    /// [`Self::new`].
    pub const fn builder(
        order_data: &OrderData,
        signature: Signature,
        from: Address,
        app_data_json: String,
    ) -> OrderCreationBuilder {
        OrderCreationBuilder::new(order_data, signature, from, app_data_json)
    }

    /// Renamed to [`OrderCreation::new`]; retained as a delegating alias.
    #[deprecated(since = "0.2.0", note = "renamed to OrderCreation::new")]
    pub fn from_signed_order_data(
        order_data: &OrderData,
        signature: Signature,
        from: Address,
        app_data_json: String,
        quote_id: Option<i64>,
    ) -> Result<Self> {
        Self::new(order_data, signature, from, app_data_json, quote_id)
    }
}

/// Argument-collecting builder for [`OrderCreation`], the discoverable
/// counterpart to [`OrderCreation::new`].
///
/// Start it with [`OrderCreation::builder`], which takes the required
/// fields (signed [`OrderData`], `signature`, owner `from`, and the
/// canonical app-data JSON). The optional `quote_id` is set through
/// [`Self::with_quote_id`]. [`Self::build`] validates and assembles the
/// body exactly as [`OrderCreation::new`] does, returning the same
/// [`Result`].
#[derive(Clone, Debug)]
pub struct OrderCreationBuilder {
    order_data: OrderData,
    signature: Signature,
    from: Address,
    app_data_json: String,
    quote_id: Option<i64>,
    valid_to_max_horizon_secs: u64,
}

impl OrderCreationBuilder {
    const fn new(
        order_data: &OrderData,
        signature: Signature,
        from: Address,
        app_data_json: String,
    ) -> Self {
        Self {
            order_data: *order_data,
            signature,
            from,
            app_data_json,
            quote_id: None,
            valid_to_max_horizon_secs: DEFAULT_ORDER_VALID_TO_MAX_HORIZON_SECS,
        }
    }

    /// Set the optional `quote_id` returned by `POST /api/v1/quote`.
    #[must_use]
    pub const fn with_quote_id(mut self, quote_id: i64) -> Self {
        self.quote_id = Some(quote_id);
        self
    }

    /// Override the default client-side `valid_to` maximum horizon.
    ///
    /// The default is [`DEFAULT_ORDER_VALID_TO_MAX_HORIZON_SECS`]. Passing
    /// `u64::MAX` effectively disables the upper-bound check.
    #[must_use]
    pub const fn with_valid_to_max_horizon_secs(mut self, seconds: u64) -> Self {
        self.valid_to_max_horizon_secs = seconds;
        self
    }

    /// Validate and assemble the [`OrderCreation`], delegating to
    /// [`OrderCreation::new`].
    pub fn build(self) -> Result<OrderCreation> {
        OrderCreation::new_with_valid_to_max_horizon_secs(
            &self.order_data,
            self.signature,
            self.from,
            self.app_data_json,
            self.quote_id,
            self.valid_to_max_horizon_secs,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_data::{EMPTY_APP_DATA_HASH, EMPTY_APP_DATA_JSON};
    use crate::domain::{DomainSeparator, settlement_domain};
    use crate::signing_scheme::EcdsaSigningScheme;
    use alloy_primitives::address;
    use alloy_signer_local::PrivateKeySigner;

    const SETTLEMENT: Address = address!("9008D19f58AAbD9eD0D60971565AA8510560ab41");

    /// All-zero EIP-712 placeholder signature for wire-shape tests. Not
    /// recoverable; never pass it to recovery paths.
    fn zero_eip712_signature() -> Signature {
        Signature::Eip712(crate::signature::EcdsaSignature::from_bytes_and_parity(
            &[0u8; 64], false,
        ))
    }

    fn empty_app_data_order() -> OrderData {
        OrderData {
            sell_token: address!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            buy_token: address!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            receiver: None,
            sell_amount: U256::from(1_000_000u64),
            buy_amount: U256::from(999u64),
            valid_to: valid_to_after_for_tests(3_600),
            app_data: EMPTY_APP_DATA_HASH,
            fee_amount: U256::ZERO,
            kind: OrderKind::Sell,
            partially_fillable: false,
            sell_token_balance: SellTokenSource::default(),
            buy_token_balance: BuyTokenDestination::default(),
        }
    }

    fn signer() -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&U256::from(1u64).to_be_bytes().into()).unwrap()
    }

    /// `new` rejects a zero `from` address locally rather than letting
    /// the orderbook reject it.
    #[test]
    fn new_rejects_zero_from_address() {
        let err = OrderCreation::new(
            &OrderData::default(),
            zero_eip712_signature(),
            Address::ZERO,
            EMPTY_APP_DATA_JSON.to_owned(),
            None,
        )
        .unwrap_err();
        assert!(
            matches!(err, Error::OrderCreationInvalid { field: "from", .. }),
            "got: {err}"
        );
    }

    /// R21: `new` rejects an `app_data` JSON document whose keccak256 does
    /// not match the `OrderData::app_data` digest the user signed against.
    #[test]
    fn new_rejects_app_data_digest_mismatch() {
        let err = OrderCreation::new(
            &empty_app_data_order(),
            zero_eip712_signature(),
            address!("70997970C51812dc3A010C7d01b50e0d17dc79C8"),
            // Document does NOT hash to `EMPTY_APP_DATA_HASH`.
            r#"{"version":"1.6.0","metadata":{}}"#.to_owned(),
            None,
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                Error::OrderCreationInvalid {
                    field: "app_data",
                    ..
                }
            ),
            "got: {err}"
        );
    }

    /// R21b: the `TryFrom<OrderCreationWire>` deserialisation path applies
    /// the same digest check. Serialise a valid body, swap the `appData`
    /// document for one whose keccak256 differs while leaving `appDataHash`
    /// untouched, and confirm `serde_json` rejects it before the body can
    /// be relayed downstream.
    #[test]
    fn deserialise_rejects_app_data_digest_mismatch() {
        let creation = OrderCreation::new(
            &empty_app_data_order(),
            zero_eip712_signature(),
            address!("70997970C51812dc3A010C7d01b50e0d17dc79C8"),
            EMPTY_APP_DATA_JSON.to_owned(),
            None,
        )
        .unwrap();
        let mut body = serde_json::to_value(creation).unwrap();
        body["appData"] = serde_json::Value::String(r#"{"version":"1.6.0","metadata":{}}"#.into());
        let err = serde_json::from_value::<OrderCreation>(body).unwrap_err();
        assert!(
            err.to_string().contains("app_data"),
            "expected app_data digest mismatch surfaced through serde, got: {err}"
        );
    }

    /// R22: `verify_owner` rejects a synthesised EIP-1271 / PreSign body
    /// whose `from` is the zero address. The `Ok` arm must never act as a
    /// positive owner assertion for an obviously bogus body.
    #[test]
    fn verify_owner_rejects_zero_from_for_onchain_schemes() {
        // Build the OrderCreation directly, bypassing `new` (which already
        // rejects zero-from), so we reproduce the wire shape an attacker
        // could synthesise.
        let creation = OrderCreation {
            sell_token: Address::ZERO,
            buy_token: Address::ZERO,
            receiver: None,
            sell_amount: U256::ZERO,
            buy_amount: U256::ZERO,
            valid_to: 0,
            app_data: EMPTY_APP_DATA_JSON.to_owned(),
            app_data_hash: EMPTY_APP_DATA_HASH,
            fee_amount: U256::ZERO,
            kind: OrderKind::Sell,
            partially_fillable: false,
            sell_token_balance: SellTokenSource::default(),
            buy_token_balance: BuyTokenDestination::default(),
            signing_scheme: SigningScheme::PreSign,
            signature: Signature::PreSign,
            from: Address::ZERO,
            quote_id: None,
        };
        let err = creation
            .verify_owner(&DomainSeparator::default())
            .unwrap_err();
        assert!(matches!(
            err,
            crate::error::VerifyOwnerError::SignerMismatch { .. }
        ));
    }

    /// R23: `verify_owner` rejects an ECDSA-signed body whose declared
    /// `from` is not the address recovered from the signature. The
    /// typo-and-wallet-switch case the WASM `build_order_creation` shim
    /// relies on to fail fast client-side instead of pushing the bad pair
    /// to the orderbook.
    #[test]
    fn verify_owner_rejects_signer_mismatch_for_ecdsa() {
        let signer = signer();
        let real_signer = signer.address();
        let impostor = address!("dead0000dead0000dead0000dead0000dead0000");
        assert_ne!(real_signer, impostor);

        let domain = settlement_domain(1, SETTLEMENT);
        let order_data = empty_app_data_order();
        let signature = order_data
            .sign(EcdsaSigningScheme::Eip712, &domain, &signer)
            .unwrap();
        // Build the body with the *wrong* declared owner.
        let creation = OrderCreation::new(
            &order_data,
            signature,
            impostor,
            EMPTY_APP_DATA_JSON.to_owned(),
            None,
        )
        .unwrap();
        let err = creation.verify_owner(&domain).unwrap_err();
        match err {
            crate::error::VerifyOwnerError::SignerMismatch {
                declared,
                recovered,
            } => {
                assert_eq!(declared, impostor);
                assert_eq!(recovered, real_signer);
            }
            other => panic!("expected SignerMismatch, got {other:?}"),
        }
    }

    /// `verify_owner` returns the owner when the declared `from` matches the
    /// address recovered from a real ECDSA signature: the success path the
    /// mismatch test guards.
    #[test]
    fn verify_owner_succeeds_for_matching_ecdsa_signer() {
        let signer = signer();
        let owner = signer.address();
        let domain = settlement_domain(1, SETTLEMENT);
        let order_data = empty_app_data_order();
        let signature = order_data
            .sign(EcdsaSigningScheme::Eip712, &domain, &signer)
            .unwrap();
        let creation = OrderCreation::new(
            &order_data,
            signature,
            owner,
            EMPTY_APP_DATA_JSON.to_owned(),
            None,
        )
        .unwrap();
        assert_eq!(creation.verify_owner(&domain).unwrap(), owner);
    }

    /// `verify_owner` surfaces a signature-primitive failure as
    /// [`VerifyOwnerError::Signature`], distinct from the `SignerMismatch`
    /// owner-check semantic. An all-zero ECDSA payload is not recoverable,
    /// so the wrapped `recover_signer` returns a [`SignatureError`] that
    /// must propagate through the `#[from]` arm, including once lifted into
    /// [`Error::VerifyOwner`].
    #[test]
    fn verify_owner_wraps_signature_recovery_failure() {
        // Non-zero `from` so we reach recovery rather than the zero-from
        // guard; an Eip712 scheme so recovery is actually attempted.
        let creation = OrderCreation {
            sell_token: Address::ZERO,
            buy_token: Address::ZERO,
            receiver: None,
            sell_amount: U256::ZERO,
            buy_amount: U256::ZERO,
            valid_to: 0,
            app_data: EMPTY_APP_DATA_JSON.to_owned(),
            app_data_hash: EMPTY_APP_DATA_HASH,
            fee_amount: U256::ZERO,
            kind: OrderKind::Sell,
            partially_fillable: false,
            sell_token_balance: SellTokenSource::default(),
            buy_token_balance: BuyTokenDestination::default(),
            signing_scheme: SigningScheme::Eip712,
            signature: zero_eip712_signature(),
            from: address!("70997970C51812dc3A010C7d01b50e0d17dc79C8"),
            quote_id: None,
        };
        let err = creation
            .verify_owner(&DomainSeparator::default())
            .unwrap_err();
        assert!(
            matches!(err, crate::error::VerifyOwnerError::Signature(_)),
            "got: {err:?}"
        );
        assert!(
            matches!(
                Error::from(err),
                Error::VerifyOwner(crate::error::VerifyOwnerError::Signature(_))
            ),
            "signature failure must survive lifting into Error::VerifyOwner"
        );
    }

    /// The deprecated `from_signed_order_data` alias must still delegate to
    /// [`OrderCreation::new`], producing an identical body.
    #[test]
    #[allow(deprecated)]
    fn from_signed_order_data_delegates_to_new() {
        let owner = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
        let via_new = OrderCreation::new(
            &empty_app_data_order(),
            zero_eip712_signature(),
            owner,
            EMPTY_APP_DATA_JSON.to_owned(),
            Some(42),
        )
        .unwrap();
        let via_alias = OrderCreation::from_signed_order_data(
            &empty_app_data_order(),
            zero_eip712_signature(),
            owner,
            EMPTY_APP_DATA_JSON.to_owned(),
            Some(42),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&via_new).unwrap(),
            serde_json::to_value(&via_alias).unwrap()
        );
    }

    /// [`OrderCreationBuilder`] produces the same body as
    /// [`OrderCreation::new`] with the equivalent `quote_id`.
    #[test]
    fn builder_matches_new() {
        let owner = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
        let via_new = OrderCreation::new(
            &empty_app_data_order(),
            zero_eip712_signature(),
            owner,
            EMPTY_APP_DATA_JSON.to_owned(),
            Some(7),
        )
        .unwrap();
        let via_builder = OrderCreation::builder(
            &empty_app_data_order(),
            zero_eip712_signature(),
            owner,
            EMPTY_APP_DATA_JSON.to_owned(),
        )
        .with_quote_id(7)
        .build()
        .unwrap();
        assert_eq!(
            serde_json::to_value(&via_new).unwrap(),
            serde_json::to_value(&via_builder).unwrap()
        );
    }

    /// [`OrderCreationBuilder::build`] surfaces the same validation error
    /// as [`OrderCreation::new`], locking the one-line delegation against
    /// future divergence.
    #[test]
    fn builder_build_surfaces_new_validation_error() {
        let err = OrderCreation::builder(
            &empty_app_data_order(),
            zero_eip712_signature(),
            Address::ZERO,
            EMPTY_APP_DATA_JSON.to_owned(),
        )
        .build()
        .unwrap_err();
        assert!(
            matches!(err, Error::OrderCreationInvalid { field: "from", .. }),
            "got: {err}"
        );
    }

    /// The builder's `quote_id` is optional; omitting it matches
    /// [`OrderCreation::new`] with `None`.
    #[test]
    fn builder_defaults_quote_id_to_none() {
        let owner = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
        let creation = OrderCreation::builder(
            &empty_app_data_order(),
            zero_eip712_signature(),
            owner,
            EMPTY_APP_DATA_JSON.to_owned(),
        )
        .build()
        .unwrap();
        assert_eq!(creation.quote_id, None);
    }

    #[test]
    fn new_rejects_valid_to_beyond_default_horizon() {
        let mut order = empty_app_data_order();
        order.valid_to = valid_to_after_for_tests(DEFAULT_ORDER_VALID_TO_MAX_HORIZON_SECS + 60);

        let err = OrderCreation::new(
            &order,
            zero_eip712_signature(),
            address!("70997970C51812dc3A010C7d01b50e0d17dc79C8"),
            EMPTY_APP_DATA_JSON.to_owned(),
            None,
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                Error::OrderCreationInvalid {
                    field: "valid_to",
                    ..
                }
            ),
            "got: {err}"
        );
    }

    #[test]
    fn builder_can_override_valid_to_horizon() {
        let mut order = empty_app_data_order();
        order.valid_to = valid_to_after_for_tests(DEFAULT_ORDER_VALID_TO_MAX_HORIZON_SECS + 60);

        let creation = OrderCreation::builder(
            &order,
            zero_eip712_signature(),
            address!("70997970C51812dc3A010C7d01b50e0d17dc79C8"),
            EMPTY_APP_DATA_JSON.to_owned(),
        )
        .with_valid_to_max_horizon_secs(DEFAULT_ORDER_VALID_TO_MAX_HORIZON_SECS + 120)
        .build()
        .unwrap();
        assert_eq!(creation.valid_to, order.valid_to);
    }

    #[test]
    fn presign_orders_are_exempt_from_valid_to_max_horizon() {
        let mut order = empty_app_data_order();
        order.valid_to = u32::MAX;

        let creation = OrderCreation::new(
            &order,
            Signature::PreSign,
            address!("70997970C51812dc3A010C7d01b50e0d17dc79C8"),
            EMPTY_APP_DATA_JSON.to_owned(),
            None,
        )
        .unwrap();
        assert_eq!(creation.valid_to, u32::MAX);
    }
}
