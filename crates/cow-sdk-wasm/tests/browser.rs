//! In-browser tests for the `cow-sdk-wasm` crate.
//!
//! Run with: `wasm-pack test --headless --firefox crates/cow-sdk-wasm`
//!
//! These tests exercise the wasm-bindgen exports as a JS caller would —
//! crossing the wasm/JS boundary via the same marshalling that
//! `serde-wasm-bindgen` does in production. The native `cargo test`
//! cannot do this; the unit suite in `lib.rs` runs on the host and
//! never touches the wasm runtime.
//!
//! Coverage falls into two groups:
//!
//! 1. **Pure-compute exports** (no network): UID derivation, domain
//!    separators, chain metadata, app-data hashing. Lock the byte-exact
//!    output against known fixtures so a serde / wasm-bindgen drift
//!    breaks the test rather than the test-harness page.
//! 2. **Transport exports** (mock fetch): replace the global `fetch`
//!    with a closure that returns a synthetic Response, then call
//!    `version()` / `get_quote()` and assert the parsed result. Proves
//!    the JS-fetch transport path end-to-end without going to
//!    `api.cow.fi`.

#![cfg(target_arch = "wasm32")]

use {
    js_sys::{JSON, Object, Promise, Reflect, global},
    wasm_bindgen::{JsCast, JsValue, closure::Closure},
    wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure},
};

use cow_sdk_wasm::{
    app_data_cid_from_hash, app_data_hash_from_json, build_order_cancellation,
    build_order_creation, cancel_order, cancellation_eip712_payload, cancellation_ethsign_digest,
    chain_info, domain_separator, empty_app_data_hash, get_quote, get_quote_simple, order_uid,
    post_order, sdk_app_data_hash, sdk_app_data_json, to_signed_order_data, version,
};

wasm_bindgen_test_configure!(run_in_browser);

fn valid_to_after(seconds: u64) -> u32 {
    let millis = js_sys::Date::now();
    let now_secs = if millis.is_finite() && millis > 0.0 {
        (millis / 1_000.0) as u64
    } else {
        0
    };
    now_secs.saturating_add(seconds).min(u64::from(u32::MAX)) as u32
}

// ===== Pure-compute tests ==============================================

/// `order_uid` should produce a 0x-prefixed 56-byte hex string
/// (2 + 112 chars) for a minimal sell order. The exact UID is byte-exact
/// across native Rust and wasm; this test would catch a serde rename or
/// a wasm-bindgen marshalling regression that silently shifts bytes.
#[wasm_bindgen_test]
fn order_uid_returns_56_byte_hex() {
    let order = serde_json::json!({
        "sellToken": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48", // USDC
        "buyToken":  "0x6B175474E89094C44Da98b954EedeAC495271d0F", // DAI
        "receiver": null,
        "sellAmount": "100000000",
        "buyAmount":  "99000000000000000000",
        "validTo": 4_294_967_295u32,
        "appData": empty_app_data_hash(),
        "feeAmount": "0",
        "kind": "sell",
        "partiallyFillable": false,
        "sellTokenBalance": "erc20",
        "buyTokenBalance":  "erc20",
    });
    // Round-trip through JSON.parse so the JsValue is a plain JS Object
    // (not a Map). `from_js<OrderData>` expects sibling fields, which
    // serde-wasm-bindgen's default Map serialisation does not satisfy.
    let order_json = serde_json::to_string(&order).expect("order to json");
    let order = JSON::parse(&order_json).expect("JSON.parse order");
    let uid = order_uid(
        order,
        "mainnet",
        "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
    )
    .expect("order_uid");
    assert!(uid.starts_with("0x"), "no 0x prefix: {uid}");
    assert_eq!(
        uid.len(),
        2 + 112,
        "expected 56 bytes (112 hex chars): {uid}"
    );
}

