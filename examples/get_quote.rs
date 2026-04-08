use cow_rs::{ChainId, Environment, OrderBookClient, OrderKind, OrderQuoteRequest, SdkConfig};

const SEPOLIA_WETH: &str = "0xfFf9976782d46CC05630D1f6eBAb18b2324d6B14";
const SEPOLIA_USDC_TEST: &str = "0xbe72E441BF55620febc26715db68d3494213D8Cb";
const FROM: &str = "0x0000000000000000000000000000000000000001";
const SELL_AMOUNT: &str = "100000000000000000"; // 0.1 WETH

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = SdkConfig::new("cow-rs-example", ChainId::Sepolia, Environment::Staging);
    let client = OrderBookClient::new(config)?;
    let request = OrderQuoteRequest::from_trade_amount(
        OrderKind::Sell,
        SEPOLIA_WETH.to_owned(),
        SEPOLIA_USDC_TEST.to_owned(),
        SELL_AMOUNT.to_owned(),
        FROM.to_owned(),
    );
    let quote = client.get_quote(&request).await?;

    println!("{}", serde_json::to_string_pretty(&quote)?);
    Ok(())
}
