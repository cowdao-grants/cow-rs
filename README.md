# cow-rs

First scaffold for a Rust SDK targeting CoW Protocol.

Current scope:

- Orderbook client for quote and order retrieval/submission
- EIP-712 order signing for EOA private keys
- Thin trading helper for `quote -> sign -> submit`
- Devcontainer-based development setup

The local `references/` directory is intentionally gitignored and can hold shallow clones of:

- `https://github.com/cowprotocol/cow-sdk`
- `https://github.com/cowdao-grants/cow-py`

## Quick start

```bash
cargo test
cargo run --example get_quote
COW_LIVE_TESTS=1 cargo test live_get_quote_on_sepolia -- --nocapture
```

## Current module layout

- `src/api/`
- `src/config.rs`
- `src/domain/`
- `src/error.rs`
- `src/signing/`
- `src/trading/`
