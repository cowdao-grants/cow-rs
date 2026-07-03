//! CoW Protocol signing schemes, signatures, and cancellation helpers.
//!
//! # Choosing a signing path
//!
//! Every high-level entry point comes in two flavours. The `sign` /
//! `sign_ecdsa` methods are bound on [`alloy_signer::SignerSync`], which
//! in practice only a raw in-memory key
//! (`alloy_signer_local::PrivateKeySigner`) implements. Production
//! backends (hardware wallet, remote signer, KMS) implement the async
//! [`alloy_signer::Signer`] trait instead, so reach for the `sign_async`
//! / [`sign_ecdsa_async`] twins.
//!
//! When the signer sits behind an interface that fits neither trait (an
//! injected browser wallet, an HTTP signing service), use the
//! digest-and-lift recipe:
//!
//! 1. Compute the 32-byte digest with [`OrderData::signing_hash`] (or
//!    [`signing_message`] for any [`alloy_sol_types::SolStruct`]).
//! 2. Sign those bytes externally, however your backend produces a
//!    `65`-byte `r || s || v` signature.
//! 3. Lift the result back into a [`Signature`] with
//!    [`Signature::from_ecdsa`], or assemble it from scalar components
//!    with [`ecdsa_from_components`].

#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod cancellation;
pub mod order;
pub mod signature;
pub mod signing_scheme;

pub use cowprotocol_primitives::{contracts, domain};

// Re-exported so downstream crates (the orderbook pipeline, integrator
// code) can name the signer bound without a direct alloy-signer
// dependency; this crate already owns the alloy-signer edge for
// `OrderData::sign`.
pub use alloy_signer::SignerSync;

pub mod app_data {
    //! Shared app-data digest primitives used by signed orders.

    pub use cowprotocol_primitives::{AppDataHash, EMPTY_APP_DATA_HASH, EMPTY_APP_DATA_JSON};
}

pub use self::{
    cancellation::{
        OrderCancellation, OrderCancellations, SignedOrderCancellation, SignedOrderCancellations,
        cancellation_typed_data,
    },
    order::{
        BUY_ETH_ADDRESS, BuyTokenDestination, OrderClass, OrderData, OrderKind, OrderUid,
        OrderUidParseError, OrderUidParts, SellTokenSource, order_typed_data, parse_order_uid,
    },
    signature::{
        EcdsaSignature, Recovered, Signature, SignatureError, ecdsa_from_components, ecdsa_recover,
        parse_ecdsa, sign_ecdsa, sign_ecdsa_async, signing_message,
    },
    signing_scheme::{EcdsaSigningScheme, SigningScheme},
};
