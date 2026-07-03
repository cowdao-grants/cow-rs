//! Unit tests for the orderbook module, grouped by concern:
//! transport response cap, `http-client` constructor wiring, wire
//! shapes, the quote-to-order amount projection, request binding,
//! request validation, and the quote -> sign -> submit pipeline.

use crate::app_data::{AppDataHash, EMPTY_APP_DATA_HASH, EMPTY_APP_DATA_JSON};
use crate::error::Error;
use crate::order::{BuyTokenDestination, OrderKind, SellTokenSource};
use crate::quote_amounts::OrderCosts;
use crate::signature::Signature;
// Imported directly rather than via `super::*` because the
// module-level re-export is gated behind `http-client`; the
// signing tests below need it regardless of that feature.
use crate::signing_scheme::{EcdsaSigningScheme, SigningScheme};

use super::*;
use alloy_primitives::{Address, U256, address};

/// Token addresses used by [`fixture_quote_request`]: USDC and DAI on
/// Ethereum mainnet.
const USDC: Address = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
const DAI: Address = address!("6B175474E89094C44Da98b954EedeAC495271d0F");
const OWNER: Address = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");

fn fixture_quote_request() -> QuoteRequest {
    QuoteRequest::sell_before_fee(USDC, DAI, OWNER, U256::from(100_000_000_u64))
}

fn load_mainnet_quote() -> OrderQuoteResponse {
    serde_json::from_str(include_str!(
        "../../../cowprotocol/tests/fixtures/quote-mainnet.json"
    ))
    .unwrap()
}

/// The streaming response-size guard in the native reqwest transport.
#[cfg(all(feature = "http-client", not(target_arch = "wasm32")))]
mod transport_cap {
    use super::*;
    use crate::transport::reqwest::read_capped_body;

    /// The streaming guard in `read_capped_body` rejects an oversize body
    /// as it accumulates, independent of the `Content-Length` early-reject
    /// in `read_capped_text` (called directly here to bypass that
    /// early-out, which is what a chunked / length-less response does).
    #[tokio::test]
    async fn read_capped_body_rejects_oversize_stream() {
        let body = vec![b'a'; MAX_RESPONSE_BYTES + 1];
        let response = reqwest::Response::from(http::Response::new(body));
        let err = read_capped_body(response).await.unwrap_err();
        assert!(
            matches!(err, Error::ResponseTooLarge { .. }),
            "got: {err:?}"
        );
    }

    /// A body exactly at the cap streams through, proving the guard fires
    /// at `+1`, not `=`.
    #[tokio::test]
    async fn read_capped_body_accepts_at_cap() {
        let body = vec![b'a'; MAX_RESPONSE_BYTES];
        let response = reqwest::Response::from(http::Response::new(body));
        let text = read_capped_body(response).await.unwrap();
        assert_eq!(text.len(), MAX_RESPONSE_BYTES);
    }
}

/// Constructor and base-URL wiring behind the `http-client` feature,
/// plus client-internal wire bodies.
#[cfg(feature = "http-client")]
mod client_config {
    use super::super::api::OrdersByUidsRequest;
    use super::*;
    use crate::chain::Chain;
    use crate::order::OrderUid;

    // Delegation test for the deprecated `OrderBookApi::with_chain`
    // shortcut: it must still resolve to the same chain hint and base URL
    // as the builder path it forwards to.
    #[test]
    #[allow(deprecated)]
    fn deprecated_with_chain_delegates_to_builder() {
        let api = OrderBookApi::with_chain(Chain::Gnosis).build();

        assert_eq!(api.chain(), Some(Chain::Gnosis));
        assert_eq!(api.base_url().as_str(), "https://api.cow.fi/xdai/");
    }

    // Delegation test for the deprecated `OrderBookApi::with_client`
    // shortcut: it must still resolve to the same base URL and chainless
    // client as the advanced-HTTP builder path it is superseded by.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[allow(deprecated)]
    fn deprecated_with_client_delegates_to_builder() {
        let url = url::Url::parse("https://example.test/orderbook/").unwrap();
        let api = OrderBookApi::with_client(url.clone(), reqwest::Client::new());

        assert_eq!(api.chain(), None);
        assert_eq!(api.base_url(), &url);
    }

    #[test]
    fn new_sets_chain_hint_and_base_url() {
        let api = OrderBookApi::new(Chain::Gnosis);

        assert_eq!(api.chain(), Some(Chain::Gnosis));
        assert_eq!(api.base_url().as_str(), "https://api.cow.fi/xdai/");
    }

    #[test]
    fn base_url_gets_trailing_slash_added() {
        let api = OrderBookApi::new_with_base_url(
            url::Url::parse("https://example.test/orderbook").unwrap(),
        );
        assert!(api.base_url().path().ends_with('/'));
        let endpoint = api.base_url().join("api/v1/quote").unwrap();
        assert_eq!(endpoint.path(), "/orderbook/api/v1/quote");
    }

    #[test]
    fn chain_base_url_composes_correctly() {
        let api = OrderBookApi::new(Chain::Mainnet);
        let endpoint = api.base_url().join("api/v1/quote").unwrap();
        assert_eq!(endpoint.as_str(), "https://api.cow.fi/mainnet/api/v1/quote");
    }

    #[test]
    fn orders_by_uids_request_serialises_with_camel_case_key() {
        let uids = vec![OrderUid::from([0x11; 56])];
        let req = OrdersByUidsRequest { order_uids: &uids };
        let body = serde_json::to_value(&req).unwrap();
        assert!(body["orderUids"].is_array());
    }
}

/// Serialisation contracts: the JSON wire shapes of requests, responses,
/// and `OrderCreation`, locked against drift.
mod wire_shape {
    use super::*;

    /// All-zero EIP-712 placeholder signature for wire-shape tests. Not
    /// recoverable; never pass it to recovery paths.
    fn zero_eip712_signature() -> Signature {
        Signature::Eip712(crate::signature::EcdsaSignature::from_bytes_and_parity(
            &[0u8; 64], false,
        ))
    }

