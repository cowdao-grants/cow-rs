# conformance-gen

Generates the signed-order fixture from services' `model` crate at
`df6a14b12eab016f1ae939aa5b666658208b0bc0`.

cow-rs's existing signed-order golden vectors come from `tools/vector-gen`,
which computes them with ethers' `TypedDataEncoder`. This tool takes the same
kind of values from services itself, the Rust implementation the orderbook runs.
`vector-gen` records one order on ten chains, seven struct-hash permutations
and one EIP-712 signature; this tool records 19 orders on mainnet, each with
EIP-712 and EthSign signatures.
`parity/source-lock.toml` still records an older services pin (`47af30fe`);
this tool does not read it.

## Coverage

The 19 orders cover all 12 combinations of order kind, sell-token source and
buy-token destination, plus three receiver cases, two amount boundaries and two
`validTo` boundaries. Each order is signed under EIP-712 and EthSign: 38
signing-scheme checks in total. Changing the matrix means updating the case
count in `src/main.rs` and `minItems`/`maxItems` in the schema.

The base order starts from the order in services' `compute_order_uid` test,
which is also cow-rs's `sample_order()` and the `vector-gen` base order;
only `validTo` and `appData` differ. Token addresses, app data, chain ID,
settlement contract and signer are fixed. Every order sets `partiallyFillable`
to false. The two amount boundary vectors set the sell, buy and fee amounts to
the same boundary value. The absent and zero-address receiver cases produce the
same hash, signatures and UID, because services encodes an absent receiver as
the zero address. The non-zero fee is included in the order fields used for
hashing and signing. These cases do not test orderbook acceptance. EIP-1271
and presign are out of scope.

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

After the first merge, incompatible changes to field names, required fields,
types or structure require a new schema version. A change to calculated values
alone does not necessarily change the format. `servicesSha` records which
services implementation supplied those values.

A version bump changes `SCHEMA_VERSION` and the schema's `schemaVersion`
constant, and renames the `.v1.` files: the output path in `src/main.rs`, the
schema file, the `.gitignore` entry and the paths in this README.

## Regeneration

Run from the repository root:

```sh
cargo run --locked --manifest-path tools/conformance-gen/Cargo.toml
```

The generated `crates/cowprotocol-signing/tests/fixtures/conformance_vectors.v1.json`
is gitignored and not committed; regenerate it before anything reads it.
Dependencies need network access on first build; cached builds can use
`--offline`.

To change the pin, update both `rev` values in the tool's `Cargo.toml`,
`SERVICES_PIN` in `src/main.rs` and the revision at the top of this README.
Refresh the tool's lockfile without `--locked`, then regenerate the fixture
with `--locked`. Run the tool's formatting and Clippy checks, validate the
fixture against the schema and review any changed values or serialisation.
A pin change can change services' serialisation as well as its calculations.

The lockfile is committed so that `--locked` reproduces the same dependency
graph. The services crates in it resolve to the commit in `SERVICES_PIN`.
That is checked by hand: the generator does not compare the lockfile with
`SERVICES_PIN` or with the revision stated in this README. `--locked` stops
resolution from changing the lockfile; it does not check those references,
the documentation or security advisories.

The tool has its own lockfile and is excluded from the workspace to keep native
services dependencies out of SDK/WASM builds. Run its formatting and Clippy
checks separately using the same `--manifest-path`.

No test reads the fixture in Milestone 1. The cow-rs consumer test, the
source-lock update and CI belong to Milestone 2.
