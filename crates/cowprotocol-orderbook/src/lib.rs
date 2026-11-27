//! CoW Protocol orderbook DTOs, quote builders, and HTTP client.
//!
//! # HTTP transport
//!
//! The `http-client` feature (enabled by default) gives every
//! constructor a working backend without naming a transport. The
//! ergonomic constructors build over [`DefaultTransport`], a type alias
//! resolved per target: `ReqwestTransport` on native builds and
//! `FetchTransport` (the browser `fetch` global) on `wasm32`. Only one
//! concrete type exists per target, so `ReqwestTransport` is absent on
//! `wasm32` and `FetchTransport` is absent on native; name
//! [`DefaultTransport`] in cross-target code rather than a concrete
//! backend.
//!
//! On native targets `http-client` links `reqwest`; on `wasm32` it
//! links `js-sys`, `wasm-bindgen`, and `wasm-bindgen-futures` and never
//! links `reqwest`. To supply your own backend, implement
//! [`HttpTransport`] and pass it to
//! [`OrderBookApi::new_with_transport`]: the trait itself stays
//! feature-independent, so an out-of-tree backend needs neither
//! `http-client` nor either concrete transport.

#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub use cowprotocol_appdata::{
    AppDataDoc, AppDataHash, EMPTY_APP_DATA_HASH, EMPTY_APP_DATA_JSON, app_data,
};
pub use cowprotocol_primitives::{Chain, chain, contracts, domain};
pub use cowprotocol_signing::{
    BUY_ETH_ADDRESS, BuyTokenDestination, EcdsaSignature, EcdsaSigningScheme, OrderCancellation,
    OrderCancellations, OrderClass, OrderData, OrderKind, OrderUid, OrderUidParseError,
    OrderUidParts, Recovered, SellTokenSource, Signature, SignatureError, SignedOrderCancellation,
    SignedOrderCancellations, SignerSync, SigningScheme, cancellation, cancellation_typed_data,
    ecdsa_from_components, ecdsa_recover, order, order_typed_data, parse_ecdsa, parse_order_uid,
    sign_ecdsa, signature, signing_scheme,
};

pub mod error;
pub mod eth_flow;
pub mod order_book;
pub mod quote_amounts;
#[cfg(feature = "subgraph")]
pub mod subgraph;
pub mod transport;

pub use self::{
    error::{ApiError, Error, OrderbookApiErrorType, Result, RetryHint, VerifyOwnerError},
    eth_flow::{ETH_FLOW_PRODUCTION, ETH_FLOW_STAGING, EthFlowOrder},
    order_book::{
        AppDataDocument, Auction, AuctionStatus, AuctionStatusType, NativePrice, Order,
        OrderBookApi, OrderCreation, OrderCreationBuilder, OrderQuote, OrderQuoteResponse,
        OrderStatus, OrderSubmission, PriceQuality, QuoteAppData, QuoteRequest,
        QuoteRequestBuilder, QuotedOrder, ReadyQuoteRequestBuilder, TokenMetadata, TotalSurplus,
        Trade,
    },
    quote_amounts::{
        Amounts as QuoteAmounts, DEFAULT_SLIPPAGE_BPS, OrderCosts, ProtocolFeeBps,
        QuoteAmountsAndCosts, QuoteAmountsParams, QuoteCosts,
    },
    transport::{HttpMethod, HttpRequest, HttpResponse, HttpTransport},
};

#[cfg(all(feature = "http-client", target_arch = "wasm32"))]
pub use self::transport::FetchTransport;
#[cfg(all(feature = "http-client", not(target_arch = "wasm32")))]
pub use self::transport::ReqwestTransport;
#[cfg(feature = "http-client")]
pub use self::{order_book::OrderBookApiBuilder, transport::DefaultTransport};

#[cfg(feature = "subgraph")]
pub use self::subgraph::{
    ChainSubgraphUnavailable, DailyTotal, GraphQlError, HourlyTotal, SubgraphClient, SubgraphError,
    Totals,
};
