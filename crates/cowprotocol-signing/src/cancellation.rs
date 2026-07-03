//! Off-chain order cancellation.
//!
//! The CoW orderbook exposes two cancel-by-UID flows:
//!
//! - **Single**: [`OrderCancellation`]: an unsigned `OrderCancellation(bytes orderUid)`
//!   EIP-712 struct; sign it into a [`SignedOrderCancellation`] for
//!   `DELETE /api/v1/orders/{uid}`.
//! - **Collection**: [`OrderCancellations`]: an unsigned
//!   `OrderCancellations(bytes[] orderUids)` EIP-712 struct; sign it into a
//!   [`SignedOrderCancellations`] to cancel many orders in one
//!   `DELETE /api/v1/orders` body.
//!
//! Both unsigned values follow the crate's build-then-`.sign(..)` idiom:
//! `<unsigned>.sign(scheme, &domain, &signer)`, so single and collection
//! cancellation read identically.
//!
//! Both flows are "soft": they remove the order from the matching pool
//! but cannot recall an order that is already in flight. For pre-signed
//! orders, cancellation is done on-chain via
//! `GPv2Settlement::setPreSignature(uid, false)`; for EthFlow orders, via
//! `EthFlow::invalidateOrder`. Those are out of scope for this module.
//!
//! Adapted from [`cowprotocol/services`] (MIT OR Apache-2.0).
//!
//! [`cowprotocol/services`]: https://github.com/cowprotocol/services/blob/main/crates/model/src/order.rs

use {
    crate::{
        domain::DomainSeparator,
        order::OrderUid,
        signature::{
            EcdsaSignature, SignatureError, ecdsa_recover, ecdsa_wire, sign_ecdsa, sign_ecdsa_async,
        },
        signing_scheme::EcdsaSigningScheme,
    },
    alloy_primitives::{Address, B256},
    serde::{Deserialize, Serialize},
};

/// Private `sol!` views of the two cancellation EIP-712 structs. Lives
/// in a sub-module so the generated `pub` types are not part of the
/// crate's public API.
///
/// The Solidity type names and field names are load-bearing: they
/// appear verbatim in the EIP-712 type string the contract verifies.
/// Single-cancel uses singular `orderUid`; the array variant uses
/// plural `orderUids`.
mod eip712 {
    use {super::OrderUid, alloy_primitives::Bytes, alloy_sol_types::sol};

    sol! {
        struct OrderCancellation {
            bytes orderUid;
        }

        struct OrderCancellations {
            bytes[] orderUids;
        }
    }

    /// The single-cancel EIP-712 payload for `uid`. The one place the
    /// `OrderUid` to `bytes orderUid` projection is written.
    pub(super) fn single(uid: &OrderUid) -> OrderCancellation {
        OrderCancellation {
            orderUid: Bytes::from(uid.0),
        }
    }

    /// The collection-cancel EIP-712 payload for `uids`. The one place
    /// the `&[OrderUid]` to `bytes[] orderUids` projection is written.
    pub(super) fn collection(uids: &[OrderUid]) -> OrderCancellations {
        OrderCancellations {
            orderUids: uids.iter().map(|u| Bytes::from(u.0)).collect(),
        }
    }
}

/// Unsigned cancellation of a single order.
///
/// The single-order counterpart to [`OrderCancellations`]. Wrap the
/// [`OrderUid`] you want to cancel, then call [`OrderCancellation::sign`]
/// to produce a [`SignedOrderCancellation`] for `DELETE /api/v1/orders/{uid}`.
/// This restores the build-an-unsigned-value-then-`.sign(..)` idiom every
/// other signable type in the crate follows, so single and collection
/// cancellation read identically.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderCancellation {
    /// UID of the order being cancelled.
    pub order_uid: OrderUid,
}

impl From<OrderUid> for OrderCancellation {
    fn from(order_uid: OrderUid) -> Self {
        Self { order_uid }
    }
}

impl OrderCancellation {
    /// EIP-712 `hashStruct` for the single-order cancellation type.
    /// Delegates to [`alloy_sol_types::SolStruct`] applied to the private
    /// `eip712::OrderCancellation` declaration.
    pub fn hash_struct(&self) -> B256 {
        use alloy_sol_types::SolStruct;
        eip712::single(&self.order_uid).eip712_hash_struct()
    }

