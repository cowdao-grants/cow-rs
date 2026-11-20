//! The `appData` field of a CoW Protocol order: a 32-byte digest of the
//! application metadata document, encoded as `0x`-prefixed hex when sent
//! over the wire.
//!
//! This module also exposes [`AppDataDoc`], a builder for the canonical
//! JSON document the digest points at. The doc serialises to a
//! deterministic, sorted-keys, whitespace-free JSON string so the
//! resulting [`AppDataHash`] is stable across runs and matches the digest
//! the orderbook pins to IPFS.
//!
//! [`app_data_cid`] derives the IPFS CID under which the orderbook pins
//! the document, returning a [`::cid::Cid`] whose `Display` already
//! emits the base32 lower-case (`b`-prefixed) multibase string the
//! orderbook indexes by.

use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_with::{DisplayFromStr, serde_as};

use cowprotocol_primitives::order_id::{OrderClass, OrderUid};
pub use cowprotocol_primitives::{AppDataHash, EMPTY_APP_DATA_HASH, EMPTY_APP_DATA_JSON};

/// SemVer of the schema this crate emits. Bump in lock-step with
/// upstream; see `cow-protocol/reference/core/intents/app-data.mdx`.
pub const LATEST_APP_DATA_VERSION: &str = "1.6.0";

/// `appCode` tag for the native Rust SDK. Apply via
/// [`AppDataDoc::sdk_attribution`].
pub const COW_RS_APP_CODE: &str = "cow-rs";

/// `appCode` tag for the wasm shim (`cow-sdk-wasm` on npm).
pub const COW_RS_WASM_APP_CODE: &str = "cow-rs-wasm";

/// Maximum `fullAppData` size the orderbook accepts on
/// `PUT /api/v1/app_data/{hash}`. Mirrors
/// `Validator::DEFAULT_SIZE_LIMIT` in `cowprotocol/services`.
pub const APP_DATA_SIZE_LIMIT: usize = 8192;

/// Canonical app-data JSON document. Models the common fields of
/// cow-sdk's `@cowprotocol/app-data` v1.x document and cow-py's
/// `AppDataDoc`; it is not a schema validator. This crate shape-validates
/// through serde and bounds the canonical-JSON size, but does not check
/// the document against the published `@cowprotocol/app-data` JSON
/// schema: the canonical-JSON encoding and the orderbook are the source
/// of truth. All modelled fields, including hooks, are typed. Every
/// field except `version` is optional and skipped when unset.
///
/// Construct via [`AppDataDoc::new`] or [`AppDataDoc::sdk_attribution`],
/// which both pin `version` to [`LATEST_APP_DATA_VERSION`]. There is
/// deliberately no `Default`: an empty `version` is not a valid schema
/// version and would hash to a digest no other tooling reproduces.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDataDoc {
    /// Schema SemVer, e.g. `"1.6.0"`.
    pub version: String,
    /// Integration identifier (freeform).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_code: Option<String>,
    /// `"prod"`, `"staging"`, etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Optional sub-document carrying SDK / partner attribution and hooks.
    #[serde(default)]
    pub metadata: AppDataMetadata,
}

/// `metadata` sub-document of [`AppDataDoc`]. Carries SDK / partner
/// attribution, typed pre/post [`AppDataHooks`], and the other optional
/// metadata fields.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDataMetadata {
    /// Slippage / quote-time attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<AppDataQuote>,
    /// Market / limit / liquidity classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_class: Option<AppDataOrderClass>,
    /// Optional partner-fee policy and recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partner_fee: Option<AppDataPartnerFee>,
    /// Optional referrer for analytics / rev-share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referrer: Option<AppDataReferrer>,
    /// Optional UTM campaign tracking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utm: Option<AppDataUtm>,
    /// Pre- and post-trade hooks ([`AppDataHooks`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<AppDataHooks>,
    /// Optional flashloan parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flashloan: Option<AppDataFlashloan>,
    /// UID of the order this one replaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_order: Option<AppDataReplacedOrder>,
    /// Skipped when empty so a wrapper-free document hashes the same
    /// digest it did before the field was added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wrappers: Vec<AppDataWrapperCall>,
}

