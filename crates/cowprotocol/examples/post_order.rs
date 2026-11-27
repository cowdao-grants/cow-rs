//! End-to-end signed submission of a CoW Protocol order on Sepolia,
//! driven through the fluent quote -> sign -> submit pipeline.
//!
//! Under the hood the chain performs the four-step trade flow:
//!
//! 1. Ask the Sepolia orderbook for a quote on a small WETH -> COW swap
//!    (`quote_builder()...build()`), binding the response to the request.
//! 2. Apply the submission adjustments (fee fold plus the default
//!    50 bps slippage) via the parity-locked amount projection.
//! 3. Sign the resulting [`cowprotocol::OrderData`] under the GPv2Settlement
//!    domain with an EIP-712 ECDSA signature, owner-verifying the body.
//! 4. POST the signed body to `/api/v1/orders` and print the assigned UID
//!    together with a CoW Explorer URL.
//!
//! # Environment
//!
//! - `SEPOLIA_PRIVATE_KEY`: hex-encoded 32-byte secp256k1 key, with or
//!   without the `0x` prefix. When the variable is absent the example
//!   prints a short hint and exits successfully so the binary still
//!   compiles and runs cleanly in CI.
//!
//! # Funding
//!
//! Signing a quote and POSTing it to the orderbook does NOT require any
//! on-chain balance: the orderbook accepts the order, it just will not
//! settle until the signer holds enough sell-token (here, Sepolia WETH)
//! and has granted the Vault relayer an allowance. The wallet still
//! needs a small amount of Sepolia ETH for any future on-chain action
//! (wrapping, approvals, cancellation) but not for this submission
//! itself.
//!
//! # Explorer
//!
//! Submitted orders are viewable at
//! `https://explorer.cow.fi/sepolia/orders/<uid>`.
//!
//! # Run
//!
//! ```sh
//! SEPOLIA_PRIVATE_KEY=0x... cargo run --example post_order
//! ```

use {
    alloy_primitives::{Address, U256, address},
    alloy_signer_local::PrivateKeySigner,
    cowprotocol::{Chain, OrderBookApi},
    std::str::FromStr,
};

/// Sepolia WETH. Source: `cowprotocol/contracts/networks.json`.
const WETH_SEPOLIA: Address = address!("fFf9976782d46CC05630D1f6eBAb18b2324d6B14");
/// Sepolia COW. Source: `cowprotocol/contracts/networks.json`.
const COW_SEPOLIA: Address = address!("0625aFB445C3B6B7B929342a04A22599fd5dBB59");
/// Sell amount: 0.01 WETH expressed in 18-decimal atomic units. Small
/// enough that the orderbook will quote without bumping into liquidity
/// floors, large enough that the quote response is meaningful.
const SELL_AMOUNT_WEI: u128 = 10_000_000_000_000_000;

#[tokio::main]
async fn main() -> cowprotocol::Result<()> {
    let Ok(raw_key) = std::env::var("SEPOLIA_PRIVATE_KEY") else {
        println!("set SEPOLIA_PRIVATE_KEY=... to run live (skipping)");
        return Ok(());
    };

    // Configuration error: the env-supplied key must be valid hex. We
    // surface this immediately rather than smuggling it through
    // `cowprotocol::Error`, whose variants describe wire-format / signing /
    // orderbook failures, not user-input parsing.
    let signer = PrivateKeySigner::from_str(raw_key.trim())
        .expect("SEPOLIA_PRIVATE_KEY must be a 0x-prefixed 32-byte hex string");
    let owner = signer.address();
    println!("signer:       {owner:?}");

    // Quote: the builder binds the response to the request, so a
    // hostile orderbook cannot trick us into signing a swapped token /
    // receiver. The default 50 bps slippage protection applies; tune it
    // with `.with_slippage_bps(..)`.
    let quoted = OrderBookApi::new(Chain::Sepolia)
        .quote_builder()
        .with_sell_token(WETH_SEPOLIA)
        .with_buy_token(COW_SEPOLIA)
        .with_from(owner)
        .with_sell_amount(U256::from(SELL_AMOUNT_WEI))
        .build()
        .await?;
    let response = quoted.response();
    println!("quote id:     {}", response.id);
    println!("buy amount:   {}", response.quote.buy_amount);
    println!("fee amount:   {}", response.quote.fee_amount);
    println!("valid to:     {}", response.quote.valid_to);

    // Sign (EIP-712 under the Sepolia GPv2Settlement domain, owner
    // verified) and submit. `sign` projects the quote through the
    // partner-fee / protocol-fee / slippage composition first.
    let uid = quoted.sign(&signer)?.submit().await?;

    println!("order uid:    {uid}");
    println!("explorer:     https://explorer.cow.fi/sepolia/orders/{uid}");

    Ok(())
}
