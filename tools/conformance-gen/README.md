# conformance-gen

Generates the signed-order fixture from services' `model` crate at
`df6a14b12eab016f1ae939aa5b666658208b0bc0`.

cow-rs's existing signed-order golden vectors come from `tools/vector-gen`,
which computes them with ethers' `TypedDataEncoder`. This tool takes the same
kind of values from services itself, the Rust implementation the orderbook runs.
`vector-gen` records one order on ten chains, seven struct-hash permutations
and one EIP-712 signature; this tool records 19 orders on mainnet, each with
EIP-712 and EthSign signatures.

## Coverage

The 19 orders cover all 12 combinations of order kind, sell-token source and
buy-token destination, plus three receiver cases, two amount boundaries and two
`validTo` boundaries. Each order is signed under EIP-712 and EthSign: 38
signing-scheme checks in total. Changing the matrix means updating the case
count in `src/main.rs`, `minItems`/`maxItems` in the schema and the coverage
checks in `crates/cowprotocol-signing/tests/conformance.rs`.

The base order starts from the order in services' `compute_order_uid` test,
which is also cow-rs's `sample_order()` and the `vector-gen` base order;
only `validTo` and `appData` differ. Token addresses, app data, chain ID,
settlement contract and signer are fixed. Every order sets `partiallyFillable`
to false. The two amount boundary vectors set the sell, buy and fee amounts to
the same boundary value. The absent and zero-address receiver cases produce the
same hash, signatures and UID, because services encodes an absent receiver as
the zero address. The base order keeps the non-zero fee from services' test,
and the fee is part of the signed order. The orderbook rejects new orders with
a non-zero fee, so these cases do not test orderbook acceptance. EIP-1271 and
presign are out of scope.

## Format

The top-level fields are `schemaVersion`, `servicesSha`, `settlement`,
`chainId`, `signer` and `vectors`. The fixture uses camelCase field names.

Each vector is services' `OrderData`, flattened, with its domain separator,
struct hash and UID. These results are named `domainSeparator`, `hashStruct`
and `uid`. The vector's `id` names the case; in ids such as
`sell_sb_external_bb_internal`, `sb` is the sell-token source
(`sellTokenBalance`) and `bb` the buy-token destination (`buyTokenBalance`).
`signatures` holds two expectations: one for EIP-712 and one for EthSign.

Each expectation contains `signingScheme`, `signature` and `digest`.
The first two fields come from services' `Signature`; `digest` comes from
`Recovered::message`. The signature is a single `0x`-prefixed hex string
encoding 65 bytes, not an object with separate `r`, `s` and `v` fields as in
`vector-gen`.

The top-level `signer` is the expected owner. The generator checks that both
signatures of every vector recover to this address.

The schema,
`crates/cowprotocol-signing/tests/fixtures/conformance_vectors.v1.schema.json`,
defines the structure, not cryptographic validity. A consumer must recompute
the struct hash, digests and UID from the order fields, recover the owner from
each signature and compare it with `signer`.

## Schema version

Incompatible changes to field names, required fields, types or structure
require a new schema version. A change to calculated values alone does not
necessarily change the format. `servicesSha` records which services
implementation supplied those values.

A version bump changes `SCHEMA_VERSION`, the schema's `schemaVersion` constant
and the version the test expects, and renames the `.v1.` files: the output path
in `src/main.rs`, the test's fixture path, the schema file and its `$id`, the
`.gitignore` entry and the paths in this README.

## Regeneration

From the repository root, generate the fixture and compare cow-rs with it:

```sh
cargo run --locked --manifest-path tools/conformance-gen/Cargo.toml
cargo test --locked -p cowprotocol-signing --test conformance -- --ignored
```

The `conformance` CI job runs the same commands. The fixture,
`crates/cowprotocol-signing/tests/fixtures/conformance_vectors.v1.json`, is
gitignored. If a vector does not match, the failure names the vector and the
value that differs.

CI catches cow-rs results that disagree with the pinned services commit for
these vectors. It does not see newer services changes until the pin moves.

## Changing the pin

1. Set the new commit in both `rev` values in `Cargo.toml`, `SERVICES_PIN` in
   `src/main.rs`, the services `sha` in `parity/source-lock.toml` and the first
   paragraph of this README.
2. Run the generator once without `--locked` to update its lockfile.
   Check that the resolved services dependencies use the new commit;
   `--locked` does not check `SERVICES_PIN` against those dependencies.
3. Run the two commands above. The test fails if the fixture and
   `source-lock.toml` name different commits.
4. Validate the fixture against the schema and review the changed values.

The tool is outside the workspace so that services dependencies stay out of SDK
and WASM builds. Run `cargo fmt` and `cargo clippy` on it with
`--manifest-path`.
