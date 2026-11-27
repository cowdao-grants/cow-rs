//! The signed order payload (`OrderData`) and supporting types.
//!
//! `OrderData` is the exact struct that is hashed and signed by the order
//! owner and verified by the GPv2Settlement contract. The other types in
//! this module ([`OrderKind`], [`SellTokenSource`], [`BuyTokenDestination`]
//! and [`OrderUid`]) exist to make the payload typeable and the resulting
//! identifier addressable.
//!
//! Adapted from [`cowprotocol/services`] (MIT OR Apache-2.0).
//!
//! [`cowprotocol/services`]: https://github.com/cowprotocol/services/blob/main/crates/model/src/order.rs

use alloy_primitives::{Address, B256, U256, b256};
use serde::{Deserialize, Serialize};
use serde_with::{DisplayFromStr, serde_as};
use std::fmt::{self, Display};

use crate::app_data::AppDataHash;
use crate::domain::DomainSeparator;

/// Private `sol!` view of [`OrderData`] used to derive the EIP-712
/// `typeHash` and `hashStruct` via [`alloy_sol_types::SolStruct`]. Lives
/// in a sub-module so the generated `pub struct Order` is not part of
/// the public API: callers should not have to know about it. The
/// Solidity name `Order` is load-bearing: it appears verbatim in the
/// EIP-712 type string that `GPv2Settlement` verifies.
///
/// `kind`, `sellTokenBalance` and `buyTokenBalance` are declared `string`
/// here even though the on-chain [`crate::contracts::GPv2OrderData`]
/// stores them as `bytes32`: the EIP-712 type string the contract
/// verifies against uses `string`, and the resulting 32-byte slots are
/// identical because the bytes32 values are pre-computed `keccak256` of
/// the same strings.
#[doc(hidden)]
pub mod eip712 {
    use alloy_sol_types::sol;

    sol! {
        #[derive(Debug)]
        struct Order {
            address sellToken;
            address buyToken;
            address receiver;
            uint256 sellAmount;
            uint256 buyAmount;
            uint32 validTo;
            bytes32 appData;
            uint256 feeAmount;
            string kind;
            bool partiallyFillable;
            string sellTokenBalance;
            string buyTokenBalance;
        }
    }
}

/// Sentinel buy token meaning the order pays out in the chain's
/// native currency (ETH on mainnet, xDAI on Gnosis, etc.).
pub const BUY_ETH_ADDRESS: Address = Address::repeat_byte(0xee);

/// The 12 fields signed by the order owner and verified by
/// `GPv2Settlement`. Mirrors [`GPv2Order.Data`].
///
/// [`GPv2Order.Data`]: https://github.com/cowprotocol/contracts/blob/v1.1.2/src/contracts/libraries/GPv2Order.sol
#[serde_as]
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderData {
    /// Token the owner is selling.
    pub sell_token: Address,
    /// Token the owner is buying.
    pub buy_token: Address,
    /// `None` means the owner receives the buy token. `Some(Address::ZERO)`
    /// is semantically equal to `None`, but raw `OrderData` you build and
    /// sign yourself is not normalised: the `Some(ZERO)` to `None` collapse
    /// only happens when you go through the orderbook crate's
    /// `OrderCreation::new`.
    #[serde(default)]
    pub receiver: Option<Address>,
    /// Atomic units of `sell_token`.
    #[serde_as(as = "DisplayFromStr")]
    pub sell_amount: U256,
    /// Atomic units of `buy_token`.
    #[serde_as(as = "DisplayFromStr")]
    pub buy_amount: U256,
    /// Unix seconds.
    pub valid_to: u32,
    /// 32-byte digest of the app-data document.
    pub app_data: AppDataHash,
    /// Protocol fee in `sell_token` atomic units. Zero for limit and
    /// liquidity orders (fee taken from surplus).
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
}

impl OrderData {
    /// EIP-712 `hashStruct` over the order. Delegates to
    /// [`alloy_sol_types::SolStruct`] applied to the private
    /// `eip712::Order` declaration, whose `typeHash` and field encoding
    /// are conformance-locked against the canonical contract type string.
    pub fn hash_struct(&self) -> B256 {
        use alloy_sol_types::SolStruct;
        eip712::Order::from(self).eip712_hash_struct()
    }

    /// 56-byte order UID on `domain` for `owner`.
    pub fn uid(&self, domain: &DomainSeparator, owner: Address) -> OrderUid {
        use alloy_sol_types::SolStruct;
        let signing_hash = eip712::Order::from(self).eip712_signing_hash(domain);
        OrderUid::from_parts(signing_hash, owner, self.valid_to)
    }

