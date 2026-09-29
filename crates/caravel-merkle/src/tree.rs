//! Tree building and proof generation (needs `alloc`).

use alloc::vec::Vec;

use crate::{depth, node, MerkleError, Sha256, MAX_LEAVES, ZERO};

fn leaf_count(leaves: &[[u8; 32]]) -> Result<u32, MerkleError> {
    u32::try_from(leaves.len())
        .ok()
        .filter(|n| *n <= MAX_LEAVES)
        .ok_or(MerkleError::TooManyLeaves)
}

/// The root of `leaves` (spec §9.9). Computes in place, one level at a time.
pub fn root<H: Sha256 + ?Sized>(h: &H, leaves: &[[u8; 32]]) -> Result<[u8; 32], MerkleError> {
    let n = leaf_count(leaves)?;
    if n == 0 {
        return Ok(ZERO);
    }
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    level.resize(n.next_power_of_two() as usize, ZERO);
    while level.len() > 1 {
        let half = level.len() / 2;
        for i in 0..half {
            level[i] = node(h, &level[2 * i], &level[2 * i + 1]);
        }
        level.truncate(half);
    }
    Ok(level[0])
}

/// The proof for leaf `index` (spec §9.9).
pub fn proof<H: Sha256 + ?Sized>(
    h: &H,
    leaves: &[[u8; 32]],
    index: u32,
) -> Result<Vec<[u8; 32]>, MerkleError> {
    MerkleTree::build(h, leaves)?.proof(index)
}

/// A built tree, for serving many proofs from one checkpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MerkleTree {
    n: u32,
    /// `levels[0]` is the padded leaf level; the last level is the root.
    levels: Vec<Vec<[u8; 32]>>,
}

impl MerkleTree {
    pub fn build<H: Sha256 + ?Sized>(h: &H, leaves: &[[u8; 32]]) -> Result<Self, MerkleError> {
        let n = leaf_count(leaves)?;
        if n == 0 {
            return Ok(Self {
                n,
                levels: Vec::new(),
            });
        }
        let mut level = leaves.to_vec();
        level.resize(n.next_power_of_two() as usize, ZERO);
        let mut levels = alloc::vec![level];
        while levels[levels.len() - 1].len() > 1 {
            let prev = &levels[levels.len() - 1];
            let next = prev
                .chunks_exact(2)
                .map(|pair| node(h, &pair[0], &pair[1]))
                .collect();
            levels.push(next);
        }
        Ok(Self { n, levels })
    }

    pub fn leaf_count(&self) -> u32 {
        self.n
    }

    pub fn root(&self) -> [u8; 32] {
        self.levels.last().map_or(ZERO, |top| top[0])
    }

    pub fn proof(&self, index: u32) -> Result<Vec<[u8; 32]>, MerkleError> {
        if index >= self.n {
            return Err(MerkleError::IndexOutOfRange);
        }
        let d = depth(self.n) as usize;
        let mut out = Vec::with_capacity(d);
        let mut i = index as usize;
        for level in &self.levels[..d] {
            out.push(level[i ^ 1]);
            i /= 2;
        }
        Ok(out)
    }
}
