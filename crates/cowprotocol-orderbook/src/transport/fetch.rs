//! JS `fetch` implementation of [`HttpTransport`] for wasm32 targets.
//!
//! Invokes the global `fetch` function via `js_sys::Reflect` so wasm
//! builds never link an HTTP stack of their own (reqwest stays out of the
//! wasm32 dependency graph entirely). Skipping `web-sys` keeps the
//! binding overhead to a few kilobytes.
//!
//! [`FetchTransport`] only performs the I/O: it returns the raw
//! `(status, body)` and lets the shared [`OrderBookApi`] endpoint logic
//! map the status to an error and decode the JSON, so the wasm and native
//! paths share one implementation of that logic.
//!
//! Every request is bounded in two dimensions:
//!
//! - a wall-clock timeout enforced via a globalThis `AbortController`
//!   (see [`FETCH_TIMEOUT_MS`]). A stuck or hostile orderbook cannot hold
//!   the caller's task open indefinitely;
//! - a response body size cap (see [`MAX_RESPONSE_BYTES`]). The body's
//!   UTF-16 length, a lower bound on its UTF-8 byte length, is checked
//!   before the JS string is copied into wasm linear memory, and the
//!   exact byte length is re-checked after the copy. An adversarial
//!   multi-byte body can still allocate up to roughly three times the
//!   cap during the conversion before the byte-exact backstop fires, but
//!   it can never hand an over-cap body to the decoder.
//!
//! [`OrderBookApi`]: crate::OrderBookApi
//! [`MAX_RESPONSE_BYTES`]: crate::order_book::MAX_RESPONSE_BYTES

use {
    crate::{
        error::{Error, Result},
        order_book::{DEFAULT_HTTP_TIMEOUT, MAX_RESPONSE_BYTES},
        transport::{HttpMethod, HttpRequest, HttpResponse, HttpTransport},
    },
    js_sys::{Function, Object, Promise, Reflect, global},
    wasm_bindgen::{JsCast, JsValue, closure::Closure},
    wasm_bindgen_futures::JsFuture,
};

/// Wall-clock cap applied to every `fetch` call this module issues.
/// Derived from [`DEFAULT_HTTP_TIMEOUT`], the same timeout the reqwest
/// client applies on native targets, so the two transports cannot drift.
const FETCH_TIMEOUT_MS: u32 = DEFAULT_HTTP_TIMEOUT.as_secs() as u32 * 1000;

/// The JS `fetch`-backed [`HttpTransport`]. Stateless: each call reads the
/// global `fetch` afresh, so a single instance is reused across requests.
///
/// # Target caveat
///
/// This type is gated on bare `target_arch = "wasm32"`, which also
/// matches `wasm32-wasip1` / `wasm32-wasip2`. Those targets have no JS
/// host, so the crate compiles there but every request fails at runtime
/// (deliberate, and no regression versus the former reqwest wasm
/// backend, which also assumed a JS host). Tightening the gate to
/// `target_os = "unknown"` repo-wide is a deliberate follow-up.
#[derive(Debug, Clone, Copy, Default)]
pub struct FetchTransport;

impl HttpTransport for FetchTransport {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse> {
        let method = match request.method {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
        };
        // `json_body` is serialised JSON, so it is valid UTF-8.
        let body = request
            .json_body
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        let (status, body) = fetch(
            method,
            request.url.as_str(),
            body.as_deref(),
            request.bearer.as_deref(),
        )
        .await
        .map_err(FetchError::into_cow)?;
        Ok(HttpResponse { status, body })
    }
}

/// Transport-internal failure, mapped to a [`crate::error::Error`] at the
/// [`FetchTransport::execute`] boundary. `TooLarge` becomes
/// [`Error::ResponseTooLarge`]; everything else carries the JS error text
/// into [`Error::TransportFailed`].
enum FetchError {
    Js(JsValue),
    TooLarge,
}

impl From<JsValue> for FetchError {
    fn from(value: JsValue) -> Self {
        Self::Js(value)
    }
}

impl FetchError {
    fn into_cow(self) -> Error {
        match self {
            Self::TooLarge => Error::ResponseTooLarge {
                max: MAX_RESPONSE_BYTES,
            },
            Self::Js(value) => {
                Error::TransportFailed(value.as_string().unwrap_or_else(|| format!("{value:?}")))
            }
        }
    }
}

/// Look up `name` on `target` via `Reflect::get` and downcast it to a
/// callable [`Function`], with the property name in the error message.
fn get_fn(target: &JsValue, name: &str) -> std::result::Result<Function, JsValue> {
    Reflect::get(target, &JsValue::from_str(name))?
        .dyn_into::<Function>()
        .map_err(|_| JsValue::from_str(&format!("{name} is not a function")))
}