/// Mainnet's domain separator is the EIP-712 hash of
/// `{ name: "Gnosis Protocol", version: "v2", chainId: 1, verifyingContract: GPv2Settlement }`.
/// The expected value is locked elsewhere in cowprotocol's tests; here
/// we only assert the shape so this test stays self-contained.
#[wasm_bindgen_test]
fn domain_separator_is_32_byte_hex() {
    let hex = domain_separator("mainnet").expect("domain_separator");
    assert!(hex.starts_with("0x"), "no 0x prefix: {hex}");
    assert_eq!(hex.len(), 2 + 64, "expected 32 bytes (64 hex chars): {hex}");
}

/// `empty_app_data_hash` is the keccak-256 of `"{}"`, hard-coded across
/// the codebase. The exact value is
/// `0xb48d38f93eaa084033fc5970bf96e559c33c4cdc07d889ab00b4d63f9590739d`;
/// it must not drift between native and wasm.
#[wasm_bindgen_test]
fn empty_app_data_hash_stable() {
    let hex = empty_app_data_hash();
    assert_eq!(
        hex,
        "0xb48d38f93eaa084033fc5970bf96e559c33c4cdc07d889ab00b4d63f9590739d",
    );
}

/// `chain_info` returns a JS object (not a Map, thanks to
/// `serialize_maps_as_objects(true)`). Reach into it via `Reflect::get`
/// the way a JS caller would.
#[wasm_bindgen_test]
fn chain_info_returns_plain_object() {
    let info = chain_info("mainnet").expect("chain_info");
    let id = Reflect::get(&info, &JsValue::from_str("id"))
        .expect("get id")
        .as_f64()
        .expect("id is number");
    assert_eq!(id, 1.0, "mainnet id should be 1");

    let settlement = Reflect::get(&info, &JsValue::from_str("settlement"))
        .expect("get settlement")
        .as_string()
        .expect("settlement is string");
    assert!(
        settlement.starts_with("0x") && settlement.len() == 42,
        "settlement is not a 20-byte address: {settlement}"
    );
}

/// All ten chains should yield a parseable info object. Catches an
/// accidentally missing `Chain` arm in `parse_chain`.
#[wasm_bindgen_test]
fn chain_info_works_for_every_chain() {
    for name in [
        "mainnet",
        "bnb",
        "gnosis",
        "polygon",
        "base",
        "plasma",
        "arbitrum",
        "avalanche",
        "linea",
        "sepolia",
    ] {
        let info = chain_info(name).unwrap_or_else(|err| {
            panic!(
                "chain_info({name}) failed: {}",
                err.as_string().unwrap_or_default()
            )
        });
        assert!(Reflect::has(&info, &JsValue::from_str("id")).unwrap_or(false));
    }
}

/// `app_data_cid_from_hash` runs the IPFS CIDv1 derivation against a
/// known digest and returns a string starting with the multibase prefix.
#[wasm_bindgen_test]
fn app_data_cid_from_hash_is_base32_cid() {
    let cid = app_data_cid_from_hash(&empty_app_data_hash()).expect("cid");
    assert!(!cid.is_empty(), "empty CID");
    // CIDv1 base32 strings begin with 'b'; other multibase prefixes
    // (e.g. 'f' for base16) are also valid -- so just sanity-check the
    // length is in the expected band.
    assert!(
        (46..=80).contains(&cid.len()),
        "unexpected CID length: {cid}"
    );
}

// ===== Transport test (mocked fetch) ===================================
//
// Replace `globalThis.fetch` with a closure that returns a synthetic
// Response, then call a transport-touching export and assert it parses
// the body correctly. This exercises the full lifecycle:
//   wasm export -> serde encode body -> cowprotocol's FetchTransport
//     -> JS fetch (mock) -> Promise resolve -> text() -> Promise resolve
//     -> serde decode -> JsValue back to the test.