/// `metadata.quote`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDataQuote {
    /// Slippage in basis points (`10_000 == 100 %`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slippage_bips: Option<u32>,
    /// Optional quote schema version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// `metadata.orderClass` envelope around [`OrderClass`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDataOrderClass {
    /// Inner [`OrderClass`] discriminant.
    pub order_class: OrderClass,
}

/// `metadata.partnerFee`. Policy fields are flattened alongside
/// `recipient` on the wire, matching
/// `cowprotocol/services::app_data::PartnerFee`.
///
/// Both fields are crate-private. `Deserialize` and
/// [`AppDataPartnerFee::new`] validate the bps cap eagerly; the
/// [`AppDataDoc`] builders defer it to finalisation
/// ([`AppDataDoc::try_hash`] / [`AppDataDoc::hash`] /
/// [`AppDataDoc::prepare`]). Either way an over-cap fee cannot reach a
/// signed digest. Read the fields back with
/// [`AppDataPartnerFee::policy`] / [`AppDataPartnerFee::recipient`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppDataPartnerFee {
    /// Policy describing how the fee is computed.
    policy: FeePolicy,
    /// Address that receives the partner fee.
    recipient: Address,
}

impl Serialize for AppDataPartnerFee {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap as _;

        let entry_count = match self.policy {
            FeePolicy::Volume { .. } => 2,
            FeePolicy::Surplus { .. } | FeePolicy::PriceImprovement { .. } => 3,
        };
        let mut map = serializer.serialize_map(Some(entry_count))?;
        match self.policy {
            FeePolicy::Volume { bps } => {
                // Legacy `bps` key: preserves existing app-data hashes
                // and is still accepted by the upstream deserializer.
                map.serialize_entry("bps", &bps)?;
            }
            FeePolicy::Surplus {
                bps,
                max_volume_bps,
            } => {
                map.serialize_entry("surplusBps", &bps)?;
                map.serialize_entry("maxVolumeBps", &max_volume_bps)?;
            }
            FeePolicy::PriceImprovement {
                bps,
                max_volume_bps,
            } => {
                map.serialize_entry("priceImprovementBps", &bps)?;
                map.serialize_entry("maxVolumeBps", &max_volume_bps)?;
            }
        }
        map.serialize_entry("recipient", &self.recipient)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for AppDataPartnerFee {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Helper {
            recipient: Address,
            #[serde(default)]
            bps: Option<u16>,
            #[serde(default)]
            volume_bps: Option<u16>,
            #[serde(default)]
            surplus_bps: Option<u16>,
            #[serde(default)]
            price_improvement_bps: Option<u16>,
            #[serde(default)]
            max_volume_bps: Option<u16>,
        }

        let h = Helper::deserialize(deserializer)?;
        // Exactly one policy shape may be populated; mixed or absent
        // fee keys fail closed. Tuple order:
        // (bps, volume_bps, surplus_bps, price_improvement_bps, max_volume_bps).
        let policy = match (
            h.bps,
            h.volume_bps,
            h.surplus_bps,
            h.price_improvement_bps,
            h.max_volume_bps,
        ) {
            (Some(bps), None, None, None, None) | (None, Some(bps), None, None, None) => {
                FeePolicy::Volume { bps }
            }
            (None, None, Some(bps), None, Some(max_volume_bps)) => FeePolicy::Surplus {
                bps,
                max_volume_bps,
            },
            (None, None, None, Some(bps), Some(max_volume_bps)) => FeePolicy::PriceImprovement {
                bps,
                max_volume_bps,
            },
            _ => {
                return Err(D::Error::custom("unknown partner-fee policy shape"));
            }
        };
        Self::new(policy, h.recipient).map_err(D::Error::custom)
    }
}

