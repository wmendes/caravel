//! caravel-merkle tests: a naive reference tree, the committed vectors, and
//! the Soroban hasher inside a real contract (T-002 acceptance).

use caravel_merkle::vectors::{leaves, SIZES};
use caravel_merkle::{
    depth, node, proof, root, verify, MerkleError, MerkleTree, NativeSha256, Sha256, SorobanSha256,
    ZERO,
};
use proptest::prelude::*;

/// A deliberately different implementation: recursive halving of the padded
/// leaf list, straight from the §9.9 prose.
fn reference_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    fn rec(level: &[[u8; 32]]) -> [u8; 32] {
        if level.len() == 1 {
            return level[0];
        }
        let (l, r) = level.split_at(level.len() / 2);
        node(&NativeSha256, &rec(l), &rec(r))
    }
    if leaves.is_empty() {
        return ZERO;
    }
    let mut padded = leaves.to_vec();
    padded.resize(leaves.len().next_power_of_two(), ZERO);
    rec(&padded)
}

#[test]
fn committed_vectors_are_current() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-vectors/merkle.json");
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing {}: run `cargo gen-vectors`", path.display()));
    assert!(
        on_disk == caravel_merkle::vectors::file(),
        "merkle.json is stale: run `cargo gen-vectors`"
    );
}

#[test]
fn required_sizes_match_the_reference() {
    for n in SIZES {
        let ls = leaves(n);
        assert_eq!(root(&NativeSha256, &ls), Ok(reference_root(&ls)), "n = {n}");
    }
}

#[test]
fn edge_shapes() {
    let h = NativeSha256;
    assert_eq!(root(&h, &[]), Ok(ZERO));
    let one = leaves(1);
    assert_eq!(root(&h, &one), Ok(one[0]));
    assert_eq!(proof(&h, &one, 0), Ok(vec![]));
    assert!(verify(&h, &one[0], 0, 1, &[], &one[0]));
    assert_eq!(depth(0), 0);
    assert_eq!(depth(1), 0);
    assert_eq!(depth(2), 1);
    assert_eq!(depth(3), 2);
    assert_eq!(depth(5), 3);
    assert_eq!(depth(1024), 10);
    assert_eq!(depth(1025), 11);
    assert_eq!(proof(&h, &leaves(3), 3), Err(MerkleError::IndexOutOfRange));
    // Padding leaves are zero, not hashed: n = 3 pairs leaf 2 with ZERO.
    let three = leaves(3);
    assert_eq!(
        root(&h, &three).unwrap(),
        node(
            &h,
            &node(&h, &three[0], &three[1]),
            &node(&h, &three[2], &ZERO)
        )
    );
}

#[test]
fn a_leaf_count_above_2_pow_20_is_rejected() {
    // Building needs 1M+ leaves; verification rejects n > MAX_LEAVES cheaply.
    let h = NativeSha256;
    let leaf = [1u8; 32];
    assert!(!verify(
        &h,
        &leaf,
        0,
        caravel_merkle::MAX_LEAVES + 1,
        &[[0; 32]; 21],
        &leaf
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn every_proof_verifies_and_matches_the_reference(n in 1u32..70, pick in any::<u32>()) {
        let h = NativeSha256;
        let ls = leaves(n);
        let tree = MerkleTree::build(&h, &ls).unwrap();
        prop_assert_eq!(tree.root(), reference_root(&ls));
        let i = pick % n;
        let p = tree.proof(i).unwrap();
        prop_assert_eq!(p.len(), depth(n) as usize);
        prop_assert!(verify(&h, &ls[i as usize], i, n, &p, &tree.root()));
        // Any other index with the same proof fails, unless it is the same leaf value.
        let j = (i + 1) % n;
        if j != i {
            prop_assert!(!verify(&h, &ls[i as usize], j, n, &p, &tree.root()));
        }
    }

    #[test]
    fn flipping_any_bit_breaks_the_proof(n in 2u32..40, pick in any::<u32>(), bit in 0usize..256, which in any::<u8>()) {
        let h = NativeSha256;
        let ls = leaves(n);
        let tree = MerkleTree::build(&h, &ls).unwrap();
        let i = pick % n;
        let mut p = tree.proof(i).unwrap();
        let k = usize::from(which) % p.len();
        p[k][bit / 8] ^= 1 << (bit % 8);
        prop_assert!(!verify(&h, &ls[i as usize], i, n, &p, &tree.root()));
    }
}

mod contract {
    //! A test contract that verifies proofs and builds roots with the Soroban
    //! host hasher, compared against the native hasher.
    use caravel_merkle::{verify, SorobanSha256};
    use soroban_sdk::{contract, contractimpl, BytesN, Env, Vec};

    #[contract]
    pub struct MerkleProbe;

    #[contractimpl]
    impl MerkleProbe {
        pub fn verify(
            env: Env,
            leaf: BytesN<32>,
            index: u32,
            n: u32,
            siblings: Vec<BytesN<32>>,
            root: BytesN<32>,
        ) -> bool {
            let sib: std::vec::Vec<[u8; 32]> = siblings.iter().map(|s| s.to_array()).collect();
            verify(
                &SorobanSha256(&env),
                &leaf.to_array(),
                index,
                n,
                &sib,
                &root.to_array(),
            )
        }

        pub fn root(env: Env, leaves: Vec<BytesN<32>>) -> BytesN<32> {
            let ls: std::vec::Vec<[u8; 32]> = leaves.iter().map(|l| l.to_array()).collect();
            BytesN::from_array(
                &env,
                &caravel_merkle::root(&SorobanSha256(&env), &ls).unwrap(),
            )
        }
    }
}

#[test]
fn soroban_hasher_agrees_with_native_in_a_contract() {
    use soroban_sdk::{BytesN, Env, Vec};
    let env = Env::default();
    let id = env.register(contract::MerkleProbe, ());
    let client = contract::MerkleProbeClient::new(&env, &id);

    assert_eq!(SorobanSha256(&env).hash(b"abc"), NativeSha256.hash(b"abc"));
    for n in [1u32, 2, 3, 5, 8] {
        let ls = leaves(n);
        let native_root = root(&NativeSha256, &ls).unwrap();
        let soroban_leaves = Vec::from_iter(&env, ls.iter().map(|l| BytesN::from_array(&env, l)));
        assert_eq!(
            client.root(&soroban_leaves).to_array(),
            native_root,
            "root n = {n}"
        );
        for i in 0..n {
            let p = proof(&NativeSha256, &ls, i).unwrap();
            let sib = Vec::from_iter(&env, p.iter().map(|s| BytesN::from_array(&env, s)));
            let leaf = BytesN::from_array(&env, &ls[i as usize]);
            assert!(client.verify(&leaf, &i, &n, &sib, &BytesN::from_array(&env, &native_root)));
            let mut wrong = ls[i as usize];
            wrong[0] ^= 1;
            assert!(!client.verify(
                &BytesN::from_array(&env, &wrong),
                &i,
                &n,
                &sib,
                &BytesN::from_array(&env, &native_root)
            ));
        }
    }
}
