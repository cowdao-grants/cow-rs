//! The reqwest [`HttpTransport`] backend and its capped body readers.
//!
//! Gated behind the `http-client` feature on non-wasm32 targets; wasm32
//! builds ship `FetchTransport` instead, so reqwest never enters their
//! dependency graph. The transport-generic
//! [`OrderBookApi`](crate::OrderBookApi) endpoint logic and the
//! [`HttpTransport`] trait live feature-independently in the parent
//! [`transport`](crate::transport) module.

use crate::error::{Error, Result};
use crate::order_book::{DEFAULT_HTTP_TIMEOUT, MAX_RESPONSE_BYTES};
use crate::transport::{HttpMethod, HttpRequest, HttpResponse, HttpTransport};

/// The reqwest-backed [`HttpTransport`]. Wraps a [`reqwest::Client`] and
/// applies the [`MAX_RESPONSE_BYTES`] body cap as it reads each response.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// Wrap a pre-configured [`reqwest::Client`]. Use for custom timeouts,
    /// proxies, TLS roots, or auth middleware.
    pub const fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// The default reqwest client, enforcing [`DEFAULT_HTTP_TIMEOUT`].
    fn default_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(DEFAULT_HTTP_TIMEOUT)
            .build()
            .expect("reqwest defaults cannot fail")
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new(Self::default_client())
    }
}

impl HttpTransport for ReqwestTransport {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse> {
        let mut builder = match request.method {
            HttpMethod::Get => self.client.get(request.url),
            HttpMethod::Post => self.client.post(request.url),
            HttpMethod::Put => self.client.put(request.url),
            HttpMethod::Delete => self.client.delete(request.url),
        };
        if let Some(body) = request.json_body {
            builder = builder
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body);
        }
        if let Some(token) = request.bearer {
            builder = builder.bearer_auth(token);
        }
        let response = builder.send().await?;
        let status = response.status().as_u16();
        let body = read_capped_text(response).await?;
        Ok(HttpResponse { status, body })
    }
}

/// Read a response body as UTF-8 text, rejecting payloads above
/// [`MAX_RESPONSE_BYTES`]. Early-rejects on a declared `Content-Length`,
/// then bounds the body as it streams in (see [`read_capped_body`]) so a
/// chunked or length-less response cannot buffer past the cap before the
/// check fires.
async fn read_capped_text(response: reqwest::Response) -> Result<String> {
    if let Some(declared_len) = response.content_length()
        && declared_len > MAX_RESPONSE_BYTES as u64
    {
        return Err(Error::ResponseTooLarge {
            max: MAX_RESPONSE_BYTES,
        });
    }
    read_capped_body(response).await
}

/// Accumulate the body chunk-by-chunk, failing the moment the running
/// length would exceed [`MAX_RESPONSE_BYTES`]. This is the stream-bounded
/// guard the `Content-Length` early-reject cannot provide for chunked
/// transfers.
pub(crate) async fn read_capped_body(mut response: reqwest::Response) -> Result<String> {
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(Error::ResponseTooLarge {
                max: MAX_RESPONSE_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    // The orderbook always returns UTF-8 JSON; a non-UTF-8 body is
    // pathological and would fail the downstream `serde_json` parse
    // anyway, so a lossy decode is acceptable and avoids a panic.
    Ok(String::from_utf8_lossy(&body).into_owned())
}