/// Reject [`FeePolicy`] values whose bps fields exceed
/// [`PARTNER_FEE_BPS_MAX`]. A hostile document otherwise pins a
/// `bps = u64::MAX` that the contract silently clamps. Private: called
/// from [`AppDataPartnerFee::new`], the deserialiser, and the
/// [`AppDataDoc`] finalisation path.
fn validate_fee_policy(policy: &FeePolicy) -> Result<(), AppDataError> {
    let check = |field: &'static str, value: u16| -> Result<(), AppDataError> {
        if value > PARTNER_FEE_BPS_MAX {
            Err(AppDataError::FeeOutOfRange {
                field,
                value,
                max: PARTNER_FEE_BPS_MAX,
            })
        } else {
            Ok(())
        }
    };
    match *policy {
        FeePolicy::Volume { bps } => check("bps", bps),
        FeePolicy::Surplus {
            bps,
            max_volume_bps,
        } => {
            check("surplusBps", bps)?;
            check("maxVolumeBps", max_volume_bps)
        }
        FeePolicy::PriceImprovement {
            bps,
            max_volume_bps,
        } => {
            check("priceImprovementBps", bps)?;
            check("maxVolumeBps", max_volume_bps)
        }
    }
}

impl AppDataPartnerFee {
    /// Construct a partner-fee binding with bps validation. Prefer
    /// this over hand-building when the policy values come from
    /// untrusted input.
    pub fn new(policy: FeePolicy, recipient: Address) -> Result<Self, AppDataError> {
        validate_fee_policy(&policy)?;
        Ok(Self { policy, recipient })
    }

    /// The validated [`FeePolicy`] describing how the fee is computed.
    pub const fn policy(&self) -> FeePolicy {
        self.policy
    }

    /// The address that receives the partner fee.
    pub const fn recipient(&self) -> Address {
        self.recipient
    }
}

/// Fee-policy variant for [`AppDataPartnerFee`]. Flattened alongside
/// `recipient` on the wire; matches the upstream `FeePolicy`
/// deserializer. `Volume` serialises as the legacy `bps` key (not
/// `volumeBps`) so previously-hashed digests stay stable; the
/// deserialiser accepts either.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeePolicy {
    /// `bps` charged on swap volume.
    Volume {
        /// Fee in basis points (`10_000 == 100 %`).
        bps: u16,
    },
    /// `bps` of captured surplus, capped at `max_volume_bps` of swap
    /// volume.
    Surplus {
        /// Surplus-capture rate, in basis points.
        bps: u16,
        /// Hard cap on the resulting fee, expressed as `bps` of volume.
        max_volume_bps: u16,
    },
    /// `bps` of price improvement vs the reference quote, capped at
    /// `max_volume_bps` of swap volume.
    PriceImprovement {
        /// Improvement-capture rate, in basis points.
        bps: u16,
        /// Hard cap on the resulting fee, expressed as `bps` of volume.
        max_volume_bps: u16,
    },
}

/// `metadata.flashloan`. Describes a flashloan attached to the order
/// (lender, adapter, receiver, token, atomic amount). Mirrors
/// `ProtocolAppData::flashloan` in `cowprotocol/services`.
#[serde_as]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDataFlashloan {
    /// Address of the lending pool funding the loan.
    pub liquidity_provider: Address,
    /// Adapter contract called by the solver to draw and repay the loan.
    pub protocol_adapter: Address,
    /// Address that receives the loaned tokens during settlement.
    pub receiver: Address,
    /// Token being borrowed.
    pub token: Address,
    /// Atomic-unit amount to borrow.
    #[serde_as(as = "DisplayFromStr")]
    pub amount: U256,
}

/// `metadata.replacedOrder`. UID of the order this one replaces;
/// solvers cancel the prior order when settling the replacement.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AppDataReplacedOrder {
    /// UID of the order being replaced.
    pub uid: OrderUid,
}

/// `metadata.wrappers[]` entry: wrapper-contract calls the solver
/// invokes as part of the settlement transaction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDataWrapperCall {
    /// Wrapper-contract address invoked during settlement.
    pub address: Address,
    /// Wrapper calldata; serialises as `0x`-prefixed hex.
    pub data: Bytes,
    /// If `true`, solvers may settle without invoking the wrapper.
    #[serde(default)]
    pub is_omittable: bool,
}

/// `metadata.referrer`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDataReferrer {
    /// Referrer address (analytics / rev-share recipient).
    pub address: Address,
    /// Optional referrer schema version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// `metadata.utm`: campaign attribution.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDataUtm {
    /// UTM `source` parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utm_source: Option<String>,
    /// UTM `medium` parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utm_medium: Option<String>,
    /// UTM `campaign` parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utm_campaign: Option<String>,
    /// UTM `content` parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utm_content: Option<String>,
    /// UTM `term` parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utm_term: Option<String>,
}

