//! Generates the signed-order conformance fixture for cow-rs from the services
//! `model` crate pinned in `Cargo.toml`. See the README for coverage and format.
//!
//! Run with `cargo run --locked --manifest-path tools/conformance-gen/Cargo.toml`.

use {
    alloy_primitives::{Address, B256, U256, address, b256, keccak256},
    alloy_signer_local::PrivateKeySigner,
    app_data::AppDataHash,
    model::{
        DomainSeparator,
        order::{BuyTokenDestination, OrderData, OrderKind, OrderUid, SellTokenSource},
        signature::{EcdsaSignature, EcdsaSigningScheme, Signature},
    },
    serde::Serialize,
    std::{collections::HashSet, fs, path::Path},
};

/// Version of the fixture format. After the first merge, incompatible format
/// changes require a new version; see the README's schema-version policy.
const SCHEMA_VERSION: u32 = 1;

// Must match both services dependency revisions in Cargo.toml. See the README
// for the pin-update steps, including the lockfile and documentation.
const SERVICES_PIN: &str = "df6a14b12eab016f1ae939aa5b666658208b0bc0";

// Defined here, not taken from cow-rs, so the reference does not depend on
// the code under test.
// GPv2Settlement address used for these mainnet vectors.
const SETTLEMENT: Address = address!("9008D19f58AAbD9eD0D60971565AA8510560ab41");
const CHAIN_ID: u64 = 1;

// Hardhat account 0 test key. Do not use it for funded accounts.
const TEST_PRIVATE_KEY: B256 =
    b256!("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80");

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    schema_version: u32,
    services_sha: &'static str,
    settlement: Address,
    chain_id: u64,
    /// Address of the test key; every recovered signer must equal it.
    signer: Address,
    vectors: Vec<Vector>,
}

/// One order and its expected results. The domain separator, struct hash and
/// UID do not depend on the signing scheme, so each is stored once.
#[derive(Serialize)]
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

/// Services' `Signature` for one scheme, with the digest returned by recovery.
#[derive(Serialize)]
struct SigningExpectation {
    #[serde(flatten)]
    signature: Signature,
    digest: B256,
}

struct Case {
    id: String,
    order: OrderData,
}

/// The order from services' `compute_order_uid` test, except `app_data` and
/// `valid_to`; `valid_to` avoids the `1` and `u32::MAX` boundary cases.
fn base_order() -> OrderData {
    OrderData {
        sell_token: Address::repeat_byte(0x01),
        buy_token: Address::repeat_byte(0x02),
        receiver: Some(Address::repeat_byte(0x03)),
        sell_amount: U256::from(0x0002_46dd_f979_7668_0000_u128),
        buy_amount: U256::from(0xb98b_c829_a6f9_0000_u128),
        valid_to: 1_893_456_000,
        app_data: AppDataHash(*keccak256(b"cow-rs conformance vector")),
        fee_amount: U256::from(0x0de0_b6b3_a764_0000_u128),
        kind: OrderKind::Sell,
        partially_fillable: false,
        sell_token_balance: SellTokenSource::Erc20,
        buy_token_balance: BuyTokenDestination::Erc20,
    }
}

