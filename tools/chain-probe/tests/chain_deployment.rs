//! On-chain deployment-parity probe for the canonical CoW Protocol
//! singletons. The SDK hard-codes one address per contract and
//! assumes that address carries the canonical bytecode on every chain
//! the SDK claims to support. If even one chain has a different
//! settlement deployment, the EIP-712 domain separator is wrong on
//! that chain and orders cannot settle on-chain.
//!
//! Each test in this file probes a configured chain's RPC for
//! `eth_getCode` at the relevant addresses and asserts the expected
//! presence (or absence, for the ComposableCoW periphery on chains
//! that should not host it).
//!
//! Marked `#[ignore]` so it does not run under default `cargo test`:
//! a dedicated workflow (see `.github/workflows/chain-deployment.yml`)
//! invokes it explicitly with
//! `cargo test -p chain-probe --test chain_deployment -- --ignored`.
//! Lives in its own workspace crate so the published `cowprotocol`
//! tarball does not carry probe code that no SDK consumer ever runs.
//!
//! RPC endpoints are read from per-chain env vars; chains whose env
//! var is unset are skipped with a logged note rather than failing
//! the test, so a partial run (e.g. only mainnet + sepolia in CI) is
//! still useful. If no chain is configured at all, the probe logs a
//! note and passes: an unconfigured repo is a no-op, not a failure.

#![cfg(not(target_arch = "wasm32"))]

use alloy_primitives::Address;
use cowprotocol::{
    COMPOSABLE_COW, CURRENT_BLOCK_TIMESTAMP_FACTORY, Chain, ETH_FLOW_PRODUCTION, ETH_FLOW_STAGING,
    EXTENSIBLE_FALLBACK_HANDLER, TWAP_HANDLER,
};
use serde_json::json;

/// Per-chain RPC endpoint env var. Set any subset; unset chains are
/// skipped. Use **authenticated** provider URLs (Alchemy, dRPC,
/// QuickNode, or the chain team's own keyed endpoint) so the weekly
/// workflow run does not bounce off shared-IP rate limits on public
/// RPCs.
const CHAIN_ENV: &[(Chain, &str)] = &[
    (Chain::Mainnet, "COW_RPC_MAINNET"),
    (Chain::Bnb, "COW_RPC_BNB"),
    (Chain::Gnosis, "COW_RPC_GNOSIS"),
    (Chain::Polygon, "COW_RPC_POLYGON"),
    (Chain::Base, "COW_RPC_BASE"),
    (Chain::Plasma, "COW_RPC_PLASMA"),
    (Chain::ArbitrumOne, "COW_RPC_ARBITRUM"),
    (Chain::Avalanche, "COW_RPC_AVALANCHE"),
    (Chain::Linea, "COW_RPC_LINEA"),
    (Chain::Sepolia, "COW_RPC_SEPOLIA"),
];

/// Stringify a `reqwest::Error` without echoing the request URL. The
/// authenticated RPC endpoints under test embed provider API keys in
/// their path or query; `reqwest::Error`'s default `Display` includes
/// `url()` verbatim, which GitHub's secret masker only redacts on a
/// best-effort basis for exact-substring matches.
fn redact_reqwest(e: reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connect"
    } else if e.is_decode() {
        "decode"
    } else if e.is_status() {
        "status"
    } else if e.is_body() {
        "body"
    } else if e.is_request() {
        "request"
    } else {
        "other"
    };
    e.status()
        .map_or_else(|| kind.to_string(), |s| format!("{kind} (status {s})"))
}

async fn eth_get_code(rpc: &str, address: Address) -> Result<Vec<u8>, String> {
    let body = json!({
        "jsonrpc": "2.0",
        "method": "eth_getCode",
        "params": [format!("{address:#x}"), "latest"],
        "id": 1,
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("client build: {e}"))?;
    let resp = client
        .post(rpc)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("rpc send: {}", redact_reqwest(e)))?
        .error_for_status()
        .map_err(|e| format!("rpc http: {}", redact_reqwest(e)))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("rpc body: {}", redact_reqwest(e)))?;
    if let Some(err) = json.get("error") {
        return Err(format!("rpc error: {err}"));
    }
    let hex = json["result"]
        .as_str()
        .ok_or_else(|| format!("no result in: {json}"))?;
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    const_hex::decode(hex).map_err(|e| format!("invalid hex {hex:?}: {e}"))
}

