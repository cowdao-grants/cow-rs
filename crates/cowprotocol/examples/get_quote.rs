//! Ask the production CoW Protocol orderbook for a quote that swaps a
//! fixed amount of USDC into DAI on Ethereum mainnet.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example get_quote
//! ```

use {
    alloy_primitives::{Address, U256, address},
    cowprotocol::{Chain, EMPTY_APP_DATA_HASH, OrderBookApi, OrderCosts, QuoteRequest},
};

// Ethereum mainnet USDC.
const USDC: Address = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
// Ethereum mainnet DAI.
const DAI: Address = address!("6B175474E89094C44Da98b954EedeAC495271d0F");
// Hardhat account #1: used as a no-stake `from` address so the orderbook
// will quote without checking allowances.
const OWNER: Address = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");

#[tokio::main]
async fn main() -> cowprotocol::Result<()> {
    let api = OrderBookApi::new(Chain::Mainnet);
    let request = QuoteRequest::sell_before_fee(
        USDC,
        DAI,
        OWNER,
        U256::from(100_000_000_u64), // 100 USDC, 6 decimals.
    );
    let response = api.quote(&request).await?;

    println!("buy amount:   {}", response.quote.buy_amount);
    println!("fee amount:   {}", response.quote.fee_amount);
    println!("valid to:     {}", response.quote.valid_to);
    println!("expiration:   {}", response.expiration);
    println!("quote id:     {}", response.id);
    println!("signing:      {:?}", response.quote.signing_scheme);

    // Project the quote into the signed payload and print its UID for the
    // configured chain: this is what we would sign in the next step.
    // Binding the response to `request` here makes the SDK reject a
    // hostile orderbook that tries to swap tokens / receiver / kind.
    let order_data =
        response.try_to_order_data(&request, EMPTY_APP_DATA_HASH, &OrderCosts::default())?;
    let domain = cowprotocol::settlement_domain(
        Chain::Mainnet.id(),
        address!("9008D19f58AAbD9eD0D60971565AA8510560ab41"),
    );
    let uid = order_data.uid(&domain, response.from);
    println!("order uid:    {uid}");

    Ok(())
}