    #[test]
    fn quote_request_builder_matches_constructor_wire_shape() {
        let request = pipeline::mock_api(&pipeline::MockTransport::default())
            .quote_builder()
            .with_sell_token(USDC)
            .with_buy_token(DAI)
            .with_from(OWNER)
            .with_sell_amount(U256::from(100_000_000_u64))
            .into_request();

        assert_eq!(request.kind(), OrderKind::Sell);
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::to_value(fixture_quote_request()).unwrap()
        );
    }

    #[test]
    fn quote_request_emits_app_data_hash_form() {
        let mut request = fixture_quote_request();
        request.app_data = Some(QuoteAppData::Hash(crate::EMPTY_APP_DATA_HASH));
        let body = serde_json::to_value(request).unwrap();
        assert_eq!(
            body["appData"],
            serde_json::Value::String(
                "0xb48d38f93eaa084033fc5970bf96e559c33c4cdc07d889ab00b4d63f9590739d".to_owned()
            )
        );
        // QuoteRequest's `appData` field is overloaded by content; there
        // is no separate `appDataHash` key on the quote wire shape.
        assert!(body.get("appDataHash").is_none());
    }

    #[test]
    fn quote_request_emits_app_data_full_form() {
        let mut request = fixture_quote_request();
        request.app_data = Some(QuoteAppData::Full(crate::EMPTY_APP_DATA_JSON.to_owned()));
        let body = serde_json::to_value(request).unwrap();
        assert_eq!(body["appData"], serde_json::Value::String("{}".to_owned()));
        assert!(body.get("appDataHash").is_none());
    }

    #[test]
    fn quote_request_round_trips_price_quality_field() {
        let mut request = QuoteRequest::sell_before_fee(USDC, DAI, OWNER, U256::from(1_u64));
        request.price_quality = Some(PriceQuality::Verified);
        let body = serde_json::to_value(&request).unwrap();
        assert_eq!(body["priceQuality"], "verified");
    }

    #[test]
    fn price_quality_serialises_lowercase() {
        for (variant, wire) in [
            (PriceQuality::Fast, "\"fast\""),
            (PriceQuality::Optimal, "\"optimal\""),
            (PriceQuality::Verified, "\"verified\""),
        ] {
            let serialised = serde_json::to_string(&variant).unwrap();
            assert_eq!(serialised, wire);
            let parsed: PriceQuality = serde_json::from_str(wire).unwrap();
            assert_eq!(parsed, variant);
        }
    }

    #[test]
    fn quote_request_serialises_to_expected_wire_shape() {
        let body = serde_json::to_value(fixture_quote_request()).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "sellToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
                "buyToken": "0x6b175474e89094c44da98b954eedeac495271d0f",
                "from": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
                "kind": "sell",
                "sellAmountBeforeFee": "100000000",
            })
        );
    }

    #[test]
    fn quote_request_includes_buy_kind_when_built_with_buy_amount() {
        let request = QuoteRequest::buy_after_fee(USDC, DAI, OWNER, U256::from(1_000_u64));
        let body = serde_json::to_value(request).unwrap();
        assert_eq!(body["kind"], serde_json::Value::String("buy".into()));
        assert_eq!(
            body["buyAmountAfterFee"],
            serde_json::Value::String("1000".into())
        );
        assert!(body.get("sellAmountBeforeFee").is_none());
    }

    #[test]
    fn quote_request_emits_valid_for_and_eip1271_extras() {
        let mut request = fixture_quote_request();
        request.valid_for = Some(1_800);
        request.signing_scheme = Some(SigningScheme::Eip1271);
        request.verification_gas_limit = Some(50_000);
        request.onchain_order = Some(true);
        let body = serde_json::to_value(request).unwrap();
        assert_eq!(body["validFor"], serde_json::Value::from(1_800));
        assert_eq!(
            body["signingScheme"],
            serde_json::Value::String("eip1271".into())
        );
        assert_eq!(
            body["verificationGasLimit"],
            serde_json::Value::from(50_000)
        );
        assert_eq!(body["onchainOrder"], serde_json::Value::from(true));
        // validTo stays absent when only validFor is set.
        assert!(body.get("validTo").is_none());
    }

    #[test]
    fn native_price_deserialises_float_number() {
        let body = serde_json::json!({ "price": 1.23e9 });
        let parsed: NativePrice = serde_json::from_value(body).unwrap();
        assert!((parsed.price - 1.23e9).abs() < 1.0);
    }

    #[test]
    fn app_data_document_round_trips() {
        let doc = AppDataDocument {
            full_app_data: "{}".into(),
        };
        let json = serde_json::to_value(&doc).unwrap();
        assert_eq!(json, serde_json::json!({ "fullAppData": "{}" }));
        let parsed: AppDataDocument = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.full_app_data, "{}");
    }

    #[test]
    fn total_surplus_keeps_decimal_string() {
        let body = serde_json::json!({ "totalSurplus": "1234567.89" });
        let parsed: TotalSurplus = serde_json::from_value(body).unwrap();
        assert_eq!(parsed.total_surplus, "1234567.89");
    }

    /// Locks the deserialisation of an `OrderQuoteResponse` against a real
    /// body captured from the production mainnet orderbook
    /// (`tools/vector-gen` is not involved; the fixture is the raw HTTP
    /// response). Catches any drift in the wire schema.
    #[test]
    fn deserialise_mainnet_quote_fixture() {
        let body = include_str!("../../../cowprotocol/tests/fixtures/quote-mainnet.json");
        let response: OrderQuoteResponse = serde_json::from_str(body).unwrap();
        assert_eq!(response.from, OWNER);
        assert!(response.verified);
        assert_eq!(response.quote.sell_token, USDC);
        assert_eq!(response.quote.buy_token, DAI);
        assert_eq!(response.quote.kind, OrderKind::Sell);
        assert_eq!(response.quote.signing_scheme, SigningScheme::Eip712);
        assert_eq!(response.quote.app_data, AppDataHash::default());

        // The order data projected from the quote round-trips into the
        // signed-payload type and hashes deterministically.
        let order_data = response
            .try_to_order_data(
                &fixture_quote_request(),
                EMPTY_APP_DATA_HASH,
                &OrderCosts::default(),
            )
            .unwrap();
        let _ = order_data.hash_struct();
    }

    /// `OrderCreation` serialises to the wire shape documented by the
    /// orderbook OpenAPI: 12 signed fields, plus `appData` (JSON string),
    /// `appDataHash` (bytes32), `signingScheme`, `signature`, `from` and
    /// optional `quoteId`. Verifies the field-name overrides that make
    /// `OrderCreation` distinct from a flattened `OrderData`.
    #[test]
    fn order_creation_serialises_to_expected_wire_shape() {
        let quote = load_mainnet_quote();
        let signed = quote
            .try_to_order_data(
                &fixture_quote_request(),
                EMPTY_APP_DATA_HASH,
                &OrderCosts::default(),
            )
            .unwrap();
        let signature = zero_eip712_signature();
        let creation = OrderCreation::new(
            &signed,
            signature,
            quote.from,
            EMPTY_APP_DATA_JSON.to_owned(),
            Some(quote.id),
        )
        .unwrap();

        let body = serde_json::to_value(&creation).unwrap();
        assert_eq!(body["feeAmount"], "0");
        assert_eq!(body["appData"], "{}");
        assert_eq!(
            body["appDataHash"],
            "0xb48d38f93eaa084033fc5970bf96e559c33c4cdc07d889ab00b4d63f9590739d"
        );
        assert_eq!(body["signingScheme"], "eip712");
        assert!(body["signature"].as_str().unwrap().starts_with("0x"));
        assert_eq!(body["from"], format!("{:?}", quote.from).to_lowercase());
        assert_eq!(body["quoteId"], 1_176_992_200_i64);
        assert!(body["sellAmount"].is_string());
        // Sell-side adjustment is visible in the serialised body.
        let expected_sell = quote.quote.sell_amount + quote.quote.fee_amount;
        assert_eq!(body["sellAmount"], expected_sell.to_string());
    }

    /// JSON round-trip for [`OrderCreation`]. Deserialising what we
    /// serialise lets wasm / JS consumers hand the type back across the
    /// boundary without losing fields.
    fn round_trip_with_signature(signature: Signature) -> OrderCreation {
        let quote = load_mainnet_quote();
        let signed = quote
            .try_to_order_data(
                &fixture_quote_request(),
                EMPTY_APP_DATA_HASH,
                &OrderCosts::default(),
            )
            .unwrap();
        let original = OrderCreation::new(
            &signed,
            signature,
            quote.from,
            EMPTY_APP_DATA_JSON.to_owned(),
            Some(quote.id),
        )
        .unwrap();
        let json = serde_json::to_string(&original).unwrap();
        let parsed: OrderCreation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.sell_token, original.sell_token);
        assert_eq!(parsed.buy_token, original.buy_token);
        assert_eq!(parsed.sell_amount, original.sell_amount);
        assert_eq!(parsed.buy_amount, original.buy_amount);
        assert_eq!(parsed.from, original.from);
        assert_eq!(parsed.quote_id, original.quote_id);
        assert_eq!(parsed.app_data, original.app_data);
        assert_eq!(parsed.app_data_hash, original.app_data_hash);
        assert_eq!(parsed.signing_scheme, original.signing_scheme);
        parsed
    }

    #[test]
    fn order_creation_json_round_trip() {
        let parsed = round_trip_with_signature(zero_eip712_signature());
        assert!(matches!(parsed.signature, Signature::Eip712(_)));
    }

    /// EthSign round-trip exercises the 65-byte EcdsaSignature path with a
    /// non-zero v; `parse_ecdsa` normalises 0 / 27 to 27 and 1 / 28 to
    /// 28, so we use 27 here to keep the wire shape stable.
    #[test]
    fn order_creation_json_round_trip_ethsign() {
        let bytes = {
            let mut buf = [0u8; 65];
            buf[64] = 27;
            buf
        };
        let signature = Signature::from_ecdsa(
            crate::signature::parse_ecdsa(&bytes).unwrap(),
            EcdsaSigningScheme::EthSign,
        );
        let parsed = round_trip_with_signature(signature);
        match &parsed.signature {
            Signature::EthSign(sig) => assert_eq!(sig.as_bytes(), bytes),
            other => panic!("expected EthSign, got {other:?}"),
        }
    }

    /// Eip1271 carries variable-length wrapper bytes; the round trip needs
    /// to preserve them exactly (length and content).
    #[test]
    fn order_creation_json_round_trip_eip1271() {
        let payload: Vec<u8> = (0..32).collect();
        let signature = Signature::Eip1271(payload.clone());
        let parsed = round_trip_with_signature(signature);
        match &parsed.signature {
            Signature::Eip1271(bytes) => assert_eq!(bytes, &payload),
            other => panic!("expected Eip1271, got {other:?}"),
        }
    }

    /// PreSign carries no bytes; the on-wire `signature` is the empty hex
    /// string `0x`. Make sure the round trip survives that.
    #[test]
    fn order_creation_json_round_trip_presign() {
        let parsed = round_trip_with_signature(Signature::PreSign);
        assert!(matches!(parsed.signature, Signature::PreSign));
    }

    /// JSON round-trip for [`QuoteRequest`]: serialise, deserialise,
    /// serialise again, compare the JSON values. Locks the wire shape
    /// against drift introduced by future serde-attribute tweaks.
    #[test]
    fn quote_request_json_round_trip() {
        let original = fixture_quote_request();
        let first = serde_json::to_value(&original).unwrap();
        let parsed: QuoteRequest = serde_json::from_value(first.clone()).unwrap();
        let second = serde_json::to_value(&parsed).unwrap();
        assert_eq!(first, second);
    }

    /// `quoteId` is omitted when not provided rather than emitted as `null`.
    #[test]
    fn order_creation_skips_optional_quote_id() {
        let quote = load_mainnet_quote();
        let signed = quote
            .try_to_order_data(
                &fixture_quote_request(),
                EMPTY_APP_DATA_HASH,
                &OrderCosts::default(),
            )
            .unwrap();
        let creation = OrderCreation::new(
            &signed,
            zero_eip712_signature(),
            quote.from,
            EMPTY_APP_DATA_JSON.to_owned(),
            None,
        )
        .unwrap();
        let body = serde_json::to_value(&creation).unwrap();
        assert!(body.get("quoteId").is_none());
    }

    /// `OrderQuoteResponse::expiration` is preserved verbatim through a
    /// JSON round-trip. Fix 3 (Option A) leaves the field as an opaque
    /// `String`; this test pins the contract so a future typed parse
    /// cannot silently mutate the wire shape.
    #[test]
    fn order_quote_response_expiration_round_trips_verbatim() {
        let mut quote = load_mainnet_quote();
        quote.expiration = "2026-05-27T12:34:56.789Z".to_owned();
        let json = serde_json::to_value(&quote).unwrap();
        assert_eq!(json["expiration"], "2026-05-27T12:34:56.789Z");
        let parsed: OrderQuoteResponse = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.expiration, "2026-05-27T12:34:56.789Z");
    }
}

