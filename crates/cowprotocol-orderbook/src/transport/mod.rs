//! Transport abstraction decoupling [`OrderBookApi`] from a concrete HTTP
//! backend.
//!
//! [`OrderBookApi`] builds a transport-agnostic [`HttpRequest`] for each
//! endpoint and hands it to an [`HttpTransport`]; the transport performs
//! the actual I/O and returns an [`HttpResponse`] whose body has already
//! been bounded by [`MAX_RESPONSE_BYTES`]. This is what lets the native
//! (reqwest) and wasm (`fetch`) builds share one client and one set of
//! endpoint logic instead of re-implementing request shaping, status
//! handling and JSON decoding per target.
//!
//! The concrete backends live as submodules behind the `http-client`
//! feature; the trait itself stays feature-independent so out-of-tree
//! backends can implement it.
//!
//! [`OrderBookApi`]: crate::OrderBookApi
//! [`MAX_RESPONSE_BYTES`]: crate::order_book::MAX_RESPONSE_BYTES

use serde::de::DeserializeOwned;

use crate::error::{ApiError, Error, Result};

// Pathed as `self::reqwest` so the submodule never shadows the extern
// crate of the same name under uniform paths.
#[cfg(all(feature = "http-client", not(target_arch = "wasm32")))]
pub(crate) mod reqwest;
/// Native HTTP backend (`reqwest`). Present only on non-`wasm32`
/// targets; `wasm32` builds expose `FetchTransport` instead. Prefer
/// [`DefaultTransport`] in cross-target code so the right backend is
/// selected automatically.
#[cfg(all(feature = "http-client", not(target_arch = "wasm32")))]
pub use self::reqwest::ReqwestTransport;

#[cfg(all(feature = "http-client", target_arch = "wasm32"))]
mod fetch;
/// Browser `fetch` HTTP backend. Present only on `wasm32` targets;
/// native builds expose `ReqwestTransport` instead. Prefer
/// [`DefaultTransport`] in cross-target code so the right backend is
/// selected automatically.
#[cfg(all(feature = "http-client", target_arch = "wasm32"))]
pub use self::fetch::FetchTransport;

/// The HTTP backend the `http-client` feature ships for the build
/// target: [`ReqwestTransport`] natively, `FetchTransport` on wasm32.
/// [`OrderBookApi`](crate::OrderBookApi) and
/// [`SubgraphClient`](crate::subgraph::SubgraphClient) default to it, so
/// `cowprotocol` consumers get a working client on either target without
/// naming a transport.
#[cfg(all(feature = "http-client", not(target_arch = "wasm32")))]
pub type DefaultTransport = ReqwestTransport;
/// The HTTP backend the `http-client` feature ships for the build
/// target: `ReqwestTransport` natively, [`FetchTransport`] on wasm32.
/// [`OrderBookApi`](crate::OrderBookApi) and
/// [`SubgraphClient`](crate::subgraph::SubgraphClient) default to it, so
/// `cowprotocol` consumers get a working client on either target without
/// naming a transport.
#[cfg(all(feature = "http-client", target_arch = "wasm32"))]
pub type DefaultTransport = FetchTransport;

/// HTTP method for an orderbook request. Kept minimal: the orderbook only
/// uses these four verbs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpMethod {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
    /// `DELETE`.
    Delete,
}

