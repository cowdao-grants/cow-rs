//! Order signatures.
//!
//! Every order is authenticated by a [`Signature`], which is one of four
//! schemes: two off-chain ECDSA variants (`EIP-712` typed data and
//! `EthSign` personal-sign), one smart-contract scheme (`EIP-1271`), and
//! one purely on-chain scheme (`PreSign`). The orderbook serialises the
//! choice as a `signingScheme` field alongside the signature bytes.
//!
//! [`EcdsaSignature`] is a type alias for
//! [`alloy_primitives::Signature`]; the 65-byte `r || s || v` packing,
//! `v` normalisation (0/1/27/28 → parity), recovery and signer-error
//! plumbing all come from alloy. Helpers in this module produce the
//! domain-scoped EIP-712 / EthSign message hash and lift the resulting
//! signature into the [`Signature`] enum.
//!
//! Adapted from [`cowprotocol/services`] (MIT OR Apache-2.0).
//!
//! [`cowprotocol/services`]: https://github.com/cowprotocol/services/blob/main/crates/model/src/signature.rs

use alloy_primitives::{Address, B256, Bytes, Signature as PrimSignature, eip191_hash_message};
use alloy_signer::{Signer, SignerSync, k256::ecdsa::Error as K256Error};
use alloy_sol_types::SolStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt::{self, Debug, Formatter};

use crate::domain::DomainSeparator;
use crate::signing_scheme::{EcdsaSigningScheme, SigningScheme};

/// Maximum accepted EIP-1271 payload, in bytes. Matches the
/// `cowprotocol/services` cap (32 KiB), bounding how large an
/// over-the-wire signature payload may be before it buffers into a
/// `Vec<u8>`.
pub const EIP1271_MAX_LEN: usize = 32 * 1024;

/// Raw ECDSA signature (`r || s || v`, 65 bytes). Type-aliased onto
/// alloy's [`alloy_primitives::Signature`] so the parsing, byte
/// packing, `v` normalisation (0/1/27/28 → parity) and recovery
/// primitives come from there for free. Use [`sign_ecdsa`],
/// [`parse_ecdsa`] and [`ecdsa_from_components`] to construct one, and
/// [`Signature::from_ecdsa`] to lift it into the scheme-tagged
/// [`Signature`] enum.
pub type EcdsaSignature = PrimSignature;

/// Errors specific to signature parsing, signing, or recovery.
///
/// Every variant is reachable from this crate's own functions. The
/// order-verification `SignerMismatch` semantic (a recovered signer that
/// differs from the declared owner) lives in the orderbook crate's
/// `VerifyOwnerError` instead, since no signing primitive here produces
/// it.
#[derive(Debug, thiserror::Error)]
pub enum SignatureError {
    /// ECDSA payload was not 65 bytes (`r || s || v`).
    #[error("expected 65 ecdsa signature bytes, got {0}")]
    Length(usize),
    /// PreSign payload was neither empty nor a 20-byte owner.
    #[error("presign payload must be empty or a 20-byte owner, got {0} bytes")]
    PreSignLength(usize),
    /// EIP-1271 payload exceeded [`EIP1271_MAX_LEN`].
    #[error("eip1271 signature payload too long: {len} bytes (max {max})")]
    Eip1271TooLong {
        /// Observed payload length, in bytes.
        len: usize,
        /// Configured cap (`EIP1271_MAX_LEN`).
        max: usize,
    },
    /// `v` recovery byte was not in `{0, 1, 27, 28}`.
    #[error("invalid signature v value: {0}; expected 0, 1, 27 or 28")]
    InvalidV(u8),
    /// ECDSA recovery failed.
    #[error("ecdsa recovery failed: {0}")]
    Recovery(#[from] alloy_primitives::SignatureError),
    /// `k256` signer error.
    #[error("k256 signer error: {0}")]
    Signer(#[from] K256Error),
    /// Non-`k256` signer error (remote signer, hardware wallet, KMS).
    /// Owned message so attacker-controllable bytes are never leaked.
    #[error("signer error: {0}")]
    SignerOther(String),
}

/// Signature over the EIP-712 order hash.
#[derive(Clone, Eq, PartialEq, Hash)]
pub enum Signature {
    /// EIP-712 typed-data signature.
    Eip712(EcdsaSignature),
    /// EIP-191 personal-sign over the EIP-712 hash.
    EthSign(EcdsaSignature),
    /// EIP-1271 contract signature payload, in the **orderbook wire
    /// form**: the raw verifier bytes only. The orderbook re-prepends the
    /// owner from the submission body's `from` field (the orderbook
    /// crate's `OrderCreation`) before settlement. This is *not* the on-chain calldata form:
    /// `GPv2Signing.recoverEip1271Signer` expects
    /// `abi.encodePacked(owner, signature)` (owner in the first 20
    /// bytes), so feeding these bytes straight into
    /// `GPv2Settlement.settle()` yields a malformed signature.
    ///
    /// Construct it with [`Signature::eip1271`] rather than the bare
    /// variant: the constructor enforces the [`EIP1271_MAX_LEN`] cap,
    /// which direct variant construction skips.
    Eip1271(Vec<u8>),
    /// On-chain pre-signature via `GPv2Signing::setPreSignature`. The
    /// orderbook wire form is an empty payload (the owner is carried on
    /// the order); the on-chain calldata form is the 20-byte owner
    /// address (`GPv2Signing.sol` requires `length == 20`).
    /// [`Signature::from_bytes`] accepts both.
    ///
    /// Construct it with [`Signature::pre_sign`] rather than the bare
    /// variant; `to_bytes` then yields the empty wire payload:
    ///
    /// ```
    /// use cowprotocol_signing::Signature;
    /// assert!(Signature::pre_sign().to_bytes().is_empty());
    /// ```
    PreSign,
}

impl Debug for Signature {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::PreSign => f.write_str("PreSign"),
            other => {
                let scheme = format!("{:?}", other.scheme());
                let bytes = Bytes::from(other.to_bytes()).to_string();
                f.debug_tuple(&scheme).field(&bytes).finish()
            }
        }
    }
}

