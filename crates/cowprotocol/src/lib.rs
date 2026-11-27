//! # cowprotocol
//!
//! Crate name on crates.io. Hosted at
//! [`cowdao-grants/cow-rs`](https://github.com/cowdao-grants/cow-rs).
//!
//! Rust SDK for the [CoW Protocol](https://cow.fi).
//!
//! ## One entry point, native and wasm
//!
//! This meta crate is the single entry point for every Rust consumer,
//! native and wasm alike: `cargo add cowprotocol` gives the same API on
//! both targets, with the `http-client` feature resolving to a
//! reqwest-backed transport natively and a browser-`fetch`-backed one on
//! `wasm32-unknown-unknown` (see [`transport::DefaultTransport`]; reqwest
//! never enters a wasm build). Rust-in-the-browser stacks (for example a
//! Flutter or Yew app driving Rust compiled to wasm) should depend on
//! this crate directly. The sibling `cow-sdk-wasm` crate is JS bindings
//! only: a thin `wasm-bindgen` shim over this crate, published to npm
//! for JavaScript consumers, not for Rust ones.
//!
//! ## What this crate exposes
//!
//! - [`OrderData`]: the 12-field signed payload, with
//!   [`OrderData::hash_struct`] for EIP-712 hashing,
//!   [`OrderData::uid`] for the 56-byte order identifier and
//!   [`OrderData::sign`] for ECDSA signing.
//! - [`DomainSeparator`] (a type alias for
//!   `alloy_sol_types::Eip712Domain`) and [`settlement_domain`] for the
//!   `GPv2Settlement` EIP-712 domain. The typed-data envelope is supplied
//!   by `alloy_sol_types::SolStruct` and the EIP-191 personal-sign wrap
//!   by `alloy_primitives::eip191_hash_message`.
//! - [`Signature`], [`EcdsaSignature`] and [`SignatureError`] covering
//!   EIP-712, EthSign, EIP-1271 and PreSign schemes.
//! - [`Chain`]: all ten chains the orderbook supports
//!   (Mainnet, Bnb, Gnosis, Polygon, Base, Plasma, Arbitrum One, Avalanche,
//!   Linea, Sepolia), with their `api.cow.fi` URL slugs.
//! - [`OrderBookApi`]: async client for the orderbook HTTP API, with
//!   methods for quoting, posting, lookup, cancellation, trade and account
//!   queries, native-price lookups and app-data pinning.
//! - [`OrderCreation`], [`SignedOrderCancellation`] and [`OrderCancellations`]
//!   for the submission and cancellation flows.
//! - [`AppDataHash`] and [`AppDataDoc`] for the canonical metadata
//!   document and its keccak digest.
//! - [`EthFlowOrder`], [`eth_flow::EthFlowDeployment`] and
//!   [`CoWSwapEthFlow`] for native-token sells via the per-chain EthFlow
//!   periphery contract.
//! - [`GPv2Settlement`] and [`GPv2OrderData`] for typed contract bindings,
//!   plus [`GPV2_SETTLEMENT`], [`GPV2_VAULT_RELAYER`], [`WETH9`] and
//!   [`ERC20`] address constants.
//! - [`ConditionalOrderParams`] and [`Proof`] for the
//!   `ComposableCoW` conditional-order primitives, plus the
//!   [`COMPOSABLE_COW`], [`EXTENSIBLE_FALLBACK_HANDLER`] and
//!   [`CURRENT_BLOCK_TIMESTAMP_FACTORY`] address constants. Dispatch
//!   helpers (`ComposableCoW::createCall`, `setRootCall`, `removeCall`,
//!   `hashCall`, etc.) are available via the [`ComposableCoW`]
//!   interface namespace; integrators feed them into
//!   `alloy_contract::CallBuilder` to assemble transactions.
//! - [`TwapData`] / [`TwapStaticInput`] for the canonical TWAP handler
//!   `staticInput`, locked byte-conformant against cow-py's
//!   `test_twap.py` vector.
//! - [`Multiplexer`] plus [`conditional_order_leaf`] / [`verify_proof`]
//!   for batched conditional orders behind a single
//!   `ComposableCoW.setRoot`, matching the contract's OpenZeppelin
//!   sorted-pair merkle proof verifier exactly.
//!
//! ## Quote example
//!
//! Requires the `http-client` feature (on by default).
//!
//! ```no_run
//! # #[cfg(feature = "http-client")]
//! # mod example {
//! use cowprotocol::{Chain, OrderBookApi};
//! use alloy_primitives::{U256, address};
//!
//! # pub async fn run() -> cowprotocol::Result<()> {
//! let quote = OrderBookApi::new(Chain::Mainnet)
//!     .quote_builder()
//!     .with_sell_token(address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48")) // USDC
//!     .with_buy_token(address!("6B175474E89094C44Da98b954EedeAC495271d0F")) // DAI
//!     .with_from(address!("70997970C51812dc3A010C7d01b50e0d17dc79C8"))
//!     .with_sell_amount(U256::from(100_000_000_u64))
//!     .build()
//!     .await?;
//! println!("buy amount: {}", quote.response().quote.buy_amount);
//! # Ok(()) }
//! # }
//! ```
//!
//! See `examples/post_order.rs` for the full sign-and-submit flow on
//! Sepolia.
//!
//! ## Parity references
//!
//! - [`cowprotocol/cow-sdk`](https://github.com/cowprotocol/cow-sdk) (TypeScript, canonical)
//! - [`cowdao-grants/cow-py`](https://github.com/cowdao-grants/cow-py) (Python)
//! - [`cowprotocol/services`](https://github.com/cowprotocol/services) (Rust, server-side)
//!
//! ## Licence
//!
//! GPL-3.0-or-later, matching the upstream `cowdao-grants/cow-rs` repository.
//! Portions of the source are adapted from [`cowprotocol/services`] under its
//! MIT OR Apache-2.0 licence.
//!
//! [`cowprotocol/services`]: https://github.com/cowprotocol/services

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub use cowprotocol_appdata::app_data;
#[cfg(feature = "subgraph")]
pub use cowprotocol_orderbook::subgraph;
pub use cowprotocol_orderbook::{error, eth_flow, order_book, quote_amounts, transport};
pub use cowprotocol_primitives::{chain, composable, contracts, domain, multiplexer};
pub use cowprotocol_signing::{cancellation, order, signature, signing_scheme};

