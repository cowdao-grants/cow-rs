use cow_rs::{
    ChainId, Environment, OrderBookClient, OrderKind, OrderQuoteRequest, Result, SdkConfig,
};
use std::env;

const DEFAULT_FROM: &str = "0x0000000000000000000000000000000000000001";
const SEPOLIA_WETH: &str = "0xfFf9976782d46CC05630D1f6eBAb18b2324d6B14";
const SEPOLIA_USDC_TEST: &str = "0xbe72E441BF55620febc26715db68d3494213D8Cb";
const SELL_AMOUNT: &str = "100000000000000000"; // 0.1 WETH
const SELL_SYMBOL: &str = "WETH";
const BUY_SYMBOL: &str = "USDC(test)";

fn live_tests_enabled() -> bool {
    matches!(
        env::var("COW_LIVE_TESTS").as_deref(),
        Ok("1" | "true" | "TRUE")
    )
}

fn format_units(amount: &str, decimals: usize) -> String {
    let negative = amount.starts_with('-');
    let digits = amount.trim_start_matches('-');

    if digits.len() <= decimals {
        let padded = format!("{digits:0>width$}", width = decimals);
        let trimmed = padded.trim_end_matches('0');
        let fraction = if trimmed.is_empty() { "0" } else { trimmed };
        return if negative {
            format!("-0.{fraction}")
        } else {
            format!("0.{fraction}")
        };
    }

    let split = digits.len() - decimals;
    let whole = &digits[..split];
    let fraction = digits[split..].trim_end_matches('0');

    let formatted = if fraction.is_empty() {
        whole.to_owned()
    } else {
        format!("{whole}.{fraction}")
    };

    if negative {
        format!("-{formatted}")
    } else {
        formatted
    }
}

#[tokio::test]
async fn live_get_quote_on_sepolia() -> Result<()> {
    dotenvy::dotenv().ok();

    if !live_tests_enabled() {
        eprintln!("skipping live_get_quote_on_sepolia: set COW_LIVE_TESTS=1 to enable");
        return Ok(());
    }

    let config = SdkConfig::new("cow-rs-live-test", ChainId::Sepolia, Environment::Staging);
    let base_url = config.orderbook_base_url()?;
    let client = OrderBookClient::new(config)?;

    let request = OrderQuoteRequest::from_trade_amount(
        OrderKind::Sell,
        SEPOLIA_WETH.to_owned(),
        SEPOLIA_USDC_TEST.to_owned(),
        SELL_AMOUNT.to_owned(),
        DEFAULT_FROM.to_owned(),
    );
    let response = client.get_quote(&request).await?;

    eprintln!(
        "live quote ok: base_url={} | {} -> {} | {} ({}) -> {} ({}) | expiration={} quote_id={:?}",
        base_url,
        SELL_SYMBOL,
        BUY_SYMBOL,
        response.quote.sell_token,
        format_units(&response.quote.sell_amount, 18),
        response.quote.buy_token,
        format_units(&response.quote.buy_amount, 18),
        response.expiration,
        response.id
    );

    assert_eq!(
        response.quote.sell_token.to_lowercase(),
        SEPOLIA_WETH.to_lowercase()
    );
    assert_eq!(
        response.quote.buy_token.to_lowercase(),
        SEPOLIA_USDC_TEST.to_lowercase()
    );
    assert_eq!(response.from.to_lowercase(), DEFAULT_FROM.to_lowercase());
    assert!(!response.quote.sell_amount.is_empty());
    assert!(!response.quote.buy_amount.is_empty());
    assert!(!response.expiration.is_empty());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::format_units;

    #[test]
    fn formats_units_for_human_logs() {
        assert_eq!(format_units("100000000000000000", 18), "0.1");
        assert_eq!(
            format_units("9096115532868233398", 18),
            "9.096115532868233398"
        );
        assert_eq!(format_units("1000", 18), "0.000000000000001");
    }
}
