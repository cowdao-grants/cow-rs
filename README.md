# cow-rs

[![crates.io](https://img.shields.io/crates/v/cowprotocol.svg)](https://crates.io/crates/cowprotocol)
[![docs.rs](https://docs.rs/cowprotocol/badge.svg)](https://docs.rs/cowprotocol)
[![CI](https://github.com/cowdao-grants/cow-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/cowdao-grants/cow-rs/actions/workflows/ci.yml)
[![Licence: GPL-3.0-or-later](https://img.shields.io/badge/licence-GPL--3.0--or--later-blue)](./LICENSE)

The Rust SDK for the [CoW Protocol](https://cow.fi).

`cow-rs` is the idiomatic Rust client for trading on CoW Protocol:
build and sign orders, talk to the orderbook, decode on-chain
settlement events. Built on [alloy](https://github.com/alloy-rs/alloy);
ports the canonical types from
[`cowprotocol/services`](https://github.com/cowprotocol/services); locks
every protocol-critical path byte-for-byte against
[`@cowprotocol/cow-sdk`](https://github.com/cowprotocol/cow-sdk),
[`cowdao-grants/cow-py`](https://github.com/cowdao-grants/cow-py) and
`ethers`.

## At a glance

- **Full order lifecycle**: quote, sign, submit, look up, cancel.
- **All four signing schemes**: EIP-712, EthSign, EIP-1271, pre-sign.
- **All ten chains**: Mainnet, BNB, Gnosis, Polygon, Base, Plasma,
  Arbitrum One, Avalanche, Linea, Sepolia, plus their barn
  staging endpoints where the orderbook team publishes them.
- **Conformance-locked**: 219 native tests (180 lib + 26 wiremock + 5
  schema-drift + 3 source-lock + 1 trading-mock + 4 doctests) plus 15
  headless-Firefox wasm-bindgen cases (16 with `in_shim_signing`), with
  byte-exact goldens
  cross-checked against `cowprotocol/services`, `cowprotocol/contracts`,
  ethers, cow-sdk and cow-py.
- **Hostile-orderbook hardened**: every quote response is bound to the
  originating `QuoteRequest` before the SDK produces signable bytes,
  app-data digests round-trip on get/put, fee-math fails closed via
  `checked_*` (no saturation), and `EthFlowOrder::receiver` is
  non-zero by construction.
- **Sync core, async client**: hashing, signing and contract decoding
  are pure-compute and need no runtime; the HTTP client is async-tokio
  and the only piece that depends on one.
- **WASM-ready**: compiles cleanly to `wasm32-unknown-unknown` and has
  an in-browser end-to-end harness (see `test-harness/`) that exercises
  the live orderbook from the browser; the poll helper is
  runtime-agnostic so you can drop in `gloo_timers::future::sleep`.

## Install

```toml
[dependencies]
cowprotocol = "0.1.0"
```

The crate is published as `cowprotocol` on crates.io (the `cow-rs` name was already taken on
crates.io by an unrelated publisher before this SDK existed); the source lives at
[`cowdao-grants/cow-rs`](https://github.com/cowdao-grants/cow-rs).

The `cowprotocol` meta crate is the single entry point for every Rust
consumer, native and wasm alike: the same `cargo add cowprotocol` works on
`wasm32-unknown-unknown`, where the `http-client` feature resolves to a
browser-`fetch`-backed transport instead of reqwest (which never enters a
wasm build). Rust-in-the-browser stacks (for example a Flutter or Yew app
driving Rust compiled to wasm) should depend on it directly. The
`cow-sdk-wasm` crate is JS bindings only: a thin `wasm-bindgen` shim over
`cowprotocol`, published to npm for JavaScript consumers, not for Rust
ones.

MSRV `1.91`, edition `2024`.

## Quick start: quote, sign, submit

```rust,no_run
use cowprotocol::{Chain, OrderBookApi};
use alloy_primitives::{U256, address};
use alloy_signer_local::PrivateKeySigner;

# async fn run(signer: PrivateKeySigner) -> cowprotocol::Result<()> {
let owner = alloy_signer::Signer::address(&signer);
let uid = OrderBookApi::new(Chain::Mainnet)
    .quote_builder()
    .with_sell_token(address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48")) // USDC
    .with_buy_token(address!("6B175474E89094C44Da98b954EedeAC495271d0F")) // DAI
    .with_from(owner)
    .with_sell_amount(U256::from(100_000_000_u64))
    .build()
    .await?
    .sign(&signer)?
    .submit()
    .await?;

println!("https://explorer.cow.fi/orders/{uid}");
# Ok(()) }
```

See [`examples/post_order.rs`](crates/cowprotocol/examples/post_order.rs)
for the same flow on Sepolia, runnable with a private key in the
environment.

## No-async core

Every protocol-critical primitive is synchronous and runtime-free:
`OrderData::hash_struct`, `OrderData::uid`, `EcdsaSignature::sign`,
`Signature::recover`, `DomainSeparator::new`, the `sol!`-generated
contract bindings. You can use cow-rs in a Postgres extension
([`pgrx`](https://github.com/pgcentralfoundation/pgrx)), an FFI shim,
an embedded context, or anywhere else a tokio reactor is hostile,
without pulling in `reqwest` or `tokio`. This is enforced via the
`http-client` cargo feature (on by default); build with
`--no-default-features` to drop the orderbook / trading clients and
their `reqwest` transitive graph from the build.

```rust
use cowprotocol::{OrderBuilder, OrderKind, DomainSeparator, Chain};
use alloy_primitives::{U256, address};

let order = OrderBuilder::new(
    address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"),
    address!("6B175474E89094C44Da98b954EedeAC495271d0F"),
)
.sell_amount(U256::from(100_000_000_u64))
.buy_amount(U256::from(99_000_000_000_000_000_000_u128))
.valid_to(u32::MAX)
.kind(OrderKind::Sell)
.build();

let domain = DomainSeparator::new(Chain::Mainnet.id(), Chain::Mainnet.settlement());
let owner = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
let uid = order.uid(&domain, owner);
assert_eq!(uid.0.len(), 56);
```

## Modules

| Module | What it exposes |
|---|---|
| `order` | `OrderData` (12-field signed payload), `OrderBuilder`, `OrderUid`, `OrderKind`, `SellTokenSource`, `BuyTokenDestination`, `BUY_ETH_ADDRESS`, plus the full GET-orders `Order`, `OrderStatus`, `OrderClass` |
| `order_book` | `OrderBookApi` with quote / submit / lookup / status / cancel, trades, native price, account orders, app-data PUT / GET (with digest round-trip), version, total surplus; the runtime-agnostic `poll_until` helper; and the fluent quote → bind → sign → put-app-data → submit pipeline (`quote_builder()`, `QuotedOrder`, `OrderSubmission`), the successor to `TradingSdk.postSwapOrder` in `@cowprotocol/cow-sdk` |
| `quote_amounts` | `compute()` (the partner-fee + protocol-fee + slippage composition the TS SDK uses, byte-for-byte against `cow-sdk` PR #867); fail-closed via `Error::QuoteFeeMathOverflow { stage }` on every intermediate |
| `signature` | `Signature` (all four schemes), `EcdsaSignature`, `Recovered`, `SignatureError` |
| `domain` | `DomainSeparator`, `hashed_eip712_message`, `hashed_ethsign_message` |
| `chain` | `Chain` (ten networks) with `orderbook_base_url`, `orderbook_barn_url`, `settlement`, `vault_relayer`, `subgraph_gateway_deployment_id` |
| `cancellation` | `OrderCancellation` (unsigned single), `OrderCancellations` (unsigned collection), `SignedOrderCancellation` / `SignedOrderCancellations` (signed bodies), `cancellation_typed_data` (EIP-712 envelope) |
| `app_data` | `AppDataHash`, `AppDataDoc` (canonical JSON + keccak digest), `AppDataCid` (IPFS CIDv1 derivation), `AppDataDoc::sdk_attribution` for the SDK's `appCode` tag |
| `eth_flow` | `EthFlowOrder` (non-zero `receiver` enforced at construction), `ETH_FLOW_PRODUCTION`, `ETH_FLOW_STAGING` |
| `composable` | `ConditionalOrderParams`, `Proof`, `ComposableCoW` events, `TwapData` + `TwapStaticInput`, plus deployment addresses |
| `multiplexer` | OZ-style commutative double-hashed merkle leaves, watch-tower-side proof verification |
| `contracts` | `GPv2Settlement` (settle + events), `CoWSwapOnchainOrders` (ETH-flow events), `ERC20`, `WETH9`, `GPV2_SETTLEMENT`, `GPV2_VAULT_RELAYER` |
| `subgraph` | `SubgraphClient` typed access to CoW's subgraph; totals, daily / hourly volume; opt-in bearer-token auth for the gateway URL |

Everything is re-exported at the crate root: `use cowprotocol::...`.

## WASM and JavaScript

cow-rs targets `wasm32-unknown-unknown`:

- Reqwest's browser fetch backend kicks in automatically on wasm.
- `OrderBookApi::poll_until` is runtime-agnostic; pair it with
  `gloo_timers::future::sleep` instead of `tokio::time::sleep`.
- `wait_for_order_fulfilled` (the tokio-bound convenience) is
  non-wasm only.
- CI gates `cargo check --target wasm32-unknown-unknown` on every
  push.
- `crates/cow-sdk-wasm/` ships a `#[wasm_bindgen]` shim published to
  npm as
  [`@cowdao-grants/cow-sdk-wasm`](crates/cow-sdk-wasm/README.md);
  `test-harness/index.html` exercises it end-to-end against the live
  orderbook from a real browser. Run with `just wasm-harness`.

### JavaScript quick-start

Two signing flows. Pick one.

**In-shim signing** (tests, scripts, fast iteration): the wasm crate
holds the private key and signs inside linear memory. Requires the
`in_shim_signing` cargo feature at build time (off by default).

```js
import init, {
  get_quote_simple,
  sign_eip712,
  build_order_creation,
  post_order,
} from '@cowdao-grants/cow-sdk-wasm';

await init();

// 1. Quote (network).
const { response } = await get_quote_simple(
  '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48', // USDC
  '0x6B175474E89094C44Da98b954EedeAC495271d0F', // DAI
  '0x70997970C51812dc3A010C7d01b50e0d17dc79C8', // owner
  '100000000', // 100 USDC, 6 decimals
  'mainnet',
);

// 2. Sign in-shim.
const sig = sign_eip712(response.quote, 'mainnet', PRIVATE_KEY_HEX);

// 3. Submit (network).
const creation = build_order_creation(
  response.quote, sig, 'mainnet', response.from, '{}', response.id,
);
const uid = await post_order(creation, 'mainnet');
console.log(`https://explorer.cow.fi/orders/${uid}`);
```

**External signing** (production, Safe / WalletConnect / browser
wallets): the wasm crate never sees the private key. The shim's
`eip712_payload(orderData, chain)` returns a ready-to-use
`{ domain, primaryType, types, message }` object — the exact shape
both viem and ethers's `signTypedData` accept. Works against the
default (no-feature) build.

```js
import init, {
  eip712_payload,
  get_quote_simple,
  build_order_creation,
  post_order,
} from '@cowdao-grants/cow-sdk-wasm';

await init();

const { response } = await get_quote_simple(
  '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
  '0x6B175474E89094C44Da98b954EedeAC495271d0F',
  ACCOUNT,
  '100000000',
  'mainnet',
);
const payload = eip712_payload(response.quote, 'mainnet');
```

Hand `payload` to whichever wallet you have. Split the resulting
65-byte signature into `(r, s, v)` and feed it back through
`build_order_creation`.

```js
// viem
import { hexToBytes } from 'viem';
const signatureHex = await walletClient.signTypedData({ account: ACCOUNT, ...payload });

// ethers v6
const signatureHex = await ethersSigner.signTypedData(
  payload.domain, payload.types, payload.message,
);

// raw EIP-1193 (window.ethereum, WalletConnect, Safe SDK): viem and
// ethers both throw if `types` contains an `EIP712Domain` entry, so
// the shim deliberately omits it. The raw `eth_signTypedData_v4` RPC
// needs it, so inject before stringifying:
const v4 = {
  ...payload,
  types: {
    EIP712Domain: [
      { name: 'name',              type: 'string'  },
      { name: 'version',           type: 'string'  },
      { name: 'chainId',           type: 'uint256' },
      { name: 'verifyingContract', type: 'address' },
    ],
    ...payload.types,
  },
};
const signatureHex = await window.ethereum.request({
  method: 'eth_signTypedData_v4',
  params: [ACCOUNT, JSON.stringify(v4)],
});

// any path → (r, s, v)
const bytes = hexToBytes(signatureHex); // or ethers.getBytes
const sig = {
  signingScheme: 'eip712',
  r: '0x' + Buffer.from(bytes.slice(0, 32)).toString('hex'),
  s: '0x' + Buffer.from(bytes.slice(32, 64)).toString('hex'),
  v: bytes[64],
};

const creation = build_order_creation(response.quote, sig, 'mainnet', ACCOUNT, '{}', response.id);
const uid = await post_order(creation, 'mainnet');
```

**Conformance**. `eip712_payload` produces the digest
`OrderData::hash_struct` computes server-side; the Rust test suite
already locks that hash byte-for-byte against ethers's
`TypedDataEncoder` on all ten chains. The in-browser harness
(`test-harness/index.html`, panel 3) re-asserts the equality at
runtime across the shim, viem, and ethers. `@cowprotocol/cow-sdk`'s
own typed-data path delegates to ethers's `TypedDataEncoder`, so
parity with cow-sdk follows transitively.

For Safe wallets, replace `signTypedData` with the Safe SDK's
`signMessage` flow and use `build_order_creation_eip1271` instead;
the shim wraps the bytes into a `Signature::Eip1271` envelope.

See [`crates/cow-sdk-wasm/README.md`](crates/cow-sdk-wasm/README.md)
for the full exported function list, the `in_shim_signing` feature
trade-off, and the npm publish flow for maintainers.

### Build targets and bundle size

`wasm-pack` produces a different JS glue per target, with the same
underlying `.wasm` binary. The release recipes are:

| Target  | Recipe                    | Output dir   | Consumers                |
| ------- | ------------------------- | ------------ | ------------------------ |
| web     | `just wasm-build-web`     | `pkg-web/`   | Browser ES modules.      |
| bundler | `just wasm-build-bundler` | `pkg-bundler/` | webpack / Vite / Rollup. |
| nodejs  | `just wasm-build-nodejs`  | `pkg-nodejs/` | Node 18+, CommonJS.      |

`just wasm-build-all` builds all three; `just wasm-size` reports the
post-`wasm-opt` `.wasm` byte counts. The wasm binary is byte-identical
across targets, so an eventual npm package can ship one `.wasm` plus
three JS glues with an `exports` map.

Size knobs applied (`crates/cow-sdk-wasm/Cargo.toml` +
workspace `[profile.release]`):

- `wasm-opt -Oz` over the default `-O`: binaryen biases for binary
  size (~30% smaller).
- `[profile.release]`: `lto = "fat"`, `opt-level = "z"`,
  `panic = "abort"`, `strip = true`. Workspace-only — crates.io
  consumers use their own profile.
- `cowprotocol = { default-features = false }`: drops the
  orderbook/trading HTTP client and `subgraph` GraphQL client while
  keeping quote/order DTOs available through the facade.
- `lol_alloc` global allocator (~5 KB vs dlmalloc's ~10 KB). Active by
  default — `mod allocator;` is wired in at the top of the wasm crate's
  `lib.rs`.
- `in_shim_signing` cargo feature, default-off: gates
  `alloy-signer` + `alloy-signer-local` so the default build ships
  hash builders only. Saves ~68 KB; integrators sign with
  viem / ethers / Safe and submit the (r, s, v) bag back through
  `build_order_creation`.
- **No reqwest in the wasm output**: HTTP-touching exports
  (`get_quote`, `post_order`, etc.) call the JS `fetch` global directly
  via `js_sys::Reflect`, side-stepping reqwest's 150 KB bundle. The
  meta crate still exposes the reqwest-backed `OrderBookApi` for native
  consumers behind its default `http-client` feature.

Current `.wasm` after `wasm-opt -Oz`:

```
default features              584 KB
+ --features in_shim_signing  ~650 KB
```

(Down from 652 KB / 720 KB before lever 1.)

## Conformance

cow-rs locks byte-exact equivalence on every protocol-critical path:

- ethers `TypedDataEncoder` for all ten chains: signed UID, struct
  hash, domain separator, six order-shape permutations
- `cowprotocol/services` for signature recovery and cancellation
  struct hashes
- `cowprotocol/contracts` for the canonical `TYPE_HASH` derivation,
  `packOrderUidParams` layout, and event topic hashes
- ethers `Wallet.signTypedData` for the ECDSA `(r, s, v)` golden
- cow-py for the `ConditionalOrder` leaf-id derivation
- The empty-document app-data digest `keccak256("{}")`

Regenerate the cross-implementation vectors with:

```sh
cd tools/vector-gen && npm install && npm run gen > vectors.json
```

## Status

**0.1.x**: pre-1.0, so the public API may still change. Patch releases
(`0.1.N`) bring additive features and bug fixes; breaking changes land on
minor bumps (`0.2.0`, ...) per Cargo's `0.x` SemVer rules.

Production readiness:

- ✅ Byte-conformance with services / contracts / cow-sdk / cow-py
- ✅ All ten documented chains
- ✅ All four signing schemes
- ✅ Mock-server integration coverage for every `OrderBookApi` method
- ✅ WASM compilation gate in CI plus an in-browser e2e harness
- ✅ `cargo clippy -- -Dwarnings`, no `unsafe`, no `anyhow` in lib code
- ✅ Published to crates.io as [`cowprotocol`](https://crates.io/crates/cowprotocol)

## Building

```
just build        # cargo build --all-targets --all-features --workspace
just test         # cargo test --all-targets --all-features --workspace
just clippy       # cargo clippy ... -- -Dwarnings
just fmt-check
just wasm-check   # cargo check --target wasm32-unknown-unknown ...
just wasm-harness # build cow-sdk-wasm and serve test-harness/ on :8765
just doc          # cargo doc with -D warnings
```

## Layout

```
crates/cowprotocol/                # Library crate; everything re-exported from the root
crates/cowprotocol/examples/       # get_quote.rs, post_order.rs
crates/cow-sdk-wasm/           # #[wasm_bindgen] shim driving the in-browser harness (unpublished)
test-harness/                 # Static HTML harness; `just wasm-harness` to run
tools/vector-gen/             # Node.js golden-vector generator (ethers reference)
```

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md). Briefly: Oxford English in
prose, no em dashes, Conventional Commits, AI-assistance disclosure
in the PR body (never in commits), PRs ≤ 1,500 LoC against `develop`.

## Licence

GPL-3.0-or-later; see [LICENSE](./LICENSE). Portions adapted from
[`cowprotocol/services`](https://github.com/cowprotocol/services)
under MIT / Apache-2.0 with attribution in each affected file.