fn install_mock_fetch(
    status: u16,
    body: &'static str,
) -> Closure<dyn FnMut(JsValue, JsValue) -> Promise> {
    let body = body.to_string();
    let mock = Closure::wrap(Box::new(move |_url: JsValue, _init: JsValue| -> Promise {
        // Build a Response-like object: `{ status, text: () => Promise<string> }`.
        let response = Object::new();
        Reflect::set(
            &response,
            &JsValue::from_str("status"),
            &JsValue::from_f64(status as f64),
        )
        .unwrap();
        let body = body.clone();
        let text_fn = Closure::wrap(Box::new(move || -> Promise {
            Promise::resolve(&JsValue::from_str(&body))
        }) as Box<dyn FnMut() -> Promise>);
        Reflect::set(
            &response,
            &JsValue::from_str("text"),
            text_fn.as_ref().unchecked_ref(),
        )
        .unwrap();
        // Leak the inner closure; the mock outlives the test.
        text_fn.forget();
        let response_js: JsValue = response.into();
        Promise::resolve(&response_js)
    }) as Box<dyn FnMut(JsValue, JsValue) -> Promise>);
    Reflect::set(
        &global(),
        &JsValue::from_str("fetch"),
        mock.as_ref().unchecked_ref(),
    )
    .unwrap();
    mock
}

fn restore_real_fetch() {
    // After the test, deleting our shim lets subsequent tests use the
    // real `fetch` (or fail to find one, which is fine for pure-compute
    // tests).
    let _ = Reflect::delete_property(
        &global().unchecked_into::<Object>(),
        &JsValue::from_str("fetch"),
    );
}

/// `version()` is the smallest transport-touching export: GET
/// `/api/v1/version`, response is a bare string. Mock fetch to return
/// `"1.2.3"` and assert the SDK returns the same.
#[wasm_bindgen_test]
async fn version_returns_text_body() {
    let _mock = install_mock_fetch(200, "1.2.3");
    let v = version("mainnet")
        .await
        .unwrap_or_else(|err| panic!("version: {}", err.as_string().unwrap_or_default()));
    assert_eq!(v, "1.2.3", "version body should round-trip verbatim");
    restore_real_fetch();
}

/// `get_quote_simple` issues a POST whose JSON response shape includes
/// `quote.buyAmount` and the derived UID. Mock a minimal valid response
/// and assert the wasm shim returns a plain JS object (not a Map) with
/// those fields reachable via `.response.quote.buyAmount`.
#[wasm_bindgen_test]
async fn get_quote_simple_parses_response_via_mock_fetch() {
    // Minimal fixture: orderbook's QuoteResponse with one nested quote
    // and a `from` address. Fields the wasm shim does NOT use are
    // omitted to keep the fixture under control; serde_with's
    // `DisplayFromStr` handles the string-encoded U256s.
    let body = r#"{
        "quote": {
            "sellToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            "buyToken": "0x6b175474e89094c44da98b954eedeac495271d0f",
            "receiver": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
            "sellAmount": "99500000",
            "buyAmount": "99000000000000000000",
            "validTo": 4294967295,
            "appData": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "feeAmount": "500000",
            "kind": "sell",
            "partiallyFillable": false,
            "sellTokenBalance": "erc20",
            "buyTokenBalance": "erc20",
            "signingScheme": "eip712"
        },
        "from": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
        "id": 42,
        "expiration": "1700000000",
        "verified": false
    }"#;
    let _mock = install_mock_fetch(200, Box::leak(body.to_string().into_boxed_str()));
    let payload = get_quote_simple(
        "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
        "0x6B175474E89094C44Da98b954EedeAC495271d0F",
        "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
        "100000000",
        "mainnet",
    )
    .await
    .unwrap_or_else(|err| panic!("get_quote_simple: {}", err.as_string().unwrap_or_default()));
    let uid = Reflect::get(&payload, &JsValue::from_str("uid"))
        .expect("uid present")
        .as_string()
        .expect("uid string");
    assert!(uid.starts_with("0x"), "uid not hex-prefixed: {uid}");
    assert_eq!(uid.len(), 2 + 112, "uid not 56 bytes: {uid}");

    let response =
        Reflect::get(&payload, &JsValue::from_str("response")).expect("response present");
    let quote = Reflect::get(&response, &JsValue::from_str("quote")).expect("quote present");
    let buy_amount = Reflect::get(&quote, &JsValue::from_str("buyAmount"))
        .expect("buyAmount")
        .as_string()
        .expect("buyAmount string");
    assert_eq!(buy_amount, "99000000000000000000");
    restore_real_fetch();
}