/// The quote-to-order amount projection
/// ([`OrderQuoteResponse::try_to_order_data`] through
/// [`crate::quote_amounts::compute`]): fee folding, parity with the TS
/// reference, and fail-closed overflow handling.
mod projection {
    use super::*;

    /// `try_to_order_data` for a sell-side quote adds `feeAmount` back
    /// into `sellAmount` and zeroes the fee: the documented submission
    /// adjustment.
    #[test]
    fn try_to_order_data_adjusts_sell_amount_and_zeroes_fee() {
        let quote = load_mainnet_quote();
        assert_eq!(quote.quote.kind, OrderKind::Sell);
        let original_sell = quote.quote.sell_amount;
        let original_fee = quote.quote.fee_amount;

        let signed = quote
            .try_to_order_data(
                &fixture_quote_request(),
                EMPTY_APP_DATA_HASH,
                &OrderCosts::default(),
            )
            .unwrap();

        assert_eq!(signed.sell_amount, original_sell + original_fee);
        assert_eq!(signed.buy_amount, quote.quote.buy_amount);
        assert_eq!(signed.fee_amount, U256::ZERO);
        assert_eq!(signed.app_data, EMPTY_APP_DATA_HASH);
        assert_eq!(signed.kind, OrderKind::Sell);
    }

