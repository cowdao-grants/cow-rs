# cow-sdk-wasm

Rust source for the `@cowdao-grants/cow-sdk-wasm` npm package: a thin
`wasm-bindgen` shim that exposes the `cowprotocol` SDK to JavaScript
callers.

Published to npm as `@cowdao-grants/cow-sdk-wasm`. Not published to
crates.io.

## Status

Alpha. The exported functions mirror the cow-py / cow-sdk feature set
we considered most useful for browser and Node consumers, but the
binding is young and the API may evolve before a 1.0.

## Quick example

```js
import init, {
  chain_info,
  get_quote_simple,
  order_uid,
} from '@cowdao-grants/cow-sdk-wasm';

await init();

const info = chain_info('mainnet');
console.log(info.settlement, info.orderbookBaseUrl);

const { response, uid } = await get_quote_simple(
  '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48', // USDC
  '0x6B175474E89094C44Da98b954EedeAC495271d0F', // DAI
  '0x70997970C51812dc3A010C7d01b50e0d17dc79C8',
  '100000000',
  'mainnet',
);
```

See `test-harness/index.html` in the repository root for a complete
end-to-end example running against the live orderbook.

## Submit an order with your own wallet

The wasm shim never holds private keys in the default build. The
typical flow is: quote on this side, sign with `viem` /
`window.ethereum` / Safe, submit on this side. The orderbook attributes
the order back to this SDK because every exported helper that hashes
app-data carries the `appCode: "cow-rs-wasm"` tag plus this package's
version in `metadata.quote.version`.

```js
import init, {
  get_quote,
  to_signed_order_data,
  eip712_payload,
  build_order_creation,
  post_order,
  sdk_app_data_json,
  sdk_app_data_hash,
} from '@cowdao-grants/cow-sdk-wasm';
import { createWalletClient, custom } from 'viem';
import { base } from 'viem/chains';

await init();

const account = '0x26394373F96950025DA55b07809c976a4768c995';

// 1. Quote. The shim cross-checks the response against the request
//    before returning, so a hostile orderbook cannot hand us a swapped
//    sellToken / buyToken / receiver / from / kind to feed downstream.
const request = {
  sellToken: '0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913', // USDC on Base
  buyToken:  '0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE', // ETH placeholder
  from:      account,
  kind:      'sell',
  sellAmountBeforeFee: '25000000', // 25 USDC, 6 decimals
};
const response = await get_quote(request, 'base');

// 2. Build this SDK's attribution app-data so the orderbook indexer
//    can count the order. Pure compute, no network.
const appDataJson = sdk_app_data_json();   // canonical JSON string
const appDataHash = sdk_app_data_hash();   // keccak256 hex

// 3. Project the response into the 12-field OrderData the wallet will
//    sign. `to_signed_order_data` is the only blessed assembly path:
//    it re-validates the response against `request` (now also against
//    the pinned appData hash) and refuses to project mismatched
//    fields, so a swapped-token response never reaches `signTypedData`.
const orderData = to_signed_order_data(request, response, appDataHash);

// 4. Ask the SDK for the EIP-712 typed-data envelope; sign it with
//    whatever wallet you have. viem returns a 0x-prefixed 65-byte hex.
const typedData = eip712_payload(orderData, 'base');
const wallet = createWalletClient({
  chain: base,
  transport: custom(window.ethereum),
});
const signatureHex = await wallet.signTypedData({
  account,
  ...typedData,
});

// 5. Unpack (r, s, v) for build_order_creation. The orderbook expects
//    the bytes split out from the 0x{r}{s}{v} hex blob.
const r = '0x' + signatureHex.slice(2, 66);
const s = '0x' + signatureHex.slice(66, 130);
const v = parseInt(signatureHex.slice(130, 132), 16);

const creation = build_order_creation(
  orderData,
  { signingScheme: 'eip712', r, s, v },
  'base', // chain: selects the EIP-712 domain used for owner recovery
  account,
  appDataJson, // PUT against /api/v1/app_data/{hash} by the orderbook
  response.id, // quote_id for solver fee accounting
);

// 6. POST /api/v1/orders. Returns the assigned 56-byte UID.
const uid = await post_order(creation, 'base');
console.log(`https://explorer.cow.fi/base/orders/${uid}`);
```

For ethers v6 the signing block is the same shape — call
`signer.signTypedData(typedData.domain, typedData.types, typedData.message)`
and slice the result the same way.

If you want the SDK to hold the private key in wasm linear memory (for
tests, scripts, or local replays), build with the `in_shim_signing`
cargo feature and replace step 4 with one of `sign_eip712` /
`sign_ethsign`. The default build leaves those off so the shipped
`.wasm` stays slim.

## Cargo features

| Feature | Default | What it adds |
| --- | --- | --- |
| `in_shim_signing` | off | Exports `sign_eip712`, `sign_ethsign`, `cancel_order_signed`. Pulls in `alloy-signer-local` so the shim can accept a private-key hex string and produce signatures inside wasm linear memory. Adds ~80–120 KB to the wasm binary. |

Production integrations sign with viem / ethers / Safe outside the wasm
boundary and feed the `(r, s, v)` triple back through `build_order_creation`.
For tests, scripts, or browser playgrounds where private keys already
exist in JS memory, enabling `in_shim_signing` is convenient.

## Build targets

Three wasm-pack targets, all built via the workspace `justfile`:

```sh
just wasm-build-web      # ES modules; what the test-harness uses
just wasm-build-bundler  # webpack / Vite / Rollup
just wasm-build-nodejs   # Node 18+ CommonJS
just wasm-build-all      # all three
just wasm-size           # build all three, then print .wasm byte sizes
```

Each lands in its own `pkg-{web,bundler,nodejs}/` directory under
`crates/cow-sdk-wasm/`, all of which are git-ignored.

## Publishing to npm (maintainer flow)

The crate is configured to publish under the `@cowdao-grants` npm scope.
The Cargo `version` in `crates/cow-sdk-wasm/Cargo.toml` is the source of
truth; `wasm-pack` propagates it into the generated `package.json`.

```sh
# 1. Bump version in crates/cow-sdk-wasm/Cargo.toml.
# 2. Build each target you want to publish.
just wasm-build-all

# 3. Pick a target's pkg directory (usually pkg-bundler for general use)
#    and publish.
cd crates/cow-sdk-wasm/pkg-bundler
npm publish --access public
```

`wasm-pack publish` also works if you prefer one command; it accepts the
same `--scope cowdao-grants` flag the build commands use.

## Why is the crate name `cow-sdk-wasm` and not `cowprotocol-wasm`?

The Rust crate on crates.io is `cowprotocol`. The npm equivalent at
`@cowprotocol/cow-sdk` is the TypeScript SDK maintained by the
cowprotocol organisation. To avoid name collisions and signal that this
is a community-built wasm binding, the npm package uses
`@cowdao-grants/cow-sdk-wasm` (under the grants org, where the parent
repository lives) and the Cargo crate matches.
