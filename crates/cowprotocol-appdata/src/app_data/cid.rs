//! IPFS CID derivation for app-data documents.
//!
//! The orderbook pins every app-data document under
//! `cidv1(raw=0x55, multihash=keccak-256(hash))`. This module derives
//! that CID offline from an [`AppDataHash`] and recovers the digest
//! from untrusted CID strings with fail-closed validation.

use cid::multihash::Multihash;
use cowprotocol_primitives::AppDataHash;

/// IPFS CID the orderbook pins app-data under: `cidv1(raw=0x55,
/// multihash=keccak-256(hash))`. Aliased onto [`cid::Cid`] so `Display`
/// (base32 lower-case with the `b` multibase prefix), `FromStr` and
/// validation come from the upstream crate. Build one with
/// [`app_data_cid`] and recover the embedded digest with
/// [`app_data_hash_from_cid`].
pub type AppDataCid = cid::Cid;

/// Raw codec (`0x55`) used for the app-data CID payload.
pub(crate) const CID_CODEC_RAW: u64 = 0x55;
/// Keccak-256 multihash code (`0x1b`).
pub(crate) const MULTIHASH_KECCAK_256: u64 = 0x1b;
/// Upper bound on an app-data CID string. A CIDv1 wrapping a 32-byte
/// keccak-256 digest is ~59 chars in canonical base32 and ~75 in
/// base16; anything far longer is malformed or pathologically long.
/// Capping before `cid::Cid::from_str` bounds the allocation the
/// upstream multibase decoder would otherwise make for the input.
pub const MAX_CID_STR_LEN: usize = 128;

/// Parse an [`AppDataCid`] from its string form, rejecting input above
/// [`MAX_CID_STR_LEN`] before the upstream multibase decoder allocates.
/// Prefer this over `s.parse::<AppDataCid>()` whenever the string comes
/// from untrusted input (user-supplied or arbitrary off-chain metadata).
pub fn parse_app_data_cid(s: &str) -> Result<AppDataCid, AppDataCidError> {
    if s.len() > MAX_CID_STR_LEN {
        return Err(AppDataCidError::CidTooLong {
            len: s.len(),
            max: MAX_CID_STR_LEN,
        });
    }
    Ok(s.parse::<AppDataCid>()?)
}

/// Build the IPFS CID the orderbook pins for an app-data digest. Pure
/// offline derivation: wraps `hash` in a keccak-256 multihash and folds
/// it into a CIDv1 with the raw codec. The resulting [`cid::Cid`]
/// displays as the canonical `b...` base32 string and round-trips
/// through `cid::Cid::from_str`.
pub fn app_data_cid(hash: AppDataHash) -> AppDataCid {
    let multihash = Multihash::<32>::wrap(MULTIHASH_KECCAK_256, hash.as_slice())
        .expect("digest fits a 32-byte multihash by construction");
    AppDataCid::new_v1(CID_CODEC_RAW, multihash.resize().expect("32 <= 64"))
}

/// Recover the embedded 32-byte digest from an [`AppDataCid`].
/// Validates the codec, multihash code, and digest length match
/// `cidv1(raw=0x55, multihash=keccak-256/32)`, so a CID with the wrong
/// codec, multihash, or digest length is rejected rather than silently
/// mapping to a different digest.
pub fn app_data_hash_from_cid(cid: &AppDataCid) -> Result<AppDataHash, AppDataCidError> {
    if cid.codec() != CID_CODEC_RAW {
        return Err(AppDataCidError::UnexpectedCodec(cid.codec()));
    }
    let multihash = cid.hash();
    if multihash.code() != MULTIHASH_KECCAK_256 {
        return Err(AppDataCidError::UnexpectedMultihashCode(multihash.code()));
    }
    let digest = multihash.digest();
    if digest.len() != 32 {
        return Err(AppDataCidError::UnexpectedDigestLength(digest.len()));
    }
    Ok(AppDataHash::from_slice(digest))
}

/// Errors raised while parsing an [`AppDataCid`] back into an
/// [`AppDataHash`]. Wraps [`cid::Error`] for syntactic failures and
/// surfaces dedicated variants for codec / multihash / digest-length
/// drift, which the upstream parser would otherwise silently accept.
#[derive(Debug, thiserror::Error)]
pub enum AppDataCidError {
    /// The string could not be parsed as a CID at all (bad multibase
    /// prefix, invalid varint, truncated body, etc).
    #[error("invalid CID: {0}")]
    InvalidCid(#[from] cid::Error),
    /// The CID string was longer than [`MAX_CID_STR_LEN`], so it was
    /// rejected before the multibase decoder allocated for it.
    #[error("CID string exceeds {max}-char cap (got {len})")]
    CidTooLong {
        /// Length of the offending input, in chars.
        len: usize,
        /// Configured cap ([`MAX_CID_STR_LEN`]).
        max: usize,
    },
    /// The CID codec was not the raw codec (`0x55`).
    #[error("expected raw codec (0x55), got 0x{0:02x}")]
    UnexpectedCodec(u64),
    /// The multihash code was not keccak-256 (`0x1b`).
    #[error("expected keccak-256 multihash (0x1b), got 0x{0:02x}")]
    UnexpectedMultihashCode(u64),
    /// The multihash digest was not 32 bytes long.
    #[error("expected 32-byte digest, got {0}")]
    UnexpectedDigestLength(usize),
}
