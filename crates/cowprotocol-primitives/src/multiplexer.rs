//! Merkle-tree multiplexer for batched ComposableCoW conditional orders.
//!
//! `ComposableCoW.setRoot(root, proof)` commits a 32-byte merkle root that
//! enumerates many conditional orders at once. A watch tower wanting to
//! submit one of them calls `ComposableCoW.hashesFromContext` with the
//! `(params, proof, offchainInput)` triple, and the contract validates the
//! proof via OpenZeppelin's `MerkleProof.verify`. The on-chain leaf form
//! is `keccak256(bytes.concat(keccak256(abi.encode(params))))`: the
//! conditional-order id, hashed again. The double hashing is OZ's
//! standard collision-prevention trick (so a leaf can't be mistaken for
//! an internal node).
//!
//! This module provides the four primitives a multiplexer-using
//! integrator needs:
//!
//! - [`conditional_order_leaf`]: derive the contract-canonical leaf hash
//!   from a [`ConditionalOrderParams`].
//! - [`Multiplexer::new`]: build a tree from a list of leaves.
//! - [`Multiplexer::proof`] / [`Multiplexer::root`]: ship the
//!   `(proof, root)` pair to the contract and to off-chain watch towers.
//! - [`verify_proof`]: client-side gate against the same OZ
//!   commutative-pair algorithm the contract uses; rejecting bad proofs
//!   here saves an on-chain revert.
//!
//! Reference: [`MerkleProof.verify`][oz] in OpenZeppelin Contracts and the
//! invocation site in
//! [`ComposableCoW._auth`](https://github.com/cowprotocol/composable-cow/blob/main/src/ComposableCoW.sol).
//!
//! [oz]: https://github.com/OpenZeppelin/openzeppelin-contracts/blob/master/contracts/utils/cryptography/MerkleProof.sol

use alloy_primitives::{B256, keccak256};
use alloy_sol_types::SolValue;

use crate::composable::ConditionalOrderParams;

/// Derive the contract-canonical merkle leaf for a single conditional
/// order: `keccak256(keccak256(abi.encode(params)))`.
///
/// Matches `_auth`'s `leaf = keccak256(bytes.concat(hash(params)))` in
/// `ComposableCoW.sol`. The outer keccak is what protects against
/// second-preimage attacks where a leaf could collide with an internal
/// node.
pub fn conditional_order_leaf(params: &ConditionalOrderParams) -> B256 {
    let id = keccak256(params.abi_encode());
    keccak256(id.as_slice())
}

/// Hash a single tree node pair, commutatively, with the smaller value
/// concatenated first.
///
/// Implements OZ `MerkleProof._hashPair`:
/// `keccak256(min(a, b) || max(a, b))`.
fn hash_pair(a: B256, b: B256) -> B256 {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let mut buf = [0_u8; 64];
    buf[..32].copy_from_slice(lo.as_slice());
    buf[32..].copy_from_slice(hi.as_slice());
    keccak256(buf)
}

/// Owner-friendly view over a merkle tree of conditional-order leaves.
///
/// The tree is stored as a vector of levels (level 0 is the leaves;
/// level N is the root). Leaves are NOT re-ordered: index `i` here is
/// the index the [`Multiplexer::proof`] caller will pass to the contract.
#[derive(Clone, Debug)]
pub struct Multiplexer {
    /// `levels[0]` is the leaves; the last entry is a single-element
    /// vector holding the root.
    levels: Vec<Vec<B256>>,
}

/// Failure modes for [`Multiplexer::new`] and [`Multiplexer::proof`].
#[derive(Clone, Copy, Debug, thiserror::Error, Eq, PartialEq)]
pub enum MultiplexerError {
    /// `new` was called with an empty slice.
    #[error("Multiplexer needs at least one leaf")]
    Empty,
    /// `proof` was called with an out-of-range index.
    #[error("leaf index out of range")]
    IndexOutOfRange,
}

/// A merkle proof bound to the leaf and index it authenticates.
///
/// Returned by [`Multiplexer::proof_with_leaf`]. Unlike the bare
/// `Vec<B256>` from [`Multiplexer::proof`], this couples the sibling
/// hashes to the `index` and `leaf` they prove, so a caller cannot
/// accidentally verify the proof against the wrong leaf.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MerkleProof {
    /// Index of the proved leaf, in the tree's input order.
    pub index: usize,
    /// The leaf hash this proof authenticates.
    pub leaf: B256,
    /// Sibling hashes from the leaf up to the root, in the order the
    /// contract's verifier consumes them.
    pub siblings: Vec<B256>,
}

impl MerkleProof {
    /// Verify this proof against `root` with the same commutative
    /// sorted-pair algorithm the on-chain `MerkleProof.verify` runs.
    /// Convenience wrapper over [`verify_proof`].
    pub fn verify(&self, root: B256) -> bool {
        verify_proof(root, &self.siblings, self.leaf)
    }
}