    /// Buy-side quote at zero costs folds `feeAmount` into the signed
    /// `sellAmount`, matching the TS reference (`getQuoteAmountsAndCosts`
    /// adds network costs to the BUY sell side; the orderbook reports them
    /// outside `sellAmount`). This is the deliberate behavioural fix over
    /// the old basic projection, which signed the bare `sellAmount` and
    /// produced under-funded BUY orders whenever `feeAmount` was non-zero.
    #[test]
    fn try_to_order_data_buy_side_folds_fee_into_signed_sell() {
        let mut quote = load_mainnet_quote();
        quote.quote.kind = OrderKind::Buy;
        let original_sell = quote.quote.sell_amount;
        let original_fee = quote.quote.fee_amount;
        let original_buy = quote.quote.buy_amount;
        assert!(
            original_fee > U256::ZERO,
            "fixture must carry a network fee"
        );
        // Match the mutated `kind` and the quote's fixed buy leg so the
        // request-bound projection does not reject this synthetic
        // Buy-side quote.
        let request = QuoteRequest::buy_after_fee(USDC, DAI, OWNER, original_buy);

        let signed = quote
            .try_to_order_data(&request, EMPTY_APP_DATA_HASH, &OrderCosts::default())
            .unwrap();

        assert_eq!(signed.sell_amount, original_sell + original_fee);
        assert_eq!(signed.buy_amount, original_buy);
        assert_eq!(signed.fee_amount, U256::ZERO);
    }

    /// BUY-at-zero-costs vector against the TS reference: with no partner
    /// fee, no slippage, and the quote's own `protocolFeeBps`, the signed
    /// pair is exactly `(sellAmount + feeAmount, buyAmount)`. The
    /// protocol-fee leg cancels through `beforeAllFees` and
    /// `afterProtocolFees`, so its presence must not perturb the amounts.
    #[test]
    fn try_to_order_data_buy_at_zero_costs_matches_ts_vector() {
        let mut quote = load_mainnet_quote();
        quote.quote.kind = OrderKind::Buy;
        quote.quote.sell_amount = U256::from(1_000_000_000_000_000_000_u128);
        quote.quote.buy_amount = U256::from(2_000_000_000_000_000_000_u128);
        quote.quote.fee_amount = U256::from(1_000_000_000_000_000_u128);
        assert_eq!(
            quote.protocol_fee_bps,
            Some("0.3".parse().unwrap()),
            "fixture pins a non-zero protocolFeeBps"
        );
        let request = QuoteRequest::buy_after_fee(USDC, DAI, OWNER, quote.quote.buy_amount);

        let signed = quote
            .try_to_order_data(&request, EMPTY_APP_DATA_HASH, &OrderCosts::default())
            .unwrap();

        assert_eq!(
            signed.sell_amount,
            U256::from(1_001_000_000_000_000_000_u128),
            "signed sell must be sellAmount + feeAmount",
        );
        assert_eq!(
            signed.buy_amount,
            U256::from(2_000_000_000_000_000_000_u128)
        );
    }

    /// At `OrderCosts::default()` the unified SELL projection equals the
    /// old basic path exactly: `(sellAmount + feeAmount, buyAmount)`,
    /// fee zeroed. Locks the equivalence so routing the basic path through
    /// `quote_amounts::compute` cannot drift the signed amounts.
    #[test]
    fn try_to_order_data_sell_at_default_costs_equals_old_basic_path() {
        let quote = load_mainnet_quote();
        assert_eq!(quote.quote.kind, OrderKind::Sell);

        let signed = quote
            .try_to_order_data(
                &fixture_quote_request(),
                EMPTY_APP_DATA_HASH,
                &OrderCosts::default(),
            )
            .unwrap();

        // The old basic path: fold the fee into the sell side, pass the buy
        // side through, zero the fee.
        let old_sell = quote.quote.sell_amount + quote.quote.fee_amount;
        assert_eq!(signed.sell_amount, old_sell);
        assert_eq!(signed.buy_amount, quote.quote.buy_amount);
        assert_eq!(signed.fee_amount, U256::ZERO);
    }

    /// The unified projection inherits `compute`'s fail-closed guard on a
    /// degenerate `sellAmount = 0` quote, which the old basic path
    /// tolerated. Such a quote never describes a settleable order.
    #[test]
    fn try_to_order_data_rejects_zero_sell_amount_quote() {
        let mut quote = load_mainnet_quote();
        quote.quote.kind = OrderKind::Buy;
        quote.quote.sell_amount = U256::ZERO;
        let request = QuoteRequest::buy_after_fee(USDC, DAI, OWNER, quote.quote.buy_amount);
        let err = quote
            .try_to_order_data(&request, EMPTY_APP_DATA_HASH, &OrderCosts::default())
            .unwrap_err();
        assert!(matches!(err, Error::QuoteSellAmountZero), "got: {err:?}");
    }

    /// `try_to_order_data` rejects an overflowing sell-side adjustment
    /// instead of silently saturating, which would ship an on-chain order
    /// different from what the user signed off on.
    #[test]
    fn try_to_order_data_rejects_overflowing_sell_adjustment() {
        let mut quote = load_mainnet_quote();
        quote.quote.kind = OrderKind::Sell;
        quote.quote.sell_amount = U256::MAX;
        quote.quote.fee_amount = U256::from(1u64);

        let err = quote
            .try_to_order_data(
                &fixture_quote_request(),
                EMPTY_APP_DATA_HASH,
                &OrderCosts::default(),
            )
            .unwrap_err();
        // The request-binding fold fires first; both it and the projection
        // share the fail-closed `QuoteFeeMathOverflow` contract (the old
        // dedicated `QuoteAmountOverflow` variant is folded into it).
        assert!(
            matches!(err, Error::QuoteFeeMathOverflow { .. }),
            "got: {err:?}"
        );
    }

    /// Same fail-closed contract as the basic path: the
    /// fee-composition projection used by
    /// [`OrderQuoteResponse::try_to_order_data`] must
    /// reject an overflowing sell-side adjustment rather than copy a
    /// saturated `U256::MAX` into the signed `OrderData`.
    #[test]
    fn try_to_order_data_rejects_buy_side_fee_math_overflow() {
        // Buy-side so the fixed-leg amount binding (which only pins the
        // buy amount for a Buy order) passes, letting the fee-math
        // overflow on the sell side actually be reached: `sellAmount +
        // feeAmount` in `after_network_costs.sell` overflows.
        let mut quote = load_mainnet_quote();
        quote.quote.kind = OrderKind::Buy;
        quote.quote.buy_amount = U256::MAX;
        quote.quote.sell_amount = U256::MAX;
        quote.quote.fee_amount = U256::from(1u64);
        let request = QuoteRequest::buy_after_fee(
            quote.quote.sell_token,
            quote.quote.buy_token,
            quote.from,
            U256::MAX,
        );

        let err = quote
            .try_to_order_data(&request, EMPTY_APP_DATA_HASH, &OrderCosts::default())
            .unwrap_err();
        assert!(
            matches!(err, Error::QuoteFeeMathOverflow { .. }),
            "got: {err:?}",
        );
    }
}

/// R20 request binding: [`OrderQuoteResponse::try_to_order_data`] must
/// fail closed with [`Error::QuoteFieldMismatch`] whenever the response
/// disagrees with a field the caller authorised in the request, so a
/// hostile orderbook can never have the user sign a different trade
/// than the one they asked to be quoted.
mod request_binding {
    use super::*;