pub use crate::{
    app_data::{
        AppDataCid, AppDataCidError, AppDataDoc, AppDataFlashloan, AppDataHash, AppDataHooks,
        AppDataMetadata, AppDataOrderClass, AppDataPartnerFee, AppDataQuote, AppDataReferrer,
        AppDataReplacedOrder, AppDataUtm, AppDataWrapperCall, COW_RS_APP_CODE,
        COW_RS_WASM_APP_CODE, EMPTY_APP_DATA_HASH, EMPTY_APP_DATA_JSON, FeePolicy, Hook,
        LATEST_APP_DATA_VERSION, MAX_CID_STR_LEN, PreparedAppData, app_data_cid,
        app_data_hash_from_cid, parse_app_data_cid,
    },
    cancellation::{
        OrderCancellation, OrderCancellations, SignedOrderCancellation, SignedOrderCancellations,
        cancellation_typed_data,
    },
    chain::{Chain, UnsupportedChain},
    composable::{
        COMPOSABLE_COW, CURRENT_BLOCK_TIMESTAMP_FACTORY, ComposableCoW, ConditionalOrderParams,
        ConditionalOrderRevert, EXTENSIBLE_FALLBACK_HANDLER, IConditionalOrder, PayloadStruct,
        Proof, ProofLocation, TWAP_HANDLER, TwapData, TwapDataBuilder, TwapDuration, TwapError,
        TwapStart, TwapStaticInput, decode_conditional_order_revert, forwarder_signature,
        safe_handler_signature,
    },
    contracts::{
        CoWSwapEthFlow, CoWSwapOnchainOrders, ERC20, EthFlowOrderData, GPV2_ORDER_TYPE_HASH,
        GPV2_SETTLEMENT, GPV2_VAULT_RELAYER, GPv2OrderData, GPv2Settlement, OnchainSignature,
        OnchainSigningScheme, WETH9,
    },
    domain::{
        DOMAIN_NAME, DOMAIN_VERSION, DomainSeparator, eip712_message_hash, settlement_domain,
    },
    error::{ApiError, Error, OrderbookApiErrorType, Result, RetryHint, VerifyOwnerError},
    eth_flow::{ETH_FLOW_PRODUCTION, ETH_FLOW_STAGING, EthFlowOrder},
    multiplexer::{
        MerkleProof, Multiplexer, MultiplexerError, conditional_order_leaf, verify_proof,
    },
    order::{
        BUY_ETH_ADDRESS, BuyTokenDestination, OrderClass, OrderData, OrderKind, OrderUid,
        OrderUidParseError, OrderUidParts, SellTokenSource, order_typed_data, parse_order_uid,
    },
    order_book::{
        AppDataDocument, Auction, AuctionStatus, AuctionStatusType,
        DEFAULT_ORDER_VALID_TO_MAX_HORIZON_SECS, NativePrice, Order, OrderBookApi, OrderCreation,
        OrderCreationBuilder, OrderQuote, OrderQuoteResponse, OrderStatus, OrderSubmission,
        PriceQuality, QuoteAppData, QuoteRequest, QuoteRequestBuilder, QuotedOrder,
        ReadyQuoteRequestBuilder, TokenMetadata, TotalSurplus, Trade,
    },
    quote_amounts::{
        Amounts as QuoteAmounts, DEFAULT_SLIPPAGE_BPS, OrderCosts, ProtocolFeeBps,
        QuoteAmountsAndCosts, QuoteAmountsParams, QuoteCosts,
    },
    signature::{
        EcdsaSignature, Recovered, Signature, SignatureError, ecdsa_from_components, ecdsa_recover,
        parse_ecdsa, sign_ecdsa, sign_ecdsa_async, signing_message,
    },
    signing_scheme::{EcdsaSigningScheme, SigningScheme},
    transport::{HttpMethod, HttpRequest, HttpResponse, HttpTransport},
};

