//! [`OrderBookApi`]: the transport-generic HTTP client for the CoW Protocol
//! orderbook and the request/response plumbing behind its endpoints.
//!
//! This module is feature-independent: it is generic over an
//! [`HttpTransport`] and never names a concrete backend. The per-target
//! backends (reqwest natively, browser `fetch` on wasm32) live in
//! [`crate::transport`], and the ergonomic constructors in the
//! `http-client`-gated [`client`](super::client) sibling. Every backend
//! drives the same endpoint logic here.

use alloy_primitives::{Address, B256};
use serde::{Deserialize, Serialize};

use crate::app_data::AppDataHash;
use crate::cancellation::{SignedOrderCancellation, SignedOrderCancellations};
use crate::chain::Chain;
use crate::error::{Error, Result};
use crate::order::OrderUid;
use crate::signature::{EcdsaSignature, ecdsa_wire};
use crate::signing_scheme::EcdsaSigningScheme;
use crate::transport::{HttpMethod, HttpRequest, HttpTransport};

use super::orders::{Order, OrderCreation};
use super::quote::{OrderQuoteResponse, QuoteRequest};
use super::types::{
    AppDataDocument, Auction, AuctionStatus, NativePrice, TokenMetadata, TotalSurplus, Trade,
};

#[cfg(feature = "http-client")]
use crate::transport::DefaultTransport;

/// Wire body for `POST /api/v1/orders/by_uids`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OrdersByUidsRequest<'a> {
    pub(crate) order_uids: &'a [OrderUid],
}

/// Wire body for `DELETE /api/v1/orders/{uid}`. The UID lives in the URL,
/// so the body is just the signature material; this mirrors the upstream
/// `CancellationPayload` shape in `cowprotocol/services/crates/model/
/// src/order.rs`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CancellationPayload {
    #[serde(with = "ecdsa_wire")]
    signature: EcdsaSignature,
    signing_scheme: EcdsaSigningScheme,
}

/// Transport-generic client for the CoW Protocol orderbook.
///
/// `T` is the [`HttpTransport`] backend. With the `http-client` feature
/// it defaults to the target's [`DefaultTransport`]: reqwest natively,
/// browser `fetch` on wasm32.
// NOTE: keep in sync with the cfg twin below; only the default type
// parameter differs.
#[cfg(feature = "http-client")]
#[derive(Debug, Clone)]
pub struct OrderBookApi<T = DefaultTransport> {
    base_url: url::Url,
    transport: T,
    // `Some` when built from a [`Chain`]; `None` when built from an
    // arbitrary URL (staging / mock), where the chain is not known.
    // [`QuotedOrder::sign_with`](super::QuotedOrder::sign_with) uses it
    // to refuse a chain that disagrees with the signing domain, and
    // [`Self::post_order`] to owner-verify bodies before posting.
    chain: Option<Chain>,
}

/// Transport-generic client for the CoW Protocol orderbook.
///
/// `T` is the [`HttpTransport`] backend (e.g. the `cow-sdk-wasm` `fetch`
/// transport when the `http-client` feature is off).
// NOTE: keep in sync with the cfg twin above; only the default type
// parameter differs.
#[cfg(not(feature = "http-client"))]
#[derive(Debug, Clone)]
pub struct OrderBookApi<T> {
    base_url: url::Url,
    transport: T,
    // `Some` when built from a [`Chain`]; `None` when built from an
    // arbitrary URL (staging / mock), where the chain is not known.
    // [`QuotedOrder::sign_with`](super::QuotedOrder::sign_with) uses it
    // to refuse a chain that disagrees with the signing domain, and
    // [`Self::post_order`] to owner-verify bodies before posting.
    chain: Option<Chain>,
}

impl<T: HttpTransport> OrderBookApi<T> {
    /// Build a client around an arbitrary [`HttpTransport`] and base URL:
    /// the custom-transport tier of the constructor hierarchy, feature-
    /// independent so an out-of-tree backend needs neither `http-client`
    /// nor a concrete transport. With `http-client` enabled, prefer the
    /// quick-start [`OrderBookApi::new`] / [`OrderBookApi::new_with_base_url`]
    /// or the advanced [`OrderBookApi::builder`] instead.
    ///
    /// The chain is left unknown; supply it with [`Self::with_chain_hint`]
    /// when the transport targets a known production chain so the quote
    /// pipeline can cross-check it and [`Self::post_order`] can
    /// owner-verify bodies.
    pub fn new_with_transport(base_url: url::Url, transport: T) -> Self {
        Self {
            base_url: ensure_trailing_slash(base_url),
            transport,
            chain: None,
        }
    }