    /// The exact 32-byte message a signer signs for this cancellation
    /// under `scheme`: the EIP-712 typed-data hash for
    /// [`Eip712`](crate::signing_scheme::EcdsaSigningScheme::Eip712), or
    /// that hash wrapped in the EIP-191 personal-sign envelope for
    /// [`EthSign`](crate::signing_scheme::EcdsaSigningScheme::EthSign).
    /// Hand this to an external or async signer (hardware wallet, KMS,
    /// injected provider), then lift the result back with
    /// [`Signature::from_ecdsa`](crate::signature::Signature::from_ecdsa).
    /// The cancellation counterpart to
    /// [`OrderData::signing_hash`](crate::order::OrderData::signing_hash).
    pub fn signing_hash(&self, scheme: EcdsaSigningScheme, domain: &DomainSeparator) -> B256 {
        crate::signature::signing_message(scheme, domain, &eip712::single(&self.order_uid))
    }

    /// Sign the cancellation with an ECDSA signer. The caller chooses the
    /// ECDSA scheme; `EthSign` adds the EIP-191 personal-sign envelope.
    /// Consumes `self`, mirroring [`OrderCancellations::sign`].
    ///
    /// Requires a [`SignerSync`](alloy_signer::SignerSync) signer (a raw
    /// local key); production hardware, remote or KMS signers should use
    /// [`Self::sign_async`] or the [`Self::signing_hash`] digest-and-lift
    /// recipe.
    pub fn sign<S: alloy_signer::SignerSync>(
        self,
        scheme: EcdsaSigningScheme,
        domain: &DomainSeparator,
        signer: &S,
    ) -> Result<SignedOrderCancellation, SignatureError> {
        let signature = sign_ecdsa(scheme, domain, &eip712::single(&self.order_uid), signer)?;
        Ok(SignedOrderCancellation {
            order_uid: self.order_uid,
            signature,
            signing_scheme: scheme,
        })
    }

    /// Async counterpart to [`Self::sign`], bound on the async
    /// [`alloy_signer::Signer`] trait rather than
    /// [`SignerSync`](alloy_signer::SignerSync). Prefer this for
    /// hardware, remote or KMS signers, which implement only the async
    /// trait.
    pub async fn sign_async<S: alloy_signer::Signer>(
        self,
        scheme: EcdsaSigningScheme,
        domain: &DomainSeparator,
        signer: &S,
    ) -> Result<SignedOrderCancellation, SignatureError> {
        let signature =
            sign_ecdsa_async(scheme, domain, &eip712::single(&self.order_uid), signer).await?;
        Ok(SignedOrderCancellation {
            order_uid: self.order_uid,
            signature,
            signing_scheme: scheme,
        })
    }
}

/// Signed cancellation of a single order. Mirrors `cowprotocol/services`
/// `OrderCancellation` exactly so any future on-chain verification path
/// stays interoperable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedOrderCancellation {
    /// UID of the order being cancelled.
    pub order_uid: OrderUid,
    /// ECDSA signature over the EIP-712 struct hash. Wire form is the
    /// 65-byte `0x`-hex `r || s || v` blob, not alloy's default
    /// `{r, s, yParity, v}` map.
    #[serde(with = "ecdsa_wire")]
    pub signature: EcdsaSignature,
    /// Off-chain ECDSA scheme used to produce the signature.
    pub signing_scheme: EcdsaSigningScheme,
}

