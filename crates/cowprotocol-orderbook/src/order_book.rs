//! Thin client for the CoW Protocol orderbook HTTP API.
//!
//! The first endpoint implemented here is [`OrderBookApi::quote`],
//! which mirrors the `getQuote` flow exposed by `@cowprotocol/cow-sdk`
//! and `cow-py`. The request and response shapes reflect the
//! production orderbook OpenAPI as of 2026-05.

use std::time::Duration;

/// Default per-request timeout, enforced by both bundled transports
/// (reqwest natively, browser `fetch` on wasm32). A stuck or hostile
/// orderbook cannot hold a caller's task open longer; on native targets,
/// override it by passing a pre-configured client to
/// `OrderBookApiBuilder::with_client`. Exposed feature-independently so
/// out-of-tree [`HttpTransport`](crate::transport::HttpTransport)
/// backends can reuse it without pulling in the `http-client` stack.
pub const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum HTTP response body. Larger payloads return
/// [`Error::ResponseTooLarge`] before allocating. Exposed
/// feature-independently so out-of-tree
/// [`HttpTransport`](crate::transport::HttpTransport) backends can reuse
/// it without pulling in the `http-client` stack.
///
/// [`Error::ResponseTooLarge`]: crate::Error::ResponseTooLarge
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Marker types used by this module's type-state builders
/// ([`QuoteRequestBuilder`], `OrderBookApiBuilder`) to track required
/// fields.
pub mod builder_state {
    /// Required field has not been supplied yet.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Missing;

    /// Required field has been supplied.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Set;
}

mod types;
pub use types::*;

mod quote;
pub use quote::*;

mod orders;
pub use orders::{
    DEFAULT_ORDER_VALID_TO_MAX_HORIZON_SECS, Order, OrderCreation, OrderCreationBuilder,
    OrderStatus,
};

mod api;
pub use api::OrderBookApi;

mod flow;
pub use flow::{OrderSubmission, QuoteRequestBuilder, QuotedOrder, ReadyQuoteRequestBuilder};

#[cfg(feature = "http-client")]
mod client;
#[cfg(feature = "http-client")]
pub use client::OrderBookApiBuilder;

#[cfg(test)]
#[path = "order_book/tests.rs"]
mod tests;