    /// Sign with an ECDSA signer; equivalent to
    /// [`crate::signature::sign_ecdsa`] applied to the EIP-712 view of
    /// this order.
    ///
    /// Requires a [`SignerSync`](alloy_signer::SignerSync) signer, which
    /// in practice means a raw local key
    /// (`alloy_signer_local::PrivateKeySigner`). Production hardware,
    /// remote or KMS signers implement the async
    /// [`alloy_signer::Signer`] trait instead, so use [`Self::sign_async`]
    /// or the [`Self::signing_hash`] digest-and-lift recipe for those.
    pub fn sign<S: alloy_signer::SignerSync>(
        &self,
        scheme: crate::signing_scheme::EcdsaSigningScheme,
        domain: &DomainSeparator,
        signer: &S,
    ) -> Result<crate::signature::Signature, crate::signature::SignatureError> {
        let ecdsa = self.sign_ecdsa(scheme, domain, signer)?;
        Ok(crate::signature::Signature::from_ecdsa(ecdsa, scheme))
    }

    /// Sign with an ECDSA signer and return the raw
    /// [`crate::signature::EcdsaSignature`] (`r || s || v`). Useful for
    /// callers that want the unwrapped signature components without
    /// promoting through the [`crate::signature::Signature`] enum.
    ///
    /// Like [`Self::sign`], requires a
    /// [`SignerSync`](alloy_signer::SignerSync) signer; production
    /// async signers should use [`Self::sign_ecdsa_async`].
    pub fn sign_ecdsa<S: alloy_signer::SignerSync>(
        &self,
        scheme: crate::signing_scheme::EcdsaSigningScheme,
        domain: &DomainSeparator,
        signer: &S,
    ) -> Result<crate::signature::EcdsaSignature, crate::signature::SignatureError> {
        let payload = eip712::Order::from(self);
        crate::signature::sign_ecdsa(scheme, domain, &payload, signer)
    }

    /// Async counterpart to [`Self::sign`], bound on the async
    /// [`alloy_signer::Signer`] trait rather than
    /// [`SignerSync`](alloy_signer::SignerSync). Equivalent to
    /// [`crate::signature::sign_ecdsa_async`] applied to the EIP-712 view
    /// of this order, then lifted into the scheme-tagged
    /// [`Signature`](crate::signature::Signature) enum. Prefer this for
    /// hardware, remote or KMS signers, which implement only the async
    /// trait.
    pub async fn sign_async<S: alloy_signer::Signer>(
        &self,
        scheme: crate::signing_scheme::EcdsaSigningScheme,
        domain: &DomainSeparator,
        signer: &S,
    ) -> Result<crate::signature::Signature, crate::signature::SignatureError> {
        let ecdsa = self.sign_ecdsa_async(scheme, domain, signer).await?;
        Ok(crate::signature::Signature::from_ecdsa(ecdsa, scheme))
    }

    /// Async counterpart to [`Self::sign_ecdsa`], bound on the async
    /// [`alloy_signer::Signer`] trait. Returns the raw
    /// [`crate::signature::EcdsaSignature`] (`r || s || v`) for callers
    /// that do not want the scheme-tagged
    /// [`Signature`](crate::signature::Signature) enum.
    pub async fn sign_ecdsa_async<S: alloy_signer::Signer>(
        &self,
        scheme: crate::signing_scheme::EcdsaSigningScheme,
        domain: &DomainSeparator,
        signer: &S,
    ) -> Result<crate::signature::EcdsaSignature, crate::signature::SignatureError> {
        let payload = eip712::Order::from(self);
        crate::signature::sign_ecdsa_async(scheme, domain, &payload, signer).await
    }

    /// Recover the address that signed this order from a scheme-tagged
    /// [`Signature`](crate::signature::Signature), the symmetric
    /// counterpart to [`Self::sign`]. Builds the EIP-712 payload
    /// internally, so callers never touch the hidden `eip712::Order`
    /// type. Returns `Ok(None)` for the on-chain
    /// [`Eip1271`](crate::signature::Signature::Eip1271) /
    /// [`PreSign`](crate::signature::Signature::PreSign) schemes, which
    /// carry their owner explicitly rather than deriving it.
    pub fn recover_signer(
        &self,
        domain: &DomainSeparator,
        signature: &crate::signature::Signature,
    ) -> Result<Option<crate::signature::Recovered>, crate::signature::SignatureError> {
        signature.recover(domain, &eip712::Order::from(self))
    }

    /// Recover the signer of a raw
    /// [`EcdsaSignature`](crate::signature::EcdsaSignature) over this
    /// order under `scheme`, the counterpart to [`Self::sign_ecdsa`].
    pub fn recover_ecdsa(
        &self,
        scheme: crate::signing_scheme::EcdsaSigningScheme,
        domain: &DomainSeparator,
        signature: &crate::signature::EcdsaSignature,
    ) -> Result<crate::signature::Recovered, crate::signature::SignatureError> {
        crate::signature::ecdsa_recover(signature, scheme, domain, &eip712::Order::from(self))
    }

