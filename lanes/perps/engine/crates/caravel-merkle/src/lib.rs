//! Merkle trees for Caravel checkpoints (spec §9.9), shared by the engine, the
//! nodes and the settlement contract.
//!
//! - Input: an ordered list of `n` leaf hashes. Leaf hashing (with a `0x00`
//!   prefix) is done by the caller (spec §11.8).
//! - `n == 0` gives the zero root. Otherwise the leaves are padded to
//!   `P = next_power_of_two(n)` with zero leaves (not hashed), and
//!   `node = H(0x01 || left || right)`.
//! - A proof is the siblings from the leaf level upward; its length is
//!   `ceil_log2(n)` (0 when `n == 1`, where the root is the leaf).
//!
//! The verify path uses no allocation. Building and proofs need the `alloc`
//! feature.
//!
//! Consensus code: the determinism rules in spec §8 apply to everything here.
#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(feature = "gen-vectors")]
extern crate std;

#[cfg(feature = "alloc")]
mod tree;
#[cfg(feature = "alloc")]
pub use tree::{proof, root, MerkleTree};

#[cfg(feature = "gen-vectors")]
pub mod vectors;

/// Maximum tree depth (spec §9.9).
pub const MAX_DEPTH: u32 = 20;
/// Maximum leaf count for [`MAX_DEPTH`].
pub const MAX_LEAVES: u32 = 1 << MAX_DEPTH;
/// The root of an empty tree, and the padding leaf.
pub const ZERO: [u8; 32] = [0; 32];
/// Node preimages start with this byte; leaf preimages start with 0x00.
pub const NODE_PREFIX: u8 = 0x01;

/// SHA-256, supplied by the caller: native `sha2` or the Soroban host.
pub trait Sha256 {
    fn hash(&self, data: &[u8]) -> [u8; 32];
}

/// Why a tree cannot be built or a proof produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MerkleError {
    /// More than [`MAX_LEAVES`] leaves.
    TooManyLeaves,
    /// The proof index is not below the leaf count.
    IndexOutOfRange,
}

/// `H(0x01 || left || right)`.
pub fn node<H: Sha256 + ?Sized>(h: &H, left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut buf = [0u8; 65];
    buf[0] = NODE_PREFIX;
    buf[1..33].copy_from_slice(left);
    buf[33..].copy_from_slice(right);
    h.hash(&buf)
}

/// `ceil_log2(n)`: the proof length for a tree of `n ≥ 1` leaves (0 for `n ≤ 1`).
pub fn depth(n: u32) -> u32 {
    if n <= 1 {
        0
    } else {
        u32::BITS - (n - 1).leading_zeros()
    }
}

/// Checks that `leaf` is leaf `index` of an `n`-leaf tree with `root` (spec §9.9):
/// `index < n`, `siblings.len() == ceil_log2(n)`, depth ≤ 20, and at level `k`
/// the current node is on the left when bit `k` of `index` is 0.
pub fn verify<H: Sha256 + ?Sized>(
    h: &H,
    leaf: &[u8; 32],
    index: u32,
    n: u32,
    siblings: &[[u8; 32]],
    root: &[u8; 32],
) -> bool {
    if n == 0 || n > MAX_LEAVES || index >= n {
        return false;
    }
    if siblings.len() != depth(n) as usize {
        return false;
    }
    let mut cur = *leaf;
    for (k, sibling) in siblings.iter().enumerate() {
        cur = if (index >> k) & 1 == 0 {
            node(h, &cur, sibling)
        } else {
            node(h, sibling, &cur)
        };
    }
    &cur == root
}

/// SHA-256 from the `sha2` crate.
#[cfg(feature = "native")]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeSha256;

#[cfg(feature = "native")]
impl Sha256 for NativeSha256 {
    fn hash(&self, data: &[u8]) -> [u8; 32] {
        use sha2::Digest;
        sha2::Sha256::digest(data).into()
    }
}

/// SHA-256 from the Soroban host: `env.crypto().sha256`.
#[cfg(feature = "soroban")]
pub struct SorobanSha256<'a>(pub &'a soroban_sdk::Env);

#[cfg(feature = "soroban")]
impl Sha256 for SorobanSha256<'_> {
    fn hash(&self, data: &[u8]) -> [u8; 32] {
        self.0
            .crypto()
            .sha256(&soroban_sdk::Bytes::from_slice(self.0, data))
            .to_array()
    }
}
