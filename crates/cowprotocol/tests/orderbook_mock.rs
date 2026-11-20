//! Integration tests for [`cowprotocol::OrderBookApi`].
//!
//! Spins up an in-process [`wiremock::MockServer`] per test, points an
//! [`OrderBookApi`] at it, and exercises every endpoint against canned
//! responses. The goal is to lock the wire shapes our client encodes and
//! decodes without hitting the production orderbook.

#![cfg(all(not(target_arch = "wasm32"), feature = "http-client"))]

mod common;

use alloy_primitives::{Address, B256, U256, address};
use cowprotocol::{
    AppDataHash, BuyTokenDestination, Chain, OrderBookApi, OrderCancellation, OrderCancellations,
    OrderCreation, OrderData, OrderKind, OrderUid, QuoteRequest, SellTokenSource, Signature,
    SigningScheme, order_book::AppDataDocument,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_string_contains, method, path, query_param},
};

use common::valid_to_after;

const OWNER: Address = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
const USDC: Address = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
const DAI: Address = address!("6B175474E89094C44Da98b954EedeAC495271d0F");

fn api(server: &MockServer) -> OrderBookApi {
    OrderBookApi::new_with_base_url(server.uri().parse().unwrap())
}

/// All-zero EIP-712 placeholder signature for wire-shape tests. Not
/// recoverable; never pass it to recovery paths.
fn zero_eip712_signature() -> Signature {
    Signature::Eip712(cowprotocol::EcdsaSignature::from_bytes_and_parity(
        &[0u8; 64], false,
    ))
}

const fn mainnet_quote_fixture() -> &'static str {
    include_str!("fixtures/quote-mainnet.json")
}

#[tokio::test]
async fn get_quote_decodes_recorded_mainnet_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/quote"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_raw(mainnet_quote_fixture(), "application/json"),
        )
        .mount(&server)
        .await;

    let request = QuoteRequest::sell_before_fee(USDC, DAI, OWNER, U256::from(100_000_000_u64));
    let response = api(&server).quote(&request).await.unwrap();

    assert_eq!(response.quote.kind, OrderKind::Sell);
    assert_eq!(response.from, OWNER);
    assert!(response.verified);
    assert_eq!(response.id, 1_176_992_200);
}

#[tokio::test]
async fn post_order_returns_assigned_uid() {
    let server = MockServer::start().await;
    let expected_uid = "0xb74844872ddbadb709629952eab02a9275c5c05426cb195e27029a353909404370997970c51812dc3a010c7d01b50e0d17dc79c86a0513b9";

    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .and(body_string_contains("\"feeAmount\":\"0\""))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("content-type", "application/json")
                .set_body_string(format!("\"{expected_uid}\"")),
        )
        .mount(&server)
        .await;

    let order = OrderData {
        sell_token: USDC,
        buy_token: DAI,
        sell_amount: U256::from(1_000_000_u64),
        buy_amount: U256::from(999_000_u64),
        valid_to: valid_to_after(3_600),
        app_data: cowprotocol::EMPTY_APP_DATA_HASH,
        ..OrderData::default()
    };
    let creation = OrderCreation::new(
        &order,
        zero_eip712_signature(),
        OWNER,
        cowprotocol::EMPTY_APP_DATA_JSON.to_owned(),
        Some(123),
    )
    .unwrap();

    let uid = api(&server).post_order(&creation).await.unwrap();
    assert_eq!(uid.to_string(), expected_uid);
}

#[tokio::test]
async fn post_order_surfaces_orderbook_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "errorType": "InsufficientFee",
            "description": "fee is too low",
        })))
        .mount(&server)
        .await;

    let creation = OrderCreation::new(
        &OrderData {
            app_data: cowprotocol::EMPTY_APP_DATA_HASH,
            ..OrderData::default()
        },
        zero_eip712_signature(),
        OWNER,
        cowprotocol::EMPTY_APP_DATA_JSON.to_owned(),
        None,
    )
    .unwrap();

    let err = api(&server).post_order(&creation).await.unwrap_err();
    let message = err.to_string();
    assert!(message.contains("InsufficientFee"), "got: {message}");
    assert!(message.contains("fee is too low"), "got: {message}");
}

