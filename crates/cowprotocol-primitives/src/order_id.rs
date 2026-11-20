//! Order identifiers and classifications shared across the SDK crates.
//!
//! These are pure wire types with no signing or HTTP machinery: the
//! 56-byte order UID and the server-side order classification. They
//! live in the primitives crate so that, for example, the app-data
//! crate can reference an order UID without depending on the signing
//! stack.

use alloy_primitives::{Address, B256, FixedBytes};
use serde::{Deserialize, Serialize};

/// 56-byte order identifier:
/// `32-byte digest || 20-byte owner || 4-byte validTo`. The digest is
/// `keccak256(0x19 0x01 || domain_separator || order_struct_hash)`.
///
/// Type-aliased onto alloy's [`FixedBytes<56>`] so `Debug` / `Display` /
/// serde (`0x`-prefixed lower-case hex), `FromStr`, `From<[u8; 56]>`,
/// `AsRef<[u8]>` and `Index` all come from there for free; use
/// [`OrderUidParts`] to split or rebuild the 56-byte layout.
pub type OrderUid = FixedBytes<56>;

/// Split / rebuild an [`OrderUid`]'s `digest || owner || validTo` layout.
pub trait OrderUidParts {
    /// Assemble from the three parts.
    fn from_parts(hash: B256, owner: Address, valid_to: u32) -> Self;
    /// Split into `(digest, owner, validTo)`.
    fn to_parts(&self) -> (B256, Address, u32);
}

/// Errors from [`parse_order_uid`].
#[derive(Debug, thiserror::Error)]
pub enum OrderUidParseError {
    /// The string did not start with the canonical `0x` prefix.
    #[error("order UID must be 0x-prefixed")]
    MissingPrefix,
    /// The body was not valid 56-byte hex.
    #[error("invalid order UID hex: {0}")]
    Hex(#[from] alloy_primitives::hex::FromHexError),
}

/// Parse an [`OrderUid`] from its canonical `0x`-prefixed lower-case hex
/// form. alloy's blanket `FromStr` for [`FixedBytes`] also accepts the
/// unprefixed body, which can cause canonicalisation confusion in code
/// that validates, caches, authorises, or compares UID strings before
/// converting them. Use this helper for untrusted input to reject the
/// non-canonical encoding up front.
pub fn parse_order_uid(s: &str) -> Result<OrderUid, OrderUidParseError> {
    if !s.starts_with("0x") {
        return Err(OrderUidParseError::MissingPrefix);
    }
    Ok(s.parse::<OrderUid>()?)
}

impl OrderUidParts for OrderUid {
    fn from_parts(hash: B256, owner: Address, valid_to: u32) -> Self {
        let mut uid = [0u8; 56];
        uid[0..32].copy_from_slice(hash.as_slice());
        uid[32..52].copy_from_slice(owner.as_slice());
        uid[52..56].copy_from_slice(&valid_to.to_be_bytes());
        Self::new(uid)
    }

    fn to_parts(&self) -> (B256, Address, u32) {
        let bytes = self.as_slice();
        let mut valid_to = [0u8; 4];
        valid_to.copy_from_slice(&bytes[52..56]);
        (
            B256::from_slice(&bytes[0..32]),
            Address::from_slice(&bytes[32..52]),
            u32::from_be_bytes(valid_to),
        )
    }
}

/// Server-side order classification. Drives fee handling and solver
/// routing.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderClass {
    /// Standard market order.
    #[default]
    Market,
    /// Solver-internal, placed by whitelisted participants.
    Liquidity,
    /// Limit order; fee taken from surplus once the target is met.
    Limit,
}
