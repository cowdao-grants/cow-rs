//! Wire-level enumeration of order-signing schemes accepted by the CoW
//! Protocol orderbook.

use serde::{Deserialize, Serialize};

/// How an order is authenticated by its owner.
///
/// The corresponding signature payload: [`crate::signature::EcdsaSignature`]
/// for the ECDSA variants, arbitrary `bytes` for EIP-1271, or empty for
/// `PreSign`: lives in [`crate::signature::Signature`].
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SigningScheme {
    /// EIP-712 typed-data signature over the order struct hash.
    #[default]
    Eip712,
    /// EIP-191 personal_sign over the EIP-712 typed-data hash.
    EthSign,
    /// EIP-1271 contract signature.
    Eip1271,
    /// No off-chain signature; the order owner pre-signs on-chain via
    /// `GPv2Signing::setPreSignature`.
    PreSign,
}

impl SigningScheme {
    /// Project to [`EcdsaSigningScheme`] for the off-chain ECDSA variants.
    pub const fn try_to_ecdsa_scheme(self) -> Option<EcdsaSigningScheme> {
        match self {
            Self::Eip712 => Some(EcdsaSigningScheme::Eip712),
            Self::EthSign => Some(EcdsaSigningScheme::EthSign),
            Self::Eip1271 | Self::PreSign => None,
        }
    }
}

/// Subset of [`SigningScheme`] that uses off-chain ECDSA signatures.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EcdsaSigningScheme {
    /// EIP-712 typed-data signature over the order struct hash.
    Eip712,
    /// EIP-191 personal-sign wrapping the EIP-712 typed-data hash.
    EthSign,
}

impl From<EcdsaSigningScheme> for SigningScheme {
    fn from(scheme: EcdsaSigningScheme) -> Self {
        match scheme {
            EcdsaSigningScheme::Eip712 => Self::Eip712,
            EcdsaSigningScheme::EthSign => Self::EthSign,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_form_is_lowercase() {
        for (scheme, expected) in [
            (SigningScheme::Eip712, "\"eip712\""),
            (SigningScheme::EthSign, "\"ethsign\""),
            (SigningScheme::Eip1271, "\"eip1271\""),
            (SigningScheme::PreSign, "\"presign\""),
        ] {
            let serialised = serde_json::to_string(&scheme).unwrap();
            assert_eq!(serialised, expected);
            let parsed: SigningScheme = serde_json::from_str(expected).unwrap();
            assert_eq!(parsed, scheme);
        }
    }
}
