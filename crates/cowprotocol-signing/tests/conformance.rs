//! Checks that cow-rs gets the same order hashes, UIDs, signing digests and
//! signers as services. The fixture comes from `tools/conformance-gen` and is
//! not committed, so the test is `#[ignore]`d and the `conformance` CI job
//! runs it.

use std::{collections::HashSet, fs};

use alloy_primitives::{Address, B256, U256};
use cowprotocol_primitives::Chain;
use cowprotocol_signing::{EcdsaSigningScheme, OrderData, OrderUid, Signature};
use serde::Deserialize;

const FIXTURE_PATH: &str = "tests/fixtures/conformance_vectors.v1.json";
const LOCK_PATH: &str = "../../parity/source-lock.toml";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    schema_version: u32,
    services_sha: String,
    settlement: Address,
    chain_id: u64,
    signer: Address,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vector {
    id: String,
    #[serde(flatten)]
    order: OrderData,
    domain_separator: B256,
    hash_struct: B256,
    uid: OrderUid,
    signatures: [SigningExpectation; 2],
}

#[derive(Deserialize)]
struct SigningExpectation {
    #[serde(flatten)]
    signature: Signature,
    digest: B256,
}

#[test]
#[ignore = "needs the fixture from tools/conformance-gen"]
fn signed_orders_match_services() {
    let json = fs::read_to_string(FIXTURE_PATH).unwrap_or_else(|error| {
        panic!("{FIXTURE_PATH}: {error}; see tools/conformance-gen/README.md")
    });
    let fixture: Fixture = serde_json::from_str(&json).expect("fixture decodes");
    assert_eq!(fixture.schema_version, 1, "schema version");
    let orders: HashSet<_> = fixture.vectors.iter().map(|vector| vector.order).collect();
    assert_eq!(fixture.vectors.len(), 19, "vector count");
    assert_eq!(orders.len(), 19, "distinct orders");
    let ids: HashSet<_> = fixture.vectors.iter().map(|vector| &vector.id).collect();
    assert_eq!(ids.len(), 19, "distinct case ids");
    let combinations: HashSet<_> = orders
        .iter()
        .map(|order| {
            (
                order.kind,
                order.sell_token_balance,
                order.buy_token_balance,
            )
        })
        .collect();
    assert_eq!(
        combinations.len(),
        12,
        "order-kind and balance combinations"
    );
    for receiver in [None, Some(Address::ZERO)] {
        assert!(
            orders.iter().any(|order| order.receiver == receiver),
            "receiver {receiver:?}"
        );
    }
    assert!(
        orders.iter().any(|order| order
            .receiver
            .is_some_and(|receiver| receiver != Address::ZERO)),
        "nonzero receiver"
    );
    for amount in [U256::ONE, U256::MAX] {
        assert!(
            orders.iter().any(|order| order.sell_amount == amount
                && order.buy_amount == amount
                && order.fee_amount == amount),
            "amount boundary {amount}"
        );
    }
    for valid_to in [1, u32::MAX] {
        assert!(
            orders.iter().any(|order| order.valid_to == valid_to),
            "validTo boundary {valid_to}"
        );
    }

    let lock = fs::read_to_string(LOCK_PATH).expect("source-lock.toml is readable");
    let sha = format!("sha = \"{}\"", fixture.services_sha);
    assert!(
        lock.split("[[source]]").any(|source| {
            let mut lines = source.lines().map(str::trim);
            lines.clone().any(|line| line == "name = \"services\"") && lines.any(|line| line == sha)
        }),
        "services commit {} is not the one pinned in {LOCK_PATH}",
        fixture.services_sha
    );

    // Use cow-rs's own domain. One built from the fixture would not test
    // `settlement_domain`.
    let domain = Chain::Mainnet.settlement_domain();
    assert_eq!(Chain::Mainnet.id(), fixture.chain_id, "chain id");
    assert_eq!(
        Chain::Mainnet.settlement(),
        fixture.settlement,
        "settlement"
    );

    for vector in &fixture.vectors {
        let Vector { id, order, .. } = vector;
        assert_eq!(
            domain.separator(),
            vector.domain_separator,
            "{id}: domain separator"
        );
        assert_eq!(order.hash_struct(), vector.hash_struct, "{id}: struct hash");
        assert_eq!(order.uid(&domain, fixture.signer), vector.uid, "{id}: UID");

        let schemes = [EcdsaSigningScheme::Eip712, EcdsaSigningScheme::EthSign];
        for (scheme, expected) in schemes.into_iter().zip(&vector.signatures) {
            assert_eq!(
                expected.signature.scheme(),
                scheme.into(),
                "{id}: signing scheme"
            );
            assert_eq!(
                order.signing_hash(scheme, &domain),
                expected.digest,
                "{id} {scheme:?}: digest"
            );
            let recovered = order
                .recover_signer(&domain, &expected.signature)
                .unwrap_or_else(|error| panic!("{id} {scheme:?}: {error}"))
                .expect("ECDSA schemes recover a signer");
            assert_eq!(
                recovered.signer, fixture.signer,
                "{id} {scheme:?}: recovered owner"
            );
        }
    }
}
