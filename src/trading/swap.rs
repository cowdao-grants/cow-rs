use crate::{
    api::client::OrderBookClient,
    config::SdkConfig,
    domain::{
        order::SignedOrder,
        quote::{Quote, QuoteRequest, TradeParameters},
    },
    error::Result,
    signing::sign_order,
};
use ethers_signers::{LocalWallet, Signer};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostSwapOrderResponse {
    pub order_uid: String,
    pub quote: Quote,
    pub signed_order: SignedOrder,
}

#[derive(Clone, Debug)]
pub struct TradingClient {
    orderbook: OrderBookClient,
}

impl TradingClient {
    pub fn new(config: SdkConfig) -> Result<Self> {
        Ok(Self {
            orderbook: OrderBookClient::new(config)?,
        })
    }

    pub fn orderbook(&self) -> &OrderBookClient {
        &self.orderbook
    }

    pub async fn post_swap_order(
        &self,
        params: TradeParameters,
        wallet: &LocalWallet,
    ) -> Result<PostSwapOrderResponse> {
        let from = format!("{:#x}", wallet.address());
        let mut quote_request = QuoteRequest::from_trade_amount(
            params.kind.clone(),
            params.sell_token,
            params.buy_token,
            params.amount,
            from.clone(),
        );

        quote_request.receiver = params.receiver;
        quote_request.valid_for = params.valid_for;

        let quote = self.orderbook.get_quote(&quote_request).await?;
        let signing = sign_order(&quote.quote, self.orderbook.config(), wallet)?;

        let signed_order = SignedOrder {
            order: quote.quote.clone(),
            signing_scheme: signing.signing_scheme,
            signature: signing.signature,
            from: Some(from),
            quote_id: quote.id,
        };

        let order_uid = self.orderbook.send_order(&signed_order).await?;

        Ok(PostSwapOrderResponse {
            order_uid,
            quote,
            signed_order,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{ChainId, Environment},
        domain::order::{BuyTokenDestination, Order, OrderKind, SellTokenSource, SigningScheme},
        error::CowError,
    };
    use httpmock::{Method::POST, MockServer};

    fn test_client(server: &MockServer) -> TradingClient {
        let config = SdkConfig::new("cow-rs", ChainId::Sepolia, Environment::Staging)
            .with_base_url(Url::parse(&server.base_url()).unwrap());
        TradingClient::new(config).unwrap()
    }

    fn test_wallet() -> LocalWallet {
        "0x59c6995e998f97a5a0044966f0945382dbf30dd7ec4f9b89e0c5f6f9d1c1f98a"
            .parse()
            .unwrap()
    }

    fn sample_quote(from: String) -> Quote {
        Quote {
            quote: Order {
                sell_token: "0x0000000000000000000000000000000000000001".to_owned(),
                buy_token: "0x0000000000000000000000000000000000000002".to_owned(),
                receiver: None,
                sell_amount: "100".to_owned(),
                buy_amount: "95".to_owned(),
                valid_to: 1_700_000_000,
                app_data: "0xb48d38f93eaa084033fc5970bf96e559c33c4cdc07d889ab00b4d63f9590739d"
                    .to_owned(),
                fee_amount: "0".to_owned(),
                kind: OrderKind::Sell,
                partially_fillable: false,
                sell_token_balance: SellTokenSource::Erc20,
                buy_token_balance: BuyTokenDestination::Erc20,
            },
            from,
            expiration: "2026-01-01T00:00:00Z".to_owned(),
            id: Some(7),
            verified: true,
            protocol_fee_bps: None,
            protocol_fee_sell_amount: None,
        }
    }

    #[tokio::test]
    async fn posts_swap_order_happy_path() {
        let server = MockServer::start();
        let wallet = test_wallet();
        let owner = format!("{:#x}", wallet.address());
        let quote = sample_quote(owner.clone());

        let quote_mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/quote");
            then.status(200).json_body_obj(&quote);
        });

        let uid = "0x0123".to_owned();
        let order_mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/orders");
            then.status(200).json_body_obj(&uid);
        });

        let client = test_client(&server);
        let response = client
            .post_swap_order(
                TradeParameters::new(
                    OrderKind::Sell,
                    "0x0000000000000000000000000000000000000001",
                    "0x0000000000000000000000000000000000000002",
                    "100",
                ),
                &wallet,
            )
            .await
            .unwrap();

        quote_mock.assert();
        order_mock.assert();
        assert_eq!(response.order_uid, uid);
        assert_eq!(response.quote.id, Some(7));
        assert_eq!(response.signed_order.quote_id, Some(7));
        assert_eq!(response.signed_order.from.as_deref(), Some(owner.as_str()));
        assert_eq!(response.signed_order.signing_scheme, SigningScheme::Eip712);
        assert!(response.signed_order.signature.starts_with("0x"));
    }

    #[tokio::test]
    async fn propagates_quote_errors() {
        let server = MockServer::start();
        let _quote_mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/quote");
            then.status(500).body("quote failed");
        });

        let client = test_client(&server);
        let wallet = test_wallet();
        let error = client
            .post_swap_order(
                TradeParameters::new(
                    OrderKind::Sell,
                    "0x0000000000000000000000000000000000000001",
                    "0x0000000000000000000000000000000000000002",
                    "100",
                ),
                &wallet,
            )
            .await
            .unwrap_err();

        match error {
            CowError::Api { status, body } => {
                assert_eq!(status, 500);
                assert!(body.contains("quote failed"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
