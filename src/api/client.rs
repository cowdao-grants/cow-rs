use crate::{
    config::SdkConfig,
    domain::{
        order::{EnrichedOrder, GetOrdersRequest, SignedOrder},
        quote::{Quote, QuoteRequest},
        types::OrderUid,
    },
    error::{CowError, Result},
};
use reqwest::{Client, Method};
use serde::{de::DeserializeOwned, Serialize};
#[cfg(test)]
use url::Url;

#[derive(Clone, Debug)]
pub struct OrderBookClient {
    config: SdkConfig,
    http: Client,
}

impl OrderBookClient {
    pub fn new(config: SdkConfig) -> Result<Self> {
        let http = Client::builder().timeout(config.timeout).build()?;
        Ok(Self { config, http })
    }

    pub fn config(&self) -> &SdkConfig {
        &self.config
    }

    pub async fn get_quote(&self, request: &QuoteRequest) -> Result<Quote> {
        self.request(
            Method::POST,
            "api/v1/quote",
            Some(request),
            None::<&[(&str, String)]>,
        )
        .await
    }

    pub async fn send_order(&self, order: &SignedOrder) -> Result<OrderUid> {
        self.request(
            Method::POST,
            "api/v1/orders",
            Some(order),
            None::<&[(&str, String)]>,
        )
        .await
    }

    pub async fn get_order(&self, order_uid: &str) -> Result<EnrichedOrder> {
        self.request::<(), EnrichedOrder>(
            Method::GET,
            &format!("api/v1/orders/{order_uid}"),
            None,
            None::<&[(&str, String)]>,
        )
        .await
    }

    pub async fn get_orders(&self, request: &GetOrdersRequest) -> Result<Vec<EnrichedOrder>> {
        let query = [
            ("offset", request.offset.to_string()),
            ("limit", request.limit.to_string()),
        ];

        self.request::<(), Vec<EnrichedOrder>>(
            Method::GET,
            &format!("api/v1/account/{}/orders", request.owner),
            None,
            Some(&query),
        )
        .await
    }

    async fn request<B, T>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        query: Option<&[(&str, String)]>,
    ) -> Result<T>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let mut url = self.config.orderbook_base_url()?.join(path)?;
        if let Some(query) = query {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        }

        let mut request = self.http.request(method, url);
        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request.send().await?;
        let status = response.status();
        let bytes = response.bytes().await?;

        if !status.is_success() {
            return Err(CowError::Api {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }

        serde_json::from_slice(&bytes).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{ChainId, Environment},
        domain::{
            order::{
                BuyTokenDestination, Order, OrderKind, SellTokenSource, SignedOrder, SigningScheme,
            },
            quote::{Quote, QuoteRequest},
        },
    };
    use httpmock::{
        Method::{GET, POST},
        MockServer,
    };

    fn test_client(server: &MockServer) -> OrderBookClient {
        let config = SdkConfig::new("cow-rs", ChainId::Sepolia, Environment::Staging)
            .with_base_url(Url::parse(&server.base_url()).unwrap());
        OrderBookClient::new(config).unwrap()
    }

    fn sample_quote_response() -> Quote {
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
            from: "0x0000000000000000000000000000000000000003".to_owned(),
            expiration: "2026-01-01T00:00:00Z".to_owned(),
            id: Some(7),
            verified: true,
            protocol_fee_bps: None,
            protocol_fee_sell_amount: None,
        }
    }

    #[tokio::test]
    async fn requests_quote() {
        let server = MockServer::start();
        let quote = sample_quote_response();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/quote");
            then.status(200).json_body_obj(&quote);
        });

        let client = test_client(&server);
        let response = client
            .get_quote(&QuoteRequest::from_trade_amount(
                OrderKind::Sell,
                quote.quote.sell_token.clone(),
                quote.quote.buy_token.clone(),
                "100".to_owned(),
                quote.from.clone(),
            ))
            .await
            .unwrap();

        mock.assert();
        assert_eq!(response.id, Some(7));
        assert_eq!(response.quote.buy_amount, "95");
    }

    #[tokio::test]
    async fn sends_order() {
        let server = MockServer::start();
        let uid = "0x0123".to_owned();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/orders");
            then.status(200).json_body_obj(&uid);
        });

        let client = test_client(&server);
        let quote = sample_quote_response();
        let signed_order = SignedOrder {
            order: quote.quote,
            signing_scheme: SigningScheme::Eip712,
            signature: "0xdeadbeef".to_owned(),
            from: Some("0x0000000000000000000000000000000000000003".to_owned()),
            quote_id: Some(7),
        };

        let response = client.send_order(&signed_order).await.unwrap();
        mock.assert();
        assert_eq!(response, uid);
    }

    #[tokio::test]
    async fn fetches_order_and_order_list() {
        let server = MockServer::start();
        let enriched = EnrichedOrder {
            order: sample_quote_response().quote,
            uid: Some("0xabc".to_owned()),
            owner: Some("0x0000000000000000000000000000000000000003".to_owned()),
            status: Some("open".to_owned()),
            creation_date: Some("2026-01-01T00:00:00Z".to_owned()),
            executed_buy_amount: Some("0".to_owned()),
            executed_sell_amount: Some("0".to_owned()),
            executed_fee_amount: Some("0".to_owned()),
            invalidated: Some(false),
        };

        let single = server.mock(|when, then| {
            when.method(GET).path("/api/v1/orders/0xabc");
            then.status(200).json_body_obj(&enriched);
        });

        let list = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/account/0x0000000000000000000000000000000000000003/orders")
                .query_param("offset", "0")
                .query_param("limit", "1000");
            then.status(200).json_body_obj(&vec![enriched.clone()]);
        });

        let client = test_client(&server);
        let order = client.get_order("0xabc").await.unwrap();
        let orders = client
            .get_orders(&GetOrdersRequest::new(
                "0x0000000000000000000000000000000000000003".to_owned(),
            ))
            .await
            .unwrap();

        single.assert();
        list.assert();
        assert_eq!(order.uid.as_deref(), Some("0xabc"));
        assert_eq!(orders.len(), 1);
    }
}
