pub mod api;
pub mod config;
pub mod domain;
pub mod error;
pub mod signing;
pub mod trading;

pub use api::client::OrderBookClient;
pub use config::{ChainId, Environment, SdkConfig};
pub use domain::{
    order::{
        BuyTokenDestination, EnrichedOrder, GetOrdersRequest, Order as QuoteOrder, OrderKind,
        SellTokenSource, SignedOrder, SignedOrder as OrderCreation, SigningScheme,
    },
    quote::{
        PriceQuality, Quote as OrderQuoteResponse, QuoteRequest as OrderQuoteRequest,
        TradeParameters,
    },
    types::OrderUid,
};
pub use error::{CowError, Result};
pub use signing::{order_signing_hash, sign_order, SigningResult};
pub use trading::{PostSwapOrderResponse, TradingClient};