/// Services' JSON name for an enum value, so case ids reuse services' naming.
fn serde_name(value: impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        other => panic!("services enum did not serialise to a string: {other:?}"),
    }
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();

    // Every combination of order kind, sell-token source and buy-token destination.
    for kind in [OrderKind::Sell, OrderKind::Buy] {
        for sell_balance in [
            SellTokenSource::Erc20,
            SellTokenSource::External,
            SellTokenSource::Internal,
        ] {
            for buy_balance in [BuyTokenDestination::Erc20, BuyTokenDestination::Internal] {
                let id = format!(
                    "{}_sb_{}_bb_{}",
                    serde_name(kind),
                    serde_name(sell_balance),
                    serde_name(buy_balance),
                );
                cases.push(Case {
                    id,
                    order: OrderData {
                        kind,
                        sell_token_balance: sell_balance,
                        buy_token_balance: buy_balance,
                        ..base_order()
                    },
                });
            }
        }
    }

    // Services hashes a missing receiver as the zero address, so `receiver_none`
    // and `receiver_zero` give identical results; only their JSON inputs differ.
    for (label, receiver) in [
        ("receiver_none", None),
        ("receiver_zero", Some(Address::ZERO)),
        ("receiver_nonzero", Some(Address::repeat_byte(0x07))),
    ] {
        cases.push(Case {
            id: label.to_string(),
            order: OrderData {
                receiver,
                ..base_order()
            },
        });
    }

    // All three amounts take the boundary value. Equal amounts cannot reveal
    // swapped amount fields; the base cases, with distinct amounts, can.
    for (label, amount) in [("amount_one", U256::ONE), ("amount_u256_max", U256::MAX)] {
        cases.push(Case {
            id: label.to_string(),
            order: OrderData {
                sell_amount: amount,
                buy_amount: amount,
                fee_amount: amount,
                ..base_order()
            },
        });
    }

    // `valid_to` feeds both the struct hash and the last four bytes of the UID.
    for (label, valid_to) in [("valid_to_one", 1u32), ("valid_to_u32_max", u32::MAX)] {
        cases.push(Case {
            id: label.to_string(),
            order: OrderData {
                valid_to,
                ..base_order()
            },
        });
    }

    assert_eq!(
        cases.len(),
        19,
        "coverage matrix must contain exactly 19 order vectors"
    );
    let ids: HashSet<_> = cases.iter().map(|case| &case.id).collect();
    assert_eq!(ids.len(), cases.len(), "case identifiers must be unique");
    cases
}

/// Signs the order under both schemes and records services' results. Signing
/// and recovery are both services code, so their agreement shows internal
/// consistency only, not that services is correct.
fn to_vector(case: Case, signer: &PrivateKeySigner, domain: &DomainSeparator) -> Vector {
    let owner = signer.address();
    let order = case.order;
    let struct_hash = order.hash_struct();

    let signatures = [EcdsaSigningScheme::Eip712, EcdsaSigningScheme::EthSign].map(|scheme| {
        let signature =
            EcdsaSignature::sign(scheme, domain, &struct_hash, signer).to_signature(scheme);
        let recovered = signature
            .recover(domain, &struct_hash)
            .unwrap_or_else(|error| panic!("{} {scheme:?}: recovery failed: {error}", case.id))
            .expect("ECDSA schemes recover a signer");
        assert_eq!(
            recovered.signer, owner,
            "{} {scheme:?}: recovered owner",
            case.id
        );
        SigningExpectation {
            signature,
            digest: recovered.message,
        }
    });

    Vector {
        id: case.id,
        domain_separator: B256::from(domain.0),
        hash_struct: B256::from(struct_hash),
        uid: order.uid(domain, owner),
        signatures,
        order,
    }
}

/// Builds and checks every vector before writing, so a failed check writes nothing.
fn main() {
    let signer = PrivateKeySigner::from_bytes(&TEST_PRIVATE_KEY)
        .expect("the fixed public test key must be valid");
    let domain = DomainSeparator::new(CHAIN_ID, SETTLEMENT);

    let vectors: Vec<Vector> = cases()
        .into_iter()
        .map(|case| to_vector(case, &signer, &domain))
        .collect();

    let fixture = Fixture {
        schema_version: SCHEMA_VERSION,
        services_sha: SERVICES_PIN,
        settlement: SETTLEMENT,
        chain_id: CHAIN_ID,
        signer: signer.address(),
        vectors,
    };

    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/cowprotocol-signing/tests/fixtures/conformance_vectors.v1.json");
    let mut json = serde_json::to_string_pretty(&fixture).expect("fixture serialises to JSON");
    json.push('\n');
    fs::write(&path, json).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    println!(
        "wrote {} order vectors ({} signing-scheme checks) to {}",
        fixture.vectors.len(),
        fixture
            .vectors
            .iter()
            .map(|vector| vector.signatures.len())
            .sum::<usize>(),
        path.display()
    );
}