impl SignedOrderCancellation {
    /// EIP-712 `hashStruct` for the single-order cancellation type.
    ///
    /// Delegates to [`OrderCancellation::hash_struct`].
    #[deprecated(
        since = "0.2.0",
        note = "use OrderCancellation::from(uid).hash_struct() instead"
    )]
    pub fn hash_struct(uid: &OrderUid) -> B256 {
        OrderCancellation::from(*uid).hash_struct()
    }

    /// Sign a single-order cancellation. The caller chooses the ECDSA
    /// scheme; `EthSign` adds the EIP-191 personal-sign envelope.
    ///
    /// Delegates to [`OrderCancellation::sign`].
    #[deprecated(
        since = "0.2.0",
        note = "use OrderCancellation::from(uid).sign(scheme, domain, signer) instead"
    )]
    pub fn sign<S: alloy_signer::SignerSync>(
        order_uid: OrderUid,
        scheme: EcdsaSigningScheme,
        domain: &DomainSeparator,
        signer: &S,
    ) -> Result<Self, SignatureError> {
        OrderCancellation::from(order_uid).sign(scheme, domain, signer)
    }

    /// Recover the signing owner from this cancellation, given the
    /// chain's domain separator.
    pub fn recover_owner(&self, domain: &DomainSeparator) -> Result<Address, SignatureError> {
        let payload = eip712::single(&self.order_uid);
        Ok(ecdsa_recover(&self.signature, self.signing_scheme, domain, &payload)?.signer)
    }
}

/// Unsigned collection of order UIDs to cancel.
///
/// Use [`OrderCancellations::sign`] to produce a [`SignedOrderCancellations`]
/// suitable for `DELETE /api/v1/orders`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderCancellations {
    /// UIDs of the orders being cancelled.
    pub order_uids: Vec<OrderUid>,
}

impl From<Vec<OrderUid>> for OrderCancellations {
    fn from(order_uids: Vec<OrderUid>) -> Self {
        Self { order_uids }
    }
}

impl FromIterator<OrderUid> for OrderCancellations {
    fn from_iter<I: IntoIterator<Item = OrderUid>>(iter: I) -> Self {
        Self {
            order_uids: iter.into_iter().collect(),
        }
    }
}

impl IntoIterator for OrderCancellations {
    type Item = OrderUid;
    type IntoIter = std::vec::IntoIter<OrderUid>;

    fn into_iter(self) -> Self::IntoIter {
        self.order_uids.into_iter()
    }
}

impl OrderCancellations {
    /// EIP-712 `hashStruct` for the collection-cancellation type.
    /// Delegates to [`alloy_sol_types::SolStruct`] applied to the
    /// private `eip712::OrderCancellations` declaration.
    pub fn hash_struct(&self) -> B256 {
        use alloy_sol_types::SolStruct;
        eip712::collection(&self.order_uids).eip712_hash_struct()
    }

    /// Sign the collection with an ECDSA signer.
    ///
    /// Requires a [`SignerSync`](alloy_signer::SignerSync) signer (a raw
    /// local key); production hardware, remote or KMS signers should use
    /// [`Self::sign_async`].
    pub fn sign<S: alloy_signer::SignerSync>(
        self,
        scheme: EcdsaSigningScheme,
        domain: &DomainSeparator,
        signer: &S,
    ) -> Result<SignedOrderCancellations, SignatureError> {
        let payload = eip712::collection(&self.order_uids);
        let signature = sign_ecdsa(scheme, domain, &payload, signer)?;
        Ok(SignedOrderCancellations {
            order_uids: self.order_uids,
            signature,
            signing_scheme: scheme,
        })
    }

    /// Async counterpart to [`Self::sign`], bound on the async
    /// [`alloy_signer::Signer`] trait. Prefer this for hardware, remote
    /// or KMS signers, which implement only the async trait.
    pub async fn sign_async<S: alloy_signer::Signer>(
        self,
        scheme: EcdsaSigningScheme,
        domain: &DomainSeparator,
        signer: &S,
    ) -> Result<SignedOrderCancellations, SignatureError> {
        let payload = eip712::collection(&self.order_uids);
        let signature = sign_ecdsa_async(scheme, domain, &payload, signer).await?;
        Ok(SignedOrderCancellations {
            order_uids: self.order_uids,
            signature,
            signing_scheme: scheme,
        })
    }
}