    /// Attach a [`Chain`] hint and return the client, for transports built
    /// against a known production chain.
    #[must_use]
    pub const fn with_chain_hint(mut self, chain: Chain) -> Self {
        self.chain = Some(chain);
        self
    }

    /// Base URL with the trailing slash path joining relies on.
    pub const fn base_url(&self) -> &url::Url {
        &self.base_url
    }

    /// The [`Chain`] this client targets, when known. `Some` only when
    /// built from a chain; arbitrary-URL constructors leave it `None`,
    /// which also disables [`Self::post_order`]'s owner verification.
    pub const fn chain(&self) -> Option<Chain> {
        self.chain
    }

    /// `POST /api/v1/quote`. Rejects an inconsistent request via
    /// [`QuoteRequest::validate`] before issuing it.
    pub async fn quote(&self, request: &QuoteRequest) -> Result<OrderQuoteResponse> {
        request.validate()?;
        self.post_json(self.endpoint("api/v1/quote")?, request)
            .await
    }

    /// `POST /api/v1/orders`. Returns the assigned 56-byte UID.
    ///
    /// When the client carries a chain hint ([`Self::chain`] is
    /// `Some`), the body is owner-verified
    /// ([`OrderCreation::verify_owner`]) against that chain's
    /// settlement domain before anything reaches the wire. This makes
    /// the verification structural for every chain-bound submission
    /// path, including hand-assembled bodies, at the cost of one ECDSA
    /// recovery per submission. Chainless clients (arbitrary base
    /// URLs: mocks, staging) skip the check, since the signing domain
    /// is unknown to them.
    pub async fn post_order(&self, order: &OrderCreation) -> Result<OrderUid> {
        if let Some(chain) = self.chain {
            order.verify_owner(&chain.settlement_domain())?;
        }
        self.post_json(self.endpoint("api/v1/orders")?, order).await
    }

    /// `GET /api/v1/orders/{uid}`.
    pub async fn order(&self, uid: &OrderUid) -> Result<Order> {
        self.get_json(self.endpoint(&format!("api/v1/orders/{uid}"))?)
            .await
    }

    /// `GET /api/v1/orders/{uid}/status`.
    pub async fn order_status(&self, uid: &OrderUid) -> Result<AuctionStatus> {
        self.get_json(self.endpoint(&format!("api/v1/orders/{uid}/status"))?)
            .await
    }

    /// Poll [`Self::order`] until `should_stop` returns `true`,
    /// sleeping via the caller-supplied closure. Runtime-agnostic;
    /// pass `tokio::time::sleep`, `gloo_timers::future::sleep`, or
    /// any `Future<Output = ()>` producer. Callers wanting a deadline
    /// bake it into `should_stop`.
    pub async fn poll_until<P, S, Fut>(
        &self,
        uid: &OrderUid,
        mut should_stop: P,
        mut sleep: S,
    ) -> Result<Order>
    where
        P: FnMut(&Order) -> bool,
        S: FnMut() -> Fut,
        Fut: core::future::Future<Output = ()>,
    {
        loop {
            let order = self.order(uid).await?;
            if should_stop(&order) {
                return Ok(order);
            }
            sleep().await;
        }
    }

    /// `GET /api/v1/account/{owner}/orders`. Most recent first. Pass
    /// `None` for both pagers to use the server defaults.
    pub async fn account_orders(
        &self,
        owner: Address,
        offset: Option<u32>,
        limit: Option<u32>,
    ) -> Result<Vec<Order>> {
        let url = self.endpoint(&format!("api/v1/account/{owner:?}/orders"))?;
        self.get_json_paginated(url, offset, limit).await
    }

    /// `POST /api/v1/orders/by_uids`. Returns orders in request
    /// order; unknown UIDs are omitted.
    pub async fn orders_by_uids(&self, uids: &[OrderUid]) -> Result<Vec<Order>> {
        self.post_json(
            self.endpoint("api/v1/orders/by_uids")?,
            &OrdersByUidsRequest { order_uids: uids },
        )
        .await
    }

    /// `GET /api/v2/trades?owner=...`. Newest first. Pass `None` for both
    /// pagers to use the server defaults (`offset` 0, `limit` 10); the
    /// server caps `limit` at 1000. v2 replaces the deprecated,
    /// unpaginated v1 endpoint.
    pub async fn trades_by_owner(
        &self,
        owner: Address,
        offset: Option<u32>,
        limit: Option<u32>,
    ) -> Result<Vec<Trade>> {
        let mut url = self.endpoint("api/v2/trades")?;
        url.query_pairs_mut()
            .append_pair("owner", &format!("{owner:?}"));
        self.get_json_paginated(url, offset, limit).await
    }

