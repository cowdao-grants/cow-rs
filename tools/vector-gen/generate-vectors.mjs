// Generate cross-chain + permutation conformance vectors for cow-rs.
//
// Uses ethers' TypedDataEncoder (the same engine cow-sdk relies on under
// the hood) so the output is exactly what a downstream signing flow would
// produce. No RPC calls; everything is derived from the chain id, the
// settlement contract address, and a fixed test private key.
//
// Output is printed to stdout as a JSON object that the in-tree Rust
// tests assert against.

import { TypedDataEncoder, Wallet } from "ethers";

// Canonical GPv2Settlement CREATE2 deployment; identical on every CoW
// chain. Source: cowprotocol/contracts/networks.json
const SETTLEMENT = "0x9008D19f58AAbD9eD0D60971565AA8510560ab41";

// Same sample order as the services golden vector
// (cowprotocol/services/crates/model/src/order.rs::compute_order_uid).
const OWNER = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
const BASE_ORDER = {
  sellToken: "0x0101010101010101010101010101010101010101",
  buyToken: "0x0202020202020202020202020202020202020202",
  receiver: "0x0303030303030303030303030303030303030303",
  sellAmount: "0x0246ddf97976680000",
  buyAmount: "0xb98bc829a6f90000",
  validTo: 0xffffffff,
  appData: "0x" + "00".repeat(32),
  feeAmount: "0x0de0b6b3a7640000",
  kind: "sell",
  partiallyFillable: false,
  sellTokenBalance: "erc20",
  buyTokenBalance: "erc20",
};

// EIP-712 type definition for the CoW Protocol Order; verbatim from
// `cowprotocol/contracts/src/contracts/libraries/GPv2Order.sol`.
const ORDER_TYPES = {
  Order: [
    { name: "sellToken", type: "address" },
    { name: "buyToken", type: "address" },
    { name: "receiver", type: "address" },
    { name: "sellAmount", type: "uint256" },
    { name: "buyAmount", type: "uint256" },
    { name: "validTo", type: "uint32" },
    { name: "appData", type: "bytes32" },
    { name: "feeAmount", type: "uint256" },
    { name: "kind", type: "string" },
    { name: "partiallyFillable", type: "bool" },
    { name: "sellTokenBalance", type: "string" },
    { name: "buyTokenBalance", type: "string" },
  ],
};

// Every chain the CoW orderbook supports. Mirrors `cow_rs::Chain`.
const CHAINS = [
  { id: 1, name: "mainnet" },
  { id: 56, name: "bnb" },
  { id: 100, name: "gnosis" },
  { id: 137, name: "polygon" },
  { id: 8453, name: "base" },
  { id: 9745, name: "plasma" },
  { id: 42161, name: "arbitrum" },
  { id: 43114, name: "avalanche" },
  { id: 59144, name: "linea" },
  { id: 11155111, name: "sepolia" },
];

// Hash_struct permutations: vary one field at a time off the BASE order so
// each Rust assertion exercises a specific byte slot of OrderData::hash_struct.
const PERMUTATIONS = {
  base: BASE_ORDER,
  buy_kind: { ...BASE_ORDER, kind: "buy" },
  partially_fillable_true: { ...BASE_ORDER, partiallyFillable: true },
  sell_balance_external: { ...BASE_ORDER, sellTokenBalance: "external" },
  sell_balance_internal: { ...BASE_ORDER, sellTokenBalance: "internal" },
  buy_balance_internal: { ...BASE_ORDER, buyTokenBalance: "internal" },
  receiver_zero: { ...BASE_ORDER, receiver: "0x0000000000000000000000000000000000000000" },
};

// Hardhat account #0 private key; well-known and only used here for
// generating reproducible signature vectors.
const TEST_PRIVATE_KEY =
  "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

function packUid(digestHex, ownerHex, validTo) {
  const digest = digestHex.startsWith("0x") ? digestHex.slice(2) : digestHex;
  const owner = ownerHex.startsWith("0x") ? ownerHex.slice(2) : ownerHex;
  if (digest.length !== 64) throw new Error("digest must be 32 bytes");
  if (owner.length !== 40) throw new Error("owner must be 20 bytes");
  const validToHex = validTo.toString(16).padStart(8, "0");
  return "0x" + digest + owner.toLowerCase() + validToHex;
}

function splitSignature(signatureHex) {
  const stripped = signatureHex.startsWith("0x")
    ? signatureHex.slice(2)
    : signatureHex;
  if (stripped.length !== 130) {
    throw new Error(`expected 65-byte ECDSA, got ${stripped.length / 2} bytes`);
  }
  return {
    r: "0x" + stripped.slice(0, 64),
    s: "0x" + stripped.slice(64, 128),
    v: parseInt(stripped.slice(128, 130), 16),
  };
}

const out = {
  settlement: SETTLEMENT,
  owner: OWNER,
  order: BASE_ORDER,
  chains: {},
  hash_struct_permutations: {},
  ecdsa_signature: {},
};

for (const chain of CHAINS) {
  const domain = {
    name: "Gnosis Protocol",
    version: "v2",
    chainId: chain.id,
    verifyingContract: SETTLEMENT,
  };
  const domainSeparator = TypedDataEncoder.hashDomain(domain);
  const structHash = TypedDataEncoder.hashStruct("Order", ORDER_TYPES, BASE_ORDER);
  const digest = TypedDataEncoder.hash(domain, ORDER_TYPES, BASE_ORDER);
  const uid = packUid(digest, OWNER, BASE_ORDER.validTo);
  out.chains[chain.name] = {
    chainId: chain.id,
    domainSeparator,
    structHash,
    digest,
    uid,
  };
}

for (const [name, order] of Object.entries(PERMUTATIONS)) {
  out.hash_struct_permutations[name] = {
    order,
    structHash: TypedDataEncoder.hashStruct("Order", ORDER_TYPES, order),
  };
}

// ECDSA signature golden: sign the mainnet typed-data digest with the
// Hardhat #0 key and report (r, s, v). The verifying side can recover the
// signer address and compare against the wallet's known address.
const wallet = new Wallet(TEST_PRIVATE_KEY);
const mainnetDomain = {
  name: "Gnosis Protocol",
  version: "v2",
  chainId: 1,
  verifyingContract: SETTLEMENT,
};
const signature = await wallet.signTypedData(
  mainnetDomain,
  ORDER_TYPES,
  BASE_ORDER,
);
out.ecdsa_signature = {
  signer: wallet.address,
  privateKey: TEST_PRIVATE_KEY,
  domainSeparator: TypedDataEncoder.hashDomain(mainnetDomain),
  structHash: TypedDataEncoder.hashStruct("Order", ORDER_TYPES, BASE_ORDER),
  digest: TypedDataEncoder.hash(mainnetDomain, ORDER_TYPES, BASE_ORDER),
  signature,
  ...splitSignature(signature),
};

console.log(JSON.stringify(out, null, 2));