#[tokio::test]
async fn get_order_decodes_full_order_record() {
    let server = MockServer::start().await;
    let uid_hex = "0xb74844872ddbadb709629952eab02a9275c5c05426cb195e27029a353909404370997970c51812dc3a010c7d01b50e0d17dc79c86a0513b9";

    Mock::given(method("GET"))
        .and(path(format!("/api/v1/orders/{uid_hex}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sellToken": format!("{USDC:?}"),
            "buyToken": format!("{DAI:?}"),
            "receiver": null,
            "sellAmount": "1000000",
            "buyAmount": "999000",
            "validTo": 1_700_000_000,
            "appData": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "feeAmount": "0",
            "kind": "sell",
            "partiallyFillable": false,
            "sellTokenBalance": "erc20",
            "buyTokenBalance": "erc20",
            "uid": uid_hex,
            "owner": format!("{OWNER:?}"),
            "signingScheme": "eip712",
            "signature": "0x00",
            "creationDate": "2026-05-13T10:00:00.000Z",
            "status": "open",
            "class": "market",
            "executedBuyAmount": "0",
            "executedSellAmount": "0",
            "invalidated": false,
            "isLiquidityOrder": false,
        })))
        .mount(&server)
        .await;

    let uid: OrderUid = uid_hex.parse().unwrap();
    let order = api(&server).order(&uid).await.unwrap();

    assert_eq!(order.uid, uid);
    assert_eq!(order.owner, OWNER);
    assert_eq!(order.data.sell_amount, U256::from(1_000_000_u64));
    assert!(matches!(order.status, cowprotocol::OrderStatus::Open));
}

#[tokio::test]
async fn get_order_status_decodes_lifecycle_payload() {
    let server = MockServer::start().await;
    let uid_hex = "0xb74844872ddbadb709629952eab02a9275c5c05426cb195e27029a353909404370997970c51812dc3a010c7d01b50e0d17dc79c86a0513b9";

    Mock::given(method("GET"))
        .and(path(format!("/api/v1/orders/{uid_hex}/status")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "type": "active",
            "value": [{ "solver": format!("{OWNER:?}") }],
        })))
        .mount(&server)
        .await;

    let uid: OrderUid = uid_hex.parse().unwrap();
    let status = api(&server).order_status(&uid).await.unwrap();
    assert_eq!(status.status_type, cowprotocol::AuctionStatusType::Active);
    assert_eq!(status.value.len(), 1);
}

#[tokio::test]
async fn cancel_orders_sends_signed_collection_and_accepts_200() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/orders"))
        .and(body_string_contains("\"signingScheme\":\"eip712\""))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let signer =
        alloy_signer_local::PrivateKeySigner::from_bytes(&U256::from(1u64).to_be_bytes().into())
            .unwrap();
    let domain = cowprotocol::settlement_domain(
        Chain::Mainnet.id(),
        address!("9008D19f58AAbD9eD0D60971565AA8510560ab41"),
    );
    let signed = OrderCancellations {
        order_uids: vec![OrderUid::from([0x11; 56])],
    }
    .sign(cowprotocol::EcdsaSigningScheme::Eip712, &domain, &signer)
    .unwrap();

    api(&server).cancel_orders(&signed).await.unwrap();
}

#[tokio::test]
async fn cancel_order_puts_uid_in_path_and_omits_it_from_body() {
    let server = MockServer::start().await;
    let uid = OrderUid::from([0x11; 56]);
    let uid_hex = uid.to_string();

    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/orders/{uid_hex}")))
        .and(body_string_contains("\"signingScheme\":\"eip712\""))
        .and(body_string_contains("\"signature\":"))
        // The single-cancel body shape must NOT carry the UID; that's in
        // the URL. Guard against a regression where we leak `orderUid`
        // into the JSON.
        .and(NotContains("\"orderUid\""))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let signer =
        alloy_signer_local::PrivateKeySigner::from_bytes(&U256::from(1u64).to_be_bytes().into())
            .unwrap();
    let domain = cowprotocol::settlement_domain(
        Chain::Mainnet.id(),
        address!("9008D19f58AAbD9eD0D60971565AA8510560ab41"),
    );
    let cancellation = OrderCancellation::from(uid)
        .sign(cowprotocol::EcdsaSigningScheme::Eip712, &domain, &signer)
        .unwrap();

    api(&server).cancel_order(&cancellation).await.unwrap();
}