impl Signature {
    /// Lift an ECDSA signature into the scheme-tagged [`Signature`]
    /// enum. Pairs with [`sign_ecdsa`] / [`ecdsa_from_components`].
    pub const fn from_ecdsa(sig: EcdsaSignature, scheme: EcdsaSigningScheme) -> Self {
        match scheme {
            EcdsaSigningScheme::Eip712 => Self::Eip712(sig),
            EcdsaSigningScheme::EthSign => Self::EthSign(sig),
        }
    }

    /// Construct an EIP-1271 (contract-wallet) signature, enforcing the
    /// [`EIP1271_MAX_LEN`] cap that the bare [`Signature::Eip1271`]
    /// variant skips. The named, validating constructor for the Safe /
    /// contract-wallet flow, mirroring [`Signature::from_ecdsa`] for the
    /// off-chain schemes.
    pub fn eip1271(bytes: impl Into<Vec<u8>>) -> Result<Self, SignatureError> {
        let bytes = bytes.into();
        if bytes.len() > EIP1271_MAX_LEN {
            return Err(SignatureError::Eip1271TooLong {
                len: bytes.len(),
                max: EIP1271_MAX_LEN,
            });
        }
        Ok(Self::Eip1271(bytes))
    }

    /// Construct a [`Signature::PreSign`] signature for the on-chain
    /// pre-signature flow. The pre-signature carries no payload;
    /// [`Signature::to_bytes`] yields an empty `Vec`.
    pub const fn pre_sign() -> Self {
        Self::PreSign
    }

    /// Which signing scheme this signature corresponds to.
    pub const fn scheme(&self) -> SigningScheme {
        match self {
            Self::Eip712(_) => SigningScheme::Eip712,
            Self::EthSign(_) => SigningScheme::EthSign,
            Self::Eip1271(_) => SigningScheme::Eip1271,
            Self::PreSign => SigningScheme::PreSign,
        }
    }

    /// View the off-chain ECDSA payload and its scheme, or `None` for
    /// the EIP-1271 / PreSign variants. The one place "is this an
    /// off-chain ECDSA signature" is answered for a constructed value;
    /// pairs with [`SigningScheme::try_to_ecdsa_scheme`], which answers
    /// the same question for a bare scheme.
    pub const fn as_ecdsa(&self) -> Option<(&EcdsaSignature, EcdsaSigningScheme)> {
        match self {
            Self::Eip712(s) => Some((s, EcdsaSigningScheme::Eip712)),
            Self::EthSign(s) => Some((s, EcdsaSigningScheme::EthSign)),
            Self::Eip1271(_) | Self::PreSign => None,
        }
    }