    /// Request that matches [`load_mainnet_quote`] on every bound field:
    /// same token pair, owner, and side, with a pre-fee sell amount equal
    /// to the quote's `sellAmount + feeAmount` (the signed sell side).
    fn matching_request(quote: &OrderQuoteResponse) -> QuoteRequest {
        QuoteRequest::sell_before_fee(
            quote.quote.sell_token,
            quote.quote.buy_token,
            quote.from,
            quote.quote.sell_amount + quote.quote.fee_amount,
        )
    }

    /// Load the mainnet fixture and its matching request, let `tamper`
    /// skew exactly one bound field on either side, then assert the
    /// projection fails closed with [`Error::QuoteFieldMismatch`] on
    /// `field`.
    fn assert_binding_rejects(
        field: &'static str,
        tamper: impl FnOnce(&mut OrderQuoteResponse, &mut QuoteRequest),
    ) {
        let mut quote = load_mainnet_quote();
        let mut request = matching_request(&quote);
        tamper(&mut quote, &mut request);
        let err = quote
            .try_to_order_data(&request, EMPTY_APP_DATA_HASH, &OrderCosts::default())
            .unwrap_err();
        assert!(
            matches!(&err, Error::QuoteFieldMismatch { field: got, .. } if *got == field),
            "expected QuoteFieldMismatch on {field}, got: {err}"
        );
    }

    /// A tampered `buy_token` must not let the user sign an order paying
    /// out a different asset than they asked for.
    #[test]
    fn try_to_order_data_rejects_swapped_buy_token() {
        assert_binding_rejects("buyToken", |_, request| {
            request.buy_token = address!("dead000000000000000000000000000000000000");
        });
    }

    /// A quote that swaps the receiver when the request omitted it
    /// (default owner-receives) is rejected. Without normalising both
    /// sides, a hostile orderbook could redirect proceeds to an attacker
    /// while the caller never set a receiver in the request.
    #[test]
    fn try_to_order_data_rejects_swapped_receiver_when_request_omits_receiver() {
        assert_binding_rejects("receiver", |quote, _| {
            quote.quote.receiver = Some(address!("dead00000000000000000000000000000000beef"));
        });
    }

    /// A quote whose returned `app_data` digest disagrees with the digest
    /// the caller pinned in the request is rejected.
    #[test]
    fn try_to_order_data_rejects_swapped_app_data() {
        assert_binding_rejects("appData", |_, request| {
            // The response's digest is `EMPTY_APP_DATA_HASH`, not the pin.
            request.app_data = Some(QuoteAppData::Hash(AppDataHash::from([0x42; 32])));
        });
    }

    /// A quote whose `kind` flips the side of the order the user
    /// authorised is rejected. A Sell intent returning as Buy
    /// reinterprets which side is the fixed leg, so signing would commit
    /// to a different trade than the user saw.
    #[test]
    fn try_to_order_data_rejects_swapped_kind() {
        assert_binding_rejects("kind", |quote, request| {
            assert_eq!(request.kind(), OrderKind::Sell);
            quote.quote.kind = OrderKind::Buy;
        });
    }

    /// The CRITICAL finding: a hostile orderbook echoes the right token
    /// pair and `kind` but inflates `sellAmount`. Without binding the
    /// fixed leg the caller would sign an order moving more than they
    /// requested. The signed `sellAmount` is `sellAmount + feeAmount`,
    /// so the pre-fee request must equal that sum.
    #[test]
    fn try_to_order_data_rejects_inflated_sell_amount_before_fee() {
        assert_binding_rejects("sellAmountBeforeFee", |quote, _| {
            // Orderbook returns double the sell amount for the same request.
            quote.quote.sell_amount *= U256::from(2u64);
        });
    }

    /// Buy-side counterpart: the fixed leg is `buyAmount`. An inflated or
    /// deflated buy amount must be rejected before signing.
    #[test]
    fn try_to_order_data_rejects_mismatched_buy_amount_after_fee() {
        assert_binding_rejects("buyAmountAfterFee", |quote, request| {
            quote.quote.kind = OrderKind::Buy;
            *request = QuoteRequest::buy_after_fee(
                quote.quote.sell_token,
                quote.quote.buy_token,
                quote.from,
                quote.quote.buy_amount,
            );
            quote.quote.buy_amount += U256::from(1u64);
        });
    }

    /// `sell_after_fee` pins the post-fee sell leg directly against the
    /// quote's `sellAmount`.
    #[test]
    fn try_to_order_data_rejects_mismatched_sell_amount_after_fee() {
        assert_binding_rejects("sellAmountAfterFee", |quote, request| {
            *request = QuoteRequest::sell_after_fee(
                quote.quote.sell_token,
                quote.quote.buy_token,
                quote.from,
                U256::from(50_000_000u64),
            );
            quote.quote.sell_amount = U256::from(60_000_000u64);
        });
    }

    /// A quote whose envelope `from` does not match the request is
    /// rejected. The orderbook indexes the order under this address and
    /// the SDK derives the UID from it, so a swap silently changes the
    /// owner the order would settle for.
    #[test]
    fn try_to_order_data_rejects_swapped_from() {
        assert_binding_rejects("from", |quote, _| {
            quote.from = address!("dead000000000000000000000000000000000000");
        });
    }

    /// A quote whose `validTo` disagrees with the absolute expiry the
    /// caller pinned via [`QuoteRequest::valid_to`] is rejected. Unpinned
    /// `validTo` stays the orderbook's prerogative.
    #[test]
    fn try_to_order_data_rejects_swapped_valid_to_when_request_pins_it() {
        assert_binding_rejects("validTo", |quote, request| {
            request.valid_to = Some(quote.quote.valid_to.wrapping_add(1));
        });
    }

    /// A quote whose `partiallyFillable` flips the value the caller
    /// pinned is rejected. Without the check, a hostile orderbook could
    /// partially fill an order the user expected to be fill-or-kill (or
    /// vice versa).
    #[test]
    fn try_to_order_data_rejects_swapped_partially_fillable_when_request_pins_it() {
        assert_binding_rejects("partiallyFillable", |quote, request| {
            quote.quote.partially_fillable = true;
            request.partially_fillable = Some(false);
        });
    }

    /// A quote whose `sellTokenBalance` swaps the source the caller
    /// pinned (ERC20 vs Vault internal balance vs external) is rejected.
    #[test]
    fn try_to_order_data_rejects_swapped_sell_token_balance_when_request_pins_it() {
        assert_binding_rejects("sellTokenBalance", |quote, request| {
            quote.quote.sell_token_balance = SellTokenSource::Internal;
            request.sell_token_balance = Some(SellTokenSource::Erc20);
        });
    }