#[cfg(all(feature = "http-client", target_arch = "wasm32"))]
pub use crate::transport::FetchTransport;
#[cfg(all(feature = "http-client", not(target_arch = "wasm32")))]
pub use crate::transport::ReqwestTransport;
#[cfg(feature = "http-client")]
pub use crate::{order_book::OrderBookApiBuilder, transport::DefaultTransport};

/// Commonly-used SDK imports for quote, sign, and submit flows.
///
/// This is intentionally small: it re-exports the ergonomic client/builders,
/// core order types, chain/domain helpers, app-data defaults, and signing
/// schemes without glob-exporting every low-level DTO.
pub mod prelude {
    pub use crate::{
        AppDataDoc, AppDataHash, Chain, EMPTY_APP_DATA_HASH, EMPTY_APP_DATA_JSON, Error,
        OrderBookApi, OrderCreation, OrderCreationBuilder, OrderData, OrderKind,
        OrderQuoteResponse, OrderSubmission, OrderUid, PriceQuality, QuoteAppData, QuoteRequest,
        QuoteRequestBuilder, QuotedOrder, ReadyQuoteRequestBuilder, Result, Signature,
        SigningScheme, settlement_domain,
    };

    #[cfg(feature = "http-client")]
    pub use crate::OrderBookApiBuilder;

    pub use crate::EcdsaSigningScheme;
}

#[cfg(feature = "subgraph")]
pub use crate::subgraph::{
    ChainSubgraphUnavailable, DailyTotal, GraphQlError, HourlyTotal, SubgraphClient, SubgraphError,
    Totals,
};