    /// Encode the signature as the bytes the orderbook expects in the
    /// `signature` field of `POST /api/v1/orders` / `DELETE /api/v1/orders`.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::Eip712(s) | Self::EthSign(s) => s.as_bytes().to_vec(),
            Self::Eip1271(bytes) => bytes.clone(),
            Self::PreSign => Vec::new(),
        }
    }

    /// Decode a signature received over the wire.
    ///
    /// For [`SigningScheme::PreSign`] the body must be empty or exactly the
    /// owner address (legacy 20-byte encoding accepted by services). The
    /// 20-byte legacy form is accepted for services compatibility, but its
    /// address payload is intentionally not validated against the order's
    /// `from`: the owner is carried explicitly on the orderbook crate's
    /// `OrderCreation` submission body.
    pub fn from_bytes(scheme: SigningScheme, bytes: &[u8]) -> Result<Self, SignatureError> {
        if let Some(ecdsa_scheme) = scheme.try_to_ecdsa_scheme() {
            return Ok(Self::from_ecdsa(parse_ecdsa(bytes)?, ecdsa_scheme));
        }
        match scheme {
            SigningScheme::Eip1271 => {
                if bytes.len() > EIP1271_MAX_LEN {
                    return Err(SignatureError::Eip1271TooLong {
                        len: bytes.len(),
                        max: EIP1271_MAX_LEN,
                    });
                }
                Ok(Self::Eip1271(bytes.to_vec()))
            }
            SigningScheme::PreSign => {
                if !(bytes.is_empty() || bytes.len() == 20) {
                    return Err(SignatureError::PreSignLength(bytes.len()));
                }
                Ok(Self::PreSign)
            }
            SigningScheme::Eip712 | SigningScheme::EthSign => {
                unreachable!("ECDSA schemes handled by try_to_ecdsa_scheme above")
            }
        }
    }

    /// Recover the signing owner of an ECDSA signature.
    ///
    /// Returns `Ok(None)` for [`Signature::Eip1271`] and
    /// [`Signature::PreSign`]: those schemes carry the owner explicitly,
    /// they do not derive it.
    pub fn recover<T: SolStruct>(
        &self,
        domain: &DomainSeparator,
        payload: &T,
    ) -> Result<Option<Recovered>, SignatureError> {
        self.as_ecdsa()
            .map(|(s, scheme)| ecdsa_recover(s, scheme, domain, payload))
            .transpose()
    }
}

/// 32-byte signing message together with the address that produced it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Recovered {
    /// The 32-byte message that was actually signed (post-EIP-191 wrapping
    /// for `EthSign`, plain typed-data hash for `Eip712`).
    pub message: B256,
    /// Address recovered from the signature.
    pub signer: Address,
}

/// Sign an EIP-712 [`SolStruct`] payload with a
/// [`SignerSync`]-implementing signer.
///
/// In practice only a raw in-memory key
/// (`alloy_signer_local::PrivateKeySigner`) implements [`SignerSync`].
/// Production backends (hardware wallet, remote signer, KMS) implement
/// the async [`alloy_signer::Signer`] trait, so reach for the
/// [`sign_ecdsa_async`] twin or the [`signing_message`] digest-and-lift
/// recipe instead.
pub fn sign_ecdsa<T: SolStruct, S: SignerSync>(
    scheme: EcdsaSigningScheme,
    domain: &DomainSeparator,
    payload: &T,
    signer: &S,
) -> Result<EcdsaSignature, SignatureError> {
    let message = signing_message(scheme, domain, payload);
    let raw = signer.sign_hash_sync(&message).map_err(|e| match e {
        alloy_signer::Error::Ecdsa(k) => SignatureError::Signer(k),
        other => SignatureError::SignerOther(other.to_string()),
    })?;
    parse_ecdsa(&raw.as_bytes())
}

/// Async counterpart to [`sign_ecdsa`], bound on the async
/// [`alloy_signer::Signer`] trait (`sign_hash`) rather than
/// [`SignerSync`]. This makes production backends (hardware wallet,
/// remote signer, KMS), which implement only the async trait, a
/// first-class signing path.
///
/// Signs the same [`signing_message`] digest as [`sign_ecdsa`], so a
/// signer implementing both traits yields a byte-identical signature.
/// The async signer's error is mapped into [`SignatureError::Signer`]
/// for the `k256` case and [`SignatureError::SignerOther`] otherwise,
/// preserving the signer's own message.
pub async fn sign_ecdsa_async<T: SolStruct, S: Signer>(
    scheme: EcdsaSigningScheme,
    domain: &DomainSeparator,
    payload: &T,
    signer: &S,
) -> Result<EcdsaSignature, SignatureError> {
    let message = signing_message(scheme, domain, payload);
    let raw = signer.sign_hash(&message).await.map_err(|e| match e {
        alloy_signer::Error::Ecdsa(k) => SignatureError::Signer(k),
        other => SignatureError::SignerOther(other.to_string()),
    })?;
    parse_ecdsa(&raw.as_bytes())
}

