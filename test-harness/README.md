# wasm end-to-end harness

A static HTML page that loads the `cow-sdk-wasm` crate and exercises the
cow-rs API surface from a browser. This goes one step beyond the
`cargo check --target wasm32-unknown-unknown` gate in CI: it actually
loads the wasm module, calls into it from JavaScript, and hits the live
orderbook through reqwest's browser-fetch backend.

## Run it

From the repository root:

```sh
just wasm-harness
open http://localhost:8765/test-harness/
```

Install `wasm-pack` with `cargo install wasm-pack --locked` if not present.

The recipe just chains the two raw commands, in case you prefer to run
them manually:

```sh
# 1. Build the wasm package (output is git-ignored).
(cd crates/cow-sdk-wasm && wasm-pack build --target web --dev --features in_shim_signing)
# 2. Serve the workspace over HTTP so ES-module imports resolve.
#    Bind to loopback: the default binds all interfaces and would expose
#    the whole workspace (including .git) to others on the network.
python3 -m http.server 8765 --bind 127.0.0.1
```

## What it verifies

Eight panels, each reports ✅ / ❌ inline with the raw JSON beneath:

1. **`order_uid` UID parity (pure compute)**: derives the 56-byte
   `OrderUid` for a fixed sell-only order against the mainnet
   `GPv2Settlement` domain. No network, no signing; proves the EIP-712
   hashing path survives the wasm boundary byte-for-byte.
2. **`chain_info` typed lookup**: round-trips the eleven supported
   networks across the wasm/JS bridge.
3. **`eip712_payload` cross-library digest parity**: shim, viem
   (`hashTypedData`), and ethers (`TypedDataEncoder.hash`) all produce
   byte-identical hashes for the same payload. Cow-sdk parity follows
   transitively.
4. **`sign_eip712` in-shim ECDSA signing**: signs with a fixed
   Anvil-style PK (requires the `in_shim_signing` feature build).
5. **Cross-library signature parity + round-trip**: signs the same
   payload with viem, ethers, and the shim, in two modes — Anvil PK
   (deterministic ECDSA per RFC 6979; all three produce byte-identical
   `(r, s, v)`) and MetaMask (real wallet popup; same agreement against
   the wallet account). The last-clicked signature is round-tripped
   through `build_order_creation` to prove the shim accepts
   externally-produced signatures.
6. **Live `get_quote_simple`**: calls `POST /api/v1/quote` against
   `api.cow.fi/mainnet`, deserialises the response across the wasm/JS
   bridge, and re-derives the UID from the orderbook's `OrderQuote`.
7. **`version`**: hits `GET /api/v1/version` against `api.cow.fi`.
8. **Submit an order through your wallet (USDC → ETH on Base)**: full
   end-to-end against `api.cow.fi/base`: connect wallet, probe native
   vs bridged USDC balance, configure sell amount, build SDK-attributed
   app-data, sign with MetaMask via raw `eth_signTypedData_v4`, POST.
   Surfaces the order UID + explorer URL on success.

## Build targets

The harness uses the `--target web` build because it loads the wasm
package as an ES module via a `<script type="module">` tag. The crate
also ships release builds for the other two `wasm-pack` targets, each
in its own output directory so they can coexist:

| Target  | Recipe                | Output dir   | Consumers                  |
| ------- | --------------------- | ------------ | -------------------------- |
| web     | `just wasm-build-web` | `pkg-web/`   | Plain browser ES modules.  |
| bundler | `just wasm-build-bundler` | `pkg-bundler/` | webpack, Vite, Rollup. |
| nodejs  | `just wasm-build-nodejs` | `pkg-nodejs/` | Node 18+, CommonJS.    |

Run `just wasm-build-all` to produce all three, then `just wasm-size`
to print the `.wasm` byte sizes after `wasm-opt -Oz`.