/// Body of `DELETE /api/v1/orders`: the cancellation collection together
/// with the owner's ECDSA signature over its EIP-712 struct hash.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedOrderCancellations {
    /// UIDs of the orders being cancelled.
    pub order_uids: Vec<OrderUid>,
    /// ECDSA signature over the EIP-712 hash of the cancellation struct.
    /// Wire form is the 65-byte `0x`-hex `r || s || v` blob.
    #[serde(with = "ecdsa_wire")]
    pub signature: EcdsaSignature,
    /// Off-chain ECDSA scheme used to produce the signature.
    pub signing_scheme: EcdsaSigningScheme,
}

impl SignedOrderCancellations {
    /// Recover the signing owner.
    pub fn recover_owner(&self, domain: &DomainSeparator) -> Result<Address, SignatureError> {
        let payload = eip712::collection(&self.order_uids);
        Ok(ecdsa_recover(&self.signature, self.signing_scheme, domain, &payload)?.signer)
    }
}

/// Build the `{ "name": .., "type": .. }` entries of the EIP-712
/// `OrderCancellation` type from the canonical
/// [`eip712::OrderCancellation`] `sol!` declaration, so the typed-data
/// table cannot silently drift from the struct the orderbook verifies
/// against. Parses [`alloy_sol_types::SolStruct::eip712_root_type`], which
/// is `OrderCancellation(<solType> <fieldName>,..)`.
fn cancellation_type_entries() -> Vec<serde_json::Value> {
    use alloy_sol_types::SolStruct;
    let root = <eip712::OrderCancellation as SolStruct>::eip712_root_type();
    let fields = root
        .strip_prefix("OrderCancellation(")
        .and_then(|s| s.strip_suffix(')'))
        .expect("canonical OrderCancellation root type is `OrderCancellation(...)`");
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

/// Canonical EIP-712 typed-data payload for a single-order cancellation,
/// ready to feed into viem's `signTypedData` or ethers'
/// `signer.signTypedData`. The cancellation counterpart to
/// [`order_typed_data`](crate::order::order_typed_data), letting external
/// EIP-712 wallets sign cancellations without hand-redeclaring the
/// `OrderCancellation` type table.
///
/// Returns `{ domain, primaryType, types, message }`. The domain `name` /
/// `version` come from [`crate::domain::DOMAIN_NAME`] /
/// [`crate::domain::DOMAIN_VERSION`], the same constants
/// [`crate::domain::settlement_domain`] derives the separator from, so the
/// typed-data domain and the separator cannot drift.
///
/// `types` deliberately omits the `EIP712Domain` entry: ethers v6 and viem
/// build the domain typedef from the `domain` object and throw on a
/// duplicate. Raw `eth_signTypedData_v4` callers must inject it themselves.
pub fn cancellation_typed_data(
    cancellation: &OrderCancellation,
    chain_id: u64,
    verifying_contract: Address,
) -> serde_json::Value {
    let message = serde_json::to_value(cancellation).expect("OrderCancellation serialises to JSON");
    serde_json::json!({
        "domain": {
            "name": crate::domain::DOMAIN_NAME,
            "version": crate::domain::DOMAIN_VERSION,
            "chainId": chain_id,
            "verifyingContract": verifying_contract.to_string(),
        },
        "primaryType": "OrderCancellation",
        "types": { "OrderCancellation": cancellation_type_entries() },
        "message": message,
    })
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        alloy_primitives::{Bytes, U256, b256, keccak256},
        alloy_signer_local::PrivateKeySigner,
    };

    /// Locks the [`eip712::OrderCancellation`] `typeHash` against the
    /// canonical EIP-712 type signature published in services. A drift
    /// in the `sol!` declaration would change every outstanding
    /// signature.
    #[test]
    fn order_cancellation_type_hash_matches_canonical_signature() {
        use alloy_sol_types::SolStruct;

        let signature = b"OrderCancellation(bytes orderUid)";
        let sol = eip712::OrderCancellation {
            orderUid: Bytes::copy_from_slice(&[0u8; 56]),
        };
        assert_eq!(
            <eip712::OrderCancellation as SolStruct>::eip712_type_hash(&sol),
            keccak256(signature),
        );
    }

    /// Same lock for the array variant. The contract type string uses
    /// plural `orderUids`.
    #[test]
    fn order_cancellations_type_hash_matches_canonical_signature() {
        use alloy_sol_types::SolStruct;

        let signature = b"OrderCancellations(bytes[] orderUids)";
        let sol = eip712::OrderCancellations { orderUids: vec![] };
        assert_eq!(
            <eip712::OrderCancellations as SolStruct>::eip712_type_hash(&sol),
            keccak256(signature),
        );
    }

    /// Locks `OrderCancellations::hash_struct` against the golden vectors
    /// from `cowprotocol/services/.../order.rs::order_cancellations_struct_hash`,
    /// generated via ethers.js as the reference implementation.
    #[test]
    fn order_cancellations_hash_struct_matches_services_golden() {
        let empty = OrderCancellations::default();
        assert_eq!(
            empty.hash_struct(),
            b256!("56acdb3034898c6c23971cb3f92c32a4739e89a13c85282547025583a93911bd")
        );

        let two = OrderCancellations {
            order_uids: vec![OrderUid::from([0x11; 56]), OrderUid::from([0x22; 56])],
        };
        assert_eq!(
            two.hash_struct(),
            b256!("405f6cb53d87901a5385a824a99c94b43146547f5ea3623f8d2f50b925e97a8b")
        );
    }

    /// Locks `OrderCancellation::hash_struct` against an independent
    /// re-derivation of the EIP-712 `hashStruct` for the single dynamic
    /// `bytes orderUid` field, computed by hand with raw `keccak256` rather
    /// than going through alloy's [`alloy_sol_types::SolStruct`].
    ///
    /// This is NOT an external ethers.js / services golden: it is an
    /// independent EIP-712 re-derivation. Per EIP-712 a dynamic `bytes`
    /// member is encoded as its `keccak256`, so
    /// `hashStruct = keccak256(typeHash ++ keccak256(orderUid))`, with
    /// `typeHash = keccak256("OrderCancellation(bytes orderUid)")` (the
    /// internal `sol!` type string, not the renamed Rust type). Locking the
    /// `SolStruct` path against this hand rolled form catches drift in the
    /// generated encoding without inventing a value we cannot verify.
    #[test]
    fn order_cancellation_hash_struct_matches_independent_eip712_derivation() {
        let type_hash = keccak256(b"OrderCancellation(bytes orderUid)");

        for uid in [OrderUid::from([0u8; 56]), OrderUid::from([0x42; 56])] {
            let mut encoded = [0u8; 64];
            encoded[0..32].copy_from_slice(type_hash.as_slice());
            encoded[32..64].copy_from_slice(keccak256(uid.as_slice()).as_slice());
            let expected = keccak256(encoded);

            assert_eq!(OrderCancellation::from(uid).hash_struct(), expected);
        }
    }

    fn fixed_signer() -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&U256::from(1u64).to_be_bytes().into()).unwrap()
    }

    /// Synthetic but valid `Eip712Domain` for tests: the round-trip
    /// behaviour depends only on the domain being consistent between
    /// sign and recover, not on it matching a real chain.
    fn fixed_domain() -> DomainSeparator {
        crate::domain::settlement_domain(
            1,
            alloy_primitives::address!("9008D19f58AAbD9eD0D60971565AA8510560ab41"),
        )
    }

    /// Sign-and-recover round trip for a single-order cancellation,
    /// covering both ECDSA schemes.
    #[test]
    fn order_cancellation_sign_recover_round_trip() {
        let signer = fixed_signer();
        let domain = fixed_domain();
        let uid = OrderUid::from([0x42; 56]);

        for scheme in [EcdsaSigningScheme::Eip712, EcdsaSigningScheme::EthSign] {
            let cancellation = OrderCancellation::from(uid)
                .sign(scheme, &domain, &signer)
                .unwrap();
            let recovered = cancellation.recover_owner(&domain).unwrap();
            assert_eq!(recovered, signer.address());
        }
    }

    /// Sign-and-recover round trip for an order-collection cancellation.
    #[test]
    fn order_cancellations_sign_recover_round_trip() {
        let signer = fixed_signer();
        let domain = fixed_domain();
        let cancellations = OrderCancellations {
            order_uids: vec![OrderUid::from([0x11; 56]), OrderUid::from([0x22; 56])],
        };
        let signed = cancellations
            .sign(EcdsaSigningScheme::Eip712, &domain, &signer)
            .unwrap();
        let recovered = signed.recover_owner(&domain).unwrap();
        assert_eq!(recovered, signer.address());
    }

    /// The async cancellation twins yield byte-identical signed values to
    /// the sync ones for a key implementing both traits, and both still
    /// recover the signing owner. Covers the single and collection paths.
    #[tokio::test]
    async fn cancellation_sign_async_matches_sync() {
        let signer = fixed_signer();
        let domain = fixed_domain();
        let uid = OrderUid::from([0x42; 56]);

        for scheme in [EcdsaSigningScheme::Eip712, EcdsaSigningScheme::EthSign] {
            let sync = OrderCancellation::from(uid)
                .sign(scheme, &domain, &signer)
                .unwrap();
            let asynchronous = OrderCancellation::from(uid)
                .sign_async(scheme, &domain, &signer)
                .await
                .unwrap();
            assert_eq!(sync, asynchronous);
            assert_eq!(
                asynchronous.recover_owner(&domain).unwrap(),
                signer.address()
            );
        }

        let collection = OrderCancellations {
            order_uids: vec![OrderUid::from([0x11; 56]), OrderUid::from([0x22; 56])],
        };
        let sync = collection
            .clone()
            .sign(EcdsaSigningScheme::Eip712, &domain, &signer)
            .unwrap();
        let asynchronous = collection
            .sign_async(EcdsaSigningScheme::Eip712, &domain, &signer)
            .await
            .unwrap();
        assert_eq!(sync, asynchronous);
        assert_eq!(
            asynchronous.recover_owner(&domain).unwrap(),
            signer.address()
        );
    }

    /// `SignedOrderCancellations` serialises to the flat wire shape expected
    /// by `DELETE /api/v1/orders`: `orderUids` array, `signature` hex, and
    /// `signingScheme` lowercase.
    #[test]
    fn signed_cancellations_wire_format() {
        let signed = SignedOrderCancellations {
            order_uids: vec![OrderUid::from([0x11; 56])],
            signature: EcdsaSignature::from_bytes_and_parity(&[0u8; 64], false),
            signing_scheme: EcdsaSigningScheme::Eip712,
        };
        let body = serde_json::to_value(&signed).unwrap();
        assert!(body["orderUids"].is_array());
        assert_eq!(body["signingScheme"], "eip712");
        assert!(body["signature"].as_str().unwrap().starts_with("0x"));
    }

    /// `SignedOrderCancellation` round-trips through JSON: serialise, deserialise,
    /// compare. Lets wasm callers (and any other JSON consumer) hand the
    /// type back and forth without losing fields.
    #[test]
    fn order_cancellation_json_round_trip() {
        let original = OrderCancellation::from(OrderUid::from([0x77; 56]))
            .sign(EcdsaSigningScheme::Eip712, &fixed_domain(), &fixed_signer())
            .unwrap();
        let json = serde_json::to_string(&original).unwrap();
        let parsed: SignedOrderCancellation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
        // Wire keys are camelCase, matching the orderbook OpenAPI.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("orderUid").is_some());
        assert!(value.get("signingScheme").is_some());
    }

    /// `OrderCancellations` is the unsigned collection (just the UIDs).
    /// JSON round-trip ensures `serde_with` adapters around `OrderUid`
    /// stay symmetric across serialise / deserialise.
    #[test]
    fn order_cancellations_json_round_trip() {
        let original = OrderCancellations {
            order_uids: vec![OrderUid::from([0x01; 56]), OrderUid::from([0x02; 56])],
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: OrderCancellations = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("orderUids").is_some());
    }

    /// `SignedOrderCancellations` is the body of `DELETE /api/v1/orders`.
    /// Same round-trip pattern as the single-order case: serialise into
    /// camelCase JSON, deserialise back, assert byte equality plus a
    /// shape sanity check on the wire keys.
    #[test]
    fn signed_order_cancellations_json_round_trip() {
        let original = OrderCancellations {
            order_uids: vec![OrderUid::from([0x33; 56]), OrderUid::from([0x44; 56])],
        }
        .sign(
            EcdsaSigningScheme::EthSign,
            &fixed_domain(),
            &fixed_signer(),
        )
        .unwrap();
        let json = serde_json::to_string(&original).unwrap();
        let parsed: SignedOrderCancellations = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("orderUids").is_some());
        assert!(value.get("signature").is_some());
        assert!(value.get("signingScheme").is_some());
    }

    /// The deprecated single-cancel entry points on the signed type still
    /// delegate to the new [`OrderCancellation`] value type, so callers
    /// pinned to the old shape keep getting byte-identical results.
    #[test]
    #[allow(deprecated)]
    fn deprecated_single_cancel_delegates_to_new_type() {
        let signer = fixed_signer();
        let domain = fixed_domain();
        let uid = OrderUid::from([0x42; 56]);

        assert_eq!(
            SignedOrderCancellation::hash_struct(&uid),
            OrderCancellation::from(uid).hash_struct(),
        );

        for scheme in [EcdsaSigningScheme::Eip712, EcdsaSigningScheme::EthSign] {
            let old = SignedOrderCancellation::sign(uid, scheme, &domain, &signer).unwrap();
            let new = OrderCancellation::from(uid)
                .sign(scheme, &domain, &signer)
                .unwrap();
            assert_eq!(old, new);
        }
    }

    /// `OrderCancellation::signing_hash` equals the message the signature
    /// recovery reports, the same forward/inverse relationship
    /// [`crate::order::OrderData`] has between `signing_hash` and recovery.
    #[test]
    fn cancellation_signing_hash_matches_recovered_message() {
        let signer = fixed_signer();
        let domain = fixed_domain();
        let uid = OrderUid::from([0x42; 56]);

        for scheme in [EcdsaSigningScheme::Eip712, EcdsaSigningScheme::EthSign] {
            let cancellation = OrderCancellation::from(uid);
            let signed = cancellation.sign(scheme, &domain, &signer).unwrap();
            let recovered =
                ecdsa_recover(&signed.signature, scheme, &domain, &eip712::single(&uid)).unwrap();
            assert_eq!(
                recovered.message,
                cancellation.signing_hash(scheme, &domain)
            );
        }
    }

    /// Locks the typed-data `types`."OrderCancellation" table built by
    /// [`cancellation_typed_data`] against the single canonical
    /// `bytes orderUid` field. Any change to the `sol!` struct, the parser,
    /// or the JSON shaping trips this.
    #[test]
    fn cancellation_typed_data_table_matches_canonical_field() {
        let typed = cancellation_typed_data(
            &OrderCancellation::from(OrderUid::from([0x11; 56])),
            1,
            Address::ZERO,
        );
        let table = typed["types"]["OrderCancellation"].as_array().unwrap();
        assert_eq!(table.len(), 1);
        assert_eq!(table[0]["name"], "orderUid");
        assert_eq!(table[0]["type"], "bytes");
    }

    /// Pins the full [`cancellation_typed_data`] envelope: the domain reuses
    /// the `settlement_domain` constants, `verifyingContract` is the EIP-55
    /// checksummed (mixed-case) address string, `EIP712Domain` is absent from
    /// `types`, and the message carries the `orderUid` as a hex string.
    #[test]
    fn cancellation_typed_data_envelope_shape() {
        let uid = OrderUid::from([0x11; 56]);
        let contract = alloy_primitives::address!("9008D19f58AAbD9eD0D60971565AA8510560ab41");
        let typed = cancellation_typed_data(&OrderCancellation::from(uid), 1, contract);

        assert_eq!(typed["domain"]["name"], crate::domain::DOMAIN_NAME);
        assert_eq!(typed["domain"]["version"], crate::domain::DOMAIN_VERSION);
        assert_eq!(typed["domain"]["chainId"], 1);
        assert_eq!(typed["domain"]["verifyingContract"], contract.to_string());
        assert_eq!(typed["primaryType"], "OrderCancellation");
        assert!(typed["types"].get("EIP712Domain").is_none());
        assert_eq!(typed["message"]["orderUid"], uid.to_string());
    }
}