/// Regression: `get_quote` must accept a request that pins a non-empty
/// `appData` digest. It used to run the response binding eagerly with a
/// hard-coded empty-document hash, which spuriously rejected every
/// pinned-appData quote with an `appData` mismatch. The binding now
/// runs at the projection chokepoint (`to_signed_order_data` /
/// `build_order_creation`) with the caller's real digest; the
/// hostile-response rejection is locked by
/// `to_signed_order_data_rejects_tampered_response` below.
#[wasm_bindgen_test]
async fn get_quote_accepts_pinned_app_data() {
    let body = r#"{
        "quote": {
            "sellToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            "buyToken": "0x6b175474e89094c44da98b954eedeac495271d0f",
            "receiver": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
            "sellAmount": "99500000",
            "buyAmount": "99000000000000000000",
            "validTo": 4294967295,
            "appData": "0x1111111111111111111111111111111111111111111111111111111111111111",
            "feeAmount": "500000",
            "kind": "sell",
            "partiallyFillable": false,
            "sellTokenBalance": "erc20",
            "buyTokenBalance": "erc20",
            "signingScheme": "eip712"
        },
        "from": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
        "id": 42,
        "expiration": "1700000000",
        "verified": false
    }"#;
    let _mock = install_mock_fetch(200, Box::leak(body.to_string().into_boxed_str()));
    let request = JSON::parse(
        r#"{
            "sellToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            "buyToken":  "0x6b175474e89094c44da98b954eedeac495271d0f",
            "from":      "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
            "kind":      "sell",
            "sellAmountBeforeFee": "100000000",
            "appData": "0x1111111111111111111111111111111111111111111111111111111111111111"
        }"#,
    )
    .unwrap();
    let response = get_quote(request, "mainnet")
        .await
        .unwrap_or_else(|err| panic!("get_quote: {}", err.as_string().unwrap_or_default()));
    let quote = Reflect::get(&response, &JsValue::from_str("quote")).expect("quote present");
    let app_data = Reflect::get(&quote, &JsValue::from_str("appData"))
        .expect("appData")
        .as_string()
        .expect("appData string");
    assert_eq!(
        app_data,
        "0x1111111111111111111111111111111111111111111111111111111111111111",
    );
    restore_real_fetch();
}

/// `to_signed_order_data` is the JS-side chokepoint: feed it a
/// hand-built request + a tampered response and it must refuse to
/// project, matching the native guard. Pure compute, no fetch.
#[wasm_bindgen_test]
fn to_signed_order_data_rejects_tampered_response() {
    let request = JSON::parse(
        r#"{
            "sellToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            "buyToken":  "0x6b175474e89094c44da98b954eedeac495271d0f",
            "from":      "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
            "kind":      "sell",
            "sellAmountBeforeFee": "100000000"
        }"#,
    )
    .unwrap();
    // Response advertises WETH instead of the requested USDC.
    let response = JSON::parse(
        r#"{
            "quote": {
                "sellToken": "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2",
                "buyToken": "0x6b175474e89094c44da98b954eedeac495271d0f",
                "receiver": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
                "sellAmount": "99500000",
                "buyAmount": "99000000000000000000",
                "validTo": 4294967295,
                "appData": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "feeAmount": "500000",
                "kind": "sell",
                "partiallyFillable": false,
                "sellTokenBalance": "erc20",
                "buyTokenBalance": "erc20",
                "signingScheme": "eip712"
            },
            "from": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
            "id": 42,
            "expiration": "1700000000",
            "verified": false
        }"#,
    )
    .unwrap();
    let err = to_signed_order_data(request, response, &empty_app_data_hash())
        .expect_err("expected QuoteFieldMismatch on swapped sellToken");
    let msg = err.as_string().unwrap_or_default();
    assert!(
        msg.contains("sellToken") && msg.contains("mismatch"),
        "expected sellToken mismatch error, got: {msg}",
    );
}