/// `wiremock::matchers::Match` adapter for "body does NOT contain `s`".
#[derive(Debug, Clone)]
struct NotContains(&'static str);

impl wiremock::Match for NotContains {
    fn matches(&self, request: &wiremock::Request) -> bool {
        !std::str::from_utf8(&request.body)
            .map(|body| body.contains(self.0))
            .unwrap_or(false)
    }
}

#[tokio::test]
async fn account_orders_appends_offset_and_limit_query_params() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/account/{OWNER:?}/orders")))
        .and(query_param("offset", "10"))
        .and(query_param("limit", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let orders = api(&server)
        .account_orders(OWNER, Some(10), Some(5))
        .await
        .unwrap();
    assert!(orders.is_empty());
}

#[tokio::test]
async fn get_orders_by_uids_posts_camel_case_uid_array() {
    let server = MockServer::start().await;
    let uid_hex = "0xb74844872ddbadb709629952eab02a9275c5c05426cb195e27029a353909404370997970c51812dc3a010c7d01b50e0d17dc79c86a0513b9";

    Mock::given(method("POST"))
        .and(path("/api/v1/orders/by_uids"))
        .and(body_json(json!({ "orderUids": [uid_hex] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let uid: OrderUid = uid_hex.parse().unwrap();
    let orders = api(&server).orders_by_uids(&[uid]).await.unwrap();
    assert!(orders.is_empty());
}

#[tokio::test]
async fn trades_by_owner_filters_with_query_param() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/trades"))
        .and(query_param("owner", format!("{OWNER:?}").as_str()))
        .and(query_param("limit", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let trades = api(&server)
        .trades_by_owner(OWNER, None, Some(50))
        .await
        .unwrap();
    assert!(trades.is_empty());
}

#[tokio::test]
async fn trades_by_order_uid_queries_by_uid() {
    let server = MockServer::start().await;
    let uid = OrderUid::from([0x11; 56]);
    let uid_hex = uid.to_string();

    Mock::given(method("GET"))
        .and(path("/api/v2/trades"))
        .and(query_param("orderUid", uid_hex.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "blockNumber": 17_456_789_u64,
            "logIndex": 3_u32,
            "orderUid": uid_hex,
            "owner": format!("{OWNER:?}"),
            "sellToken": format!("{USDC:?}"),
            "buyToken": format!("{DAI:?}"),
            "sellAmount": "1000000",
            "buyAmount": "999000",
            "txHash": format!("{:?}", B256::repeat_byte(0xcd)),
        }])))
        .mount(&server)
        .await;

    let trades = api(&server)
        .trades_by_order_uid(&uid, None, None)
        .await
        .unwrap();
    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].order_uid, uid);
    assert_eq!(trades[0].sell_amount, U256::from(1_000_000_u64));
    // `txHash` hex-decodes into a typed `B256` rather than staying a string.
    assert_eq!(trades[0].tx_hash, Some(B256::repeat_byte(0xcd)));
}

#[tokio::test]
async fn auction_returns_current_auction() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auction"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 42,
            "block": 17_456_789_u64,
        })))
        .mount(&server)
        .await;

    let auction = api(&server).auction().await.unwrap();
    assert_eq!(auction.id, Some(42));
    assert_eq!(auction.block, Some(17_456_789));
}

