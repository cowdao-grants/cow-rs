//! Networked endpoint bindings.
//!
//! Every binding drives a [`cowprotocol::OrderBookApi`], which on wasm32
//! defaults to the core crate's `fetch`-backed transport
//! (`cowprotocol::FetchTransport`), so request shaping, pagination,
//! status handling and JSON decoding all come from the core crate rather
//! than being re-implemented here. reqwest stays out of the wasm output
//! because the core crate's `http-client` feature resolves to the fetch
//! transport on wasm32 and never links reqwest there.
//!
//! Clients are cheap to build per call: the fetch transport is a unit
//! struct and the base URL is a single join.

use {
    crate::{from_js, js_err, parse_address, parse_chain, parse_typed, parse_uid, to_js},
    alloy_primitives::U256,
    cowprotocol::{
        EMPTY_APP_DATA_HASH, OrderBookApi, OrderCosts, OrderCreation, QuoteRequest,
        SignedOrderCancellation,
    },
    wasm_bindgen::prelude::*,
};

fn parse_u256(value: &str) -> Result<U256, JsValue> {
    parse_typed(value, "u256")
}

/// `POST /api/v1/quote`. Accepts a `QuoteRequest` JSON object.
///
/// [`OrderBookApi::quote`] re-asserts the request-shape invariants
/// ([`QuoteRequest::validate`]) before issuing the request. The
/// hostile-orderbook response binding (cross-checking `sellToken` /
/// `buyToken` / `receiver` / `from` / `kind` / pinned `appData`
/// against the request) runs at the projection chokepoint instead:
/// [`to_signed_order_data`](crate::app_data::to_signed_order_data) and
/// [`build_order_creation`](crate::signing::build_order_creation) both
/// re-run it with the caller's real app-data digest, so checking here
/// with a guessed digest would only reject requests that pin a
/// non-empty `appData`.
#[wasm_bindgen]
pub async fn get_quote(request: JsValue, chain: &str) -> Result<JsValue, JsValue> {
    let request: QuoteRequest = from_js(request)?;
    let response = OrderBookApi::new(parse_chain(chain)?)
        .quote(&request)
        .await
        .map_err(js_err("quote request failed"))?;
    to_js(&response)
}

/// Convenience: same as [`get_quote`] but accepts the four most-common
/// inputs as plain strings and uses `sellAmountBeforeFee`. Returns the
/// raw response plus the derived `OrderUid` (the next signing step's
/// target).
#[wasm_bindgen]
pub async fn get_quote_simple(
    sell_token: &str,
    buy_token: &str,
    from: &str,
    sell_amount_before_fee: &str,
    chain: &str,
) -> Result<JsValue, JsValue> {
    let request = QuoteRequest::sell_before_fee(
        parse_address(sell_token)?,
        parse_address(buy_token)?,
        parse_address(from)?,
        parse_u256(sell_amount_before_fee)?,
    );
    let c = parse_chain(chain)?;
    let response = OrderBookApi::new(c)
        .quote(&request)
        .await
        .map_err(js_err("quote request failed"))?;
    let order_data = response
        .try_to_order_data(&request, EMPTY_APP_DATA_HASH, &OrderCosts::default())
        .map_err(js_err("to_signed_order_data failed"))?;
    let domain = c.settlement_domain();
    let uid = order_data.uid(&domain, response.from);
    let payload = serde_json::json!({
        "response": response,
        "uid": uid.to_string(),
    });
    to_js(&payload)
}

/// `POST /api/v1/orders`. Returns the assigned 56-byte UID.
///
/// The client carries the chain hint, so
/// [`cowprotocol::OrderBookApi::post_order`] owner-verifies the body
/// ([`cowprotocol::OrderCreation::verify_owner`]) before any network
/// call: a hand-assembled body with a typo'd `from` is rejected
/// client-side rather than as a 4xx from the orderbook. The same guard
/// runs at assembly time inside
/// [`build_order_creation`](crate::signing::build_order_creation).
#[wasm_bindgen]
pub async fn post_order(creation: JsValue, chain: &str) -> Result<String, JsValue> {
    let creation: OrderCreation = from_js(creation)?;
    OrderBookApi::new(parse_chain(chain)?)
        .post_order(&creation)
        .await
        .map(|uid| uid.to_string())
        .map_err(js_err("post_order failed"))
}