/// Non-2xx responses surface as `Err(JsValue)` carrying the HTTP status
/// and body text. Catches a future refactor that silently swallows
/// failures.
#[wasm_bindgen_test]
async fn version_surfaces_http_error_status() {
    let _mock = install_mock_fetch(503, "service unavailable");
    let err = version("mainnet").await.expect_err("expected error on 503");
    let msg = err.as_string().unwrap_or_default();
    assert!(msg.contains("503"), "error should mention status: {msg}");
    restore_real_fetch();
}

/// Subgraph-on-wasm, end to end: `SubgraphClient` defaults to the core
/// crate's `FetchTransport` on wasm32, so its queries ride the same
/// stubbed `fetch` as the orderbook bindings. The `subgraph` feature is
/// a dev-dependency: the shim's own npm bindings leave it off.
#[wasm_bindgen_test]
async fn subgraph_client_totals_via_mock_fetch() {
    let _mock = install_mock_fetch(
        200,
        r#"{"data":{"totals":[{"tokens":"10","orders":"42","traders":"7","settlements":"5"}]}}"#,
    );
    let client = cowprotocol::subgraph::SubgraphClient::new(
        "https://example.invalid/subgraphs/cow".parse().unwrap(),
    );
    let totals = client.totals().await.expect("totals over fetch transport");
    assert_eq!(totals.orders, "42");
    assert_eq!(totals.settlements, "5");
    restore_real_fetch();
}

/// `build_order_cancellation` assembles the external-signing
/// cancellation payload from a `{ signingScheme, r, s, v }` bag (no
/// private key), and its output round-trips into `cancel_order`: the same
/// JSON the builder emits deserialises straight back into the DELETE
/// binding over a mocked fetch. Proves the external-signing wire shapes
/// line up end to end without an in-shim key.
#[wasm_bindgen_test]
async fn build_order_cancellation_round_trips_into_cancel_order() {
    let uid = format!("0x{}", "11".repeat(56));
    // Syntactically valid (r, s, v): 32-byte blobs and v = 27. The
    // builder does no signer recovery, so the signature need not verify.
    let signature = JSON::parse(
        r#"{
            "signingScheme": "eip712",
            "r": "0x0000000000000000000000000000000000000000000000000000000000000001",
            "s": "0x0000000000000000000000000000000000000000000000000000000000000001",
            "v": 27
        }"#,
    )
    .expect("JSON.parse signature");

    let cancellation = build_order_cancellation(&uid, signature, "mainnet").unwrap_or_else(|err| {
        panic!(
            "build_order_cancellation: {}",
            err.as_string().unwrap_or_default()
        )
    });

    // Wire-shape sanity: the DELETE body carries orderUid, a signature
    // hex string and the lowercase scheme.
    let order_uid = Reflect::get(&cancellation, &JsValue::from_str("orderUid"))
        .expect("orderUid present")
        .as_string()
        .expect("orderUid string");
    assert_eq!(order_uid, uid);
    let scheme = Reflect::get(&cancellation, &JsValue::from_str("signingScheme"))
        .expect("signingScheme present")
        .as_string()
        .expect("signingScheme string");
    assert_eq!(scheme, "eip712");

    // Round-trip: hand the emitted payload straight to cancel_order. A
    // successful DELETE returns 2xx with an empty body.
    let _mock = install_mock_fetch(200, "");
    cancel_order(cancellation, "mainnet")
        .await
        .unwrap_or_else(|err| panic!("cancel_order: {}", err.as_string().unwrap_or_default()));
    restore_real_fetch();
}