    /// A quote whose `buyTokenBalance` swaps the destination the caller
    /// pinned is rejected.
    #[test]
    fn try_to_order_data_rejects_swapped_buy_token_balance_when_request_pins_it() {
        assert_binding_rejects("buyTokenBalance", |quote, request| {
            quote.quote.buy_token_balance = BuyTokenDestination::Internal;
            request.buy_token_balance = Some(BuyTokenDestination::Erc20);
        });
    }

    /// A quote whose `signingScheme` swaps the scheme the caller pinned
    /// is rejected. The signed payload itself does not commit to the
    /// scheme, but the orderbook routes the resulting order under the
    /// wire scheme, and a smart-contract caller pinning EIP-1271 must
    /// not have it silently downgraded to PreSign.
    #[test]
    fn try_to_order_data_rejects_swapped_signing_scheme_when_request_pins_it() {
        assert_binding_rejects("signingScheme", |quote, request| {
            quote.quote.signing_scheme = SigningScheme::PreSign;
            request.signing_scheme = Some(SigningScheme::Eip712);
        });
    }

    /// `check_response_matches_request` rejects a `Full(json)` pin when
    /// `keccak256(json)` disagrees with the digest the response binds
    /// the signed order to. Without the arm a caller handing in a JSON
    /// document still ends up signing whatever digest the orderbook
    /// produced for it.
    #[test]
    fn check_response_matches_request_rejects_full_app_data_with_server_digest_mismatch() {
        assert_binding_rejects("appData", |_, request| {
            // `keccak256("{\"foo\":1}")` is not `EMPTY_APP_DATA_HASH`.
            request.app_data = Some(QuoteAppData::Full(r#"{"foo":1}"#.to_owned()));
        });
    }

    /// `try_to_order_data` returns an `OrderData` whose 12 signed fields
    /// mirror the response when every caller-authorised field matches
    /// the request. The amount-side check pins the documented sell-side
    /// fee adjustment.
    #[test]
    fn try_to_order_data_passes_when_request_matches_response() {
        let quote = load_mainnet_quote();
        let request = matching_request(&quote);
        let signed = quote
            .try_to_order_data(&request, EMPTY_APP_DATA_HASH, &OrderCosts::default())
            .unwrap();
        assert_eq!(signed.sell_token, quote.quote.sell_token);
        assert_eq!(signed.buy_token, quote.quote.buy_token);
        assert_eq!(signed.receiver, quote.quote.receiver);
        assert_eq!(signed.kind, quote.quote.kind);
        assert_eq!(signed.app_data, EMPTY_APP_DATA_HASH);
        assert_eq!(signed.fee_amount, U256::ZERO);
        assert_eq!(
            signed.sell_amount,
            quote.quote.sell_amount + quote.quote.fee_amount,
        );
    }

    /// A response that echoes `receiver = from` for a request that
    /// omitted receiver is semantically identical to "owner receives"
    /// and must not trip a mismatch. Mirrors the orderbook's
    /// normalisation.
    #[test]
    fn try_to_order_data_accepts_owner_receiver_echo_when_request_omits_receiver() {
        let mut quote = load_mainnet_quote();
        quote.quote.receiver = Some(quote.from);
        let request = matching_request(&quote);
        quote
            .try_to_order_data(&request, EMPTY_APP_DATA_HASH, &OrderCosts::default())
            .expect("owner-as-receiver echo should normalise to owner-receives");
    }

    /// Positive counterpart: `Full(json)` whose `keccak256` matches the
    /// orderbook's `app_data` digest passes the guard and projects into
    /// a signable `OrderData`.
    #[test]
    fn check_response_matches_request_accepts_full_app_data_when_server_digest_matches() {
        let quote = load_mainnet_quote();
        let mut request = matching_request(&quote);
        request.app_data = Some(QuoteAppData::Full(EMPTY_APP_DATA_JSON.to_owned()));
        quote
            .try_to_order_data(&request, EMPTY_APP_DATA_HASH, &OrderCosts::default())
            .expect("Full(\"{}\") hashes to EMPTY_APP_DATA_HASH and must pass");
    }
}

/// [`QuoteRequest::validate`]: the request-shape invariants enforced
/// before anything reaches the orderbook.
mod validate {
    use super::*;

    /// `validate` rejects a request that sets both `valid_to` (absolute) and
    /// `valid_for` (server-relative): they are mutually exclusive, so an
    /// ambiguous request is refused before it reaches the orderbook.
    #[test]
    fn validate_rejects_valid_to_and_valid_for_together() {
        let mut request = fixture_quote_request();
        request.valid_to = Some(1_000);
        request.valid_for = Some(1_800);
        let err = request.validate().unwrap_err();
        assert!(
            matches!(
                &err,
                Error::QuoteRequestInvalid {
                    field: "validTo/validFor",
                    ..
                }
            ),
            "got: {err}"
        );
    }

    /// `validate` rejects a request with no amount set: exactly one of the
    /// three amount fields must be present.
    #[test]
    fn validate_rejects_zero_amounts() {
        let mut request = fixture_quote_request();
        request.sell_amount_before_fee = None;
        let err = request.validate().unwrap_err();
        assert!(
            matches!(
                &err,
                Error::QuoteRequestInvalid {
                    field: "amount",
                    ..
                }
            ),
            "got: {err}"
        );
    }

    /// `validate` rejects a request with more than one amount set.
    #[test]
    fn validate_rejects_two_amounts() {
        let mut request = fixture_quote_request();
        request.buy_amount_after_fee = Some(U256::from(1u64));
        let err = request.validate().unwrap_err();
        assert!(
            matches!(
                &err,
                Error::QuoteRequestInvalid {
                    field: "amount",
                    ..
                }
            ),
            "got: {err}"
        );
    }

    /// `validate` rejects a request whose `kind` disagrees with the set
    /// amount: a buy amount under a Sell kind would have the orderbook price
    /// the wrong leg.
    #[test]
    fn validate_rejects_kind_amount_mismatch() {
        let mut request = fixture_quote_request();
        assert_eq!(request.kind(), OrderKind::Sell);
        // Swap the sell amount for a buy amount while leaving `kind` Sell.
        request.sell_amount_before_fee = None;
        request.buy_amount_after_fee = Some(U256::from(1u64));
        let err = request.validate().unwrap_err();
        assert!(
            matches!(&err, Error::QuoteRequestInvalid { field: "kind", .. }),
            "got: {err}"
        );
    }

    /// `validate` accepts a well-formed request built via a constructor.
    #[test]
    fn validate_accepts_well_formed_request() {
        fixture_quote_request().validate().unwrap();
        QuoteRequest::buy_after_fee(USDC, DAI, OWNER, U256::from(1u64))
            .validate()
            .unwrap();
    }
}

/// Pipeline tests driven through a canned-response mock transport: the
/// quote -> sign -> submit flow is transport-generic, so none of these
/// need reqwest or an HTTP stack.
mod pipeline {
    use super::*;
    use crate::app_data::{APP_DATA_SIZE_LIMIT, AppDataDoc, AppDataError};
    use crate::chain::Chain;
    use crate::order_book::orders::valid_to_after_for_tests;
    use crate::transport::{HttpMethod, HttpRequest, HttpResponse, HttpTransport};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// Canned-response transport: pops one `(status, body)` per request
    /// and records every request it executes, so tests can assert which
    /// endpoints were (or were not) reached.
    #[derive(Clone, Debug, Default)]
    pub(super) struct MockTransport {
        responses: Arc<Mutex<VecDeque<(u16, String)>>>,
        seen: Arc<Mutex<Vec<(HttpMethod, String)>>>,
    }

    impl MockTransport {
        pub(super) fn enqueue(&self, status: u16, body: impl Into<String>) {
            self.responses
                .lock()
                .unwrap()
                .push_back((status, body.into()));
        }

        pub(super) fn seen(&self) -> Vec<(HttpMethod, String)> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl HttpTransport for MockTransport {
        async fn execute(&self, request: HttpRequest) -> crate::error::Result<HttpResponse> {
            self.seen
                .lock()
                .unwrap()
                .push((request.method, request.url.path().to_owned()));
            let (status, body) = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("mock transport received an unexpected request");
            Ok(HttpResponse { status, body })
        }
    }

    pub(super) fn mock_api(transport: &MockTransport) -> OrderBookApi<MockTransport> {
        OrderBookApi::new_with_transport(
            "https://orderbook.invalid/".parse().unwrap(),
            transport.clone(),
        )
    }

    const SELL: u128 = 1_000_000_000_000_000_000;
    const BUY: u128 = 2_000_000_000_000_000_000;
    const UID_BODY: &str = "\"0xb74844872ddbadb709629952eab02a9275c5c05426cb195e27029a353909404370997970c51812dc3a010c7d01b50e0d17dc79c86a0513b9\"";

    fn quote_body(from: Address) -> String {
        serde_json::json!({
            "quote": {
                "sellToken": format!("{USDC:#x}"),
                "buyToken": format!("{DAI:#x}"),
                "receiver": null,
                "sellAmount": SELL.to_string(),
                "buyAmount": BUY.to_string(),
                "validTo": valid_to_after_for_tests(3_600),
                "appData": format!("{EMPTY_APP_DATA_HASH:#x}"),
                "feeAmount": "0",
                "kind": "sell",
                "partiallyFillable": false,
                "sellTokenBalance": "erc20",
                "buyTokenBalance": "erc20",
                "signingScheme": "eip712",
            },
            "from": format!("{from:#x}"),
            "expiration": "2099-12-31T23:59:59Z",
            "id": 42,
            "verified": true,
        })
        .to_string()
    }

    fn signer() -> alloy_signer_local::PrivateKeySigner {
        alloy_signer_local::PrivateKeySigner::from_bytes(&U256::from(1u64).to_be_bytes().into())
            .unwrap()
    }

    async fn quoted(
        api: &OrderBookApi<MockTransport>,
        transport: &MockTransport,
        owner: Address,
    ) -> QuotedOrder<MockTransport> {
        transport.enqueue(200, quote_body(owner));
        api.quote_builder()
            .with_sell_token(USDC)
            .with_buy_token(DAI)
            .with_from(owner)
            .with_sell_amount(U256::from(SELL))
            .build()
            .await
            .expect("matching mock quote must bind")
    }

    /// `build()` rejects a zero `from` before any request leaves the
    /// client: the precondition the deleted `TradingClient` enforced.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn build_rejects_zero_from_before_any_request() {
        let transport = MockTransport::default();
        let err = mock_api(&transport)
            .quote_builder()
            .with_sell_token(USDC)
            .with_buy_token(DAI)
            .with_from(Address::ZERO)
            .with_sell_amount(U256::from(SELL))
            .build()
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::QuoteRequestInvalid { field: "from", .. }),
            "got: {err:?}"
        );
        assert!(transport.seen().is_empty(), "no request may reach the wire");
    }