/// `GET /api/v1/orders/{uid}`.
#[wasm_bindgen]
pub async fn get_order(uid: &str, chain: &str) -> Result<JsValue, JsValue> {
    let order = OrderBookApi::new(parse_chain(chain)?)
        .order(&parse_uid(uid)?)
        .await
        .map_err(js_err("get_order failed"))?;
    to_js(&order)
}

/// `GET /api/v1/orders/{uid}/status`.
#[wasm_bindgen]
pub async fn get_order_status(uid: &str, chain: &str) -> Result<JsValue, JsValue> {
    let status = OrderBookApi::new(parse_chain(chain)?)
        .order_status(&parse_uid(uid)?)
        .await
        .map_err(js_err("get_order_status failed"))?;
    to_js(&status)
}

/// `GET /api/v1/account/{owner}/orders`.
#[wasm_bindgen]
pub async fn account_orders(
    owner: &str,
    chain: &str,
    offset: Option<u32>,
    limit: Option<u32>,
) -> Result<JsValue, JsValue> {
    let orders = OrderBookApi::new(parse_chain(chain)?)
        .account_orders(parse_address(owner)?, offset, limit)
        .await
        .map_err(js_err("account_orders failed"))?;
    to_js(&orders)
}

/// `GET /api/v2/trades?owner=...`. Paginated; omit `offset` / `limit`
/// for the server defaults.
#[wasm_bindgen]
pub async fn trades_by_owner(
    owner: &str,
    chain: &str,
    offset: Option<u32>,
    limit: Option<u32>,
) -> Result<JsValue, JsValue> {
    let trades = OrderBookApi::new(parse_chain(chain)?)
        .trades_by_owner(parse_address(owner)?, offset, limit)
        .await
        .map_err(js_err("trades_by_owner failed"))?;
    to_js(&trades)
}

/// `GET /api/v2/trades?orderUid=...`. Paginated; omit `offset` / `limit`
/// for the server defaults.
#[wasm_bindgen]
pub async fn trades_by_order_uid(
    uid: &str,
    chain: &str,
    offset: Option<u32>,
    limit: Option<u32>,
) -> Result<JsValue, JsValue> {
    let trades = OrderBookApi::new(parse_chain(chain)?)
        .trades_by_order_uid(&parse_uid(uid)?, offset, limit)
        .await
        .map_err(js_err("trades_by_order_uid failed"))?;
    to_js(&trades)
}

/// `GET /api/v1/token/{token}/native_price`.
#[wasm_bindgen]
pub async fn native_price(token: &str, chain: &str) -> Result<JsValue, JsValue> {
    let price = OrderBookApi::new(parse_chain(chain)?)
        .native_price(parse_address(token)?)
        .await
        .map_err(js_err("native_price failed"))?;
    to_js(&price)
}

/// `GET /api/v1/version`.
#[wasm_bindgen]
pub async fn version(chain: &str) -> Result<String, JsValue> {
    OrderBookApi::new(parse_chain(chain)?)
        .version()
        .await
        .map_err(js_err("version failed"))
}

/// `DELETE /api/v1/orders/{uid}`. Caller must construct the signed
/// `SignedOrderCancellation` (via
/// [`build_order_cancellation`](crate::signing::build_order_cancellation)
/// for external wallets, or `cancel_order_signed` in-shim) and pass it
/// here.
#[wasm_bindgen]
pub async fn cancel_order(cancellation: JsValue, chain: &str) -> Result<(), JsValue> {
    let cancellation: SignedOrderCancellation = from_js(cancellation)?;
    OrderBookApi::new(parse_chain(chain)?)
        .cancel_order(&cancellation)
        .await
        .map_err(js_err("cancel_order failed"))
}