/// `cancellation_eip712_payload` returns the `{ domain, primaryType,
/// types, message }` envelope a wallet's `signTypedData` accepts. Assert
/// the wasm wiring threads the right chain accessors (`c.id()` into
/// `domain.chainId`), pins `primaryType` to `OrderCancellation`,
/// round-trips the uid into `message.orderUid`, and omits the
/// `EIP712Domain` typedef so viem / ethers do not throw on a duplicate.
#[wasm_bindgen_test]
fn cancellation_eip712_payload_has_expected_envelope() {
    let uid = format!("0x{}", "22".repeat(56));
    let payload = cancellation_eip712_payload(&uid, "mainnet").unwrap_or_else(|err| {
        panic!(
            "cancellation_eip712_payload: {}",
            err.as_string().unwrap_or_default()
        )
    });

    let primary_type = Reflect::get(&payload, &JsValue::from_str("primaryType"))
        .expect("primaryType present")
        .as_string()
        .expect("primaryType string");
    assert_eq!(primary_type, "OrderCancellation");

    let domain = Reflect::get(&payload, &JsValue::from_str("domain")).expect("domain present");
    let chain_id = Reflect::get(&domain, &JsValue::from_str("chainId"))
        .expect("chainId present")
        .as_f64()
        .expect("chainId number");
    assert_eq!(chain_id, 1.0, "mainnet chainId should be 1");

    let message = Reflect::get(&payload, &JsValue::from_str("message")).expect("message present");
    let order_uid = Reflect::get(&message, &JsValue::from_str("orderUid"))
        .expect("orderUid present")
        .as_string()
        .expect("orderUid string");
    assert_eq!(
        order_uid, uid,
        "message.orderUid should round-trip the input"
    );

    // The `EIP712Domain` typedef is deliberately omitted so ethers v6 /
    // viem build it from the `domain` object instead of throwing on a
    // duplicate entry.
    let types = Reflect::get(&payload, &JsValue::from_str("types")).expect("types present");
    assert!(
        !Reflect::has(&types, &JsValue::from_str("EIP712Domain")).unwrap_or(true),
        "types should omit the EIP712Domain entry",
    );
    assert!(
        Reflect::has(&types, &JsValue::from_str("OrderCancellation")).unwrap_or(false),
        "types should carry the OrderCancellation entry",
    );
}

/// `cancellation_ethsign_digest` returns the 0x-prefixed 32-byte digest
/// an injected wallet must `personal_sign`. Assert the shape at the wasm
/// boundary; the exact hash is locked in `cowprotocol-signing`.
#[wasm_bindgen_test]
fn cancellation_ethsign_digest_is_32_byte_hex() {
    let uid = format!("0x{}", "22".repeat(56));
    let digest = cancellation_ethsign_digest(&uid, "mainnet").unwrap_or_else(|err| {
        panic!(
            "cancellation_ethsign_digest: {}",
            err.as_string().unwrap_or_default()
        )
    });
    assert!(digest.starts_with("0x"), "no 0x prefix: {digest}");
    assert_eq!(
        digest.len(),
        2 + 64,
        "expected 32 bytes (64 hex chars): {digest}"
    );
}

// ===== In-shim signing (feature-gated) =================================
//
// Only compiled / run when wasm-pack is invoked with
// `--features in_shim_signing`. A second CI job runs this branch so a
// future change that breaks the signing path does not slip past the
// default-features test job.