/// `metadata.hooks`: the pre- and post-trade hook calls a solver runs
/// around settlement. Mirrors the `@cowprotocol/app-data` hooks schema
/// (`hooks/v0.2.0.json`): an optional `version` plus `pre` and `post`
/// arrays of [`Hook`] interactions. The arrays are always serialised
/// (empty included) so consumers can read `hooks.pre` / `hooks.post`
/// unconditionally.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDataHooks {
    /// Optional hooks-schema version (e.g. `"0.1.0"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Hook calls executed before the trade is settled.
    #[serde(default)]
    pub pre: Vec<Hook>,
    /// Hook calls executed after the trade is settled.
    #[serde(default)]
    pub post: Vec<Hook>,
}

/// A single hook interaction: one contract call a solver executes as
/// part of an order's pre- or post-trade [`AppDataHooks`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hook {
    /// Address of the contract to call.
    pub target: Address,
    /// Calldata for the call; serialises as `0x`-prefixed hex.
    pub call_data: Bytes,
    /// Gas limit for the call, as a decimal string (the schema's
    /// `gasLimit`).
    pub gas_limit: String,
}

impl AppDataDoc {
    /// Minimal constructor. Pins `version` to [`LATEST_APP_DATA_VERSION`]
    /// and leaves every other field unset.
    pub fn new(app_code: impl Into<String>) -> Self {
        Self {
            version: LATEST_APP_DATA_VERSION.to_string(),
            app_code: Some(app_code.into()),
            environment: None,
            metadata: AppDataMetadata::default(),
        }
    }

    /// SDK-attribution document. Pins `appCode` to the given tag
    /// ([`COW_RS_APP_CODE`] or [`COW_RS_WASM_APP_CODE`]) and
    /// `metadata.quote.version` to `CARGO_PKG_VERSION`. Integrators
    /// with their own `appCode` should build [`AppDataDoc`] directly.
    pub fn sdk_attribution(app_code: &str) -> Self {
        Self {
            version: LATEST_APP_DATA_VERSION.to_string(),
            app_code: Some(app_code.to_string()),
            environment: None,
            metadata: AppDataMetadata {
                quote: Some(AppDataQuote {
                    slippage_bips: None,
                    version: Some(env!("CARGO_PKG_VERSION").to_string()),
                }),
                ..AppDataMetadata::default()
            },
        }
    }

    /// Attach a referrer address.
    pub fn with_referrer(mut self, address: Address) -> Self {
        self.metadata.referrer = Some(AppDataReferrer {
            address,
            version: None,
        });
        self
    }

    /// Attach a *volume* partner fee (`bps` of the swap value to
    /// `recipient`). Infallible, so the builder chain stays fluent: the
    /// `bps <= PARTNER_FEE_BPS_MAX` (`10_000`) cap is enforced when the
    /// document is finalised by [`Self::try_hash`] / [`Self::hash`] /
    /// [`Self::prepare`], so an over-cap value still cannot reach a
    /// signed digest. Use [`AppDataPartnerFee::new`] for eager
    /// validation at value-construction time.
    #[must_use]
    pub const fn with_partner_fee(self, bps: u16, recipient: Address) -> Self {
        self.with_partner_fee_policy(FeePolicy::Volume { bps }, recipient)
    }

    /// Attach a partner fee with an explicit [`FeePolicy`]. Infallible;
    /// the over-cap `bps` / `maxVolumeBps` check runs at finalisation,
    /// see [`Self::with_partner_fee`].
    #[must_use]
    pub const fn with_partner_fee_policy(mut self, policy: FeePolicy, recipient: Address) -> Self {
        self.metadata.partner_fee = Some(AppDataPartnerFee { policy, recipient });
        self
    }

    /// Attach pre- and post-trade [`AppDataHooks`]. Restores the fluent
    /// chain for hooks, the only metadata field that previously had no
    /// builder method.
    #[must_use]
    pub fn with_hooks(mut self, hooks: AppDataHooks) -> Self {
        self.metadata.hooks = Some(hooks);
        self
    }