impl HttpMethod {
    /// Canonical uppercase HTTP verb string, suitable for host
    /// passthrough APIs that accept method names instead of an enum.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

/// A fully-formed, transport-agnostic request the orderbook client hands to
/// an [`HttpTransport`]. The body, when present, is already serialised JSON
/// and implies a `content-type: application/json` header.
#[derive(Clone)]
pub struct HttpRequest {
    /// Request method.
    pub method: HttpMethod,
    /// Absolute request URL (base URL joined with the endpoint path and any
    /// query string).
    pub url: url::Url,
    /// Pre-serialised JSON body, or `None` for body-less requests.
    pub json_body: Option<Vec<u8>>,
    /// `Authorization: Bearer` credential for authenticated gateways (The
    /// Graph). `None` for the orderbook.
    pub bearer: Option<String>,
}

/// Manual `Debug` impl that renders `bearer` as `Some("<redacted>")` so a
/// logging transport cannot leak the credential.
impl core::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("json_body", &self.json_body)
            .field("bearer", &self.bearer.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl HttpRequest {
    /// Path plus query string for host APIs that route relative to a
    /// chain-specific CoW API base URL.
    ///
    /// For example, `https://cow.host/api/v1/orders?offset=10` becomes
    /// `/api/v1/orders?offset=10`. The method intentionally omits scheme,
    /// host, and fragment so custom transports can forward only the
    /// endpoint path to a sandbox host interface.
    pub fn path_and_query(&self) -> String {
        self.url.query().map_or_else(
            || self.url.path().to_owned(),
            |query| format!("{}?{query}", self.url.path()),
        )
    }
}

/// The response an [`HttpTransport`] returns after applying the body-size
/// cap. The body is decoded UTF-8 text; status handling and JSON decoding
/// happen in [`OrderBookApi`](crate::OrderBookApi) via the `decode_*`
/// helpers so both transports share one path.
#[derive(Clone, Debug)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body as UTF-8 text, already bounded by the transport's cap.
    pub body: String,
}

/// HTTP backend [`OrderBookApi`](crate::OrderBookApi) issues requests
/// through. The `http-client` feature ships one implementation per build
/// target (see [`DefaultTransport`]): `ReqwestTransport` natively and the
/// browser-`fetch`-backed `FetchTransport` on wasm32.
///
/// The method is `async fn` rather than `-> impl Future + Send` on purpose:
/// the wasm `fetch` transport's future is `!Send`, so requiring `Send`
/// would make the trait unimplementable there. Native callers drive it with
/// a direct `.await` (no `tokio::spawn`), so the missing `Send` bound is not
/// a limitation in practice.
///
/// # Implementing a custom backend
///
/// The trait is feature-independent, so out-of-tree backends can
/// implement it without enabling `http-client`. A backend provides one
/// `async fn execute`, returning an [`HttpResponse`] whose `body` is the
/// decoded UTF-8 text (status handling and JSON decoding happen in
/// [`OrderBookApi`](crate::OrderBookApi)). This logging wrapper forwards
/// to an inner transport, then hands the result back unchanged:
///
/// ```
/// use cowprotocol_orderbook::error::Result;
/// use cowprotocol_orderbook::{HttpRequest, HttpResponse, HttpTransport};
///
/// struct Logged<T>(T);
///
/// impl<T: HttpTransport> HttpTransport for Logged<T> {
///     async fn execute(&self, request: HttpRequest) -> Result<HttpResponse> {
///         eprintln!("{:?} {}", request.method, request.url);
///         self.0.execute(request).await
///     }
/// }
/// ```
///
/// Pass an instance to
/// [`OrderBookApi::new_with_transport`](crate::OrderBookApi::new_with_transport).
///
/// For sandboxed WASM guests, the backend can be a host passthrough rather
/// than an HTTP stack in the guest. Build the API with an arbitrary base URL
/// whose path is `/`, add a [`crate::Chain`] hint for signing checks, and
/// forward the request method, relative path, and JSON body to the host:
///
/// ```
/// use cowprotocol_orderbook::error::Result;
/// use cowprotocol_orderbook::{
///     Chain, HttpRequest, HttpResponse, HttpTransport, OrderBookApi,
/// };
///
/// trait CowApiHost {
///     fn request(
///         &self,
///         method: &str,
///         path: &str,
///         body: Option<&str>,
///     ) -> Result<(u16, String)>;
/// }
///
/// struct HostTransport<H> {
///     host: H,
/// }
///
/// impl<H: CowApiHost> HttpTransport for HostTransport<H> {
///     async fn execute(&self, request: HttpRequest) -> Result<HttpResponse> {
///         let body = request
///             .json_body
///             .as_ref()
///             .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
///         let (status, body) = self.host.request(
///             request.method.as_str(),
///             &request.path_and_query(),
///             body.as_deref(),
///         )?;
///         Ok(HttpResponse { status, body })
///     }
/// }
///
/// # struct StubHost;
/// # impl CowApiHost for StubHost {
/// #     fn request(&self, _: &str, _: &str, _: Option<&str>) -> Result<(u16, String)> {
/// #         Ok((200, "{}".to_owned()))
/// #     }
/// # }
/// let transport = HostTransport { host: StubHost };
/// let base = url::Url::parse("https://cow.host/").unwrap();
/// let _api = OrderBookApi::new_with_transport(base, transport)
///     .with_chain_hint(Chain::Mainnet);
/// ```
#[allow(async_fn_in_trait)]
pub trait HttpTransport {
    /// Execute `request` and return the capped response, or an [`Error`] for
    /// a transport-level failure (connection, timeout, oversized body).
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse>;
}

impl HttpResponse {
    /// `true` for a 2xx status.
    pub(crate) const fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }

