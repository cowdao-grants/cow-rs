//! `IConditionalOrder` polling-error surface.

use alloy_primitives::{Bytes, U256};
use alloy_sol_types::{SolError, sol};

sol! {
    /// Error surface exposed by conditional order handlers.
    ///
    /// `ComposableCoW.getTradeableOrderWithSignature` forwards the
    /// handler's `IConditionalOrder.verify` result. When a conditional
    /// order is not tradeable, handlers revert with one of these custom
    /// errors to tell watch-towers whether to retry, wait for a target
    /// block or timestamp, or stop polling. The generated error types are
    /// used by [`decode_conditional_order_revert`].
    #[derive(Debug)]
    interface IConditionalOrder {
        /// The order is permanently invalid.
        error OrderNotValid(string reason);

        /// The order is not tradeable yet; retry on the next block.
        error PollTryNextBlock(string reason);

        /// The order is not tradeable yet; retry at or after this block.
        error PollTryAtBlock(uint256 blockNumber, string reason);

        /// The order is not tradeable yet; retry at or after this Unix
        /// timestamp.
        error PollTryAtEpoch(uint256 timestamp, string reason);

        /// The order should never be polled again.
        error PollNever(string reason);
    }
}

/// Decoded `IConditionalOrder` revert signal.
///
/// `Unknown` is deliberately explicit: handler-specific errors and
/// malformed payloads are contract-level responses that callers should
/// handle separately from RPC or transport failures.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConditionalOrderRevert {
    /// `OrderNotValid(string)`.
    OrderNotValid {
        /// Handler-provided reason.
        reason: String,
    },
    /// `PollTryNextBlock(string)`.
    PollTryNextBlock {
        /// Handler-provided reason.
        reason: String,
    },
    /// `PollTryAtBlock(uint256,string)`.
    PollTryAtBlock {
        /// Block number at or after which the order should be polled.
        block_number: U256,
        /// Handler-provided reason.
        reason: String,
    },
    /// `PollTryAtEpoch(uint256,string)`.
    PollTryAtEpoch {
        /// Unix timestamp at or after which the order should be polled.
        timestamp: U256,
        /// Handler-provided reason.
        reason: String,
    },
    /// `PollNever(string)`.
    PollNever {
        /// Handler-provided reason.
        reason: String,
    },
    /// Unknown selector, truncated payload, or malformed known payload.
    Unknown {
        /// Raw revert payload, including the selector when present.
        data: Bytes,
    },
}

impl ConditionalOrderRevert {
    /// Handler-provided reason for known `IConditionalOrder` reverts.
    ///
    /// Returns `None` for [`ConditionalOrderRevert::Unknown`] because
    /// the payload either belongs to a handler-specific error or could
    /// not be decoded safely.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::OrderNotValid { reason }
            | Self::PollTryNextBlock { reason }
            | Self::PollTryAtBlock { reason, .. }
            | Self::PollTryAtEpoch { reason, .. }
            | Self::PollNever { reason } => Some(reason),
            Self::Unknown { .. } => None,
        }
    }
}

/// Decode a `getTradeableOrderWithSignature` revert payload.
///
/// Known `IConditionalOrder` custom errors decode to their typed
/// variants. Unknown selectors, truncated data and malformed known
/// payloads return [`ConditionalOrderRevert::Unknown`] with the raw
/// bytes preserved so callers can make an explicit retry/drop decision
/// without confusing contract-level reverts for transport failures.
pub fn decode_conditional_order_revert(data: &[u8]) -> ConditionalOrderRevert {
    if data.len() < 4 {
        return unknown_conditional_order_revert(data);
    }

    let selector: [u8; 4] = data[..4]
        .try_into()
        .expect("length checked before selector extraction");
    match selector {
        s if s == IConditionalOrder::OrderNotValid::SELECTOR => {
            match IConditionalOrder::OrderNotValid::abi_decode(data) {
                Ok(decoded) => ConditionalOrderRevert::OrderNotValid {
                    reason: decoded.reason,
                },
                Err(_) => unknown_conditional_order_revert(data),
            }
        }
        s if s == IConditionalOrder::PollTryNextBlock::SELECTOR => {
            match IConditionalOrder::PollTryNextBlock::abi_decode(data) {
                Ok(decoded) => ConditionalOrderRevert::PollTryNextBlock {
                    reason: decoded.reason,
                },
                Err(_) => unknown_conditional_order_revert(data),
            }
        }
        s if s == IConditionalOrder::PollTryAtBlock::SELECTOR => {
            match IConditionalOrder::PollTryAtBlock::abi_decode(data) {
                Ok(decoded) => ConditionalOrderRevert::PollTryAtBlock {
                    block_number: decoded.blockNumber,
                    reason: decoded.reason,
                },
                Err(_) => unknown_conditional_order_revert(data),
            }
        }
        s if s == IConditionalOrder::PollTryAtEpoch::SELECTOR => {
            match IConditionalOrder::PollTryAtEpoch::abi_decode(data) {
                Ok(decoded) => ConditionalOrderRevert::PollTryAtEpoch {
                    timestamp: decoded.timestamp,
                    reason: decoded.reason,
                },
                Err(_) => unknown_conditional_order_revert(data),
            }
        }
        s if s == IConditionalOrder::PollNever::SELECTOR => {
            match IConditionalOrder::PollNever::abi_decode(data) {
                Ok(decoded) => ConditionalOrderRevert::PollNever {
                    reason: decoded.reason,
                },
                Err(_) => unknown_conditional_order_revert(data),
            }
        }
        _ => unknown_conditional_order_revert(data),
    }
}