    /// Attach a typed [`AppDataFlashloan`].
    pub const fn with_flashloan(mut self, flashloan: AppDataFlashloan) -> Self {
        self.metadata.flashloan = Some(flashloan);
        self
    }

    /// Mark this order as replacing an earlier one.
    pub const fn with_replaced_order(mut self, uid: OrderUid) -> Self {
        self.metadata.replaced_order = Some(AppDataReplacedOrder { uid });
        self
    }

    /// Append a wrapper-contract call.
    pub fn with_wrapper(mut self, wrapper: AppDataWrapperCall) -> Self {
        self.metadata.wrappers.push(wrapper);
        self
    }

    /// Tag the order with an order class.
    pub const fn with_order_class(mut self, order_class: OrderClass) -> Self {
        self.metadata.order_class = Some(AppDataOrderClass { order_class });
        self
    }

    /// Attach a slippage hint to the quote sub-document.
    pub fn with_slippage_bips(mut self, slippage_bips: u32) -> Self {
        self.metadata
            .quote
            .get_or_insert_with(AppDataQuote::default)
            .slippage_bips = Some(slippage_bips);
        self
    }

    /// Mark the environment (`"prod"`, `"staging"`, …).
    pub fn with_environment(mut self, environment: impl Into<String>) -> Self {
        self.environment = Some(environment.into());
        self
    }

    /// Serialise to deterministic JSON: lex-sorted keys, no
    /// whitespace, **raw UTF-8 for non-ASCII** (matching the
    /// orderbook's `keccak256(toUtf8Bytes(fullAppData))` and cow-sdk's
    /// TS implementation; cow-py's `ensure_ascii=True` default
    /// diverges on non-ASCII, the orderbook is the source of truth).
    ///
    /// Round-trips via `serde_json::Value`, whose `BTreeMap`-backed
    /// `Map` (without `preserve_order`) emits keys in sorted order at
    /// every nesting level, independently of struct declaration order.
    /// The compile-time guard in this module forbids the
    /// `preserve_order` feature so this invariant cannot silently flip;
    /// `canonical_json_sorts_keys_deterministically` locks the result.
    pub fn canonical_json(&self) -> String {
        let value = serde_json::to_value(self).expect("AppDataDoc must serialise");
        serde_json::to_string(&value).expect("Value must re-serialise")
    }

    /// `keccak256(canonical_json())`. This is the digest written into the
    /// signed `Order.appData` field.
    ///
    /// Panics if the canonical JSON exceeds [`APP_DATA_SIZE_LIMIT`];
    /// use [`Self::try_hash`] with untrusted input.
    pub fn hash(&self) -> AppDataHash {
        self.try_hash()
            .expect("AppDataDoc must fit within APP_DATA_SIZE_LIMIT")
    }

    /// Fallible [`Self::hash`]; rejects an over-cap partner fee or a
    /// document above [`APP_DATA_SIZE_LIMIT`] before the orderbook
    /// would.
    pub fn try_hash(&self) -> Result<AppDataHash, AppDataError> {
        self.try_hash_of(&self.canonical_json())
    }

    /// Validate the finalised document and hash the supplied canonical
    /// JSON. Shared by [`Self::try_hash`] and [`Self::prepare`] so both
    /// run the same partner-fee and size checks over a single
    /// [`Self::canonical_json`] call.
    fn try_hash_of(&self, json: &str) -> Result<AppDataHash, AppDataError> {
        // Partner-fee bps validation is deferred from the builder to
        // here, so the only way an over-cap fee reaches a digest is
        // blocked at the point the digest is produced.
        if let Some(partner_fee) = &self.metadata.partner_fee {
            validate_fee_policy(&partner_fee.policy)?;
        }
        if json.len() > APP_DATA_SIZE_LIMIT {
            return Err(AppDataError::DocumentTooLarge {
                len: json.len(),
                max: APP_DATA_SIZE_LIMIT,
            });
        }
        Ok(keccak256(json.as_bytes()))
    }