    /// The exact 32-byte message a signer signs for this order under
    /// `scheme`: the EIP-712 typed-data hash for
    /// [`Eip712`](crate::signing_scheme::EcdsaSigningScheme::Eip712), or
    /// that hash wrapped in the EIP-191 personal-sign envelope for
    /// [`EthSign`](crate::signing_scheme::EcdsaSigningScheme::EthSign).
    /// Hand this to an external or async signer (hardware wallet, KMS,
    /// injected provider), then lift the result back with
    /// [`Signature::from_ecdsa`](crate::signature::Signature::from_ecdsa).
    pub fn signing_hash(
        &self,
        scheme: crate::signing_scheme::EcdsaSigningScheme,
        domain: &DomainSeparator,
    ) -> B256 {
        crate::signature::signing_message(scheme, domain, &eip712::Order::from(self))
    }
}

impl From<&OrderData> for eip712::Order {
    fn from(d: &OrderData) -> Self {
        Self {
            sellToken: d.sell_token,
            buyToken: d.buy_token,
            receiver: d.receiver.unwrap_or(Address::ZERO),
            sellAmount: d.sell_amount,
            buyAmount: d.buy_amount,
            validTo: d.valid_to,
            appData: d.app_data,
            feeAmount: d.fee_amount,
            kind: d.kind.as_str().to_owned(),
            partiallyFillable: d.partially_fillable,
            sellTokenBalance: d.sell_token_balance.as_str().to_owned(),
            buyTokenBalance: d.buy_token_balance.as_str().to_owned(),
        }
    }
}

/// Build the `{ "name": .., "type": .. }` entries of the EIP-712 `Order`
/// type from the canonical [`eip712::Order`] `sol!` declaration, so the
/// typed-data table cannot silently drift from the struct the contract
/// verifies against. Parses [`alloy_sol_types::SolStruct::eip712_root_type`],
/// which is `Order(<solType> <fieldName>,..)`.
fn order_type_entries() -> Vec<serde_json::Value> {
    use alloy_sol_types::SolStruct;
    let root = <eip712::Order as SolStruct>::eip712_root_type();
    let fields = root
        .strip_prefix("Order(")
        .and_then(|s| s.strip_suffix(')'))
        .expect("canonical Order root type is `Order(...)`");
    fields
        .split(',')
        .map(|field| {
            let (sol_type, name) = field
                .split_once(' ')
                .expect("each EIP-712 field is `<type> <name>`");
            serde_json::json!({ "name": name, "type": sol_type })
        })
        .collect()
}

/// Canonical EIP-712 typed-data payload for an order, ready to feed into
/// viem's `signTypedData` or ethers' `signer.signTypedData`. This is the
/// single source of truth the wasm layer reuses instead of hand-redeclaring
/// the `Order` type table.
///
/// Returns `{ domain, primaryType, types, message }`. The `message`
/// normalises an absent or null `receiver` to `address(0)` so the hash a
/// wallet signs matches what [`OrderData::hash_struct`] computes. The
/// domain `name` / `version` come from [`crate::domain::DOMAIN_NAME`] /
/// [`crate::domain::DOMAIN_VERSION`], the same constants
/// [`crate::domain::settlement_domain`] derives the separator from, so the
/// typed-data domain and the separator cannot drift.
///
/// `types` deliberately omits the `EIP712Domain` entry: ethers v6 and viem
/// build the domain typedef from the `domain` object and throw on a
/// duplicate. Raw `eth_signTypedData_v4` callers must inject it themselves.
pub fn order_typed_data(
    order: &OrderData,
    chain_id: u64,
    verifying_contract: Address,
) -> serde_json::Value {
    let mut message = serde_json::to_value(order).expect("OrderData serialises to JSON");
    // A null or absent receiver hashes as address(0); make that explicit.
    if message
        .get("receiver")
        .is_none_or(serde_json::Value::is_null)
    {
        message["receiver"] = serde_json::Value::String(Address::ZERO.to_string());
    }
    serde_json::json!({
        "domain": {
            "name": crate::domain::DOMAIN_NAME,
            "version": crate::domain::DOMAIN_VERSION,
            "chainId": chain_id,
            "verifyingContract": verifying_contract.to_string(),
        },
        "primaryType": "Order",
        "types": { "Order": order_type_entries() },
        "message": message,
    })
}

/// Order direction.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderKind {
    /// Owner fixes the `buy_token` amount.
    #[default]
    Buy,
    /// Owner fixes the `sell_token` amount.
    Sell,
}

impl OrderKind {
    /// `keccak256("buy")`: EIP-712 encoding + on-chain `bytes32` marker.
    pub const BUY: B256 = b256!("6ed88e868af0a1983e3886d5f3e95a2fafbd6c3450bc229e27342283dc429ccc");
    /// `keccak256("sell")`: EIP-712 encoding + on-chain `bytes32` marker.
    pub const SELL: B256 =
        b256!("f3b277728b3fee749481eb3e0b3b48980dbbab78658fc419025cb16eee346775");

    /// Lower-case wire form (`"buy"` / `"sell"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }

    /// Parse the 32-byte on-chain marker (as returned by `GPv2Order.Data.kind`)
    /// into a Rust enum. Returns `None` for unknown markers; the contract
    /// itself only ever writes `BUY` or `SELL`. Compares against the
    /// precomputed [`Self::BUY`] / [`Self::SELL`] constants (locked to
    /// `keccak256(as_str())` by `order_kind_keccak_constants`), so a lookup
    /// does no per-call hashing.
    pub fn from_contract_bytes(bytes: B256) -> Option<Self> {
        if bytes == Self::BUY {
            Some(Self::Buy)
        } else if bytes == Self::SELL {
            Some(Self::Sell)
        } else {
            None
        }
    }
}

impl Display for OrderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Source the sell amount is transferred from on fulfilment.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SellTokenSource {
    /// Owner's ERC-20 allowance to the Vault relayer.
    #[default]
    Erc20,
    /// Owner's ERC-20 balance via Balancer external balances.
    External,
    /// Owner's Balancer Vault internal balance.
    Internal,
}

impl SellTokenSource {
    /// `keccak256("erc20")`.
    pub const ERC20: B256 =
        b256!("5a28e9363bb942b639270062aa6bb295f434bcdfc42c97267bf003f272060dc9");
    /// `keccak256("external")`.
    pub const EXTERNAL: B256 =
        b256!("abee3b73373acd583a130924aad6dc38cfdc44ba0555ba94ce2ff63980ea0632");
    /// `keccak256("internal")`.
    pub const INTERNAL: B256 =
        b256!("4ac99ace14ee0a5ef932dc609df0943ab7ac16b7583634612f8dc35a4289a6ce");

    /// Lower-case wire form (`"erc20"` / `"external"` / `"internal"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Erc20 => "erc20",
            Self::External => "external",
            Self::Internal => "internal",
        }
    }

    /// Parse the on-chain `GPv2Order.Data.sellTokenBalance` marker.
    /// Returns `None` for unknown values. Compares against the precomputed
    /// [`Self::ERC20`] / [`Self::EXTERNAL`] / [`Self::INTERNAL`] constants
    /// (locked to `keccak256(as_str())` by `sell_token_source_keccak_constants`),
    /// so a lookup does no per-call hashing.
    pub fn from_contract_bytes(bytes: B256) -> Option<Self> {
        if bytes == Self::ERC20 {
            Some(Self::Erc20)
        } else if bytes == Self::EXTERNAL {
            Some(Self::External)
        } else if bytes == Self::INTERNAL {
            Some(Self::Internal)
        } else {
            None
        }
    }
}

/// Destination the buy amount is paid to on fulfilment.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuyTokenDestination {
    /// Regular ERC-20 transfer.
    #[default]
    Erc20,
    /// Balancer Vault internal balance.
    Internal,
}

impl BuyTokenDestination {
    /// `keccak256("erc20")`. Identical to [`SellTokenSource::ERC20`]: both
    /// markers are `keccak256("erc20")`, so this aliases that constant
    /// rather than re-pasting the literal.
    pub const ERC20: B256 = SellTokenSource::ERC20;
    /// `keccak256("internal")`. Identical to [`SellTokenSource::INTERNAL`];
    /// aliases that constant rather than re-pasting the literal.
    pub const INTERNAL: B256 = SellTokenSource::INTERNAL;

    /// Lower-case wire form (`"erc20"` / `"internal"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Erc20 => "erc20",
            Self::Internal => "internal",
        }
    }

    /// Parse the on-chain `GPv2Order.Data.buyTokenBalance` marker.
    /// Returns `None` for unknown values. Compares against the precomputed
    /// [`Self::ERC20`] / [`Self::INTERNAL`] constants (locked to
    /// `keccak256(as_str())` by `buy_token_destination_keccak_constants`), so
    /// a lookup does no per-call hashing.
    pub fn from_contract_bytes(bytes: B256) -> Option<Self> {
        if bytes == Self::ERC20 {
            Some(Self::Erc20)
        } else if bytes == Self::INTERNAL {
            Some(Self::Internal)
        } else {
            None
        }
    }
}

// Order identifiers and the server-side classification live in the
// primitives crate (they are pure wire types shared by crates that do
// not need the signing stack); re-exported here so the canonical
// `order::OrderUid` / `order::OrderClass` paths keep working.
pub use cowprotocol_primitives::order_id::{
    OrderClass, OrderUid, OrderUidParseError, OrderUidParts, parse_order_uid,
};

#[cfg(test)]
mod order_signing_tests;

#[cfg(test)]
mod tests {
    use alloy_primitives::{address, keccak256};
    use hex_literal::hex;
    use std::str::FromStr;

    use super::*;

    use crate::contracts::GPV2_SETTLEMENT as SETTLEMENT;