/// Issue one `fetch` and return the raw `(status, body)`. Bounds the body
/// by [`MAX_RESPONSE_BYTES`] and aborts after [`FETCH_TIMEOUT_MS`]. Status
/// interpretation and JSON decoding are the caller's job.
async fn fetch(
    method: &str,
    url: &str,
    body: Option<&str>,
    bearer: Option<&str>,
) -> std::result::Result<(u16, String), FetchError> {
    let init = Object::new();
    Reflect::set(
        &init,
        &JsValue::from_str("method"),
        &JsValue::from_str(method),
    )?;
    if body.is_some() || bearer.is_some() {
        let headers = Object::new();
        if body.is_some() {
            Reflect::set(
                &headers,
                &JsValue::from_str("content-type"),
                &JsValue::from_str("application/json"),
            )?;
        }
        if let Some(token) = bearer {
            Reflect::set(
                &headers,
                &JsValue::from_str("authorization"),
                &JsValue::from_str(&format!("Bearer {token}")),
            )?;
        }
        Reflect::set(&init, &JsValue::from_str("headers"), &headers)?;
    }
    if let Some(body) = body {
        Reflect::set(&init, &JsValue::from_str("body"), &JsValue::from_str(body))?;
    }

    let global = global();

    // Wire up an AbortController so the fetch is cancellable. If the
    // setTimeout fires before the response arrives, the in-flight fetch
    // rejects with an `AbortError` we surface as a timeout.
    let abort_guard = AbortGuard::install(&global, &init, FETCH_TIMEOUT_MS)?;

    let fetch = get_fn(&global, "fetch")?;
    let promise: Promise = fetch
        .call2(&global, &JsValue::from_str(url), &init)?
        .dyn_into()
        .map_err(|_| JsValue::from_str("fetch did not return a Promise"))?;
    let response = match JsFuture::from(promise).await {
        Ok(r) => r,
        Err(err) => {
            return Err(FetchError::Js(if abort_guard.fired() {
                JsValue::from_str(&format!("request timed out after {FETCH_TIMEOUT_MS} ms"))
            } else {
                err
            }));
        }
    };

    let status: u16 = Reflect::get(&response, &JsValue::from_str("status"))?
        .as_f64()
        .ok_or_else(|| JsValue::from_str("response.status missing"))? as u16;

    // Reject oversized bodies before reading them at all. The header is
    // advisory (proxies strip it, some servers omit it); the pre-copy
    // and post-copy checks below catch the cases it does not cover.
    if let Some(headers) = Reflect::get(&response, &JsValue::from_str("headers"))
        .ok()
        .filter(|h| !h.is_undefined() && !h.is_null())
        && let Ok(get) = get_fn(&headers, "get")
        && let Ok(declared) = get.call1(&headers, &JsValue::from_str("content-length"))
        && let Some(declared) = declared.as_string()
        && let Ok(declared) = declared.parse::<u64>()
        && declared > MAX_RESPONSE_BYTES as u64
    {
        return Err(FetchError::TooLarge);
    }

    let text_fn = get_fn(&response, "text")?;
    let text_promise: Promise = text_fn
        .call0(&response)?
        .dyn_into()
        .map_err(|_| JsValue::from_str("response.text() did not return a Promise"))?;
    let body_value = JsFuture::from(text_promise).await?;

    drop(abort_guard);

    let js_text: js_sys::JsString = body_value
        .dyn_into()
        .map_err(|_| JsValue::from_str("response body not a string"))?;
    // The UTF-16 length lower-bounds the UTF-8 byte length, so an
    // oversized body is rejected here, before it is copied into wasm
    // linear memory.
    if js_text.length() as usize > MAX_RESPONSE_BYTES {
        return Err(FetchError::TooLarge);
    }
    let text = String::from(js_text);
    // Byte-exact backstop: a multi-byte body can pass the UTF-16
    // pre-check yet exceed the cap once encoded as UTF-8.
    if text.len() > MAX_RESPONSE_BYTES {
        return Err(FetchError::TooLarge);
    }

    Ok((status, text))
}

/// RAII wrapper around `AbortController` + `setTimeout`. Aborts the pending
/// fetch if `timeout_ms` elapses; cleared on drop so a fast response does
/// not leave a stray timer behind. `fired()` reports whether the timer
/// triggered the abort, so the caller can re-tag the resulting `AbortError`
/// as a transport timeout.
struct AbortGuard {
    global: JsValue,
    timer: JsValue,
    fired: std::rc::Rc<std::cell::Cell<bool>>,
    // Held to keep the JS-callable wrapper alive until drop, since the
    // setTimeout queue retains a reference to it that drops when the timer
    // fires or is cleared.
    _on_timeout: Closure<dyn FnMut()>,
}

impl AbortGuard {
    fn install(
        global: &JsValue,
        init: &Object,
        timeout_ms: u32,
    ) -> std::result::Result<Self, JsValue> {
        let ctor = get_fn(global, "AbortController")?;
        let controller = Reflect::construct(&ctor, &js_sys::Array::new())?;
        let signal = Reflect::get(&controller, &JsValue::from_str("signal"))?;
        Reflect::set(init, &JsValue::from_str("signal"), &signal)?;

        let abort_fn = get_fn(&controller, "abort")?;
        let fired = std::rc::Rc::new(std::cell::Cell::new(false));
        let fired_clone = fired.clone();
        let on_timeout = Closure::wrap(Box::new(move || {
            fired_clone.set(true);
            let _ = abort_fn.call0(&controller);
        }) as Box<dyn FnMut()>);

        let set_timeout = get_fn(global, "setTimeout")?;
        let timer = set_timeout.call2(
            global,
            on_timeout.as_ref().unchecked_ref(),
            &JsValue::from_f64(f64::from(timeout_ms)),
        )?;

        Ok(Self {
            global: global.clone(),
            timer,
            fired,
            _on_timeout: on_timeout,
        })
    }

    fn fired(&self) -> bool {
        self.fired.get()
    }
}

impl Drop for AbortGuard {
    fn drop(&mut self) {
        if let Ok(clear_timeout) = get_fn(&self.global, "clearTimeout") {
            let _ = clear_timeout.call1(&self.global, &self.timer);
        }
    }
}