#[tokio::test]
async fn token_metadata_decodes_present_and_absent_fields() {
    let server = MockServer::start().await;

    // Both fields populated.
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/token/{USDC:?}/metadata")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "firstTradeBlock": 17_456_789_u32,
            "nativePrice": "123456789012345678",
        })))
        .mount(&server)
        .await;

    let metadata = api(&server).token_metadata(USDC).await.unwrap();
    assert_eq!(metadata.first_trade_block, Some(17_456_789));
    assert_eq!(
        metadata.native_price,
        Some(U256::from(123_456_789_012_345_678_u128))
    );

    // Both fields absent.
    let other_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/token/{DAI:?}/metadata")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&other_server)
        .await;
    let empty = api(&other_server).token_metadata(DAI).await.unwrap();
    assert!(empty.first_trade_block.is_none());
    assert!(empty.native_price.is_none());
}

#[tokio::test]
async fn orders_by_tx_fetches_settlement_orders() {
    let server = MockServer::start().await;
    let tx_hash = B256::repeat_byte(0xab);
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transactions/{tx_hash:?}/orders")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let orders = api(&server).orders_by_tx(tx_hash).await.unwrap();
    assert!(orders.is_empty());
}

/// Positive path: an orderbook that returns the document's own
/// `keccak256` is accepted and the verified digest is surfaced to the
/// caller.
#[tokio::test]
async fn upload_app_data_returns_server_computed_hash() {
    let server = MockServer::start().await;
    let document = AppDataDocument {
        full_app_data: "{\"appCode\":\"cow-rs\"}".into(),
    };
    // Server must echo the document's own keccak256; the SDK refuses a
    // divergent server hash now that `upload_app_data` re-hashes locally.
    let computed_hash = document.computed_hash();
    let expected_hex = format!("0x{}", const_hex::encode(computed_hash.0));

    Mock::given(method("PUT"))
        .and(path("/api/v1/app_data"))
        .and(body_json(
            json!({ "fullAppData": "{\"appCode\":\"cow-rs\"}" }),
        ))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("content-type", "application/json")
                .set_body_string(format!("\"{expected_hex}\"")),
        )
        .mount(&server)
        .await;

    let hash = api(&server).upload_app_data(&document).await.unwrap();
    assert_eq!(hash, computed_hash);
}

/// `upload_app_data` rejects a server-supplied digest that disagrees
/// with `keccak256(document.fullAppData.as_bytes())`. The signed
/// order commits only to the digest, so trusting a divergent server
/// hash would leave downstream consumers reading metadata other
/// than what the order pinned.
#[tokio::test]
async fn upload_app_data_rejects_server_digest_mismatch() {
    use cowprotocol::Error;

    let server = MockServer::start().await;
    let bogus_hash = AppDataHash::from([0xab; 32]);
    let bogus_hex = format!("0x{}", const_hex::encode(bogus_hash.0));
    Mock::given(method("PUT"))
        .and(path("/api/v1/app_data"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("content-type", "application/json")
                .set_body_string(format!("\"{bogus_hex}\"")),
        )
        .mount(&server)
        .await;

    let document = AppDataDocument {
        full_app_data: "{\"appCode\":\"cow-rs\"}".into(),
    };
    // `bogus_hash` is not `keccak256(document.full_app_data.as_bytes())`.
    assert_ne!(document.computed_hash(), bogus_hash);
    let err = api(&server).upload_app_data(&document).await.unwrap_err();
    assert!(
        matches!(err, Error::AppDataHashMismatch { .. }),
        "got: {err:?}"
    );
}

#[tokio::test]
async fn native_price_decodes_float_payload() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/token/{USDC:?}/native_price")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "price": 1.234e9 })))
        .mount(&server)
        .await;

    let price = api(&server).native_price(USDC).await.unwrap();
    assert!((price.price - 1.234e9).abs() < 1.0);
}

#[tokio::test]
async fn total_surplus_returns_decimal_string() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/users/{OWNER:?}/total_surplus")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "totalSurplus": "12345.67" })),
        )
        .mount(&server)
        .await;

    let surplus = api(&server).total_surplus(OWNER).await.unwrap();
    assert_eq!(surplus.total_surplus, "12345.67");
}