impl Multiplexer {
    /// Construct a tree from leaves derived via [`conditional_order_leaf`].
    ///
    /// At each level pairs of nodes are combined with the commutative
    /// `hash_pair`; an odd trailing node is carried up unmodified, and
    /// input leaf order is preserved.
    ///
    /// The proofs this produces verify against the on-chain
    /// `MerkleProof.verify` (which is order-agnostic), and
    /// [`verify_proof`] round-trips them. The root is **not** byte-equal
    /// to cow-sdk's `Multiplexer` root over the same orders, and cannot
    /// be: cow-sdk ([`Multiplexer.ts` @ `00c3dbd4`][sdk], pinned in
    /// `parity/source-lock.toml`) builds its `StandardMerkleTree` with
    /// the flat types `['address', 'bytes32', 'bytes']`, so its leaves
    /// double-hash the three fields encoded as a bare parameter list,
    /// while `ComposableCoW._auth` verifies proofs against
    /// `keccak256(bytes.concat(hash(params)))`, whose inner `hash`
    /// ABI-encodes the fields as a struct: one offset-prefixed tuple,
    /// the same bytes [`conditional_order_leaf`] hashes and the
    /// `conditional_order_leaf_id_matches_cow_py_vector` test locks.
    /// The two leaf preimages differ, so a cow-sdk-derived root does
    /// not verify on-chain at all (upstream tracks finishing its merkle
    /// flow in cow-sdk issue #155). This implementation sides with the
    /// contract: it keeps the contract-canonical leaf and a layout
    /// whose proofs `_auth` accepts, at the cost of root parity with
    /// the JS SDK.
    ///
    /// [sdk]: https://github.com/cowprotocol/cow-sdk/blob/00c3dbd41c086ff9a51d5e5a30648615d4c66d0d/packages/composable/src/Multiplexer.ts
    pub fn new(leaves: &[B256]) -> Result<Self, MultiplexerError> {
        if leaves.is_empty() {
            return Err(MultiplexerError::Empty);
        }
        let mut levels: Vec<Vec<B256>> = vec![leaves.to_vec()];
        while levels.last().unwrap().len() > 1 {
            let prev = levels.last().unwrap();
            let mut next = Vec::with_capacity(prev.len().div_ceil(2));
            let mut i = 0;
            while i < prev.len() {
                if i + 1 < prev.len() {
                    next.push(hash_pair(prev[i], prev[i + 1]));
                } else {
                    // Odd trailing node carries up unmodified.
                    next.push(prev[i]);
                }
                i += 2;
            }
            levels.push(next);
        }
        Ok(Self { levels })
    }

    /// Build a multiplexer directly from conditional-order params,
    /// applying the canonical double-hash leaf derivation.
    pub fn from_params(orders: &[ConditionalOrderParams]) -> Result<Self, MultiplexerError> {
        let leaves: Vec<B256> = orders.iter().map(conditional_order_leaf).collect();
        Self::new(&leaves)
    }

    /// Merkle root: the value passed to `ComposableCoW.setRoot`.
    pub fn root(&self) -> B256 {
        *self.levels.last().unwrap().first().unwrap()
    }

    /// The leaves the tree was built from, in input order. Useful for
    /// callers that want to round-trip-verify a specific leaf's proof.
    pub fn leaves(&self) -> &[B256] {
        &self.levels[0]
    }

    /// Generate the merkle proof for the leaf at `index`. Returns an
    /// empty vector when the tree has only one leaf (the leaf IS the
    /// root, and OZ's verifier accepts an empty proof in that case).
    pub fn proof(&self, index: usize) -> Result<Vec<B256>, MultiplexerError> {
        if index >= self.leaves().len() {
            return Err(MultiplexerError::IndexOutOfRange);
        }
        let mut proof = Vec::new();
        let mut idx = index;
        for level in &self.levels[..self.levels.len() - 1] {
            let sibling = idx ^ 1;
            if sibling < level.len() {
                proof.push(level[sibling]);
            }
            idx /= 2;
        }
        Ok(proof)
    }

    /// Generate a [`MerkleProof`] for the leaf at `index`, bundling the
    /// index and leaf hash with the sibling hashes so the proof carries
    /// the leaf it authenticates. Prefer this over [`Multiplexer::proof`]
    /// when the proof and its leaf travel together.
    pub fn proof_with_leaf(&self, index: usize) -> Result<MerkleProof, MultiplexerError> {
        // `proof` bounds-checks `index`, so the leaf lookup below cannot
        // panic.
        let siblings = self.proof(index)?;
        Ok(MerkleProof {
            index,
            leaf: self.leaves()[index],
            siblings,
        })
    }
}