    /// Derive the three correlated submission artifacts in one call,
    /// all from a single [`Self::canonical_json`] so they cannot drift:
    /// the `fullAppData` body to submit, its [`AppDataHash`] for the
    /// signed `Order.appData` field, and the IPFS [`AppDataCid`] the
    /// orderbook pins. Validates the partner fee and document size like
    /// [`Self::try_hash`].
    pub fn prepare(&self) -> Result<PreparedAppData, AppDataError> {
        let full_app_data = self.canonical_json();
        let hash = self.try_hash_of(&full_app_data)?;
        let cid = app_data_cid(hash);
        Ok(PreparedAppData {
            full_app_data,
            hash,
            cid,
        })
    }
}

/// The three correlated artifacts an order submission derives from one
/// [`AppDataDoc`], produced together by [`AppDataDoc::prepare`] so the
/// bytes submitted, the digest signed, and the CID pinned cannot
/// disagree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedAppData {
    /// Canonical JSON body. Submit verbatim as
    /// `AppDataDocument.full_app_data` (and the
    /// `PUT /api/v1/app_data/{hash}` body); the orderbook hashes these
    /// exact bytes.
    pub full_app_data: String,
    /// `keccak256(full_app_data)`: the digest embedded in the signed
    /// `Order.appData` field.
    pub hash: AppDataHash,
    /// IPFS CIDv1 the orderbook pins the document under.
    pub cid: AppDataCid,
}

impl std::str::FromStr for AppDataDoc {
    type Err = AppDataError;

    /// Parse a canonical JSON document, rejecting input larger than
    /// [`APP_DATA_SIZE_LIMIT`] before allocating any nested structure.
    fn from_str(json: &str) -> Result<Self, Self::Err> {
        if json.len() > APP_DATA_SIZE_LIMIT {
            return Err(AppDataError::DocumentTooLarge {
                len: json.len(),
                max: APP_DATA_SIZE_LIMIT,
            });
        }
        serde_json::from_str(json).map_err(|e| AppDataError::Parse(e.to_string()))
    }
}

/// Errors raised while validating an [`AppDataDoc`] before signing.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum AppDataError {
    /// Canonical JSON exceeded [`APP_DATA_SIZE_LIMIT`].
    #[error("app-data document too large: {len} bytes (max {max})")]
    DocumentTooLarge {
        /// Observed canonical-JSON length, in bytes.
        len: usize,
        /// Configured limit (`APP_DATA_SIZE_LIMIT`).
        max: usize,
    },
    /// A partner-fee `bps` exceeded [`PARTNER_FEE_BPS_MAX`].
    #[error("partner fee {field} = {value} exceeds maximum {max}")]
    FeeOutOfRange {
        /// `bps`, `surplusBps`, `priceImprovementBps`, or `maxVolumeBps`.
        field: &'static str,
        /// Offending value.
        value: u16,
        /// Cap that was exceeded.
        max: u16,
    },
    /// JSON parse failure; captured as text to keep the enum `PartialEq`.
    #[error("invalid app-data JSON: {0}")]
    Parse(String),
}

/// Maximum partner-fee value, in basis points (`10_000 = 100 %`).
/// Mirrors the cap the settlement contract enforces on
/// `metadata.partnerFee.{bps,maxVolumeBps}`.
pub const PARTNER_FEE_BPS_MAX: u16 = 10_000;

// Determinism guard for [`AppDataDoc::canonical_json`]: it relies on
// `serde_json::Map` being `BTreeMap`-backed, which holds only while
// serde_json's `preserve_order` feature stays off (that feature swaps
// the inner store for an insertion-ordered `IndexMap`, dropping the
// sorted-keys guarantee). The public `serde_json::Map` type is opaque
// and identical either way, so a downstream crate cannot assert the
// backing store at compile time nor `#[cfg]` on a dependency's feature.
// The invariant is therefore locked at run time by
// `canonical_json_sorts_keys_deterministically`, which fails the moment
// emitted keys stop being sorted. Do NOT enable `preserve_order` on
// serde_json anywhere in the workspace: it would silently re-hash every
// app-data digest.

mod cid;

pub use self::cid::{
    AppDataCid, AppDataCidError, MAX_CID_STR_LEN, app_data_cid, app_data_hash_from_cid,
    parse_app_data_cid,
};
#[cfg(test)]
pub(crate) use self::cid::{CID_CODEC_RAW, MULTIHASH_KECCAK_256};

#[cfg(test)]
#[path = "app_data/tests.rs"]
mod tests;