#[cfg(feature = "in_shim_signing")]
#[wasm_bindgen_test]
fn sign_eip712_owner_matches_anvil_account_zero() {
    // Anvil / Hardhat default account #0 -- well-known across the
    // Ethereum dev tooling, never represents real funds.
    const PRIVATE_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const ADDRESS: &str = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";

    let order = serde_json::json!({
        "sellToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
        "buyToken": "0x6b175474e89094c44da98b954eedeac495271d0f",
        "receiver": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
        "sellAmount": "100000000",
        "buyAmount": "99000000000000000000",
        "validTo": 4_294_967_295u32,
        "appData": "0xb48d38f93eaa084033fc5970bf96e559c33c4cdc07d889ab00b4d63f9590739d",
        "feeAmount": "0",
        "kind": "sell",
        "partiallyFillable": false,
        "sellTokenBalance": "erc20",
        "buyTokenBalance": "erc20",
    });
    // Round-trip through JSON.parse so the JsValue is a plain JS
    // Object (not a Map). `from_js<OrderData>` on the wasm side
    // expects sibling fields, which serde-wasm-bindgen's default Map
    // serialisation does not satisfy.
    let order_json = serde_json::to_string(&order).expect("order to json");
    let order_js = JSON::parse(&order_json).expect("JSON.parse order");

    let sig = cow_sdk_wasm::sign_eip712(order_js, "mainnet", PRIVATE_KEY)
        .unwrap_or_else(|err| panic!("sign_eip712: {}", err.as_string().unwrap_or_default()));

    let owner = Reflect::get(&sig, &JsValue::from_str("owner"))
        .expect("owner present")
        .as_string()
        .expect("owner is a string");
    assert_eq!(
        owner.to_lowercase(),
        ADDRESS,
        "in-shim signer should derive the well-known Anvil address"
    );

    // (r, s, v) shape sanity. The signing path that fills these in
    // should produce 32-byte hex blobs and a v in {27, 28}.
    let r = Reflect::get(&sig, &JsValue::from_str("r"))
        .unwrap()
        .as_string()
        .unwrap();
    assert!(r.starts_with("0x") && r.len() == 2 + 64, "r: {r}");
    let v = Reflect::get(&sig, &JsValue::from_str("v"))
        .unwrap()
        .as_f64()
        .unwrap() as u8;
    assert!(v == 27 || v == 28, "v should be 27 or 28, got {v}");
}

/// Attribution doc carries `appCode: "cow-rs-wasm"` and the wasm
/// package version. The hash returned by `sdk_app_data_hash` must
/// match `app_data_hash_from_json(sdk_app_data_json())` so a JS
/// caller can recompute the digest independently and get the same
/// bytes.
#[wasm_bindgen_test]
fn sdk_app_data_json_and_hash_are_consistent() {
    let json = sdk_app_data_json();
    let value: serde_json::Value = serde_json::from_str(&json).expect("parse sdk json");
    assert_eq!(value["appCode"], "cow-rs-wasm");
    assert!(
        value["metadata"]["quote"]["version"].is_string(),
        "quote.version should be set: {json}"
    );

    let derived = app_data_hash_from_json(&json).expect("hash from json");
    let direct = sdk_app_data_hash();
    assert_eq!(
        derived, direct,
        "sdk_app_data_hash() should equal app_data_hash_from_json(sdk_app_data_json())"
    );
    assert!(direct.starts_with("0x") && direct.len() == 2 + 64);
}

// ===== Boundary-check regression tests =================================
//
// Pin the client-side guards `build_order_creation` and `post_order`
// perform before they hand control to the orderbook: `quote_id` is
// range-checked into `i64` rather than silently wrapping, and
// `post_order` runs `verify_owner` so a hand-assembled body with the
// wrong `from` is rejected before any network call.

/// `quote_id` greater than `i64::MAX` would silently wrap to a negative
/// integer under the previous `as i64` cast. The checked conversion
/// must reject it with an explicit error string callers can match on.
#[wasm_bindgen_test]
fn build_order_creation_rejects_overflowing_quote_id() {
    // App-data hash matching the canonical empty document `"{}"`, so
    // `OrderCreation::new` cannot reject the body on the
    // hash-mismatch path before the `quote_id` check fires. Use the
    // app-data hash sentinel from the SDK for the same reason.
    let empty_hash = empty_app_data_hash();
    let order = serde_json::json!({
        "sellToken": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
        "buyToken":  "0x6B175474E89094C44Da98b954EedeAC495271d0F",
        "receiver": null,
        "sellAmount": "100000000",
        "buyAmount":  "99000000000000000000",
        "validTo": valid_to_after(3_600),
        "appData": empty_hash,
        "feeAmount": "0",
        "kind": "sell",
        "partiallyFillable": false,
        "sellTokenBalance": "erc20",
        "buyTokenBalance":  "erc20",
    });
    let order_json = serde_json::to_string(&order).expect("order to json");
    let order_js = JSON::parse(&order_json).expect("JSON.parse order");

    // Syntactically valid (r, s, v): 32-byte zero blobs and v = 27. The
    // `quote_id` range check fires before signer recovery, so the
    // signature does not need to verify against `owner`.
    let signature = JSON::parse(
        r#"{
            "signingScheme": "eip712",
            "r": "0x0000000000000000000000000000000000000000000000000000000000000001",
            "s": "0x0000000000000000000000000000000000000000000000000000000000000001",
            "v": 27
        }"#,
    )
    .expect("JSON.parse signature");

    let err = build_order_creation(
        order_js,
        signature,
        "mainnet",
        "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
        "{}",
        Some(u64::MAX),
    )
    .expect_err("expected quote_id overflow error");
    let msg = err.as_string().unwrap_or_default();
    assert!(
        msg.contains("quote_id exceeds i64::MAX"),
        "expected quote_id overflow error, got: {msg}",
    );
}