/// Verify `proof` against `root` for the given `leaf`, using OZ's
/// sorted-pair commutative algorithm: the same code the on-chain
/// `MerkleProof.verify` runs.
///
/// Use this to reject bad proofs client-side before they hit the chain.
pub fn verify_proof(root: B256, proof: &[B256], leaf: B256) -> bool {
    let mut current = leaf;
    for sibling in proof {
        current = hash_pair(current, *sibling);
    }
    current == root
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Address, Bytes, hex};

    use super::*;

    fn leaf(i: u8) -> B256 {
        B256::repeat_byte(i)
    }

    /// Single-leaf tree: root == leaf, proof is empty, verify accepts
    /// the empty proof.
    #[test]
    fn single_leaf_tree() {
        let leaves = vec![leaf(1)];
        let tree = Multiplexer::new(&leaves).unwrap();
        assert_eq!(tree.root(), leaf(1));
        assert!(tree.proof(0).unwrap().is_empty());
        assert!(verify_proof(tree.root(), &[], leaf(1)));
        assert!(!verify_proof(tree.root(), &[], leaf(2)));
    }

    /// Two leaves: root == hash_pair(a, b). Both proofs are single-
    /// element, and verify accepts each.
    #[test]
    fn two_leaf_tree_root_matches_sorted_pair_keccak() {
        let leaves = vec![leaf(1), leaf(2)];
        let tree = Multiplexer::new(&leaves).unwrap();
        let expected_root = {
            let mut buf = [0_u8; 64];
            buf[..32].copy_from_slice(leaf(1).as_slice());
            buf[32..].copy_from_slice(leaf(2).as_slice());
            keccak256(buf)
        };
        assert_eq!(tree.root(), expected_root);

        let proof_for_0 = tree.proof(0).unwrap();
        assert_eq!(proof_for_0, vec![leaf(2)]);
        assert!(verify_proof(tree.root(), &proof_for_0, leaf(1)));

        let proof_for_1 = tree.proof(1).unwrap();
        assert_eq!(proof_for_1, vec![leaf(1)]);
        assert!(verify_proof(tree.root(), &proof_for_1, leaf(2)));
    }

    /// Three-leaf tree exercises the odd-trailing-node carry-up branch.
    /// Every leaf's proof must round-trip back to the root.
    #[test]
    fn three_leaf_tree_round_trips_every_proof() {
        let leaves: Vec<B256> = (1_u8..=3).map(leaf).collect();
        let tree = Multiplexer::new(&leaves).unwrap();
        for (i, l) in leaves.iter().enumerate() {
            let proof = tree.proof(i).unwrap();
            assert!(
                verify_proof(tree.root(), &proof, *l),
                "leaf {i} did not round-trip",
            );
        }
        // Tampered leaves must NOT verify.
        let proof = tree.proof(0).unwrap();
        assert!(!verify_proof(tree.root(), &proof, leaf(99)));
    }

    /// Ten-leaf tree (size not a power of two): every leaf round-trips.
    #[test]
    fn arbitrary_size_tree_round_trips() {
        let leaves: Vec<B256> = (1_u8..=10).map(leaf).collect();
        let tree = Multiplexer::new(&leaves).unwrap();
        for (i, l) in leaves.iter().enumerate() {
            let proof = tree.proof(i).unwrap();
            assert!(verify_proof(tree.root(), &proof, *l));
        }
    }

    /// `from_params` applies the contract's double-hash leaf derivation.
    /// Locks the leaf shape against the formula in `ComposableCoW._auth`.
    #[test]
    fn conditional_order_leaf_double_hashes_params() {
        let params = ConditionalOrderParams {
            handler: Address::repeat_byte(0xab),
            salt: B256::from(hex!(
                "0101010101010101010101010101010101010101010101010101010101010101"
            )),
            staticInput: Bytes::from_static(&hex!("deadbeef")),
        };
        let id = keccak256(params.abi_encode());
        let expected = keccak256(id.as_slice());
        assert_eq!(conditional_order_leaf(&params), expected);
    }

    #[test]
    fn empty_input_is_rejected() {
        assert_eq!(Multiplexer::new(&[]).unwrap_err(), MultiplexerError::Empty);
    }

    #[test]
    fn out_of_range_proof_is_rejected() {
        let tree = Multiplexer::new(&[leaf(1), leaf(2)]).unwrap();
        assert_eq!(
            tree.proof(2).unwrap_err(),
            MultiplexerError::IndexOutOfRange
        );
    }

    /// `proof_with_leaf` binds the index, leaf and siblings together and
    /// the bundle verifies against the root for every leaf.
    #[test]
    fn proof_with_leaf_binds_index_and_verifies() {
        let leaves: Vec<B256> = (1_u8..=5).map(leaf).collect();
        let tree = Multiplexer::new(&leaves).unwrap();
        for (i, l) in leaves.iter().enumerate() {
            let bundle = tree.proof_with_leaf(i).unwrap();
            assert_eq!(bundle.index, i);
            assert_eq!(bundle.leaf, *l);
            assert_eq!(bundle.siblings, tree.proof(i).unwrap());
            assert!(bundle.verify(tree.root()));
        }
        assert_eq!(
            tree.proof_with_leaf(5).unwrap_err(),
            MultiplexerError::IndexOutOfRange
        );
    }
}
