//! Pure request and response DTO types for the orderbook HTTP API.
//!
//! These serde structs and enums mirror the production orderbook
//! OpenAPI shapes and carry no client logic, so they are available
//! regardless of the `http-client` feature.

use alloy_primitives::{Address, B256, U256, keccak256};
use serde::{Deserialize, Serialize};
use serde_with::{DisplayFromStr, serde_as};
use std::collections::BTreeMap;

use crate::app_data::{AppDataDoc, AppDataHash};
use crate::order::OrderUid;

/// `appData` field on a quote request: 32-byte digest or canonical
/// JSON document. Mirrors `OrderCreationAppData` in
/// `cowprotocol/services::model::quote`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum QuoteAppData {
    /// Pre-computed digest; serialises as `0x`-prefixed hex.
    Hash(AppDataHash),
    /// Canonical JSON; orderbook computes and pins the digest.
    Full(String),
}

impl From<AppDataHash> for QuoteAppData {
    fn from(digest: AppDataHash) -> Self {
        Self::Hash(digest)
    }
}

impl From<&AppDataDoc> for QuoteAppData {
    /// Pin the document's canonical JSON, so the orderbook computes
    /// the digest from the exact bytes the SDK would sign against.
    fn from(doc: &AppDataDoc) -> Self {
        Self::Full(doc.canonical_json())
    }
}

/// Quote price-quality hint. Trades off solver latency against depth.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PriceQuality {
    /// Fastest available answer; solvers may skip simulation.
    Fast,
    /// Best solver answer within the quoting window.
    Optimal,
    /// `Optimal` plus on-chain simulation against balances/allowances.
    /// The server's default when `priceQuality` is omitted (openapi
    /// `OrderQuoteRequest.priceQuality.default: verified`), so it is the
    /// [`Default`] here too.
    #[default]
    Verified,
}

/// `GET /api/v2/trades` row: one per `GPv2Settlement.Trade` log.
///
/// The openapi `Trade` schema also carries `executedProtocolFees`; it is
/// not modelled here (cow-sdk drops it too). Serde tolerates the extra
/// field, so callers needing the per-trade fee breakdown can decode the
/// raw JSON body themselves.
#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trade {
    /// Block the settlement transaction was mined in.
    pub block_number: u64,
    /// Log index within the settlement transaction.
    pub log_index: u32,
    /// UID of the filled order.
    pub order_uid: OrderUid,
    /// Owner that signed the order.
    pub owner: Address,
    /// Sold token.
    pub sell_token: Address,
    /// Bought token.
    pub buy_token: Address,
    /// Sell amount net of fee.
    #[serde_as(as = "DisplayFromStr")]
    pub sell_amount: U256,
    /// Sell amount before fee deduction.
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    pub sell_amount_before_fees: Option<U256>,
    /// Bought amount.
    #[serde_as(as = "DisplayFromStr")]
    pub buy_amount: U256,
    /// Settlement transaction hash, when indexed. Hex-decoded from the
    /// wire `0x..` string; serialises back to the same form.
    #[serde(default)]
    pub tx_hash: Option<B256>,
}

/// Native-token-denominated price from
/// `GET /api/v1/token/{token}/native_price`. JSON number, not string.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct NativePrice {
    /// Native-token price of one atomic unit of the token.
    pub price: f64,
}

/// Cumulative user surplus from
/// `GET /api/v1/users/{user}/total_surplus`. Decimal string for
/// precision.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TotalSurplus {
    /// Cumulative surplus, decimal string in atomic native units.
    pub total_surplus: String,
}

/// `GET /api/v1/auction` snapshot. Permissioned (solver-only); the
/// per-order array is left opaque because `AuctionOrder` drifts
/// across CIPs.
#[serde_as]
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Auction {
    /// Monotonically increasing auction id.
    #[serde(default)]
    pub id: Option<u64>,
    /// Anchor block; orders, prices and settlements apply here.
    #[serde(default)]
    pub block: Option<u64>,
    /// Per-order array; left as JSON because the row shape drifts per CIP.
    #[serde(default)]
    pub orders: Option<serde_json::Value>,
    /// External prices, atomic native units per token.
    #[serde_as(as = "Option<BTreeMap<_, DisplayFromStr>>")]
    #[serde(default)]
    pub prices: Option<BTreeMap<Address, U256>>,
    /// JIT owners whose surplus counts toward solver objective.
    #[serde(default)]
    pub surplus_capturing_jit_order_owners: Option<Vec<Address>>,
}

/// `GET /api/v1/token/{token}/metadata`. Both fields are absent for
/// tokens the orderbook has not indexed.
#[serde_as]
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenMetadata {
    /// Block of the first trade the orderbook has indexed for the token.
    #[serde(default)]
    pub first_trade_block: Option<u32>,
    /// Last-known native-token price, atomic units per token.
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    pub native_price: Option<U256>,
}

/// `GET /api/v1/app_data/{hash}` body and `put_app_data` input. The
/// orderbook indexes the document under
/// `keccak256(full_app_data.as_bytes())` byte-for-byte.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDataDocument {
    /// Raw JSON document; orderbook hashes the bytes verbatim.
    pub full_app_data: String,
}

impl AppDataDocument {
    /// `keccak256(full_app_data.as_bytes())`. Canonicalise via
    /// [`crate::app_data::AppDataDoc::canonical_json`] first if
    /// deterministic key order matters.
    pub fn computed_hash(&self) -> AppDataHash {
        keccak256(self.full_app_data.as_bytes())
    }
}

/// Auction lifecycle stage returned by `GET /api/v1/orders/{uid}/status`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuctionStatusType {
    /// Quoted but not yet in an auction.
    Open,
    /// Scheduled for inclusion in an upcoming auction.
    Scheduled,
    /// In the currently active auction.
    Active,
    /// Solved by one or more solvers; awaiting settlement.
    Solved,
    /// Solver transaction is being submitted on chain.
    Executing,
    /// Settlement transaction was mined.
    Traded,
    /// Cancelled before settlement.
    Cancelled,
}

/// `GET /api/v1/orders/{uid}/status` payload. `value` carries solver
/// proposals when relevant (`solved`/`executing`); opaque to stay
/// forward-compatible across CIPs.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuctionStatus {
    /// Stage discriminant.
    #[serde(rename = "type")]
    pub status_type: AuctionStatusType,
    /// Stage-specific payload (e.g., solver proposals), left as JSON.
    #[serde(default)]
    pub value: Vec<serde_json::Value>,
}