    /// Build the canonical sample order shared with the cross-chain golden
    /// vectors generated by `tools/vector-gen`.
    fn sample_order() -> OrderData {
        OrderData {
            sell_token: Address::from(hex!("0101010101010101010101010101010101010101")),
            buy_token: Address::from(hex!("0202020202020202020202020202020202020202")),
            receiver: Some(Address::from(hex!(
                "0303030303030303030303030303030303030303"
            ))),
            sell_amount: U256::from(0x0246ddf97976680000_u128),
            buy_amount: U256::from(0xb98bc829a6f90000_u128),
            valid_to: 0xffffffff,
            app_data: AppDataHash::default(),
            fee_amount: U256::from(0x0de0b6b3a7640000_u128),
            kind: OrderKind::Sell,
            partially_fillable: false,
            sell_token_balance: SellTokenSource::Erc20,
            buy_token_balance: BuyTokenDestination::Erc20,
        }
    }

    /// Sample-order owner, also shared with `tools/vector-gen`.
    fn sample_owner() -> Address {
        address!("70997970C51812dc3A010C7d01b50e0d17dc79C8")
    }

    // The `compute_order_uid_matches_services_golden` test that lived
    // here was redundant with `cross_chain_uids_match_ethers` below and
    // baked in a synthetic raw-`B256` separator that no longer fits the
    // `DomainSeparator = Eip712Domain` alias. The cross-chain UID
    // pipeline lock is the load-bearing conformance gate.

    /// Locks `hash_struct` against the value produced by ethers
    /// `TypedDataEncoder.hashStruct("Order", types, sample_order)`.
    /// The value is identical across chains because the EIP-712 struct hash
    /// does not depend on the domain. Regenerate via `tools/vector-gen`.
    #[test]
    fn sample_order_struct_hash_matches_ethers() {
        assert_eq!(
            sample_order().hash_struct(),
            b256!("7d9bf070168f9950003bdad00194ef63a5389dd0b594a1288407d551abf147d5")
        );
    }

    /// Locks `sign_ecdsa` against the golden vector produced by
    /// ethers `Wallet.signTypedData` for the mainnet `sample_order` +
    /// the Hardhat #0 account. Regenerate via `tools/vector-gen`.
    /// Catches drift between alloy's signer and ethers' signer (which
    /// a round-trip-only test cannot, since both sides would drift
    /// together).
    #[test]
    fn eip712_signature_matches_ethers_golden() {
        use crate::signature::sign_ecdsa;
        use crate::signing_scheme::EcdsaSigningScheme;
        use alloy_signer_local::PrivateKeySigner;

        // Hardhat account #0; same key the vector-gen tool uses.
        let private_key = B256::from(hex!(
            "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
        ));
        let signer = PrivateKeySigner::from_bytes(&private_key).unwrap();
        let domain = crate::domain::settlement_domain(1, SETTLEMENT);
        let payload = eip712::Order::from(&sample_order());

        let ecdsa = sign_ecdsa(EcdsaSigningScheme::Eip712, &domain, &payload, &signer).unwrap();

        // Expected (r, s, v) from ethers Wallet.signTypedData on the same
        // inputs. v=28 (the high-order normalised form). Bytes layout
        // is `r || s || v` per alloy's `Signature::as_bytes`.
        let expected_r = hex!("78bd3f7f240eb91bf94264f1bab99a5efaf97e8c76b9f76eeb4520f46861ed13");
        let expected_s = hex!("70c2f3362f17d4668a02ad82f61bff52bd33a785afeff727ddab43210dfebea2");
        let bytes = ecdsa.as_bytes();
        assert_eq!(&bytes[..32], &expected_r, "r component");
        assert_eq!(&bytes[32..64], &expected_s, "s component");
        assert_eq!(bytes[64], 28, "v component");
    }

    /// On-chain schemes carry the owner explicitly, so `recover_signer`
    /// yields `None` rather than deriving an address.
    #[test]
    fn recover_signer_is_none_for_onchain_schemes() {
        let domain = crate::domain::settlement_domain(1, SETTLEMENT);
        let order = sample_order();
        assert!(
            order
                .recover_signer(&domain, &crate::signature::Signature::pre_sign())
                .unwrap()
                .is_none()
        );
    }

    /// Locks the [`eip712::Order`] `SolStruct::eip712_type_hash` derivation
    /// against the canonical
    /// `keccak256("Order(address sellToken,..,string buyTokenBalance)")`
    /// value the GPv2Settlement contract verifies signatures with. Any
    /// drift in the `sol!` field types or order would diverge from the
    /// contract's `typeHash` and silently invalidate every signature.
    #[test]
    fn eip712_order_type_hash_matches_canonical() {
        use alloy_sol_types::SolStruct;

        let sol_order = eip712::Order::from(&sample_order());
        assert_eq!(
            <eip712::Order as SolStruct>::eip712_type_hash(&sol_order).0,
            hex!("d5a25ba2e97094ad7d83dc28a6572da797d6b3e7fc6663bd93efb789fc17e489"),
            "Order(...) typeHash must match the GPv2Settlement-verified value",
        );
        // The exported constant must equal the derived type hash.
        assert_eq!(
            crate::contracts::GPV2_ORDER_TYPE_HASH,
            <eip712::Order as SolStruct>::eip712_type_hash(&sol_order),
        );
    }