/// `post_order` must owner-verify the body before any network call:
/// the chain-hinted shared client runs `verify_owner` inside
/// `OrderBookApi::post_order`, so a hand-assembled body with a `from`
/// that does not match the recovered signer is rejected locally with a
/// signer-mismatch error rather than reaching `fetch`.
///
/// Build a wire-shape `OrderCreation` JSON whose ECDSA signature
/// recovers to one address but whose `from` is a different non-zero
/// address. Deserialisation succeeds (the wire `try_from` only rejects
/// `from = ZERO`); the chokepoint must then reject the mismatch.
#[wasm_bindgen_test]
async fn post_order_rejects_wrong_from_locally() {
    // Install a fetch shim that panics if hit; the local guard must
    // short-circuit before the transport runs.
    let panic_fetch = Closure::wrap(Box::new(|_url: JsValue, _init: JsValue| -> Promise {
        panic!("post_order reached fetch despite verify_owner mismatch");
    }) as Box<dyn FnMut(JsValue, JsValue) -> Promise>);
    Reflect::set(
        &global(),
        &JsValue::from_str("fetch"),
        panic_fetch.as_ref().unchecked_ref(),
    )
    .unwrap();

    // Wire-shape body. `signingScheme = eip712`, a syntactically valid
    // 65-byte signature, the empty-app-data sentinel and a non-zero
    // `from` that will not match any signer recovery on this payload.
    // The wire `try_from` enforces `from != ZERO` and
    // `keccak256(app_data) == app_data_hash`, both of which hold here.
    let empty_hash = empty_app_data_hash();
    let creation = serde_json::json!({
        "sellToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
        "buyToken":  "0x6b175474e89094c44da98b954eedeac495271d0f",
        "receiver": null,
        "sellAmount": "100000000",
        "buyAmount":  "99000000000000000000",
        "validTo": valid_to_after(3_600),
        "appData": "{}",
        "appDataHash": empty_hash,
        "feeAmount": "0",
        "kind": "sell",
        "partiallyFillable": false,
        "sellTokenBalance": "erc20",
        "buyTokenBalance":  "erc20",
        "signingScheme": "eip712",
        // 65-byte ECDSA blob: r = 1, s = 1, v = 27. Recovers to some
        // address that is overwhelmingly unlikely to equal `from`
        // below, so verify_owner fires the SignerMismatch arm.
        "signature": "0x0000000000000000000000000000000000000000000000000000000000000001\
                       0000000000000000000000000000000000000000000000000000000000000001\
                       1b",
        "from": "0x000000000000000000000000000000000000dEaD",
    });
    let creation_json = serde_json::to_string(&creation).expect("creation to json");
    let creation_js = JSON::parse(&creation_json).expect("JSON.parse creation");

    let err = post_order(creation_js, "mainnet")
        .await
        .expect_err("expected signer mismatch before fetch");
    let msg = err.as_string().unwrap_or_default();
    assert!(
        msg.starts_with("post_order failed:") && msg.contains("signer mismatch"),
        "expected a local signer-mismatch rejection, got: {msg}",
    );

    restore_real_fetch();
    drop(panic_fetch);
}
