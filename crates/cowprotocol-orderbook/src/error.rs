//! Error and `Result` types for the `cow-rs` crate.

use alloy_primitives::Address;
use serde::{Deserialize, Serialize};
use std::fmt;

#[cfg(feature = "subgraph")]
use crate::subgraph::SubgraphError;
use crate::{chain::UnsupportedChain, signature::SignatureError};

/// Crate-wide `Result` alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Top-level error type for `cow-rs`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// HTTP transport, redirect, body or response error. Only present
    /// when the `http-client` feature is enabled on a non-wasm32 target
    /// (wasm32 builds carry no reqwest at all and report transport
    /// failures via [`Self::TransportFailed`]).
    #[cfg(all(feature = "http-client", not(target_arch = "wasm32")))]
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),

    /// A transport-level failure from a non-reqwest [`HttpTransport`]
    /// backend (e.g. the wasm32 `FetchTransport`): a failed or
    /// aborted/timed-out request, or a body read error. The message is the
    /// transport's own description.
    ///
    /// [`HttpTransport`]: crate::transport::HttpTransport
    #[error("transport error: {0}")]
    TransportFailed(String),

    /// JSON serialisation or deserialisation error.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    /// URL build error.
    #[error("url error: {0}")]
    Url(#[from] url::ParseError),

    /// Chain id not supported by [`crate::Chain`].
    #[error(transparent)]
    UnsupportedChain(#[from] UnsupportedChain),

    /// The CoW orderbook responded with a structured error envelope.
    #[error("orderbook error ({}{}): {}",
        api.error_type,
        api.data.as_ref().map(|_| ", +data").unwrap_or(""),
        api.description,
    )]
    OrderbookApi {
        /// HTTP status returned with the error.
        status: u16,
        /// Decoded `ApiError` body.
        api: ApiError,
    },

    /// The orderbook returned a non-2xx status with an unparseable body.
    #[error("unexpected orderbook status {status}: {body}")]
    UnexpectedStatus {
        /// HTTP status.
        status: u16,
        /// Raw body verbatim.
        body: String,
    },

    /// The CoW Protocol subgraph returned GraphQL errors or an
    /// unrecognised envelope.
    #[cfg(feature = "subgraph")]
    #[error(transparent)]
    Subgraph(#[from] SubgraphError),

    /// Signature parsing, signing, or recovery failed.
    #[error(transparent)]
    Signature(#[from] SignatureError),

    /// Owner verification failed: either the embedded signature could
    /// not be recovered, or the recovered signer did not match the
    /// declared owner. Raised by [`crate::OrderCreation::verify_owner`].
    #[error(transparent)]
    VerifyOwner(#[from] VerifyOwnerError),

    /// An app-data document failed to hash or serialise (e.g. it
    /// exceeded [`crate::app_data::APP_DATA_SIZE_LIMIT`]). Surfaced
    /// instead of panicking when the document comes from a caller.
    #[error(transparent)]
    AppData(#[from] crate::app_data::AppDataError),

    /// An [`crate::OrderCreation`] field did not satisfy the orderbook's
    /// preconditions; surfaced locally so the body is never shipped.
    #[error("invalid OrderCreation: {field} {reason}")]
    OrderCreationInvalid {
        /// Field that failed validation.
        field: &'static str,
        /// Why it failed.
        reason: &'static str,
    },

    /// The caller's [`crate::QuoteRequest`] was internally inconsistent
    /// (e.g. both `valid_to` and `valid_for` set). Surfaced at the
    /// signing chokepoint so an ambiguous request is never projected
    /// into a signed `OrderData`.
    #[error("invalid QuoteRequest: {field} {reason}")]
    QuoteRequestInvalid {
        /// Field that failed validation.
        field: &'static str,
        /// Why it failed.
        reason: &'static str,
    },

    /// The chain passed to `QuotedOrder::sign_with` disagrees with the
    /// chain its [`crate::OrderBookApi`] targets. Signing under one
    /// chain and posting to another's orderbook produces an order the
    /// orderbook rejects, so it is refused before signing.
    #[error("chain mismatch: signing for {client} but the OrderBookApi targets {api}")]
    ChainMismatch {
        /// Chain the caller asked to sign for.
        client: crate::chain::Chain,
        /// Chain the [`crate::OrderBookApi`] targets.
        api: crate::chain::Chain,
    },

    /// A field on the orderbook's quote response did not match the
    /// caller's [`crate::QuoteRequest`]. Raised by
    /// [`crate::OrderQuoteResponse::try_to_order_data`] before any
    /// `OrderData` is returned, so a hostile orderbook cannot trick the
    /// caller into signing an order with a swapped buy token, recipient,
    /// or app-data digest.
    #[error("quote field {field} mismatch: requested {requested}, returned {returned}")]
    QuoteFieldMismatch {
        /// Which field of the response disagreed with the request.
        field: &'static str,
        /// What the caller asked for, formatted via `Display`.
        requested: String,
        /// What the orderbook returned, formatted via `Display`.
        returned: String,
    },

    /// An HTTP response body exceeded the configured cap before being
    /// fully read. Defends against a hostile orderbook streaming a
    /// multi-GB body to exhaust the SDK's memory.
    #[error("orderbook response exceeded {max} byte cap")]
    ResponseTooLarge {
        /// Maximum byte length the SDK accepts for this endpoint.
        max: usize,
    },

    /// `protocol_fee_bps` could not be parsed as a non-negative decimal
    /// with at most 5 fractional digits. The orderbook serialises the
    /// field as a JSON string (e.g. `"0.3"`); this variant fires when
    /// the value is malformed or carries more precision than the
    /// internal `bps * 100_000` scale can represent.
    #[error("invalid protocol_fee_bps {value:?}: {reason}")]
    InvalidProtocolFeeBps {
        /// The string the caller (or orderbook) passed in.
        value: String,
        /// Why parsing failed.
        reason: &'static str,
    },

    /// A `quote.sellAmount` of zero made [`crate::quote_amounts::compute`]
    /// unable to project network costs into the buy currency. This is a
    /// degenerate quote (no input to sell) and never appears for orders
    /// the orderbook would settle; we refuse it explicitly so the fee
    /// math cannot divide by zero downstream.
    #[error("quote sellAmount is zero, network cost projection undefined")]
    QuoteSellAmountZero,

    /// A [`crate::quote_amounts::compute`] intermediate (or the
    /// request-binding `sellAmount + feeAmount` fold) overflowed or
    /// underflowed before reaching the signed [`crate::OrderData`].
    /// Fail-closed: a hostile or malformed
    /// orderbook response that would push `sellAmount`, `buyAmount`,
    /// `feeAmount`, or `protocolFeeBps` into a U256-saturating
    /// computation is rejected before any saturated bytes are folded
    /// into a signature. `stage` labels the leg that failed (e.g.
    /// `"before_all_fees.buy"`, `"protocol_fee.mul_div"`,
    /// `"after_slippage.sell"`) so the offending input is greppable.
    #[error("quote fee math overflow at {stage}")]
    QuoteFeeMathOverflow {
        /// Name of the projection leg whose checked arithmetic failed.
        stage: &'static str,
    },

    /// The keccak256 of an [`crate::AppDataDocument`]'s `fullAppData`
    /// bytes did not match the [`crate::AppDataHash`] it was paired with.
    /// Raised by [`crate::OrderBookApi::app_data`] when the orderbook
    /// serves a document that does not hash to the requested digest, and
    /// by [`crate::OrderBookApi::put_app_data`] before issuing a pin
    /// whose payload would be rejected server-side. The signed order
    /// commits only to the digest, so a divergent body would let
    /// downstream wallets, bots, or UIs display or validate metadata
    /// different from what the order actually commits to.
    #[error("app-data hash mismatch: expected {expected}, computed {computed}")]
    AppDataHashMismatch {
        /// The hash the caller asked for or paired with the document.
        expected: String,
        /// `keccak256(document.fullAppData.as_bytes())` as actually
        /// computed.
        computed: String,
    },
}

/// Errors from [`crate::OrderCreation::verify_owner`]: the signer
/// recovery step this crate delegates to `cowprotocol-signing`, plus the
/// order-verification `SignerMismatch` semantic that no signing primitive
/// produces. It lives here rather than in [`SignatureError`] because the
/// mismatch is a property of the owner check, not of signature parsing,
/// so a signing-only consumer's match over [`SignatureError`] stays fully
/// reachable.
#[derive(Debug, thiserror::Error)]
pub enum VerifyOwnerError {
    /// Recovering the signer from the order's ECDSA signature failed.
    #[error(transparent)]
    Signature(#[from] SignatureError),
    /// The signer recovered from the signature did not match the owner
    /// declared in the order's `from` field.
    #[error("signer mismatch: declared {declared}, recovered {recovered}")]
    SignerMismatch {
        /// Owner the order claims to be signed by.
        declared: Address,
        /// Owner recovered from the signature bytes.
        recovered: Address,
    },
}

/// Typed view of the orderbook's machine-readable `errorType` strings.
///
/// The variants mirror the bundled orderbook OpenAPI plus a few values the
/// live `cowprotocol/services` code returns but the spec has not carried
/// consistently (`Forbidden`, `NonZeroFee`, `InsufficientFee`, and
/// `DuplicateOrder`). Unknown values are preserved verbatim so callers can
/// log, metric, or apply their own policy without losing information when the
/// server grows a new error type.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum OrderbookApiErrorType {
    /// `DuplicatedOrder`
    DuplicatedOrder,
    /// Historical singular spelling seen in downstream tests.
    DuplicateOrder,
    /// `QuoteNotFound`
    QuoteNotFound,
    /// `QuoteNotVerified`
    QuoteNotVerified,
    /// `InvalidQuote`
    InvalidQuote,
    /// `MissingFrom`
    MissingFrom,
    /// `WrongOwner`
    WrongOwner,
    /// `InvalidEip1271Signature`
    InvalidEip1271Signature,
    /// `InsufficientBalance`
    InsufficientBalance,
    /// `InsufficientAllowance`
    InsufficientAllowance,
    /// `InvalidSignature`
    InvalidSignature,
    /// `SellAmountOverflow`
    SellAmountOverflow,
    /// `TransferSimulationFailed`
    TransferSimulationFailed,
    /// `ZeroAmount`
    ZeroAmount,
    /// `IncompatibleSigningScheme`
    IncompatibleSigningScheme,
    /// `TooManyLimitOrders`
    TooManyLimitOrders,
    /// `TooMuchGas`
    TooMuchGas,
    /// `UnsupportedBuyTokenDestination`
    UnsupportedBuyTokenDestination,
    /// `UnsupportedSellTokenSource`
    UnsupportedSellTokenSource,
    /// `UnsupportedOrderType`
    UnsupportedOrderType,
    /// `InsufficientValidTo`
    InsufficientValidTo,
    /// `ExcessiveValidTo`
    ExcessiveValidTo,
    /// `InvalidNativeSellToken`
    InvalidNativeSellToken,
    /// `SameBuyAndSellToken`
    SameBuyAndSellToken,
    /// `UnsupportedToken`
    UnsupportedToken,
    /// `InvalidAppData`
    InvalidAppData,
    /// `AppDataHashMismatch`
    AppDataHashMismatch,
    /// `AppDataMismatch`
    AppDataMismatch,
    /// `AppdataFromMismatch`
    AppdataFromMismatch,
    /// `MetadataSerializationFailed`
    MetadataSerializationFailed,
    /// `OldOrderActivelyBidOn`
    OldOrderActivelyBidOn,
    /// `Forbidden`
    Forbidden,
    /// `NonZeroFee`
    NonZeroFee,
    /// `InsufficientFee`
    InsufficientFee,
    /// `NoLiquidity`
    NoLiquidity,
    /// Unknown server-side `errorType` value.
    Unknown(String),
}

impl OrderbookApiErrorType {
    /// Borrow the exact wire spelling for known variants.
    pub fn as_str(&self) -> &str {
        match self {
            Self::DuplicatedOrder => "DuplicatedOrder",
            Self::DuplicateOrder => "DuplicateOrder",
            Self::QuoteNotFound => "QuoteNotFound",
            Self::QuoteNotVerified => "QuoteNotVerified",
            Self::InvalidQuote => "InvalidQuote",
            Self::MissingFrom => "MissingFrom",
            Self::WrongOwner => "WrongOwner",
            Self::InvalidEip1271Signature => "InvalidEip1271Signature",
            Self::InsufficientBalance => "InsufficientBalance",
            Self::InsufficientAllowance => "InsufficientAllowance",
            Self::InvalidSignature => "InvalidSignature",
            Self::SellAmountOverflow => "SellAmountOverflow",
            Self::TransferSimulationFailed => "TransferSimulationFailed",
            Self::ZeroAmount => "ZeroAmount",
            Self::IncompatibleSigningScheme => "IncompatibleSigningScheme",
            Self::TooManyLimitOrders => "TooManyLimitOrders",
            Self::TooMuchGas => "TooMuchGas",
            Self::UnsupportedBuyTokenDestination => "UnsupportedBuyTokenDestination",
            Self::UnsupportedSellTokenSource => "UnsupportedSellTokenSource",
            Self::UnsupportedOrderType => "UnsupportedOrderType",
            Self::InsufficientValidTo => "InsufficientValidTo",
            Self::ExcessiveValidTo => "ExcessiveValidTo",
            Self::InvalidNativeSellToken => "InvalidNativeSellToken",
            Self::SameBuyAndSellToken => "SameBuyAndSellToken",
            Self::UnsupportedToken => "UnsupportedToken",
            Self::InvalidAppData => "InvalidAppData",
            Self::AppDataHashMismatch => "AppDataHashMismatch",
            Self::AppDataMismatch => "AppDataMismatch",
            Self::AppdataFromMismatch => "AppdataFromMismatch",
            Self::MetadataSerializationFailed => "MetadataSerializationFailed",
            Self::OldOrderActivelyBidOn => "OldOrderActivelyBidOn",
            Self::Forbidden => "Forbidden",
            Self::NonZeroFee => "NonZeroFee",
            Self::InsufficientFee => "InsufficientFee",
            Self::NoLiquidity => "NoLiquidity",
            Self::Unknown(s) => s,
        }
    }
}

impl fmt::Display for OrderbookApiErrorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for OrderbookApiErrorType {
    fn from(value: &str) -> Self {
        match value {
            "DuplicatedOrder" => Self::DuplicatedOrder,
            "DuplicateOrder" => Self::DuplicateOrder,
            "QuoteNotFound" => Self::QuoteNotFound,
            "QuoteNotVerified" => Self::QuoteNotVerified,
            "InvalidQuote" => Self::InvalidQuote,
            "MissingFrom" => Self::MissingFrom,
            "WrongOwner" => Self::WrongOwner,
            "InvalidEip1271Signature" => Self::InvalidEip1271Signature,
            "InsufficientBalance" => Self::InsufficientBalance,
            "InsufficientAllowance" => Self::InsufficientAllowance,
            "InvalidSignature" => Self::InvalidSignature,
            "SellAmountOverflow" => Self::SellAmountOverflow,
            "TransferSimulationFailed" => Self::TransferSimulationFailed,
            "ZeroAmount" => Self::ZeroAmount,
            "IncompatibleSigningScheme" => Self::IncompatibleSigningScheme,
            "TooManyLimitOrders" => Self::TooManyLimitOrders,
            "TooMuchGas" => Self::TooMuchGas,
            "UnsupportedBuyTokenDestination" => Self::UnsupportedBuyTokenDestination,
            "UnsupportedSellTokenSource" => Self::UnsupportedSellTokenSource,
            "UnsupportedOrderType" => Self::UnsupportedOrderType,
            "InsufficientValidTo" => Self::InsufficientValidTo,
            "ExcessiveValidTo" => Self::ExcessiveValidTo,
            "InvalidNativeSellToken" => Self::InvalidNativeSellToken,
            "SameBuyAndSellToken" => Self::SameBuyAndSellToken,
            "UnsupportedToken" => Self::UnsupportedToken,
            "InvalidAppData" => Self::InvalidAppData,
            "AppDataHashMismatch" => Self::AppDataHashMismatch,
            "AppDataMismatch" => Self::AppDataMismatch,
            "AppdataFromMismatch" => Self::AppdataFromMismatch,
            "MetadataSerializationFailed" => Self::MetadataSerializationFailed,
            "OldOrderActivelyBidOn" => Self::OldOrderActivelyBidOn,
            "Forbidden" => Self::Forbidden,
            "NonZeroFee" => Self::NonZeroFee,
            "InsufficientFee" => Self::InsufficientFee,
            "NoLiquidity" => Self::NoLiquidity,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

impl From<String> for OrderbookApiErrorType {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

/// Caller policy hint for a structured orderbook API error.
///
/// This is intentionally a hint, not middleware: callers still own their
/// retry loops and idempotency writes, but no longer need to string-match
/// `ApiError.error_type` to decide the broad outcome.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum RetryHint {
    /// Try again on the next block/tick.
    Retry,
    /// Delay before trying again.
    Backoff {
        /// Suggested backoff duration in seconds.
        seconds: u64,
    },
    /// Treat the order as terminally rejected.
    Drop,
    /// The orderbook already has the order; record it as submitted.
    AlreadySubmitted,
}

// Mirrors cowprotocol/watch-tower's API_ERRORS_BACKOFF table.
const WATCH_TOWER_APP_DATA_BACKOFF_SECS: u64 = 60;
const WATCH_TOWER_BALANCE_ALLOWANCE_BACKOFF_SECS: u64 = 10 * 60;
const WATCH_TOWER_LIMIT_ORDER_BACKOFF_SECS: u64 = 60 * 60;

impl RetryHint {
    /// `true` when retrying the same payload later may succeed.
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::Retry | Self::Backoff { .. })
    }
}

/// Structured error envelope returned by the CoW orderbook for 4xx / 5xx
/// responses. Mirrors the `Error` schema declared by the orderbook OpenAPI.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiError {
    /// Short machine-readable code (e.g. `"InvalidSignature"`).
    #[serde(rename = "errorType")]
    pub error_type: String,
    /// Human-readable description.
    pub description: String,
    /// Optional structured data attached by the orderbook.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl ApiError {
    /// Typed view of [`Self::error_type`].
    pub fn error_kind(&self) -> OrderbookApiErrorType {
        OrderbookApiErrorType::from(self.error_type.as_str())
    }

    /// Classify this orderbook error into a caller retry policy.
    ///
    /// The table mirrors the CoW watch-tower policy: quote freshness and
    /// EIP-1271 timing issues are retried on the next block; balance,
    /// allowance, limit-order quota and app-data propagation use explicit
    /// backoff; malformed or unsupported orders are dropped. `DuplicatedOrder`
    /// and the historical `DuplicateOrder` spelling are separated from
    /// ordinary drops so submitters can record the order as already accepted
    /// without deleting the parent watch.
    pub fn retry_hint(&self) -> RetryHint {
        match self.error_kind() {
            OrderbookApiErrorType::DuplicatedOrder | OrderbookApiErrorType::DuplicateOrder => {
                RetryHint::AlreadySubmitted
            }
            OrderbookApiErrorType::QuoteNotFound
            | OrderbookApiErrorType::InvalidQuote
            | OrderbookApiErrorType::InsufficientValidTo
            | OrderbookApiErrorType::InvalidEip1271Signature
            | OrderbookApiErrorType::InsufficientFee => RetryHint::Retry,
            OrderbookApiErrorType::InsufficientAllowance
            | OrderbookApiErrorType::InsufficientBalance => RetryHint::Backoff {
                seconds: WATCH_TOWER_BALANCE_ALLOWANCE_BACKOFF_SECS,
            },
            OrderbookApiErrorType::TooManyLimitOrders => RetryHint::Backoff {
                seconds: WATCH_TOWER_LIMIT_ORDER_BACKOFF_SECS,
            },
            OrderbookApiErrorType::InvalidAppData => RetryHint::Backoff {
                seconds: WATCH_TOWER_APP_DATA_BACKOFF_SECS,
            },
            OrderbookApiErrorType::QuoteNotVerified
            | OrderbookApiErrorType::MissingFrom
            | OrderbookApiErrorType::WrongOwner
            | OrderbookApiErrorType::InvalidSignature
            | OrderbookApiErrorType::SellAmountOverflow
            | OrderbookApiErrorType::TransferSimulationFailed
            | OrderbookApiErrorType::ZeroAmount
            | OrderbookApiErrorType::IncompatibleSigningScheme
            | OrderbookApiErrorType::TooMuchGas
            | OrderbookApiErrorType::UnsupportedBuyTokenDestination
            | OrderbookApiErrorType::UnsupportedSellTokenSource
            | OrderbookApiErrorType::UnsupportedOrderType
            | OrderbookApiErrorType::ExcessiveValidTo
            | OrderbookApiErrorType::InvalidNativeSellToken
            | OrderbookApiErrorType::SameBuyAndSellToken
            | OrderbookApiErrorType::UnsupportedToken
            | OrderbookApiErrorType::AppDataHashMismatch
            | OrderbookApiErrorType::AppDataMismatch
            | OrderbookApiErrorType::AppdataFromMismatch
            | OrderbookApiErrorType::MetadataSerializationFailed
            | OrderbookApiErrorType::OldOrderActivelyBidOn
            | OrderbookApiErrorType::Forbidden
            | OrderbookApiErrorType::NonZeroFee
            | OrderbookApiErrorType::NoLiquidity
            | OrderbookApiErrorType::Unknown(_) => RetryHint::Drop,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.error_type, self.description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_round_trips_minimal_body() {
        let json = serde_json::json!({
            "errorType": "InsufficientFee",
            "description": "fee too low",
        });
        let parsed: ApiError = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.error_type, "InsufficientFee");
        assert_eq!(parsed.description, "fee too low");
        assert!(parsed.data.is_none());
    }

    #[test]
    fn api_error_keeps_data_field_when_present() {
        let json = serde_json::json!({
            "errorType": "QuoteNotFound",
            "description": "no quote for token pair",
            "data": { "fee_amount": "1234" },
        });
        let parsed: ApiError = serde_json::from_value(json).unwrap();
        assert!(parsed.data.is_some());
        assert_eq!(parsed.data.unwrap()["fee_amount"], "1234");
    }

    #[test]
    fn api_error_type_parses_known_and_unknown_values() {
        assert_eq!(
            OrderbookApiErrorType::from("DuplicatedOrder"),
            OrderbookApiErrorType::DuplicatedOrder
        );
        assert_eq!(
            OrderbookApiErrorType::from("DuplicateOrder"),
            OrderbookApiErrorType::DuplicateOrder
        );
        assert_eq!(
            OrderbookApiErrorType::from("NewServerError"),
            OrderbookApiErrorType::Unknown("NewServerError".to_owned())
        );
    }

    #[test]
    fn api_error_exposes_typed_error_type() {
        let api = ApiError {
            error_type: "InsufficientBalance".to_owned(),
            description: "balance too low".to_owned(),
            data: None,
        };

        assert_eq!(api.error_kind(), OrderbookApiErrorType::InsufficientBalance);
    }

    #[test]
    fn retry_hint_classifies_orderbook_errors() {
        let cases = [
            ("DuplicatedOrder", RetryHint::AlreadySubmitted),
            ("DuplicateOrder", RetryHint::AlreadySubmitted),
            ("QuoteNotFound", RetryHint::Retry),
            (
                "InsufficientBalance",
                RetryHint::Backoff {
                    seconds: WATCH_TOWER_BALANCE_ALLOWANCE_BACKOFF_SECS,
                },
            ),
            (
                "InsufficientAllowance",
                RetryHint::Backoff {
                    seconds: WATCH_TOWER_BALANCE_ALLOWANCE_BACKOFF_SECS,
                },
            ),
            (
                "TooManyLimitOrders",
                RetryHint::Backoff {
                    seconds: WATCH_TOWER_LIMIT_ORDER_BACKOFF_SECS,
                },
            ),
            ("InvalidSignature", RetryHint::Drop),
            ("NewServerError", RetryHint::Drop),
        ];

        for (error_type, expected) in cases {
            let api = ApiError {
                error_type: error_type.to_owned(),
                description: "test".to_owned(),
                data: None,
            };
            assert_eq!(api.retry_hint(), expected, "{error_type}");
        }
    }
}
