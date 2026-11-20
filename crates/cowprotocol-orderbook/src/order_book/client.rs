//! Ergonomic [`OrderBookApi`] constructors and their type-state builder.
//!
//! This module is gated behind the `http-client` feature and works on
//! both targets: it builds clients over the target's
//! [`DefaultTransport`] (reqwest natively, browser `fetch` on wasm32).
//! The transport-generic [`OrderBookApi`], its endpoint logic, and the
//! quote pipeline live in the feature-independent [`api`](super::api) /
//! [`flow`](super::flow) siblings; the transport backends live in
//! [`crate::transport`].
//!
//! # Choosing a constructor
//!
//! There is one entry point per tier, from most common to most niche:
//!
//! - Quick start: [`OrderBookApi::new`] for a production [`Chain`], or
//!   [`OrderBookApi::new_with_base_url`] for an arbitrary base URL (barn,
//!   staging, a recorded mock). Both build over the target's
//!   [`DefaultTransport`] with the standard timeout.
//! - Custom transport: [`OrderBookApi::new_with_transport`] (in the
//!   feature-independent [`api`](super::api) sibling) for an out-of-tree
//!   [`HttpTransport`](crate::transport::HttpTransport) backend, needing
//!   neither this feature nor a concrete transport.
//! - Advanced HTTP: [`OrderBookApi::builder`] when you need a
//!   pre-configured [`reqwest::Client`] (custom timeouts, proxies, TLS
//!   roots, or auth middleware) together with a chain hint or base URL.

use crate::chain::Chain;
use crate::transport::DefaultTransport;
#[cfg(not(target_arch = "wasm32"))]
use crate::transport::ReqwestTransport;

use super::api::OrderBookApi;
use super::builder_state;

/// Type-state builder for [`OrderBookApi`].
#[derive(Debug, Clone)]
pub struct OrderBookApiBuilder<Target = builder_state::Missing> {
    chain: Option<Chain>,
    base_url: Option<url::Url>,
    transport: Option<DefaultTransport>,
    _state: core::marker::PhantomData<Target>,
}

impl OrderBookApiBuilder {
    const fn new() -> Self {
        Self {
            chain: None,
            base_url: None,
            transport: None,
            _state: core::marker::PhantomData,
        }
    }
}

impl<Target> OrderBookApiBuilder<Target> {
    fn cast<NextTarget>(self) -> OrderBookApiBuilder<NextTarget> {
        OrderBookApiBuilder {
            chain: self.chain,
            base_url: self.base_url,
            transport: self.transport,
            _state: core::marker::PhantomData,
        }
    }

    /// Use a pre-configured [`reqwest::Client`] for the orderbook API.
    ///
    /// Native targets only: this method is absent on `wasm32`, where the
    /// browser's `fetch` global is the only backend. Cross-target code
    /// should call [`Self::with_transport`] with a [`DefaultTransport`]
    /// instead, which resolves to the correct backend per target.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.transport = Some(ReqwestTransport::new(client));
        self
    }

    /// Use a pre-built [`DefaultTransport`].
    // const-eligible only on wasm32, where `DefaultTransport` is a unit
    // struct with no drop glue; keep one non-const signature per target.
    #[allow(clippy::missing_const_for_fn)]
    pub fn with_transport(mut self, transport: DefaultTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Target the production orderbook for a supported chain.
    pub fn with_chain(self, chain: Chain) -> OrderBookApiBuilder<builder_state::Set> {
        let mut next = self.cast::<builder_state::Set>();
        next.chain = Some(chain);
        next.base_url = Some(chain.orderbook_base_url());
        next
    }

    /// Target an arbitrary orderbook base URL, such as barn or a mock.
    pub fn with_base_url(self, base_url: url::Url) -> OrderBookApiBuilder<builder_state::Set> {
        let mut next = self.cast::<builder_state::Set>();
        next.chain = None;
        next.base_url = Some(base_url);
        next
    }
}

impl OrderBookApiBuilder<builder_state::Set> {
    /// Build the [`OrderBookApi`].
    pub fn build(self) -> OrderBookApi {
        let base_url = self.base_url.expect("target typestate sets base_url");
        let transport = self.transport.unwrap_or_default();
        let api = OrderBookApi::new_with_transport(base_url, transport);
        match self.chain {
            Some(chain) => api.with_chain_hint(chain),
            None => api,
        }
    }
}

impl OrderBookApi {
    /// Start a type-state builder for an orderbook client.
    ///
    /// Reach for the builder only when a quick-start constructor cannot
    /// express the configuration: pair a pre-configured
    /// [`reqwest::Client`] (custom timeouts, proxies, TLS roots, or auth
    /// middleware) via [`OrderBookApiBuilder::with_client`] with either
    /// [`OrderBookApiBuilder::with_chain`] or
    /// [`OrderBookApiBuilder::with_base_url`]. For the common cases
    /// prefer [`Self::new`] (a production [`Chain`]) or
    /// [`Self::new_with_base_url`] (an arbitrary URL), which need no
    /// terminal `.build()`.
    pub const fn builder() -> OrderBookApiBuilder {
        OrderBookApiBuilder::new()
    }

    /// Start a type-state builder targeting the production orderbook on `chain`.
    #[deprecated(
        since = "0.1.1",
        note = "use OrderBookApi::new(chain) for the quick-start path, or \
                OrderBookApi::builder().with_chain(chain).build() to keep configuring the builder"
    )]
    pub fn with_chain(chain: Chain) -> OrderBookApiBuilder<builder_state::Set> {
        Self::builder().with_chain(chain)
    }

    /// Client for the production orderbook on `chain`, over the target's
    /// [`DefaultTransport`].
    /// [`Chain::orderbook_base_url`] already includes the trailing slash
    /// [`url::Url::join`] needs to append, not replace, path segments.
    pub fn new(chain: Chain) -> Self {
        Self::new_with_transport(chain.orderbook_base_url(), DefaultTransport::default())
            .with_chain_hint(chain)
    }

    /// Client against an arbitrary base URL (staging, recorded mock,
    /// etc.). The default transport enforces [`DEFAULT_HTTP_TIMEOUT`].
    /// The chain is left unknown; prefer [`Self::new`] when targeting a
    /// production chain so the quote pipeline can infer the signing
    /// domain and cross-check it.
    ///
    /// [`DEFAULT_HTTP_TIMEOUT`]: super::DEFAULT_HTTP_TIMEOUT
    pub fn new_with_base_url(base_url: url::Url) -> Self {
        Self::new_with_transport(base_url, DefaultTransport::default())
    }

    /// Client around a pre-configured [`reqwest::Client`]. Use for custom
    /// timeouts, proxies, TLS roots, or auth middleware.
    ///
    /// Native targets only: this constructor is absent on `wasm32`, where
    /// the browser's `fetch` global is the only backend. Cross-target
    /// code should build over a [`DefaultTransport`] via
    /// [`Self::new_with_transport`] (or the chain constructors) instead.
    #[cfg(not(target_arch = "wasm32"))]
    #[deprecated(
        since = "0.1.1",
        note = "use OrderBookApi::builder().with_base_url(base_url).with_client(client).build(), \
                the advanced-HTTP tier reserved for pre-configured reqwest clients"
    )]
    pub fn with_client(base_url: url::Url, client: reqwest::Client) -> Self {
        Self::new_with_transport(base_url, ReqwestTransport::new(client))
    }
}