    /// `build()` surfaces an oversized caller-supplied app-data document
    /// as an error before any network call, mirroring the deleted
    /// `TradingClient::post_swap_order` guard.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn build_rejects_oversize_app_data_before_any_request() {
        let transport = MockTransport::default();
        let doc = AppDataDoc::new("x".repeat(APP_DATA_SIZE_LIMIT + 1));
        let err = mock_api(&transport)
            .quote_builder()
            .with_sell_token(USDC)
            .with_buy_token(DAI)
            .with_from(OWNER)
            .with_sell_amount(U256::from(SELL))
            .with_app_data(&doc)
            .build()
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::AppData(AppDataError::DocumentTooLarge { .. })),
            "got: {err:?}"
        );
        assert!(transport.seen().is_empty(), "no request may reach the wire");
    }

    /// `QuotedOrder` is born response-bound: a hostile orderbook that
    /// echoes a swapped `sellToken` fails at quote time, inside
    /// `build()`, not later at signing time.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn build_binds_response_to_request() {
        let transport = MockTransport::default();
        // The mock echoes DAI as the sell token while the request asks
        // for USDC.
        transport.enqueue(
            200,
            quote_body(OWNER).replace(&format!("{USDC:#x}"), &format!("{DAI:#x}")),
        );
        let err = mock_api(&transport)
            .quote_builder()
            .with_sell_token(USDC)
            .with_buy_token(DAI)
            .with_from(OWNER)
            .with_sell_amount(U256::from(SELL))
            .build()
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::QuoteFieldMismatch {
                    field: "sellToken",
                    ..
                }
            ),
            "got: {err:?}"
        );
    }

    /// R24 fail-fast: when `from` does not match the signer, `sign()`
    /// rejects with `SignerMismatch` and `POST /api/v1/orders` is never
    /// reached. Ported from `TradingClient`'s signer-mismatch test.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sign_fails_fast_on_signer_mismatch_without_posting() {
        let transport = MockTransport::default();
        let api = mock_api(&transport).with_chain_hint(Chain::Mainnet);
        let declared_from = address!("dead0000dead0000dead0000dead0000dead0000");
        assert_ne!(signer().address(), declared_from);

        let quoted = quoted(&api, &transport, declared_from).await;
        let err = quoted.sign(signer()).unwrap_err();
        assert!(
            matches!(
                err,
                Error::VerifyOwner(crate::error::VerifyOwnerError::SignerMismatch { .. })
            ),
            "got: {err:?}"
        );
        assert_eq!(
            transport.seen(),
            vec![(HttpMethod::Post, "/api/v1/quote".to_owned())],
            "POST /api/v1/orders must not be reached on a signer mismatch"
        );
    }

    /// `sign_with` refuses a chain that disagrees with the client's
    /// chain hint, so a caller cannot sign for one chain and post to
    /// another's orderbook. Ported from `TradingClient::from_orderbook`.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sign_with_rejects_chain_mismatch() {
        let transport = MockTransport::default();
        let api = mock_api(&transport).with_chain_hint(Chain::Gnosis);
        let quoted = quoted(&api, &transport, signer().address()).await;
        let err = quoted
            .sign_with(Chain::Mainnet, EcdsaSigningScheme::Eip712, signer())
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::ChainMismatch {
                    client: Chain::Mainnet,
                    api: Chain::Gnosis,
                }
            ),
            "got: {err:?}"
        );
    }

    /// Matching chains (and chainless clients, whose chain is unknown)
    /// are accepted; the signer works both by value and by reference.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sign_with_accepts_matching_and_unknown_chain() {
        let wallet = signer();
        let owner = wallet.address();

        let transport = MockTransport::default();
        let api = mock_api(&transport).with_chain_hint(Chain::Mainnet);
        let quoted_hinted = quoted(&api, &transport, owner).await;
        // By reference, against the matching chain.
        quoted_hinted
            .sign_with(Chain::Mainnet, EcdsaSigningScheme::Eip712, &wallet)
            .expect("matching chain must be accepted");

        let chainless = mock_api(&transport);
        let quoted_chainless = quoted(&chainless, &transport, owner).await;
        // By value, against a caller-supplied chain.
        quoted_chainless
            .sign_with(Chain::Gnosis, EcdsaSigningScheme::Eip712, wallet)
            .expect("chainless client must trust the caller's chain");
    }

    /// `sign()` needs the chain hint to infer the signing domain;
    /// chainless clients are pointed at `sign_with`.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sign_requires_chain_hint() {
        let transport = MockTransport::default();
        let api = mock_api(&transport);
        let quoted = quoted(&api, &transport, signer().address()).await;
        let err = quoted.sign(signer()).unwrap_err();
        assert!(
            matches!(err, Error::OrderCreationInvalid { field: "chain", .. }),
            "got: {err:?}"
        );
    }

    /// The empty app-data document is universally known to the
    /// orderbook, so `submit()` skips the pinning PUT and goes straight
    /// to `POST /api/v1/orders`.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn submit_skips_app_data_put_for_empty_document() {
        let wallet = signer();
        let transport = MockTransport::default();
        let api = mock_api(&transport).with_chain_hint(Chain::Mainnet);
        let quoted = quoted(&api, &transport, wallet.address()).await;
        transport.enqueue(201, UID_BODY);
        quoted.sign(&wallet).unwrap().submit().await.unwrap();
        assert_eq!(
            transport.seen(),
            vec![
                (HttpMethod::Post, "/api/v1/quote".to_owned()),
                (HttpMethod::Post, "/api/v1/orders".to_owned()),
            ],
            "no PUT for the empty app-data sentinel"
        );
    }

    /// A non-empty app-data document is pinned via
    /// `PUT /api/v1/app_data/{hash}` before the order is posted, so the
    /// index never carries the digest without the document.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn submit_pins_app_data_before_posting() {
        let wallet = signer();
        let owner = wallet.address();
        let doc = AppDataDoc::sdk_attribution("cow-rs");
        let doc_hash = doc.hash();

        let transport = MockTransport::default();
        let api = mock_api(&transport).with_chain_hint(Chain::Mainnet);
        transport.enqueue(200, quote_body(owner));
        let quoted = api
            .quote_builder()
            .with_sell_token(USDC)
            .with_buy_token(DAI)
            .with_from(owner)
            .with_sell_amount(U256::from(SELL))
            .with_app_data(&doc)
            .build()
            .await
            .unwrap();
        transport.enqueue(200, ""); // PUT app_data
        transport.enqueue(201, UID_BODY); // POST orders
        quoted.sign(&wallet).unwrap().submit().await.unwrap();
        assert_eq!(
            transport.seen(),
            vec![
                (HttpMethod::Post, "/api/v1/quote".to_owned()),
                (HttpMethod::Put, format!("/api/v1/app_data/{doc_hash}")),
                (HttpMethod::Post, "/api/v1/orders".to_owned()),
            ],
            "the document must be pinned before the order is posted"
        );
    }

    /// The structural chokepoint: a chain-hinted client owner-verifies
    /// every body inside `post_order`, so a hand-assembled
    /// `OrderCreation` whose `from` does not match the recovered signer
    /// never reaches the wire. Chainless clients skip the check (the
    /// signing domain is unknown), preserving mock ergonomics.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn post_order_owner_verifies_chain_hinted_submissions() {
        use crate::domain::settlement_domain;
        use crate::order::OrderData;

        let wallet = signer();
        let impostor = address!("dead0000dead0000dead0000dead0000dead0000");
        assert_ne!(wallet.address(), impostor);

        let order_data = OrderData {
            sell_token: USDC,
            buy_token: DAI,
            sell_amount: U256::from(SELL),
            buy_amount: U256::from(BUY),
            valid_to: valid_to_after_for_tests(3_600),
            app_data: EMPTY_APP_DATA_HASH,
            ..OrderData::default()
        };
        let domain = settlement_domain(
            Chain::Mainnet.id(),
            address!("9008D19f58AAbD9eD0D60971565AA8510560ab41"),
        );
        let signature = order_data
            .sign(EcdsaSigningScheme::Eip712, &domain, &wallet)
            .unwrap();
        // Declared owner is NOT the signer: the chokepoint must refuse.
        let body = OrderCreation::new(
            &order_data,
            signature,
            impostor,
            EMPTY_APP_DATA_JSON.to_owned(),
            None,
        )
        .unwrap();

        let transport = MockTransport::default();
        let hinted = mock_api(&transport).with_chain_hint(Chain::Mainnet);
        let err = hinted.post_order(&body).await.unwrap_err();
        assert!(
            matches!(
                err,
                Error::VerifyOwner(crate::error::VerifyOwnerError::SignerMismatch { .. })
            ),
            "got: {err:?}"
        );
        assert!(
            transport.seen().is_empty(),
            "a mismatched body must never reach the wire"
        );

        // A chainless client cannot infer the domain and skips the
        // guard: the body goes through to the (mock) wire.
        let chainless = mock_api(&transport);
        transport.enqueue(201, UID_BODY);
        chainless
            .post_order(&body)
            .await
            .expect("chainless clients skip the owner check");
    }

    /// A quote pinned by a bare non-empty digest carries no document,
    /// so signing fails until the caller supplies the JSON.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn signing_a_quote_pinned_by_bare_hash_requires_document() {
        let wallet = signer();
        let owner = wallet.address();
        let pinned = AppDataHash::from([0x42; 32]);

        let transport = MockTransport::default();
        let api = mock_api(&transport).with_chain_hint(Chain::Mainnet);
        transport.enqueue(200, quote_body(owner));
        let quoted = api
            .quote_builder()
            .with_sell_token(USDC)
            .with_buy_token(DAI)
            .with_from(owner)
            .with_sell_amount(U256::from(SELL))
            .with_app_data(pinned)
            .build()
            .await
            .unwrap();
        let err = quoted.sign(&wallet).unwrap_err();
        assert!(
            matches!(
                err,
                Error::OrderCreationInvalid {
                    field: "app_data",
                    ..
                }
            ),
            "got: {err:?}"
        );
    }
}