/// Drives the settlement / vault-relayer verification across every
/// chain whose RPC is configured. Asserts:
///
/// - at least one chain was probed (catches misconfigured env vars);
/// - for every probed chain, both the settlement and vault-relayer
///   addresses return non-empty bytecode at `latest`.
#[tokio::test]
#[ignore]
async fn settlement_and_vault_relayer_are_deployed_on_every_configured_chain() {
    let mut probed = 0usize;
    let mut skipped = Vec::new();
    let mut failures = Vec::new();

    for &(chain, env_var) in CHAIN_ENV {
        let Some(rpc) = std::env::var(env_var).ok().filter(|s| !s.is_empty()) else {
            skipped.push(format!("{chain}"));
            continue;
        };

        for (label, addr) in [
            ("settlement", chain.settlement()),
            ("vault_relayer", chain.vault_relayer()),
        ] {
            match eth_get_code(&rpc, addr).await {
                Ok(code) if code.is_empty() => {
                    failures.push(format!("{chain} {label} at {addr:#x}: no bytecode"))
                }
                Ok(_) => {}
                Err(e) => failures.push(format!("{chain} {label}: {e}")),
            }
        }
        probed += 1;
    }

    if probed == 0 {
        eprintln!("[chain_deployment] no COW_RPC_<CHAIN> env var set; skipping probe");
        return;
    }
    if !skipped.is_empty() {
        eprintln!(
            "[chain_deployment] skipped {} chain(s) without RPC: {}",
            skipped.len(),
            skipped.join(", ")
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ETH-flow singletons. The `ETH_FLOW_PRODUCTION` and
/// `ETH_FLOW_STAGING` addresses are claimed identical across every
/// chain CoW Protocol supports. If a chain doesn't actually carry the
/// bytecode, native-sell orders cannot settle there. Production is
/// asserted on every configured chain; staging is best-effort (the
/// barn deployment doesn't exist on every chain) and only requires
/// production to be present.
#[tokio::test]
#[ignore]
async fn eth_flow_singletons_are_deployed_on_every_configured_chain() {
    let mut probed = 0usize;
    let mut failures = Vec::new();
    for &(chain, env_var) in CHAIN_ENV {
        let Some(rpc) = std::env::var(env_var).ok().filter(|s| !s.is_empty()) else {
            continue;
        };
        match eth_get_code(&rpc, ETH_FLOW_PRODUCTION).await {
            Ok(code) if code.is_empty() => failures.push(format!(
                "{chain} eth_flow_production at {ETH_FLOW_PRODUCTION:#x}: no bytecode"
            )),
            Ok(_) => {}
            Err(e) => failures.push(format!("{chain} eth_flow_production: {e}")),
        }
        // Staging is informational; some chains explicitly do not host it.
        if let Ok(code) = eth_get_code(&rpc, ETH_FLOW_STAGING).await
            && code.is_empty()
        {
            eprintln!(
                "[chain_deployment] {chain} has no eth_flow_staging bytecode (informational)"
            );
        }
        probed += 1;
    }
    if probed == 0 {
        eprintln!("[chain_deployment] no COW_RPC_<CHAIN> env var set; skipping probe");
        return;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ComposableCoW periphery. `COMPOSABLE_COW`,
/// `EXTENSIBLE_FALLBACK_HANDLER`, `CURRENT_BLOCK_TIMESTAMP_FACTORY`
/// and `TWAP_HANDLER` are deployed only on the four chains where
/// `Chain::supports_composable_cow()` returns true. The test asserts
/// bytecode presence on every supported chain that is configured, and
/// asserts absence on the unsupported chains (so an accidental
/// deployment elsewhere does not silently start being picked up).
#[tokio::test]
#[ignore]
async fn composable_cow_periphery_matches_supports_predicate() {
    let mut probed = 0usize;
    let mut failures = Vec::new();
    for &(chain, env_var) in CHAIN_ENV {
        let Some(rpc) = std::env::var(env_var).ok().filter(|s| !s.is_empty()) else {
            continue;
        };
        let supported = chain.supports_composable_cow();
        for (label, addr) in [
            ("composable_cow", COMPOSABLE_COW),
            ("extensible_fallback_handler", EXTENSIBLE_FALLBACK_HANDLER),
            (
                "current_block_timestamp_factory",
                CURRENT_BLOCK_TIMESTAMP_FACTORY,
            ),
            ("twap_handler", TWAP_HANDLER),
        ] {
            match (eth_get_code(&rpc, addr).await, supported) {
                (Ok(code), true) if code.is_empty() => failures.push(format!(
                    "{chain} {label} at {addr:#x}: no bytecode (chain is in supports_composable_cow set)"
                )),
                (Ok(code), false) if !code.is_empty() => failures.push(format!(
                    "{chain} {label} at {addr:#x}: unexpected bytecode on a chain outside the supports_composable_cow set"
                )),
                (Ok(_), _) => {}
                (Err(e), _) => failures.push(format!("{chain} {label}: {e}")),
            }
        }
        probed += 1;
    }
    if probed == 0 {
        eprintln!("[chain_deployment] no COW_RPC_<CHAIN> env var set; skipping probe");
        return;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
