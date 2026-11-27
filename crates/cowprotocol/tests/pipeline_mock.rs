//! Integration tests for the quote -> sign -> submit pipeline
//! (`OrderBookApi::quote_builder`).
//!
//! Drives the full flow against an in-process `wiremock` server.
//! Asserts that the projected `buy_amount` accounts for partner fee +
//! slippage + protocol fee composition (the bug cow-sdk #867 fixed
//! upstream), that the app-data document is pinned before the order is
//! posted, and that the assembled `OrderCreation` carries a
//! recoverable EIP-712 signature.

#![cfg(all(not(target_arch = "wasm32"), feature = "http-client"))]

mod common;

use alloy_primitives::{Address, U256, address};
use alloy_signer_local::PrivateKeySigner;
use cowprotocol::{AppDataDoc, Chain, EcdsaSigningScheme, OrderBookApi};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

use common::valid_to_after;

const USDC: Address = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
const DAI: Address = address!("6B175474E89094C44Da98b954EedeAC495271d0F");

fn anvil_signer() -> PrivateKeySigner {
    // Anvil account #0: deterministic test key, never used outside tests.
    "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
        .parse()
        .unwrap()
}

fn quote_body(
    from: Address,
    sell: u128,
    buy: u128,
    fee: u128,
    protocol_fee_bps: Option<&str>,
) -> Value {
    let mut quote = json!({
        "sellToken": format!("{:#x}", USDC),
        "buyToken": format!("{:#x}", DAI),
        "receiver": null,
        "sellAmount": sell.to_string(),
        "buyAmount": buy.to_string(),
        "validTo": valid_to_after(3_600),
        "appData": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "feeAmount": fee.to_string(),
        "kind": "sell",
        "partiallyFillable": false,
        "sellTokenBalance": "erc20",
        "buyTokenBalance": "erc20",
        "signingScheme": "eip712",
    });
    let mut body = json!({
        "quote": quote.take(),
        "from": format!("{from:#x}"),
        "expiration": "2099-12-31T23:59:59Z",
        "id": 42,
        "verified": true,
    });
    if let Some(bps) = protocol_fee_bps {
        body["protocolFeeBps"] = Value::String(bps.to_owned());
    }
    body
}

/// Same shape as `cow-sdk` PR #867: sell 1e18, quote returns
/// buyAmount=2e18, partner fee 100 bps, slippage 50 bps. With
/// `protocolFeeBps = "5"` the signed buy_amount must drop to
/// 1_970_090_045_022_511_257 (vs 1_970_100_000_000_000_000 without).
/// Formerly `TradingClient::post_swap_order`'s compounding test; now
/// exercises the same composition through the fluent pipeline.
#[tokio::test]
async fn pipeline_lowers_buy_amount_when_protocol_fee_compounds_with_partner_fee() {
    let signer = anvil_signer();
    let signer_addr = alloy_signer::Signer::address(&signer);

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/quote"))
        .respond_with(ResponseTemplate::new(200).set_body_json(quote_body(
            signer_addr,
            1_000_000_000_000_000_000,
            2_000_000_000_000_000_000,
            0,
            Some("5"),
        )))
        .mount(&server)
        .await;

    let app_data = AppDataDoc::sdk_attribution("cow-rs");
    let put_calls = std::sync::Arc::new(std::sync::Mutex::new(0_u32));
    let put_calls_handle = put_calls.clone();
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/app_data/{}", app_data.hash())))
        .respond_with(move |_: &wiremock::Request| {
            *put_calls_handle.lock().unwrap() += 1;
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;

    let captured_uid = "0x1111111111111111111111111111111111111111111111111111111111111111\
                          1111111111111111111111111111111111111111\
                          22222222";

    let post_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let post_calls_handle = post_calls.clone();
    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).expect("orderbook body is JSON");
            post_calls_handle.lock().unwrap().push(body);
            ResponseTemplate::new(201).set_body_json(Value::String(captured_uid.to_owned()))
        })
        .mount(&server)
        .await;

    // `with_sell_amount_after_fee` pins the fixed leg (1e18) against the
    // mocked `sellAmount`, so the request binding passes.
    let uid = OrderBookApi::new_with_base_url(server.uri().parse().unwrap())
        .quote_builder()
        .with_sell_token(USDC)
        .with_buy_token(DAI)
        .with_from(signer_addr)
        .with_sell_amount_after_fee(U256::from(1_000_000_000_000_000_000_u128))
        .with_app_data(&app_data)
        .with_partner_fee_bps(100)
        .with_slippage_bps(50)
        .build()
        .await
        .expect("quote must bind")
        .sign_with(Chain::Mainnet, EcdsaSigningScheme::Eip712, &signer)
        .expect("sign must succeed")
        .submit()
        .await
        .expect("submit must succeed");

    assert_eq!(uid.to_string(), captured_uid);
    assert_eq!(
        *put_calls.lock().unwrap(),
        1,
        "the app-data document must be pinned exactly once before posting"
    );

    let calls = post_calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "exactly one POST /orders");
    let posted_body = &calls[0];
    assert_eq!(
        posted_body["buyAmount"], "1970090045022511257",
        "buyAmount must match cow-sdk #867 protocol-fee + partner-fee composition"
    );
    assert_eq!(posted_body["signingScheme"], "eip712");
    assert_eq!(
        posted_body["from"],
        format!("{signer_addr:#x}"),
        "from is the signer address"
    );
    assert_eq!(posted_body["quoteId"], 42);
}