    /// Locks the `settlement_domain` separator against ethers
    /// `TypedDataEncoder.hashDomain` for every one of the ten chains
    /// cow-rs supports, using the canonical GPv2Settlement deployment.
    /// Regenerate via `tools/vector-gen`.
    #[test]
    fn cross_chain_domain_separators_match_ethers() {
        let cases: [(u64, [u8; 32]); 10] = [
            (
                1,
                hex!("c078f884a2676e1345748b1feace7b0abee5d00ecadb6e574dcdd109a63e8943"),
            ),
            (
                56,
                hex!("0cbb18dfca28d2ceac8c72a17289168e03c1ad121338f5573e3b0c3255207fc7"),
            ),
            (
                100,
                hex!("8f05589c4b810bc2f706854508d66d447cd971f8354a4bb0b3471ceb0a466bc7"),
            ),
            (
                137,
                hex!("132e0e39721b0cb53216fc42764f69c300d4d21e0caf24e0713b1e3e11120dc2"),
            ),
            (
                8453,
                hex!("d72ffa789b6fae41254d0b5a13e6e1e92ed947ec6a251edf1cf0b6c02c257b4b"),
            ),
            (
                9745,
                hex!("e1f9c97768e45812440cd3317c07069178cc2f69971fb204c0211d8bfb1f8e76"),
            ),
            (
                42161,
                hex!("69d78e7a7cafcaf924483f99f65e8f4e303a99a446db7ab319f9d40e940bced2"),
            ),
            (
                43114,
                hex!("81fd4ff99b8f80b96c946c146cd5b79181aaf08ecb5808eeee1d047c1de267a5"),
            ),
            (
                59144,
                hex!("b219bb2b8733b80b7ebef0229e7f0c91436f9a0a5b9705fa519237ae0493addb"),
            ),
            (
                11_155_111,
                hex!("daee378bd0eb30ddf479272accf91761e697bc00e067a268f95f1d2732ed230b"),
            ),
        ];

        for (chain_id, expected) in cases {
            let separator = crate::domain::settlement_domain(chain_id, SETTLEMENT).separator();
            assert_eq!(separator, expected, "domain separator for chain {chain_id}");
        }
    }

    /// Locks the full UID pipeline against ethers
    /// `TypedDataEncoder.hash(domain, types, sample_order)` packed with the
    /// sample owner and `validTo`, for every chain cow-rs supports.
    /// Regenerate via `tools/vector-gen`.
    #[test]
    fn cross_chain_uids_match_ethers() {
        const TAIL: [u8; 24] = hex!("70997970c51812dc3a010c7d01b50e0d17dc79c8ffffffff");
        let cases: [(u64, [u8; 32]); 10] = [
            (
                1,
                hex!("8295b35c74972663a29a02be0fa8de8a157215b36938caa461fdf183e02cd82e"),
            ),
            (
                56,
                hex!("f676f63e14dc6a9da6bbe9e57398f060a2cc24d79e12345709ac15c8e4f5b8c1"),
            ),
            (
                100,
                hex!("3dee66b2accacd71dd607b281d1485ef960c37beff85374f4b7c65eb05ed1252"),
            ),
            (
                137,
                hex!("f2a78d43cf0922ef45e56a7f36d7eba13a8eb9407c1dc59e087322388e622fbb"),
            ),
            (
                8453,
                hex!("28862a0c28aab4b8a4403fdf5cfd71686e7dd665db1469ec7f84bf45d1a3dd9b"),
            ),
            (
                9745,
                hex!("f7941fdf92e8d5b4815995973c1cdf0e58cbe3f404ba4a8f672da5a951832e4c"),
            ),
            (
                42161,
                hex!("c5677211ea383a13f4d47c092fc48fb3a0a5ade451c82f19dff69a400080f34b"),
            ),
            (
                43114,
                hex!("310880582f800792d606e89b94c8f23529469003cf71da6aa172737702b8a4be"),
            ),
            (
                59144,
                hex!("5aa640f484ef090ab25171d6cbc6adff0cbda7a342ed7ef6371636a1575eca40"),
            ),
            (
                11_155_111,
                hex!("d69c063b99b74a6690df5541787acc942828219a0ba12fded27eff853da8f6fd"),
            ),
        ];

        let order = sample_order();
        let owner = sample_owner();
        for (chain_id, expected_digest) in cases {
            let domain = crate::domain::settlement_domain(chain_id, SETTLEMENT);
            let uid = order.uid(&domain, owner);
            let mut expected = [0u8; 56];
            expected[0..32].copy_from_slice(&expected_digest);
            expected[32..56].copy_from_slice(&TAIL);
            assert_eq!(uid.0, expected, "uid for chain {chain_id}");
        }
    }