    /// Decode a 2xx body as JSON, or map a non-2xx `(status, body)` to an
    /// [`Error`] via [`Self::into_status_error`].
    pub(crate) fn decode_json<T: DeserializeOwned>(self) -> Result<T> {
        if self.is_success() {
            serde_json::from_str(&self.body).map_err(Error::from)
        } else {
            Err(self.into_status_error())
        }
    }

    /// Accept a 2xx response that carries no meaningful body (`PUT` /
    /// `DELETE`), or map a non-2xx status through [`Self::into_status_error`].
    pub(crate) fn decode_empty(self) -> Result<()> {
        if self.is_success() {
            Ok(())
        } else {
            Err(self.into_status_error())
        }
    }

    /// Return a 2xx body verbatim (the `/version` endpoint is plain text,
    /// not JSON), or map a non-2xx status through [`Self::into_status_error`].
    pub(crate) fn decode_text(self) -> Result<String> {
        if self.is_success() {
            Ok(self.body)
        } else {
            Err(self.into_status_error())
        }
    }

    /// Map a non-success response to an [`Error`]. Decodes the body as an
    /// [`ApiError`] when it parses, falling back to
    /// [`Error::UnexpectedStatus`] with the raw body otherwise.
    fn into_status_error(self) -> Error {
        serde_json::from_str::<ApiError>(&self.body).map_or_else(
            |_| Error::UnexpectedStatus {
                status: self.status,
                body: self.body,
            },
            |api| Error::OrderbookApi {
                status: self.status,
                api,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_as_str_returns_http_verbs() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Post.as_str(), "POST");
        assert_eq!(HttpMethod::Put.as_str(), "PUT");
        assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
    }

    #[test]
    fn request_path_and_query_preserves_endpoint_and_query_string() {
        let request = HttpRequest {
            method: HttpMethod::Get,
            url: url::Url::parse(
                "https://api.cow.fi/mainnet/api/v1/account/0xabc/orders?offset=10&limit=5",
            )
            .unwrap(),
            json_body: None,
            bearer: None,
        };

        assert_eq!(
            request.path_and_query(),
            "/mainnet/api/v1/account/0xabc/orders?offset=10&limit=5"
        );
    }

    #[test]
    fn request_path_and_query_omits_empty_query() {
        let request = HttpRequest {
            method: HttpMethod::Post,
            url: url::Url::parse("https://api.cow.fi/xdai/api/v1/orders").unwrap(),
            json_body: Some(br#"{"foo":"bar"}"#.to_vec()),
            bearer: None,
        };

        assert_eq!(request.path_and_query(), "/xdai/api/v1/orders");
    }
}