/// Decode an `r || s || v` (65-byte) payload through alloy's `v`
/// normalisation; legacy `v ∈ {0, 1}` and Electrum `v ∈ {27, 28}` are
/// both accepted. Length is validated explicitly so callers get a
/// dedicated [`SignatureError::Length`] before alloy's generic
/// `FromBytes` error.
pub fn parse_ecdsa(bytes: &[u8]) -> Result<EcdsaSignature, SignatureError> {
    if bytes.len() != 65 {
        return Err(SignatureError::Length(bytes.len()));
    }
    if !matches!(bytes[64], 0 | 1 | 27 | 28) {
        return Err(SignatureError::InvalidV(bytes[64]));
    }
    Ok(PrimSignature::from_raw(bytes)?)
}

/// Assemble a signature from its three scalar components. `v` is
/// rejected for any value outside `{0, 1, 27, 28}` and normalised to
/// the canonical Electrum form alloy stores internally.
pub fn ecdsa_from_components(r: B256, s: B256, v: u8) -> Result<EcdsaSignature, SignatureError> {
    let mut bytes = [0u8; 65];
    bytes[..32].copy_from_slice(r.as_slice());
    bytes[32..64].copy_from_slice(s.as_slice());
    bytes[64] = v;
    parse_ecdsa(&bytes)
}

/// Recover the signer address from an ECDSA signature against the
/// given domain and payload.
pub fn ecdsa_recover<T: SolStruct>(
    sig: &EcdsaSignature,
    scheme: EcdsaSigningScheme,
    domain: &DomainSeparator,
    payload: &T,
) -> Result<Recovered, SignatureError> {
    let message = signing_message(scheme, domain, payload);
    let signer = sig.recover_address_from_prehash(&message)?;
    Ok(Recovered { message, signer })
}

/// Compute the message bytes the owner actually signs for the given
/// scheme. `Eip712` returns the typed-data hash supplied directly by
/// [`SolStruct::eip712_signing_hash`]; `EthSign` wraps that hash in the
/// EIP-191 personal-sign envelope via
/// [`alloy_primitives::eip191_hash_message`].
///
/// This is the forward counterpart to [`ecdsa_recover`]: it yields the
/// exact 32 bytes to hand an external or async signer (hardware wallet,
/// KMS, injected provider), whose returned signature is then lifted back
/// with [`Signature::from_ecdsa`] / [`ecdsa_from_components`]. For an
/// [`crate::order::OrderData`] prefer the
/// [`OrderData::signing_hash`](crate::order::OrderData::signing_hash)
/// wrapper, which builds the payload for you.
pub fn signing_message<T: SolStruct>(
    signing_scheme: EcdsaSigningScheme,
    domain: &DomainSeparator,
    payload: &T,
) -> B256 {
    let eip712 = payload.eip712_signing_hash(domain);
    match signing_scheme {
        EcdsaSigningScheme::Eip712 => eip712,
        EcdsaSigningScheme::EthSign => eip191_hash_message(eip712),
    }
}

/// Serde adapter for [`EcdsaSignature`] fields that must travel as a
/// 65-byte `0x`-prefixed hex string (the cow orderbook wire form), not
/// the `{r, s, yParity, v}` map alloy emits by default. Apply with
/// `#[serde(with = "cowprotocol::signature::ecdsa_wire")]` on the field.
pub mod ecdsa_wire {
    use super::{EcdsaSignature, parse_ecdsa};
    use alloy_primitives::FixedBytes;
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

    /// Serialise as a 65-byte `0x`-prefixed hex string.
    pub fn serialize<S>(sig: &EcdsaSignature, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        FixedBytes::from(sig.as_bytes()).serialize(serializer)
    }

