use crate::domain::{
    order::{BuyTokenDestination, Order, OrderKind, SellTokenSource, SigningScheme},
    types::Address,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PriceQuality {
    Fast,
    Optimal,
    Verified,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRequest {
    pub sell_token: Address,
    pub buy_token: Address,
    pub from: Address,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receiver: Option<Address>,
    pub kind: OrderKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sell_amount_before_fee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buy_amount_after_fee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_for: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_data_hash: Option<String>,
    #[serde(default)]
    pub sell_token_balance: SellTokenSource,
    #[serde(default)]
    pub buy_token_balance: BuyTokenDestination,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_quality: Option<PriceQuality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signing_scheme: Option<SigningScheme>,
}

impl QuoteRequest {
    pub fn from_trade_amount(
        kind: OrderKind,
        sell_token: Address,
        buy_token: Address,
        amount: String,
        from: Address,
    ) -> Self {
        let (sell_amount_before_fee, buy_amount_after_fee) = match kind {
            OrderKind::Sell => (Some(amount), None),
            OrderKind::Buy => (None, Some(amount)),
        };

        Self {
            sell_token,
            buy_token,
            from,
            receiver: None,
            kind,
            sell_amount_before_fee,
            buy_amount_after_fee,
            valid_for: Some(1800),
            app_data: None,
            app_data_hash: None,
            sell_token_balance: SellTokenSource::Erc20,
            buy_token_balance: BuyTokenDestination::Erc20,
            price_quality: Some(PriceQuality::Verified),
            signing_scheme: Some(SigningScheme::Eip712),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub quote: Order,
    pub from: Address,
    pub expiration: String,
    #[serde(default)]
    pub id: Option<u64>,
    pub verified: bool,
    #[serde(default)]
    pub protocol_fee_bps: Option<String>,
    #[serde(default)]
    pub protocol_fee_sell_amount: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TradeParameters {
    pub kind: OrderKind,
    pub sell_token: String,
    pub buy_token: String,
    pub amount: String,
    #[serde(default)]
    pub receiver: Option<String>,
    #[serde(default)]
    pub valid_for: Option<u32>,
}

impl TradeParameters {
    pub fn new(
        kind: OrderKind,
        sell_token: impl Into<String>,
        buy_token: impl Into<String>,
        amount: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            sell_token: sell_token.into(),
            buy_token: buy_token.into(),
            amount: amount.into(),
            receiver: None,
            valid_for: Some(1800),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_trade_amount_sets_sell_amount_for_sell_orders() {
        let request = QuoteRequest::from_trade_amount(
            OrderKind::Sell,
            "0x0000000000000000000000000000000000000001".to_string(),
            "0x0000000000000000000000000000000000000002".to_string(),
            "100".to_string(),
            "0x0000000000000000000000000000000000000003".to_string(),
        );

        assert_eq!(request.sell_amount_before_fee, Some("100".to_string()));
        assert_eq!(request.buy_amount_after_fee, None);
    }

    #[test]
    fn from_trade_amount_sets_buy_amount_for_buy_orders() {
        let request = QuoteRequest::from_trade_amount(
            OrderKind::Buy,
            "0x0000000000000000000000000000000000000001".to_string(),
            "0x0000000000000000000000000000000000000002".to_string(),
            "100".to_string(),
            "0x0000000000000000000000000000000000000003".to_string(),
        );

        assert_eq!(request.sell_amount_before_fee, None);
        assert_eq!(request.buy_amount_after_fee, Some("100".to_string()));
    }

    #[test]
    fn trade_parameters_new_uses_expected_defaults() {
        let params = TradeParameters::new(
            OrderKind::Sell,
            "0x0000000000000000000000000000000000000001",
            "0x0000000000000000000000000000000000000002",
            "100",
        );

        assert_eq!(params.receiver, None);
        assert_eq!(params.valid_for, Some(1800));
    }
}
