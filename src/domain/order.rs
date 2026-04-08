use crate::{
    domain::types::{Address, OrderUid},
    error::{CowError, Result},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderKind {
    Buy,
    Sell,
}

impl std::str::FromStr for OrderKind {
    type Err = CowError;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "buy" => Ok(Self::Buy),
            "sell" => Ok(Self::Sell),
            other => Err(CowError::UnsupportedOrderKind(other.to_owned())),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SellTokenSource {
    Erc20,
    External,
    Internal,
}

impl Default for SellTokenSource {
    fn default() -> Self {
        Self::Erc20
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuyTokenDestination {
    Erc20,
    Internal,
}

impl Default for BuyTokenDestination {
    fn default() -> Self {
        Self::Erc20
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SigningScheme {
    Eip712,
    Ethsign,
    Presign,
    Eip1271,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    pub sell_token: String,
    pub buy_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    pub sell_amount: String,
    pub buy_amount: String,
    pub valid_to: u32,
    pub app_data: String,
    pub fee_amount: String,
    pub kind: OrderKind,
    pub partially_fillable: bool,
    #[serde(default)]
    pub sell_token_balance: SellTokenSource,
    #[serde(default)]
    pub buy_token_balance: BuyTokenDestination,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedOrder {
    #[serde(flatten)]
    pub order: Order,
    pub signing_scheme: SigningScheme,
    pub signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_id: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrichedOrder {
    #[serde(flatten)]
    pub order: Order,
    #[serde(default)]
    pub uid: Option<OrderUid>,
    #[serde(default)]
    pub owner: Option<Address>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub creation_date: Option<String>,
    #[serde(default)]
    pub executed_buy_amount: Option<String>,
    #[serde(default)]
    pub executed_sell_amount: Option<String>,
    #[serde(default)]
    pub executed_fee_amount: Option<String>,
    #[serde(default)]
    pub invalidated: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetOrdersRequest {
    pub owner: Address,
    pub offset: usize,
    pub limit: usize,
}

impl GetOrdersRequest {
    pub fn new(owner: Address) -> Self {
        Self {
            owner,
            offset: 0,
            limit: 1000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_order_kind_from_string() {
        assert_eq!("buy".parse::<OrderKind>().unwrap(), OrderKind::Buy);
        assert_eq!("sell".parse::<OrderKind>().unwrap(), OrderKind::Sell);
        assert!("nope".parse::<OrderKind>().is_err());
    }

    #[test]
    fn uses_erc20_as_default_balance_source_and_destination() {
        let order: Order = serde_json::from_str(
            r#"{
                "sellToken": "0x0000000000000000000000000000000000000001",
                "buyToken": "0x0000000000000000000000000000000000000002",
                "sellAmount": "100",
                "buyAmount": "95",
                "validTo": 123,
                "appData": "0x0",
                "feeAmount": "5",
                "kind": "sell",
                "partiallyFillable": false
            }"#,
        )
        .unwrap();

        assert_eq!(order.sell_token_balance, SellTokenSource::Erc20);
        assert_eq!(order.buy_token_balance, BuyTokenDestination::Erc20);
    }

    #[test]
    fn flattens_order_fields_when_serializing_signed_order() {
        let signed = SignedOrder {
            order: Order {
                sell_token: "0x0000000000000000000000000000000000000001".to_string(),
                buy_token: "0x0000000000000000000000000000000000000002".to_string(),
                receiver: None,
                sell_amount: "100".to_string(),
                buy_amount: "95".to_string(),
                valid_to: 123,
                app_data: "0x0".to_string(),
                fee_amount: "5".to_string(),
                kind: OrderKind::Sell,
                partially_fillable: false,
                sell_token_balance: SellTokenSource::Erc20,
                buy_token_balance: BuyTokenDestination::Erc20,
            },
            signing_scheme: SigningScheme::Eip712,
            signature: "0xdeadbeef".to_string(),
            from: None,
            quote_id: Some(7),
        };

        let value = serde_json::to_value(&signed).unwrap();

        assert_eq!(
            value["sellToken"],
            "0x0000000000000000000000000000000000000001"
        );
        assert_eq!(
            value["buyToken"],
            "0x0000000000000000000000000000000000000002"
        );
        assert_eq!(value["signature"], "0xdeadbeef");
        assert_eq!(value["quoteId"], 7);
        assert!(value.get("order").is_none());
        assert!(value.get("receiver").is_none());
    }
}
