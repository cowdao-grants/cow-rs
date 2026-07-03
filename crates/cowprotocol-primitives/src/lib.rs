//! Primitive CoW Protocol chain, domain, order, and contract types.

#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use alloy_primitives::{B256, b256};

pub mod chain;
pub mod composable;
pub mod contracts;
pub mod domain;
pub mod eth_flow;
pub mod multiplexer;
pub mod order_id;

/// 32-byte keccak256 digest of an app-data document, embedded directly
/// in a signed order's `appData` field.
pub type AppDataHash = B256;

/// `keccak256("{}")`: digest of the canonical empty app-data document.
pub const EMPTY_APP_DATA_HASH: AppDataHash =
    b256!("b48d38f93eaa084033fc5970bf96e559c33c4cdc07d889ab00b4d63f9590739d");

/// JSON representation of the empty app-data document (`"{}"`).
pub const EMPTY_APP_DATA_JSON: &str = "{}";

pub use self::{
    chain::{Chain, UnsupportedChain},
    composable::{
        COMPOSABLE_COW, CURRENT_BLOCK_TIMESTAMP_FACTORY, ComposableCoW, ConditionalOrderParams,
        ConditionalOrderRevert, EXTENSIBLE_FALLBACK_HANDLER,
        GET_TRADEABLE_ORDER_WITH_SIGNATURE_SELECTOR, GetTradeableOrderWithSignatureCall,
        IConditionalOrder, PayloadStruct, Proof, ProofLocation, TWAP_HANDLER,
        TradeableOrderWithSignature, TradeableOrderWithSignatureRevert, TwapData, TwapDataBuilder,
        TwapDuration, TwapError, TwapStart, TwapStaticInput, decode_conditional_order_revert,
        forwarder_signature, safe_handler_signature,
    },
    contracts::{
        CoWSwapEthFlow, CoWSwapOnchainOrders, ERC20, EthFlowOrderData, GPV2_ORDER_TYPE_HASH,
        GPV2_SETTLEMENT, GPV2_VAULT_RELAYER, GPv2OrderData, GPv2Settlement, OnchainSignature,
        OnchainSigningScheme, WETH9,
    },
    domain::{
        DOMAIN_NAME, DOMAIN_VERSION, DomainSeparator, eip712_message_hash, settlement_domain,
    },
    eth_flow::{ETH_FLOW_PRODUCTION, ETH_FLOW_STAGING, EthFlowDeployment, EthFlowEnvironment},
    multiplexer::{
        MerkleProof, Multiplexer, MultiplexerError, conditional_order_leaf, verify_proof,
    },
    order_id::{OrderClass, OrderUid, OrderUidParseError, OrderUidParts, parse_order_uid},
};