fn unknown_conditional_order_revert(data: &[u8]) -> ConditionalOrderRevert {
    ConditionalOrderRevert::Unknown {
        data: Bytes::copy_from_slice(data),
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Bytes, U256, hex, keccak256};
    use alloy_sol_types::SolError;

    use super::*;

    /// `IConditionalOrder` error selectors must equal
    /// `keccak256(signature)[..4]`; watch-towers branch on these bytes
    /// when decoding `getTradeableOrderWithSignature` reverts.
    #[test]
    fn conditional_order_error_selectors_match_keccak() {
        let cases: &[(&[u8; 4], &[u8])] = &[
            (
                &IConditionalOrder::OrderNotValid::SELECTOR,
                b"OrderNotValid(string)",
            ),
            (
                &IConditionalOrder::PollTryNextBlock::SELECTOR,
                b"PollTryNextBlock(string)",
            ),
            (
                &IConditionalOrder::PollTryAtBlock::SELECTOR,
                b"PollTryAtBlock(uint256,string)",
            ),
            (
                &IConditionalOrder::PollTryAtEpoch::SELECTOR,
                b"PollTryAtEpoch(uint256,string)",
            ),
            (
                &IConditionalOrder::PollNever::SELECTOR,
                b"PollNever(string)",
            ),
        ];
        for (selector, signature) in cases {
            assert_eq!(
                selector.as_slice(),
                &keccak256(signature)[..4],
                "selector for {} does not match keccak256(signature)",
                std::str::from_utf8(signature).unwrap(),
            );
        }
    }

    #[test]
    fn decodes_order_not_valid_revert() {
        let data = IConditionalOrder::OrderNotValid {
            reason: "expired".to_string(),
        }
        .abi_encode();

        assert_eq!(
            decode_conditional_order_revert(&data),
            ConditionalOrderRevert::OrderNotValid {
                reason: "expired".to_string(),
            }
        );
    }

    #[test]
    fn decodes_poll_try_next_block_revert() {
        let data = IConditionalOrder::PollTryNextBlock {
            reason: "not ready".to_string(),
        }
        .abi_encode();

        assert_eq!(
            decode_conditional_order_revert(&data),
            ConditionalOrderRevert::PollTryNextBlock {
                reason: "not ready".to_string(),
            }
        );
    }

    #[test]
    fn decodes_poll_try_at_block_revert() {
        let data = IConditionalOrder::PollTryAtBlock {
            blockNumber: U256::from(12_345_678_u64),
            reason: "wait for block".to_string(),
        }
        .abi_encode();

        assert_eq!(
            decode_conditional_order_revert(&data),
            ConditionalOrderRevert::PollTryAtBlock {
                block_number: U256::from(12_345_678_u64),
                reason: "wait for block".to_string(),
            }
        );
    }

    #[test]
    fn decodes_poll_try_at_epoch_revert() {
        let data = IConditionalOrder::PollTryAtEpoch {
            timestamp: U256::from(1_700_000_000_u64),
            reason: "wait for time".to_string(),
        }
        .abi_encode();

        assert_eq!(
            decode_conditional_order_revert(&data),
            ConditionalOrderRevert::PollTryAtEpoch {
                timestamp: U256::from(1_700_000_000_u64),
                reason: "wait for time".to_string(),
            }
        );
    }

    #[test]
    fn decodes_poll_never_revert() {
        let data = IConditionalOrder::PollNever {
            reason: "cancelled".to_string(),
        }
        .abi_encode();

        assert_eq!(
            decode_conditional_order_revert(&data),
            ConditionalOrderRevert::PollNever {
                reason: "cancelled".to_string(),
            }
        );
    }

    #[test]
    fn unknown_selector_preserves_raw_payload() {
        let data = Bytes::from_static(&hex!("7a9332340000000000000000000000000000000000000000"));

        assert_eq!(
            decode_conditional_order_revert(&data),
            ConditionalOrderRevert::Unknown { data }
        );
    }

    #[test]
    fn malformed_known_selector_preserves_raw_payload() {
        let mut data = IConditionalOrder::PollTryAtBlock::SELECTOR.to_vec();
        data.extend_from_slice(&hex!("000102"));
        let data = Bytes::from(data);

        assert_eq!(
            decode_conditional_order_revert(&data),
            ConditionalOrderRevert::Unknown { data }
        );
    }

    #[test]
    fn truncated_revert_preserves_raw_payload() {
        let data = Bytes::from_static(&hex!("010203"));

        assert_eq!(
            decode_conditional_order_revert(&data),
            ConditionalOrderRevert::Unknown { data }
        );
    }

    #[test]
    fn known_revert_reason_is_exposed() {
        let revert = ConditionalOrderRevert::PollTryNextBlock {
            reason: "wait".to_string(),
        };

        assert_eq!(revert.reason(), Some("wait"));
        assert_eq!(
            ConditionalOrderRevert::Unknown { data: Bytes::new() }.reason(),
            None
        );
    }
}
