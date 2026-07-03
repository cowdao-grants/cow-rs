//! Checks our hand-written wire types against upstream specifications
//! checked into `specs/`. The point is drift detection: when the
//! upstream OpenAPI gains a new required field, the AppData schema
//! renames a property, or the subgraph adds an entity we now claim to
//! support, these tests fail with a clear message instead of letting
//! production silently break.
//!
//! No code generation. The cow-rs orderbook / app-data / subgraph
//! modules remain hand-written; these tests just confirm the
//! field names match what's documented upstream.

use std::{collections::HashSet, fs};

const ORDERBOOK_SPEC: &str = "../../specs/orderbook-api.yml";
const APP_DATA_SCHEMA: &str = "../../specs/app-data-v1.6.0.json";
const APP_DATA_PARTNER_FEE: &str = "../../specs/app-data-partner-fee-v1.0.0.json";
const APP_DATA_REFERRER: &str = "../../specs/app-data-referrer-v0.2.0.json";
const SUBGRAPH_SCHEMA: &str = "../../specs/subgraph.graphql";

fn read(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("spec file {path} is missing: {e}; run `just fetch-specs` to refresh")
    })
}

/// Returns the set of property names declared anywhere in the OpenAPI
/// YAML (anywhere a `<name>:` appears in a `properties:` block). The
/// scan is intentionally permissive: we want to catch field absence,
/// not type drift.
fn openapi_properties(body: &str) -> HashSet<String> {
    let mut props: HashSet<String> = HashSet::new();
    let mut in_props = false;
    let mut props_indent: usize = 0;
    for line in body.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if trimmed.starts_with("properties:") {
            in_props = true;
            props_indent = indent;
            continue;
        }
        if in_props {
            // Skip until a child line (indent > props_indent).
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if indent <= props_indent {
                in_props = false;
                continue;
            }
            // Properties are first-level children of `properties:` so
            // only consider keys at indent == props_indent + 2.
            if indent == props_indent + 2
                && let Some(name_end) = trimmed.find(':')
            {
                let name = &trimmed[..name_end];
                if name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
                {
                    props.insert(name.to_owned());
                }
            }
        }
    }
    props
}

#[test]
fn orderbook_openapi_declares_every_field_we_serialise() {
    let body = read(ORDERBOOK_SPEC);
    let props = openapi_properties(&body);
    // These names cross the wire on `POST /orders`, `POST /quote`, and
    // `GET /orders/{uid}`. If any are renamed upstream our serde
    // round-trip silently breaks; this test forces a refresh of
    // `specs/orderbook-api.yml`.
    for required in [
        "sellToken",
        "buyToken",
        "receiver",
        "sellAmount",
        "buyAmount",
        "feeAmount",
        "validTo",
        "appData",
        "appDataHash",
        "kind",
        "partiallyFillable",
        "sellTokenBalance",
        "buyTokenBalance",
        "signingScheme",
        "signature",
        "from",
        "quoteId",
        "protocolFeeBps",
    ] {
        assert!(
            props.contains(required),
            "orderbook OpenAPI no longer declares property {required:?}; \
             refresh specs/orderbook-api.yml or rename the field in cow-rs"
        );
    }
}

#[test]
fn app_data_root_schema_declares_every_top_level_key_we_write() {
    let body = read(APP_DATA_SCHEMA);
    let schema: serde_json::Value =
        serde_json::from_str(&body).expect("app-data schema is valid JSON");
    let root = schema["properties"]
        .as_object()
        .expect("root schema has properties");
    for required in ["version", "appCode", "environment", "metadata"] {
        assert!(
            root.contains_key(required),
            "app-data root schema no longer declares {required:?}"
        );
    }
    let metadata = root["metadata"]["properties"]
        .as_object()
        .expect("metadata.properties exists");
    for required in [
        "referrer",
        "quote",
        "orderClass",
        "hooks",
        "partnerFee",
        "replacedOrder",
        "flashloan",
    ] {
        assert!(
            metadata.contains_key(required),
            "app-data metadata schema no longer declares {required:?}"
        );
    }
}

#[test]
fn app_data_partner_fee_schema_uses_recipient_and_one_of_policies() {
    let body = read(APP_DATA_PARTNER_FEE);
    let schema: serde_json::Value =
        serde_json::from_str(&body).expect("partner-fee schema is valid JSON");
    // We rely on `recipient` and a oneOf{volumeBps, surplus, priceImprovement}
    // policy split. Refresh the file or update our serde shape if these go away.
    let one_of = schema["oneOf"]
        .as_array()
        .expect("partner fee schema is a oneOf");
    assert!(
        !one_of.is_empty(),
        "partner fee schema must keep at least one policy variant"
    );
    let json = serde_json::to_string(&schema).unwrap();
    for needle in ["recipient", "volumeBps", "surplus", "priceImprovement"] {
        assert!(
            json.contains(needle),
            "partner fee schema no longer references {needle:?}; \
             AppDataPartnerFee in app_data.rs may need to change"
        );
    }
}

#[test]
fn app_data_referrer_schema_uses_address_not_code() {
    // Upstream rename trap: pre-2024 the field was `code`. cow-py and
    // PR #2 both caught the drift. This test pins the post-rename
    // shape so we cannot regress.
    let body = read(APP_DATA_REFERRER);
    let schema: serde_json::Value =
        serde_json::from_str(&body).expect("referrer schema is valid JSON");
    let props = schema["properties"]
        .as_object()
        .expect("referrer schema has properties");
    assert!(
        props.contains_key("address"),
        "referrer schema must keep the `address` field"
    );
    assert!(
        !props.contains_key("code"),
        "referrer schema unexpectedly reverted to `code`; AppDataReferrer \
         in app_data.rs is currently `address`-shaped"
    );
}

#[test]
fn subgraph_schema_declares_every_entity_we_query() {
    let body = read(SUBGRAPH_SCHEMA);
    // Look for `type <Name> @entity` declarations.
    let mut entities: HashSet<String> = HashSet::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("type ")
            && let Some(name_end) = rest.find(|c: char| c.is_whitespace())
        {
            let name = &rest[..name_end];
            if rest.contains("@entity") {
                entities.insert(name.to_owned());
            }
        }
    }
    for required in [
        "Total",
        "DailyTotal",
        "HourlyTotal",
        "Token",
        "Trade",
        "Order",
    ] {
        assert!(
            entities.contains(required),
            "subgraph schema no longer declares entity {required:?}; \
             cowprotocol::subgraph queries assume it exists"
        );
    }
}