    /// `GET /api/v2/trades?orderUid=...`. Newest first; see
    /// [`OrderBookApi::trades_by_owner`] for the pagination semantics.
    pub async fn trades_by_order_uid(
        &self,
        uid: &OrderUid,
        offset: Option<u32>,
        limit: Option<u32>,
    ) -> Result<Vec<Trade>> {
        let mut url = self.endpoint("api/v2/trades")?;
        url.query_pairs_mut()
            .append_pair("orderUid", &uid.to_string());
        self.get_json_paginated(url, offset, limit).await
    }

    /// `GET /api/v1/token/{token}/native_price`. One atomic unit of
    /// `token` in the chain's native gas token; solvers use this to
    /// denominate gas uniformly across pairs.
    pub async fn native_price(&self, token: Address) -> Result<NativePrice> {
        self.get_json(self.endpoint(&format!("api/v1/token/{token:?}/native_price"))?)
            .await
    }

    /// `GET /api/v1/token/{token}/metadata`.
    ///
    /// The handler ships in upstream `services`, so this works against
    /// production today, but the route is not documented in the bundled
    /// orderbook OpenAPI and `@cowprotocol/cow-sdk` does not expose it.
    /// Treat it as best-effort: it could be removed without an OpenAPI
    /// bump.
    pub async fn token_metadata(&self, token: Address) -> Result<TokenMetadata> {
        self.get_json(self.endpoint(&format!("api/v1/token/{token:?}/metadata"))?)
            .await
    }

    /// `GET /api/v1/transactions/{hash}/orders`. Empty list for an
    /// unknown settlement.
    pub async fn orders_by_tx(&self, tx_hash: B256) -> Result<Vec<Order>> {
        self.get_json(self.endpoint(&format!("api/v1/transactions/{tx_hash:?}/orders"))?)
            .await
    }

    /// `GET /api/v1/auction`. Permissioned (solver-only); the
    /// public-facing proxy returns 403. Shipped for parity with
    /// cow-py / cow-sdk; per-order array is opaque JSON because the
    /// auction shape drifts across CIPs.
    pub async fn auction(&self) -> Result<Auction> {
        self.get_json(self.endpoint("api/v1/auction")?).await
    }

    /// `GET /api/v1/users/{user}/total_surplus`.
    pub async fn total_surplus(&self, user: Address) -> Result<TotalSurplus> {
        self.get_json(self.endpoint(&format!("api/v1/users/{user:?}/total_surplus"))?)
            .await
    }

    /// `GET /api/v1/app_data/{hash}`. Re-hashes the returned body
    /// locally and rejects with [`Error::AppDataHashMismatch`] when
    /// the digest disagrees with `hash`; the signed order commits to
    /// the digest, so this closes the loop between what was signed
    /// and what downstream code displays.
    pub async fn app_data(&self, hash: &AppDataHash) -> Result<AppDataDocument> {
        let document: AppDataDocument = self
            .get_json(self.endpoint(&format!("api/v1/app_data/{hash}"))?)
            .await?;
        let computed = document.computed_hash();
        if computed != *hash {
            return Err(Error::AppDataHashMismatch {
                expected: hash.to_string(),
                computed: computed.to_string(),
            });
        }
        Ok(document)
    }

    /// `PUT /api/v1/app_data/{hash}`. Hashes `document.full_app_data`
    /// locally first and refuses with [`Error::AppDataHashMismatch`]
    /// on a digest disagreement.
    pub async fn put_app_data(&self, hash: &AppDataHash, document: &AppDataDocument) -> Result<()> {
        let computed = document.computed_hash();
        if computed != *hash {
            return Err(Error::AppDataHashMismatch {
                expected: hash.to_string(),
                computed: computed.to_string(),
            });
        }
        self.send(
            HttpMethod::Put,
            self.endpoint(&format!("api/v1/app_data/{hash}"))?,
            Some(serde_json::to_vec(document)?),
        )
        .await?
        .decode_empty()
    }

    /// `PUT /api/v1/app_data`. Lets the orderbook compute and return
    /// the digest, then verifies it against
    /// [`AppDataDocument::computed_hash`] locally and rejects with
    /// [`Error::AppDataHashMismatch`] on disagreement. The signed order
    /// commits only to the digest, so a server-supplied hash that does
    /// not match the document the caller uploaded would silently swap
    /// the metadata bound to the order.
    pub async fn upload_app_data(&self, document: &AppDataDocument) -> Result<AppDataHash> {
        let computed = document.computed_hash();
        let server_hash: AppDataHash = self
            .put_json(self.endpoint("api/v1/app_data")?, document)
            .await?;
        if server_hash != computed {
            return Err(Error::AppDataHashMismatch {
                expected: server_hash.to_string(),
                computed: computed.to_string(),
            });
        }
        Ok(server_hash)
    }

