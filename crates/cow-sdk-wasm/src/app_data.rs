//! App-data document bindings: hashing JSON documents, deriving the
//! IPFS CID the orderbook pins, the empty-document sentinel, the SDK
//! attribution document, and projecting a quote response into the
//! signable `OrderData`.

use {
    crate::{from_js, js_err, parse_b256, to_js},
    cowprotocol::{
        AppDataDoc, AppDataHash, EMPTY_APP_DATA_HASH, OrderCosts, QuoteRequest, app_data_cid,
    },
    wasm_bindgen::prelude::*,
};

/// Parse a JSON app-data document and return its keccak256 digest.
/// The caller is responsible for canonicalising the JSON before
/// passing it in (the orderbook indexes documents byte-exactly; any
/// reformatting after this call changes the hash).
#[wasm_bindgen]
pub fn app_data_hash_from_json(canonical_json: &str) -> Result<String, JsValue> {
    let doc = canonical_json
        .parse::<AppDataDoc>()
        .map_err(js_err("parse failed"))?;
    let hash = doc.try_hash().map_err(js_err("hash failed"))?;
    Ok(hash.to_string())
}

/// IPFS CIDv1 the orderbook pins for a given app-data digest.
#[wasm_bindgen]
pub fn app_data_cid_from_hash(hash_hex: &str) -> Result<String, JsValue> {
    let hash: AppDataHash = parse_b256(hash_hex)?;
    Ok(app_data_cid(hash).to_string())
}

/// The three correlated submission artifacts for an app-data document,
/// derived together so they cannot drift: `{ fullAppData, hash, cid }`.
/// `fullAppData` is the canonical JSON to submit, `hash` the digest to
/// embed in `OrderData.app_data` before signing, and `cid` the IPFS CID
/// the orderbook pins. Mirrors the native
/// [`AppDataDoc::prepare`](cowprotocol::AppDataDoc::prepare); prefer it
/// over calling [`app_data_hash_from_json`] and
/// [`app_data_cid_from_hash`] separately. The input is re-canonicalised,
/// so `fullAppData` is what you should submit regardless of the input
/// formatting.
#[wasm_bindgen]
pub fn app_data_prepared(json: &str) -> Result<JsValue, JsValue> {
    let doc = json.parse::<AppDataDoc>().map_err(js_err("parse failed"))?;
    let prepared = doc.prepare().map_err(js_err("prepare failed"))?;
    to_js(&serde_json::json!({
        "fullAppData": prepared.full_app_data,
        "hash": prepared.hash.to_string(),
        "cid": prepared.cid.to_string(),
    }))
}

/// 32-byte digest of `keccak256("{}")`: the empty app-data sentinel.
#[wasm_bindgen]
pub fn empty_app_data_hash() -> String {
    EMPTY_APP_DATA_HASH.to_string()
}

/// Canonical SDK-attribution app-data document JSON, with
/// `appCode: "cow-rs-wasm"` and the wasm crate's version pinned in
/// `metadata.quote.version`. Pass this to
/// [`build_order_creation`](crate::signing::build_order_creation) as
/// the `app_data_json` argument so the orderbook indexer can
/// attribute the order back to this SDK; pair with
/// [`sdk_app_data_hash`] for the signed `appData` field.
#[wasm_bindgen]
pub fn sdk_app_data_json() -> String {
    cowprotocol::AppDataDoc::sdk_attribution(cowprotocol::COW_RS_WASM_APP_CODE).canonical_json()
}

/// 32-byte keccak256 digest of [`sdk_app_data_json`], 0x-prefixed.
/// Embed in [`OrderData`](cowprotocol::OrderData)'s `app_data` field
/// before signing so the wire shape matches what the orderbook will
/// hash server-side.
#[wasm_bindgen]
pub fn sdk_app_data_hash() -> String {
    cowprotocol::AppDataDoc::sdk_attribution(cowprotocol::COW_RS_WASM_APP_CODE)
        .hash()
        .to_string()
}

/// Project an `OrderQuoteResponse` into the 12-field `OrderData` the
/// owner will sign, after cross-checking the response against the
/// originating `QuoteRequest`.
///
/// The single chokepoint for assembling signable bytes from a quote
/// in JS-land. Mirrors the native
/// [`cowprotocol::OrderQuoteResponse::try_to_order_data`] at zero
/// costs (no partner fee, no slippage): rejects
/// any response whose `sellToken`, `buyToken`, normalised `receiver`,
/// `from`, `kind`, or pinned `appData` disagrees with the request,
/// plus `validTo` / `partiallyFillable` / `sellTokenBalance` /
/// `buyTokenBalance` / `signingScheme` when the caller pinned them.
/// Use this instead of hand-copying `response.quote.*` into an
/// `orderData` object before passing it to `eip712_payload` and
/// `build_order_creation`.
///
/// `app_data_hash_hex` is the 32-byte digest of the app-data document
/// the caller will submit (commonly [`sdk_app_data_hash`] for SDK
/// attribution, or [`empty_app_data_hash`] for the empty document).
#[wasm_bindgen]
pub fn to_signed_order_data(
    request: JsValue,
    response: JsValue,
    app_data_hash_hex: &str,
) -> Result<JsValue, JsValue> {
    let request: QuoteRequest = from_js(request)?;
    let response: cowprotocol::OrderQuoteResponse = from_js(response)?;
    let app_data: AppDataHash = parse_b256(app_data_hash_hex)?;
    let order_data = response
        .try_to_order_data(&request, app_data, &OrderCosts::default())
        .map_err(js_err("to_signed_order_data failed"))?;
    to_js(&order_data)
}