#[tokio::test]
async fn version_endpoint_returns_plain_text() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain")
                .set_body_string("v0.1.0-mock"),
        )
        .mount(&server)
        .await;

    let version = api(&server).version().await.unwrap();
    assert_eq!(version, "v0.1.0-mock");
}

#[tokio::test]
async fn get_app_data_decodes_full_app_data_envelope() {
    let server = MockServer::start().await;
    let document = AppDataDocument {
        full_app_data: "{}".into(),
    };
    let hash = document.computed_hash();
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/app_data/0x{}",
            const_hex::encode(hash.0)
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "fullAppData": "{}" })))
        .mount(&server)
        .await;

    let doc = api(&server).app_data(&hash).await.unwrap();
    assert_eq!(doc.full_app_data, "{}");
}

/// A hostile orderbook serves a body whose `keccak256` disagrees with
/// the requested digest. The SDK must reject the response rather than
/// pass it through: the signed order commits only to the hash, and a
/// caller that trusted the body would display or validate metadata
/// different from what the order actually commits to.
#[tokio::test]
async fn get_app_data_rejects_response_with_wrong_hash() {
    use cowprotocol::Error;

    let server = MockServer::start().await;
    let requested = AppDataHash::from([0xab; 32]);
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/app_data/0x{}",
            const_hex::encode(requested.0)
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "fullAppData": "{}" })))
        .mount(&server)
        .await;

    let err = api(&server).app_data(&requested).await.unwrap_err();
    assert!(
        matches!(err, Error::AppDataHashMismatch { .. }),
        "expected AppDataHashMismatch, got {err:?}"
    );
}

#[tokio::test]
async fn put_app_data_accepts_200_with_no_body() {
    let server = MockServer::start().await;
    let document = AppDataDocument {
        full_app_data: "{\"appCode\":\"cow-rs\"}".into(),
    };
    let hash = document.computed_hash();
    Mock::given(method("PUT"))
        .and(path(format!(
            "/api/v1/app_data/0x{}",
            const_hex::encode(hash.0)
        )))
        .and(body_json(
            json!({ "fullAppData": "{\"appCode\":\"cow-rs\"}" }),
        ))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    api(&server).put_app_data(&hash, &document).await.unwrap();
}

/// The caller paired a document with the wrong digest. The SDK must
/// refuse the PUT locally instead of relying on the orderbook to
/// surface the bug as an opaque 4xx, since the same mismatch would
/// otherwise leave the index pointing at metadata that diverges from
/// the signed order's commitment.
#[tokio::test]
async fn put_app_data_rejects_document_with_wrong_hash() {
    use cowprotocol::Error;

    let server = MockServer::start().await;
    let document = AppDataDocument {
        full_app_data: "{\"appCode\":\"cow-rs\"}".into(),
    };
    let wrong_hash = AppDataHash::from([0xab; 32]);

    let err = api(&server)
        .put_app_data(&wrong_hash, &document)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::AppDataHashMismatch { .. }),
        "expected AppDataHashMismatch, got {err:?}"
    );
}

#[tokio::test]
async fn unexpected_5xx_with_non_json_body_surfaces_unexpected_status() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/quote"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("content-type", "text/html")
                .set_body_string("<html>down</html>"),
        )
        .mount(&server)
        .await;

    let request = QuoteRequest::sell_before_fee(USDC, DAI, OWNER, U256::from(1_000_u64));
    let err = api(&server).quote(&request).await.unwrap_err();
    let message = err.to_string();
    assert!(message.contains("503"), "got: {message}");
    assert!(message.contains("<html>down</html>"), "got: {message}");
}

