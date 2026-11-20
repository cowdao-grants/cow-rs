//! Smoke tests against the real CoW Protocol orderbook.
//!
//! Marked `#[ignore]` so they do not run under `cargo test` and never
//! make CI flaky on a transient orderbook outage. The dedicated
//! `smoke.yml` workflow runs them explicitly via
//! `cargo test --test live_smoke -- --ignored`, on a daily schedule
//! and on manual dispatch. Their purpose is to catch the schema-drift
//! class of bugs that mocked-fetch unit tests cannot see: an orderbook
//! deploy renames a field, our deserialiser misses it, mocks still
//! pass, prod breaks.
//!
//! Keep these tight: minimal assertions on shape, no business
//! semantics. If api.cow.fi is down or rate-limits us, we want a
//! readable failure, not a wall of red.

#![cfg(all(not(target_arch = "wasm32"), feature = "http-client"))]

use alloy_primitives::{U256, address};

use cowprotocol::{Chain, OrderBookApi, QuoteRequest};

/// `GET /api/v1/version` against the live mainnet orderbook. Returns a
/// non-empty version string. Catches the simplest end-to-end break:
/// DNS, TLS handshake, the orderbook being up at all.
#[tokio::test]
#[ignore]
async fn live_mainnet_version_responds() {
    let api = OrderBookApi::new(Chain::Mainnet);
    let version = api
        .version()
        .await
        .expect("api.cow.fi /version should respond");
    assert!(!version.trim().is_empty(), "empty version: {version:?}");
}

/// `POST /api/v1/quote` for a 100 USDC -> DAI sell on mainnet. Locks
/// the response shape (deserialises into `OrderQuoteResponse`) so any
/// breaking rename / removed field on the orderbook side surfaces here
/// before it hits production integrators.
#[tokio::test]
#[ignore]
async fn live_mainnet_get_quote_decodes_response_shape() {
    // USDC -> DAI, 100 USDC. The owner address is the well-known
    // zero-balance "burner" so the orderbook will return a quote
    // without a balance check rejecting us.
    let request = QuoteRequest::sell_before_fee(
        address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"),
        address!("6B175474E89094C44Da98b954EedeAC495271d0F"),
        address!("70997970C51812dc3A010C7d01b50e0d17dc79C8"),
        U256::from(100_000_000u64),
    );
    let response = OrderBookApi::new(Chain::Mainnet)
        .quote(&request)
        .await
        .expect("api.cow.fi /quote should respond");
    assert!(
        response.quote.buy_amount > U256::ZERO,
        "zero buy_amount in quote"
    );
    assert_eq!(
        response.quote.sell_token, request.sell_token,
        "sellToken in response should round-trip"
    );
    assert_eq!(
        response.quote.buy_token, request.buy_token,
        "buyToken in response should round-trip"
    );
}
