//! EIP-712 domain separator for CoW Protocol orders.
//!
//! The settlement contract identifies itself with the EIP-712 domain
//! `name = "Gnosis Protocol"`, `version = "v2"`. Combined with the
//! deployment's `chainId` and `verifyingContract` address it scopes
//! every signed order hash to a specific chain.
//!
//! [`DomainSeparator`] is a type alias for
//! [`alloy_sol_types::Eip712Domain`]; the typed-data envelope is
//! normally supplied by [`alloy_sol_types::SolStruct::eip712_signing_hash`]
//! and the EIP-191 personal-sign wrap by
//! [`alloy_primitives::eip191_hash_message`]. There is no
//! `\x19Ethereum Signed Message:\n32` helper in this crate. The one
//! `keccak256(0x19 0x01 || ...)` helper that does exist,
//! [`eip712_message_hash`], is the JS/wasm interop boundary: callers
//! that already hold a raw 32-byte separator and struct hash (rather
//! than a typed [`DomainSeparator`] + [`alloy_sol_types::SolStruct`])
//! can fold them into the final typed-data hash without rebuilding an
//! `Eip712Domain`.

use alloy_primitives::{Address, B256, U256, keccak256};
use alloy_sol_types::Eip712Domain;

/// EIP-712 domain `name` for the CoW Protocol settlement contract.
/// Load-bearing: it feeds the domain separator the `GPv2Settlement`
/// contract verifies signatures against. Shared with the typed-data
/// builder so the two cannot drift.
pub const DOMAIN_NAME: &str = "Gnosis Protocol";

/// EIP-712 domain `version` for the CoW Protocol settlement contract.
/// See [`DOMAIN_NAME`] for why it is centralised here.
pub const DOMAIN_VERSION: &str = "v2";

/// EIP-712 domain for the CoW Protocol settlement contract. Re-exported
/// as a type alias so callers using `cowprotocol::DomainSeparator` do
/// not need to import the underlying alloy type, while getting all of
/// `Eip712Domain`'s `Debug`, `Display`, `Eq`, `Hash`, `Serialize`,
/// `Deserialize`, and `separator()` derivations for free.
pub type DomainSeparator = Eip712Domain;

/// Build the EIP-712 domain for the `GPv2Settlement` deployment on a
/// given chain. The Solidity domain string is `"Gnosis Protocol" v2`;
/// `contract_address` is the deployed `GPv2Settlement` for that chain.
pub fn settlement_domain(chain_id: u64, contract_address: Address) -> DomainSeparator {
    Eip712Domain {
        name: Some(DOMAIN_NAME.into()),
        version: Some(DOMAIN_VERSION.into()),
        chain_id: Some(U256::from(chain_id)),
        verifying_contract: Some(contract_address),
        salt: None,
    }
}

/// EIP-712 typed-data hash `keccak256(0x19 || 0x01 || separator ||
/// struct_hash)`.
///
/// This is the JS/wasm interop boundary: callers that already hold a raw
/// 32-byte domain `separator` and `struct_hash` (for example a wallet
/// shim that produced them out-of-band) get the final signing hash
/// without having to rebuild a typed [`DomainSeparator`] and re-run
/// [`alloy_sol_types::SolStruct::eip712_signing_hash`]. Rust callers that
/// hold a typed [`DomainSeparator`] pass its [`Eip712Domain::separator`].
pub fn eip712_message_hash(separator: B256, struct_hash: B256) -> B256 {
    let mut buf = [0u8; 66];
    buf[..2].copy_from_slice(&[0x19, 0x01]);
    buf[2..34].copy_from_slice(separator.as_slice());
    buf[34..].copy_from_slice(struct_hash.as_slice());
    keccak256(buf)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::b256;

    use super::*;
    use crate::contracts::GPV2_SETTLEMENT;

    #[test]
    fn domain_separator_sepolia() {
        // Locks the EIP-712 domain separator against the canonical
        // GPv2Settlement deployment on Sepolia chain 11_155_111.
        // https://sepolia.etherscan.io/address/0x9008d19f58aabd9ed0d60971565aa8510560ab41
        let chain_id: u64 = 11_155_111;

        let domain = settlement_domain(chain_id, GPV2_SETTLEMENT);
        let expected = b256!("daee378bd0eb30ddf479272accf91761e697bc00e067a268f95f1d2732ed230b");

        assert_eq!(domain.separator(), expected);
    }
}
