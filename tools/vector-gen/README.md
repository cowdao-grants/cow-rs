# cow-rs — conformance vector generator

A small Node.js script that produces EIP-712 hashing vectors for the
canonical CoW Protocol order, used by the `cow-rs` Rust test suite as
golden assertions.

## What it computes

For a fixed sample order, and for each of the ten supported chains
(mainnet, BNB, Gnosis, Polygon, Base, Plasma, Arbitrum One, Avalanche,
Linea, Sepolia), the script writes the domain separator, the
order's EIP-712 struct hash, the full typed-data digest, and the 56-byte
order UID. All values come from ethers'
[`TypedDataEncoder`](https://docs.ethers.org/v6/api/hashing/#TypedDataEncoder)
— the same engine `@cowprotocol/cow-sdk` uses internally — so the
vectors are byte-identical to what a downstream signing flow would
produce.

No RPC calls are made.

## Usage

```sh
cd tools/vector-gen
npm install
npm run gen > vectors.json
```

`vectors.json` is the input the Rust integration tests assert against.

## Why this lives outside the Rust workspace

Keeping the generator in JavaScript means our golden vectors are produced
by the canonical ethers implementation, not by a re-implementation in
Rust. If a future Rust change drifts from ethers we want the tests to
fail.