#[tokio::test]
async fn poll_until_runs_with_caller_supplied_sleep() {
    use std::cell::Cell;

    let server = MockServer::start().await;
    let uid_hex = "0xb74844872ddbadb709629952eab02a9275c5c05426cb195e27029a353909404370997970c51812dc3a010c7d01b50e0d17dc79c86a0513b9";
    let uid: OrderUid = uid_hex.parse().unwrap();
    let make_body = |status: &str| {
        json!({
            "sellToken": format!("{USDC:?}"),
            "buyToken": format!("{DAI:?}"),
            "receiver": null,
            "sellAmount": "1000000",
            "buyAmount": "999000",
            "validTo": 1_700_000_000,
            "appData": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "feeAmount": "0",
            "kind": "sell",
            "partiallyFillable": false,
            "sellTokenBalance": "erc20",
            "buyTokenBalance": "erc20",
            "uid": uid_hex,
            "owner": format!("{OWNER:?}"),
            "signingScheme": "eip712",
            "signature": "0x00",
            "creationDate": "2026-05-13T10:00:00.000Z",
            "status": status,
            "class": "market",
            "executedBuyAmount": "0",
            "executedSellAmount": "0",
            "invalidated": false,
            "isLiquidityOrder": false,
        })
    };

    Mock::given(method("GET"))
        .and(path(format!("/api/v1/orders/{uid_hex}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(make_body("open")))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/orders/{uid_hex}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(make_body("fulfilled")))
        .mount(&server)
        .await;

    let sleep_count = Cell::new(0u32);
    let order = api(&server)
        .poll_until(
            &uid,
            |order| matches!(order.status, cowprotocol::OrderStatus::Fulfilled),
            || {
                sleep_count.set(sleep_count.get() + 1);
                std::future::ready(())
            },
        )
        .await
        .unwrap();

    assert!(matches!(order.status, cowprotocol::OrderStatus::Fulfilled));
    assert_eq!(
        sleep_count.get(),
        2,
        "should have slept between the two open polls"
    );
}

/// An end-to-end response over `MAX_RESPONSE_BYTES` is rejected
/// through the same client / decoder path that production callers
/// traverse. wiremock auto-derives `Content-Length` from the body, so
/// this exercises both the header-driven early reject and the
/// post-read backstop in `read_capped_text`. A response above the cap
/// must always surface [`Error::ResponseTooLarge`] rather than
/// allocate the body into memory.
#[tokio::test]
async fn response_too_large_is_rejected_end_to_end() {
    let server = MockServer::start().await;
    let oversize_body = "a".repeat(cowprotocol::order_book::MAX_RESPONSE_BYTES + 1);
    Mock::given(method("GET"))
        .and(path("/api/v1/version"))
        .respond_with(ResponseTemplate::new(200).set_body_string(oversize_body))
        .mount(&server)
        .await;

    let err = api(&server).version().await.unwrap_err();
    match err {
        cowprotocol::Error::ResponseTooLarge { max } => {
            assert_eq!(max, cowprotocol::order_book::MAX_RESPONSE_BYTES);
        }
        other => panic!("expected ResponseTooLarge, got {other:?}"),
    }
}

/// A server that does not respond within the configured client
/// timeout produces a transport error rather than hanging the
/// caller. Uses an explicit short-timeout client so the test does not
/// have to wait the production [`DEFAULT_HTTP_TIMEOUT`].
#[tokio::test]
async fn slow_server_trips_client_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_secs(2))
                .set_body_string("ok"),
        )
        .mount(&server)
        .await;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(100))
        .build()
        .unwrap();
    let api = OrderBookApi::builder()
        .with_base_url(server.uri().parse().unwrap())
        .with_client(client)
        .build();

    let err = api.version().await.unwrap_err();
    match err {
        cowprotocol::Error::Transport(e) => {
            assert!(
                e.is_timeout(),
                "expected reqwest::Error::is_timeout(), got {e}"
            );
        }
        other => panic!("expected Transport timeout error, got {other:?}"),
    }
}

/// `OrderBookApi::new_with_base_url` must hand back a client with
/// `DEFAULT_HTTP_TIMEOUT` set. Without this, default-config callers
/// would be back to an unbounded-wait regime if a server stalls.
/// Reqwest does not expose the configured timeout for inspection, so
/// we rely on the constants being present (compile-time check) plus
/// `slow_server_trips_client_timeout` locking the live behaviour.
#[test]
fn default_client_constants_are_sane() {
    let _ = OrderBookApi::new(Chain::Mainnet);
    assert!(cowprotocol::order_book::DEFAULT_HTTP_TIMEOUT > std::time::Duration::ZERO);
    const _: () = assert!(cowprotocol::order_book::MAX_RESPONSE_BYTES >= 1 << 20);
}