    /// Deserialise from a 65-byte `0x`-prefixed hex string, running
    /// [`parse_ecdsa`] for length / `v` normalisation.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<EcdsaSignature, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes = FixedBytes::<65>::deserialize(deserializer)?;
        parse_ecdsa(bytes.as_slice()).map_err(de::Error::custom)
    }
}

// --- serde for the scheme-tagged Signature enum ------------------------

/// Serde-only wire shape: `{ signingScheme, signature: "0x..." }`. The
/// `signature` payload is decoded through [`deserialize_capped_hex`],
/// which rejects oversize hex strings before `const_hex::decode`
/// allocates a buffer for them.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonSignature {
    signing_scheme: SigningScheme,
    #[serde(deserialize_with = "deserialize_capped_hex")]
    signature: Bytes,
}

/// Reject `0x`-prefixed hex strings longer than the EIP-1271 cap before
/// `const_hex::decode` allocates a `Vec<u8>` for them. The cap is `0x`
/// plus two hex chars per max EIP-1271 byte; oversize inputs surface as
/// a serde error instead of buffering megabytes of attacker-controlled
/// hex into memory.
fn deserialize_capped_hex<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
where
    D: Deserializer<'de>,
{
    struct CappedHexVisitor;

    impl<'de> de::Visitor<'de> for CappedHexVisitor {
        type Value = Bytes;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "0x-prefixed hex string of at most {EIP1271_MAX_LEN} bytes",
            )
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Bytes, E> {
            // 0x prefix plus two hex chars per byte.
            const MAX_HEX_LEN: usize = 2 + EIP1271_MAX_LEN * 2;
            if v.len() > MAX_HEX_LEN {
                return Err(E::custom(format!(
                    "signature hex exceeds {EIP1271_MAX_LEN}-byte cap (got {} chars)",
                    v.len()
                )));
            }
            v.parse::<Bytes>().map_err(E::custom)
        }
    }

    deserializer.deserialize_str(CappedHexVisitor)
}

impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        JsonSignature {
            signing_scheme: self.scheme(),
            signature: Bytes::from(self.to_bytes()),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let json = JsonSignature::deserialize(deserializer)?;
        Self::from_bytes(json.signing_scheme, &json.signature).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{U256, hex};
    use alloy_signer_local::PrivateKeySigner;
    use serde_json::json;

    use super::*;

    #[test]
    fn from_bytes_rejects_wrong_lengths() {
        assert!(matches!(
            Signature::from_bytes(SigningScheme::Eip712, &[0u8; 20]),
            Err(SignatureError::Length(20))
        ));
        assert!(matches!(
            Signature::from_bytes(SigningScheme::PreSign, &[0u8; 32]),
            Err(SignatureError::PreSignLength(32))
        ));
    }

    #[test]
    fn ecdsa_default_zero_signature_round_trips() {
        let sig = Signature::from_bytes(SigningScheme::Eip712, &[0u8; 65]).unwrap();
        let zero = Signature::Eip712(PrimSignature::from_bytes_and_parity(&[0u8; 64], false));
        assert_eq!(sig, zero);
    }

    #[test]
    fn eip1271_rejects_oversize_payload() {
        let oversize = vec![0u8; EIP1271_MAX_LEN + 1];
        assert!(matches!(
            Signature::from_bytes(SigningScheme::Eip1271, &oversize),
            Err(SignatureError::Eip1271TooLong { len, max })
                if len == EIP1271_MAX_LEN + 1 && max == EIP1271_MAX_LEN
        ));
        let at_limit = vec![0u8; EIP1271_MAX_LEN];
        assert!(Signature::from_bytes(SigningScheme::Eip1271, &at_limit).is_ok());
    }

    /// The pre-decode visitor in `deserialize_capped_hex` must reject
    /// an over-cap hex string before `const_hex::decode` allocates a
    /// buffer for it, and accept a payload exactly at the cap. Both
    /// JSON entry points (`from_str`, which streams the raw string to
    /// the visitor, and `from_value`, which hands it an owned string)
    /// must agree.
    #[test]
    fn deserialize_enforces_eip1271_cap_pre_decode() {
        let oversize_hex = format!("0x{}", "00".repeat(EIP1271_MAX_LEN + 1));
        let payload =
            format!("{{\"signingScheme\":\"eip1271\",\"signature\":\"{oversize_hex}\"}}",);
        for err in [
            serde_json::from_str::<Signature>(&payload)
                .expect_err("oversize hex string must be rejected before decode"),
            serde_json::from_value::<Signature>(serde_json::from_str(&payload).unwrap())
                .expect_err("oversize signature payload must be rejected on deserialise"),
        ] {
            let msg = err.to_string();
            assert!(
                msg.contains("cap") || msg.contains(&EIP1271_MAX_LEN.to_string()),
                "error should reference the EIP-1271 length cap, got: {msg}",
            );
        }

        // The same payload encoded exactly at the cap still decodes
        // (an all-zero EIP-1271 blob, valid per `from_bytes`'s
        // length-only check).
        let at_limit_hex = format!("0x{}", "00".repeat(EIP1271_MAX_LEN));
        let payload =
            format!("{{\"signingScheme\":\"eip1271\",\"signature\":\"{at_limit_hex}\"}}",);
        let sig: Signature = serde_json::from_str(&payload).unwrap();
        assert!(matches!(sig, Signature::Eip1271(ref b) if b.len() == EIP1271_MAX_LEN));
    }

    /// Pins the exact PreSign length boundary so a future tightening
    /// (e.g. validating the 20-byte payload against the owner, or dropping
    /// the legacy form) trips a test rather than silently changing wire
    /// behaviour: empty and exactly 20 bytes are accepted; any other length
    /// is rejected.
    #[test]
    fn from_bytes_presign_accepts_empty_and_20_bytes_rejects_others() {
        assert_eq!(
            Signature::from_bytes(SigningScheme::PreSign, &[]).unwrap(),
            Signature::PreSign
        );
        assert_eq!(
            Signature::from_bytes(SigningScheme::PreSign, &[0u8; 20]).unwrap(),
            Signature::PreSign
        );
        assert!(matches!(
            Signature::from_bytes(SigningScheme::PreSign, &[0u8; 19]),
            Err(SignatureError::PreSignLength(19))
        ));
        assert!(matches!(
            Signature::from_bytes(SigningScheme::PreSign, &[0u8; 21]),
            Err(SignatureError::PreSignLength(21))
        ));
    }

    #[test]
    fn eip1271_constructor_enforces_cap() {
        // At the cap is accepted; one byte over is rejected, unlike the
        // bare `Signature::Eip1271(..)` variant which skips the check.
        let at_cap = Signature::eip1271(vec![0u8; EIP1271_MAX_LEN]).unwrap();
        assert_eq!(at_cap.to_bytes().len(), EIP1271_MAX_LEN);
        assert!(matches!(
            Signature::eip1271(vec![0u8; EIP1271_MAX_LEN + 1]),
            Err(SignatureError::Eip1271TooLong { max, .. }) if max == EIP1271_MAX_LEN
        ));
    }

    #[test]
    fn pre_sign_constructor_yields_empty_wire_bytes() {
        let sig = Signature::pre_sign();
        assert_eq!(sig, Signature::PreSign);
        assert!(sig.to_bytes().is_empty());
    }

    #[test]
    fn v_normalisation_matches_services() {
        for (raw, expected) in [(0u8, 27u8), (1, 28), (27, 27), (28, 28)] {
            let mut bytes = [0u8; 65];
            bytes[64] = raw;
            let sig = parse_ecdsa(&bytes).unwrap();
            assert_eq!(sig.as_bytes()[64], expected);
        }
    }

    #[test]
    fn invalid_v_rejected() {
        for invalid_v in [2u8, 3, 26, 29, 30, 255] {
            let mut bytes = [0u8; 65];
            bytes[64] = invalid_v;
            assert!(matches!(
                parse_ecdsa(&bytes),
                Err(SignatureError::InvalidV(v)) if v == invalid_v
            ));
        }
    }

    #[test]
    fn json_round_trip_for_each_scheme() {
        for (signature, expected_json) in [
            (
                Signature::Eip1271(vec![1, 2, 3]),
                json!({ "signingScheme": "eip1271", "signature": "0x010203" }),
            ),
            (
                Signature::Eip1271(Vec::new()),
                json!({ "signingScheme": "eip1271", "signature": "0x" }),
            ),
            (
                Signature::PreSign,
                json!({ "signingScheme": "presign", "signature": "0x" }),
            ),
        ] {
            let serialised = serde_json::to_value(&signature).unwrap();
            assert_eq!(serialised, expected_json);
            let parsed: Signature = serde_json::from_value(expected_json).unwrap();
            assert_eq!(parsed, signature);
        }
    }

    alloy_sol_types::sol! {
        /// Minimal `SolStruct` view used only by the tests in this
        /// module: a single `bytes32` field. Decoupled from
        /// [`crate::order::eip712::Order`] so the signature primitives
        /// can be exercised without dragging in an `OrderData` fixture.
        struct Probe {
            bytes32 value;
        }
    }

    fn probe_payload(value: [u8; 32]) -> Probe {
        Probe {
            value: B256::from(value),
        }
    }

    /// Sign-and-recover round trip for both ECDSA schemes against the
    /// `alloy_signer_local::PrivateKeySigner` reference implementation.
    /// Mirrors `cowprotocol/services/.../signature.rs::test_ecdsa_signature_recovery`.
    #[test]
    fn ecdsa_sign_recover_round_trip() {
        let signer = PrivateKeySigner::from_bytes(&U256::from(1u64).to_be_bytes().into()).unwrap();
        let address = signer.address();

        let domain = crate::domain::settlement_domain(
            1,
            alloy_primitives::address!("9008D19f58AAbD9eD0D60971565AA8510560ab41"),
        );
        let payload = probe_payload(hex!(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));

        for scheme in [EcdsaSigningScheme::Eip712, EcdsaSigningScheme::EthSign] {
            let ecdsa = sign_ecdsa(scheme, &domain, &payload, &signer).unwrap();
            let typed = Signature::from_ecdsa(ecdsa, scheme);
            let recovered = typed.recover(&domain, &payload).unwrap().unwrap();
            assert_eq!(recovered.signer, address);
        }
    }

    /// [`sign_ecdsa_async`] must produce the byte-identical signature to
    /// [`sign_ecdsa`] for a signer implementing both traits, since both
    /// sign the same [`signing_message`] digest.
    #[tokio::test]
    async fn sign_ecdsa_async_matches_sync() {
        let signer = PrivateKeySigner::from_bytes(&U256::from(1u64).to_be_bytes().into()).unwrap();
        let domain = crate::domain::settlement_domain(
            1,
            alloy_primitives::address!("9008D19f58AAbD9eD0D60971565AA8510560ab41"),
        );
        let payload = probe_payload(hex!(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));

        for scheme in [EcdsaSigningScheme::Eip712, EcdsaSigningScheme::EthSign] {
            let sync = sign_ecdsa(scheme, &domain, &payload, &signer).unwrap();
            let asynchronous = sign_ecdsa_async(scheme, &domain, &payload, &signer)
                .await
                .unwrap();
            assert_eq!(sync, asynchronous);
        }
    }

    /// Async-only signer (implements [`alloy_signer::Signer`] but not
    /// [`SignerSync`]) whose `sign_hash` always fails, mirroring a remote
    /// or KMS backend that rejects the request.
    struct FailingAsyncSigner;

    #[async_trait::async_trait]
    impl Signer for FailingAsyncSigner {
        async fn sign_hash(&self, _hash: &B256) -> Result<PrimSignature, alloy_signer::Error> {
            Err(alloy_signer::Error::message("remote signer offline"))
        }

        fn address(&self) -> alloy_primitives::Address {
            alloy_primitives::Address::ZERO
        }

        fn chain_id(&self) -> Option<alloy_primitives::ChainId> {
            None
        }

        fn set_chain_id(&mut self, _chain_id: Option<alloy_primitives::ChainId>) {}
    }

    /// [`sign_ecdsa_async`] accepts a signer that implements only the async
    /// [`Signer`] trait, and maps the signer's own error into
    /// [`SignatureError::SignerOther`] with its message preserved.
    #[tokio::test]
    async fn sign_ecdsa_async_maps_signer_error() {
        let domain = crate::domain::settlement_domain(
            1,
            alloy_primitives::address!("9008D19f58AAbD9eD0D60971565AA8510560ab41"),
        );
        let payload = probe_payload([0u8; 32]);

        let err = sign_ecdsa_async(
            EcdsaSigningScheme::Eip712,
            &domain,
            &payload,
            &FailingAsyncSigner,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            SignatureError::SignerOther(ref msg) if msg.contains("remote signer offline")
        ));
    }

    #[test]
    fn recover_returns_none_for_onchain_schemes() {
        let domain = crate::domain::DomainSeparator::default();
        let payload = probe_payload([0u8; 32]);
        for signature in [Signature::PreSign, Signature::Eip1271(Vec::new())] {
            let recovered = signature.recover(&domain, &payload).unwrap();
            assert!(recovered.is_none());
        }
    }
}