    /// `GET /api/v1/version`. Plain-text liveness probe.
    pub async fn version(&self) -> Result<String> {
        self.send(HttpMethod::Get, self.endpoint("api/v1/version")?, None)
            .await?
            .decode_text()
    }

    /// `DELETE /api/v1/orders`. UIDs travel in the body, not the URL.
    /// Soft-cancel: orders already in flight may still settle.
    pub async fn cancel_orders(&self, signed: &SignedOrderCancellations) -> Result<()> {
        self.send(
            HttpMethod::Delete,
            self.endpoint("api/v1/orders")?,
            Some(serde_json::to_vec(signed)?),
        )
        .await?
        .decode_empty()
    }

    /// `DELETE /api/v1/orders/{uid}`. Soft-cancel: an order already
    /// picked up by a solver may still settle. For pre-signed and
    /// EthFlow orders, invalidate on-chain instead.
    pub async fn cancel_order(&self, cancellation: &SignedOrderCancellation) -> Result<()> {
        let body = CancellationPayload {
            signature: cancellation.signature,
            signing_scheme: cancellation.signing_scheme,
        };
        self.send(
            HttpMethod::Delete,
            self.endpoint(&format!("api/v1/orders/{}", cancellation.order_uid))?,
            Some(serde_json::to_vec(&body)?),
        )
        .await?
        .decode_empty()
    }

    /// Join `path` onto the base URL: the one-line URL construction every
    /// endpoint performs before handing its request to [`Self::send`].
    fn endpoint(&self, path: &str) -> Result<url::Url> {
        Ok(self.base_url.join(path)?)
    }

    /// Execute `method` against `url` through the transport: the single
    /// execute site every request issued by this client goes through.
    async fn send(
        &self,
        method: HttpMethod,
        url: url::Url,
        json_body: Option<Vec<u8>>,
    ) -> Result<crate::transport::HttpResponse> {
        self.transport
            .execute(HttpRequest {
                method,
                url,
                json_body,
                bearer: None,
            })
            .await
    }

    async fn post_json<TReq, TResp>(&self, url: url::Url, body: &TReq) -> Result<TResp>
    where
        TReq: Serialize + ?Sized,
        TResp: for<'de> Deserialize<'de>,
    {
        self.send(HttpMethod::Post, url, Some(serde_json::to_vec(body)?))
            .await?
            .decode_json()
    }

    async fn put_json<TReq, TResp>(&self, url: url::Url, body: &TReq) -> Result<TResp>
    where
        TReq: Serialize + ?Sized,
        TResp: for<'de> Deserialize<'de>,
    {
        self.send(HttpMethod::Put, url, Some(serde_json::to_vec(body)?))
            .await?
            .decode_json()
    }

    async fn get_json<TResp>(&self, url: url::Url) -> Result<TResp>
    where
        TResp: for<'de> Deserialize<'de>,
    {
        self.send(HttpMethod::Get, url, None).await?.decode_json()
    }

    /// `GET url` with the optional `offset` / `limit` pagination appended
    /// to its query string, then decoded as JSON. Endpoint-specific
    /// filters (`owner`, `orderUid`) are appended by the caller before
    /// the call.
    async fn get_json_paginated<TResp>(
        &self,
        mut url: url::Url,
        offset: Option<u32>,
        limit: Option<u32>,
    ) -> Result<TResp>
    where
        TResp: for<'de> Deserialize<'de>,
    {
        append_pagination(&mut url.query_pairs_mut(), offset, limit);
        self.get_json(url).await
    }
}

/// Normalise a base URL to a trailing slash so [`url::Url::join`] appends,
/// rather than replaces, path segments.
pub(crate) fn ensure_trailing_slash(mut url: url::Url) -> url::Url {
    if !url.path().ends_with('/') {
        let new_path = format!("{}/", url.path());
        url.set_path(&new_path);
    }
    url
}

/// Append the optional `offset` / `limit` pagination pair to a query.
/// `None` leaves the parameter off so the server applies its default.
fn append_pagination(
    q: &mut url::form_urlencoded::Serializer<'_, url::UrlQuery<'_>>,
    offset: Option<u32>,
    limit: Option<u32>,
) {
    if let Some(offset) = offset {
        q.append_pair("offset", &offset.to_string());
    }
    if let Some(limit) = limit {
        q.append_pair("limit", &limit.to_string());
    }
}