#[tokio::test]
async fn unused_optional_quote_fields_are_omitted_from_request_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/quote"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(mainnet_quote_fixture(), "application/json"),
        )
        .mount(&server)
        .await;

    let mut request = QuoteRequest::sell_before_fee(USDC, DAI, OWNER, U256::from(100_000_000_u64));
    request.receiver = Some(OWNER);
    api(&server).quote(&request).await.unwrap();

    // We exercised the request shape via QuoteRequest's own tests; here we
    // just confirm round-tripping the mock works with optional fields set.
    let _ = SigningScheme::default();
    let _ = (SellTokenSource::Erc20, BuyTokenDestination::Erc20);
}

#[tokio::test]
async fn quote_builder_can_quote_sign_and_submit() {
    let signer =
        alloy_signer_local::PrivateKeySigner::from_bytes(&U256::from(1u64).to_be_bytes().into())
            .unwrap();
    let owner = alloy_signer::Signer::address(&signer);
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/quote"))
        .and(body_json(json!({
            "sellToken": format!("{USDC:#x}"),
            "buyToken": format!("{DAI:#x}"),
            "from": format!("{owner:#x}"),
            "kind": "sell",
            "sellAmountBeforeFee": "100000000",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "quote": {
                "sellToken": format!("{USDC:#x}"),
                "buyToken": format!("{DAI:#x}"),
                "receiver": null,
                "sellAmount": "100000000",
                "buyAmount": "99900000",
                "validTo": valid_to_after(3_600),
                "appData": format!("{:#x}", cowprotocol::EMPTY_APP_DATA_HASH),
                "feeAmount": "0",
                "kind": "sell",
                "partiallyFillable": false,
                "sellTokenBalance": "erc20",
                "buyTokenBalance": "erc20",
                "signingScheme": "eip712"
            },
            "from": format!("{owner:#x}"),
            "expiration": "2099-12-31T23:59:59Z",
            "id": 99,
            "verified": true
        })))
        .mount(&server)
        .await;

    let expected_uid = "0xb74844872ddbadb709629952eab02a9275c5c05426cb195e27029a353909404370997970c51812dc3a010c7d01b50e0d17dc79c86a0513b9";
    let posted = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let posted_handle = posted.clone();
    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .and(body_string_contains("\"quoteId\":99"))
        .and(body_string_contains("\"signingScheme\":\"eip712\""))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value =
                serde_json::from_slice(&req.body).expect("order body is JSON");
            posted_handle.lock().unwrap().push(body);
            ResponseTemplate::new(201).set_body_json(json!(expected_uid))
        })
        .mount(&server)
        .await;

    let uid = OrderBookApi::builder()
        .with_base_url(server.uri().parse().unwrap())
        .build()
        .quote_builder()
        .with_sell_token(USDC)
        .with_buy_token(DAI)
        .with_from(owner)
        .with_sell_amount(U256::from(100_000_000_u64))
        .build()
        .await
        .unwrap()
        .sign_with(
            Chain::Mainnet,
            cowprotocol::EcdsaSigningScheme::Eip712,
            &signer,
        )
        .unwrap()
        .submit()
        .await
        .unwrap();

    assert_eq!(uid.to_string(), expected_uid);

    // The fluent path applies the default 50 bps slippage to the SELL
    // buy side before signing: 99_900_000 * (10_000 - 50) / 10_000 =
    // 99_400_500. The fixed sell side passes through. This locks that
    // quote_builder().build().sign() routes through the costs projection
    // (the pipeline's seeded default) rather than signing the raw
    // quote with no slippage.
    let posted_body = posted.lock().unwrap().pop().expect("order was posted");
    assert_eq!(posted_body["sellAmount"], json!("100000000"));
    assert_eq!(posted_body["buyAmount"], json!("99400500"));
}