    /// Locks `OrderData::hash_struct` against ethers for permutations
    /// that exercise specific byte slots: `kind = Buy`,
    /// `partiallyFillable = true`, the three `SellTokenSource` variants,
    /// the two `BuyTokenDestination` variants, and `receiver = None`.
    /// Regenerate via `tools/vector-gen`.
    #[test]
    fn hash_struct_byte_permutations_match_ethers() {
        let mut buy = sample_order();
        buy.kind = OrderKind::Buy;
        assert_eq!(
            buy.hash_struct(),
            b256!("7f6ff8bfee1c5f54ca8ac13dabf84e6646592775700fce0e5ead7049620f9ea5")
        );

        let mut partial = sample_order();
        partial.partially_fillable = true;
        assert_eq!(
            partial.hash_struct(),
            b256!("4a7892b4e3cc787cc8dbb22afb249a52b144ae7aec066d2f41f521aa05c7388c")
        );

        let mut external = sample_order();
        external.sell_token_balance = SellTokenSource::External;
        assert_eq!(
            external.hash_struct(),
            b256!("250972eafa5a01e4103f50f3987422339582583b36d2a47e3c6920b4acca3509")
        );

        let mut internal_sell = sample_order();
        internal_sell.sell_token_balance = SellTokenSource::Internal;
        assert_eq!(
            internal_sell.hash_struct(),
            b256!("c94d0a2b1c1b41042d41e0d9f2d05bc91fbe1cb053b716176850029cdb88f679")
        );

        let mut internal_buy = sample_order();
        internal_buy.buy_token_balance = BuyTokenDestination::Internal;
        assert_eq!(
            internal_buy.hash_struct(),
            b256!("4d19213af5ed0adb5ec3d67b00cdcd360ea0f9378a9392f599de132106a558d9")
        );

        // None-receiver should encode the same 20 zero bytes that an
        // explicit Address::ZERO does.
        let mut no_receiver = sample_order();
        no_receiver.receiver = None;
        let mut zero_receiver = sample_order();
        zero_receiver.receiver = Some(Address::ZERO);
        assert_eq!(no_receiver.hash_struct(), zero_receiver.hash_struct());
        assert_eq!(
            no_receiver.hash_struct(),
            b256!("5388e8a0f9cf9129fd0fd54d3192e502cf5519ee4316f0c77860bfc0c3f42994")
        );
    }

    /// Locks the [`eip712::Order`] `typeHash` against the canonical
    /// EIP-712 type signature published in
    /// [`GPv2Order.sol`](https://github.com/cowprotocol/contracts/blob/main/src/contracts/libraries/GPv2Order.sol).
    /// Note that `kind`, `sellTokenBalance` and `buyTokenBalance` are typed
    /// as `string` in the EIP-712 schema even though `GPv2Order.Data` stores
    /// them as `bytes32` markers.
    #[test]
    fn order_type_hash_matches_canonical_signature() {
        use alloy_sol_types::SolStruct;

        let signature = b"Order(\
            address sellToken,\
            address buyToken,\
            address receiver,\
            uint256 sellAmount,\
            uint256 buyAmount,\
            uint32 validTo,\
            bytes32 appData,\
            uint256 feeAmount,\
            string kind,\
            bool partiallyFillable,\
            string sellTokenBalance,\
            string buyTokenBalance\
        )";
        let sol_order = eip712::Order::from(&sample_order());
        assert_eq!(
            <eip712::Order as SolStruct>::eip712_type_hash(&sol_order),
            keccak256(signature),
        );
    }

    /// Locks the typed-data `types`."Order" table built by
    /// [`order_typed_data`] against the hardcoded 12-field GPv2 order
    /// type, in declaration order. Hardcoded rather than re-derived
    /// from `eip712_root_type()` so the table and the production
    /// parser cannot drift together: any change to the `sol!` struct,
    /// the parser, or the JSON shaping trips this independently. The
    /// same canonical type string is hash-locked by
    /// `order_type_hash_matches_canonical_signature`.
    #[test]
    fn order_typed_data_table_matches_canonical_gpv2_fields() {
        let expected: [(&str, &str); 12] = [
            ("sellToken", "address"),
            ("buyToken", "address"),
            ("receiver", "address"),
            ("sellAmount", "uint256"),
            ("buyAmount", "uint256"),
            ("validTo", "uint32"),
            ("appData", "bytes32"),
            ("feeAmount", "uint256"),
            ("kind", "string"),
            ("partiallyFillable", "bool"),
            ("sellTokenBalance", "string"),
            ("buyTokenBalance", "string"),
        ];

        let typed = order_typed_data(&sample_order(), 1, SETTLEMENT);
        let table = typed["types"]["Order"].as_array().unwrap();
        assert_eq!(table.len(), expected.len());
        for (entry, (name, ty)) in table.iter().zip(expected) {
            assert_eq!(entry["name"], name, "field name");
            assert_eq!(entry["type"], ty, "field solidity type");
        }
    }

    /// Pins the full [`order_typed_data`] envelope: the domain reuses the
    /// `settlement_domain` constants, `verifyingContract` is the EIP-55
    /// checksummed (mixed-case) address string, and a `None` receiver is
    /// materialised as `address(0)` in the message.
    #[test]
    fn order_typed_data_envelope_shape() {
        let mut order = sample_order();
        order.receiver = None;
        let typed = order_typed_data(&order, 1, SETTLEMENT);

        assert_eq!(typed["domain"]["name"], crate::domain::DOMAIN_NAME);
        assert_eq!(typed["domain"]["version"], crate::domain::DOMAIN_VERSION);
        assert_eq!(typed["domain"]["chainId"], 1);
        assert_eq!(typed["domain"]["verifyingContract"], SETTLEMENT.to_string());
        assert_eq!(typed["primaryType"], "Order");
        // EIP712Domain is intentionally absent from `types`.
        assert!(typed["types"].get("EIP712Domain").is_none());
        // Absent receiver materialises as address(0).
        assert_eq!(typed["message"]["receiver"], Address::ZERO.to_string());
    }

    #[test]
    fn buy_eth_address_matches_canonical_sentinel() {
        // Source: cowprotocol/contracts/src/ts/order.ts (BUY_ETH_ADDRESS).
        assert_eq!(
            BUY_ETH_ADDRESS,
            address!("EeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE")
        );
    }

    #[test]
    fn order_kind_keccak_constants() {
        assert_eq!(OrderKind::BUY, keccak256(b"buy"));
        assert_eq!(OrderKind::SELL, keccak256(b"sell"));
    }

    #[test]
    fn sell_token_source_keccak_constants() {
        assert_eq!(SellTokenSource::ERC20, keccak256(b"erc20"));
        assert_eq!(SellTokenSource::EXTERNAL, keccak256(b"external"));
        assert_eq!(SellTokenSource::INTERNAL, keccak256(b"internal"));
    }

    #[test]
    fn buy_token_destination_keccak_constants() {
        assert_eq!(BuyTokenDestination::ERC20, keccak256(b"erc20"));
        assert_eq!(BuyTokenDestination::INTERNAL, keccak256(b"internal"));
    }

    #[test]
    fn order_uid_round_trips_via_string() {
        let original = OrderUid::from_str(
            "0x5668997bd3fb981d1b3ec44e8483e7c369756df47d10241c1c7a26fde4d1090e89984d17af2f18f8c54873c0de68a56cc5a23e0f695ba915",
        )
        .unwrap();
        let (hash, owner, valid_to) = original.to_parts();
        assert_eq!(
            hash,
            B256::from(hex!(
                "5668997bd3fb981d1b3ec44e8483e7c369756df47d10241c1c7a26fde4d1090e"
            ))
        );
        assert_eq!(
            owner,
            address!("0x89984d17af2f18f8c54873c0de68a56cc5a23e0f")
        );
        assert_eq!(valid_to, 0x695ba915);

        let rebuilt = OrderUid::from_parts(hash, owner, valid_to);
        assert_eq!(rebuilt, original);
    }

    #[test]
    fn parse_order_uid_requires_0x_prefix() {
        let body = "11".repeat(56);
        let prefixed = format!("0x{body}");
        // The strict helper accepts the canonical prefixed form.
        assert!(parse_order_uid(&prefixed).is_ok());
        // It rejects the unprefixed body, even though alloy's blanket
        // `FromStr` would accept it (documented divergence).
        assert!(matches!(
            parse_order_uid(&body),
            Err(OrderUidParseError::MissingPrefix)
        ));
        assert!(OrderUid::from_str(&body).is_ok());
        // A prefixed-but-malformed body still fails through the hex arm.
        assert!(matches!(
            parse_order_uid("0xnothex"),
            Err(OrderUidParseError::Hex(_))
        ));
    }

    #[test]
    fn order_uid_displays_as_prefixed_hex() {
        let mut uid = OrderUid::default();
        uid.0[0] = 0x01;
        uid.0[55] = 0xff;
        let expected = "0x01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ff";
        assert_eq!(uid.to_string(), expected);
    }

    /// Mirrors `packOrderUidParams` from
    /// `cowprotocol/contracts/test/GPv2Order/PackOrderUidParams.t.sol`,
    /// which derives `(digest, owner, validTo)` from keccak-256 of UTF-8
    /// constants. Locks the 56-byte packing layout `digest || owner || validTo`.
    #[test]
    fn order_uid_pack_matches_contracts_solidity_reference() {
        let digest = keccak256(b"order digest");
        let owner_seed = keccak256(b"owner");
        let owner = Address::from_slice(&owner_seed[12..32]);
        let valid_to_seed = keccak256(b"valid to");
        let valid_to = u32::from_be_bytes(valid_to_seed[28..32].try_into().unwrap());

        let uid = OrderUid::from_parts(digest, owner, valid_to);

        let mut expected = [0u8; 56];
        expected[0..32].copy_from_slice(digest.as_slice());
        expected[32..52].copy_from_slice(owner.as_slice());
        expected[52..56].copy_from_slice(&valid_to.to_be_bytes());
        assert_eq!(uid.0, expected);

        let (round_digest, round_owner, round_valid_to) = uid.to_parts();
        assert_eq!(round_digest, digest);
        assert_eq!(round_owner, owner);
        assert_eq!(round_valid_to, valid_to);
    }
}
