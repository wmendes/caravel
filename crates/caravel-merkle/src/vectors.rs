//! Golden vectors for `test-vectors/merkle.json` (spec §9.9, §19.2, T-002).
//!
//! Leaves are `H("caravel-merkle-vector" || i u32)`. The file follows the shape
//! in §19.2: `hex` is the concatenated leaves and `hash` the root.

use std::format;
use std::string::String;
use std::vec::Vec;

use crate::{node, verify, MerkleTree, NativeSha256, Sha256};

/// Leaf counts required by T-002.
pub const SIZES: [u32; 7] = [0, 1, 2, 3, 5, 8, 1024];

pub fn leaf(i: u32) -> [u8; 32] {
    let mut pre = b"caravel-merkle-vector".to_vec();
    pre.extend_from_slice(&i.to_le_bytes());
    NativeSha256.hash(&pre)
}

pub fn leaves(n: u32) -> Vec<[u8; 32]> {
    (0..n).map(leaf).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_list(items: &[[u8; 32]], indent: &str) -> String {
    if items.is_empty() {
        return String::from("[]");
    }
    let body: Vec<String> = items
        .iter()
        .map(|x| format!("{indent}  \"{}\"", hex(x)))
        .collect();
    format!("[\n{}\n{indent}]", body.join(",\n"))
}

/// Indexes whose proofs are listed: all of them for small trees, a sample for 1024.
fn proof_indexes(n: u32) -> Vec<u32> {
    if n <= 8 {
        (0..n).collect()
    } else {
        std::vec![0, 1, 2, 511, 512, n - 2, n - 1]
    }
}

fn tree_vector(n: u32) -> String {
    let h = NativeSha256;
    let ls = leaves(n);
    let tree = MerkleTree::build(&h, &ls).expect("vector tree builds");
    let root = tree.root();
    let proofs: Vec<String> = proof_indexes(n)
        .into_iter()
        .map(|i| {
            let p = tree.proof(i).expect("index in range");
            assert!(verify(&h, &ls[i as usize], i, n, &p, &root));
            format!(
                "          {{\n            \"index\": \"{i}\",\n            \"leaf\": \"{}\",\n            \"siblings\": {}\n          }}",
                hex(&ls[i as usize]),
                hex_list(&p, "            ")
            )
        })
        .collect();
    let proofs = if proofs.is_empty() {
        String::from("[]")
    } else {
        format!("[\n{}\n        ]", proofs.join(",\n"))
    };
    let concat: Vec<u8> = ls.iter().flatten().copied().collect();
    format!(
        "    {{\n      \"name\": \"n_{n}\",\n      \"fields\": {{\n        \"n\": \"{n}\",\n        \"depth\": \"{}\",\n        \"proofs\": {proofs}\n      }},\n      \"hex\": \"{}\",\n      \"hash\": \"{}\"\n    }}",
        crate::depth(n),
        hex(&concat),
        hex(&root)
    )
}

struct Bad {
    name: &'static str,
    n: u32,
    index: u32,
    leaf: [u8; 32],
    siblings: Vec<[u8; 32]>,
    root: [u8; 32],
}

fn invalid_cases() -> Vec<Bad> {
    let h = NativeSha256;
    let ls = leaves(5);
    let tree = MerkleTree::build(&h, &ls).unwrap();
    let root = tree.root();
    let p = tree.proof(3).unwrap();
    let mut tampered = p.clone();
    tampered[1][0] ^= 0x01;
    let mut extra = p.clone();
    extra.push([0; 32]);
    let short = p[..p.len() - 1].to_vec();
    let mut wrong_root = root;
    wrong_root[31] ^= 0x80;
    // A node hashed as if it were a leaf: the level-1 node over leaves 2 and 3
    // with a level-1 proof. Its length is wrong for n = 5, so it must fail.
    let inner = node(&h, &ls[2], &ls[3]);
    std::vec![
        Bad {
            name: "tampered_sibling",
            n: 5,
            index: 3,
            leaf: ls[3],
            siblings: tampered,
            root
        },
        Bad {
            name: "wrong_index",
            n: 5,
            index: 2,
            leaf: ls[3],
            siblings: p.clone(),
            root
        },
        Bad {
            name: "extra_sibling",
            n: 5,
            index: 3,
            leaf: ls[3],
            siblings: extra,
            root
        },
        Bad {
            name: "missing_sibling",
            n: 5,
            index: 3,
            leaf: ls[3],
            siblings: short,
            root
        },
        Bad {
            name: "index_equals_n",
            n: 5,
            index: 5,
            leaf: ls[3],
            siblings: p.clone(),
            root
        },
        Bad {
            name: "wrong_root",
            n: 5,
            index: 3,
            leaf: ls[3],
            siblings: p.clone(),
            root: wrong_root
        },
        Bad {
            name: "empty_tree",
            n: 0,
            index: 0,
            leaf: [0; 32],
            siblings: std::vec![],
            root: [0; 32]
        },
        Bad {
            name: "inner_node_as_leaf",
            n: 5,
            index: 1,
            leaf: inner,
            siblings: p[1..].to_vec(),
            root
        },
    ]
}

fn invalid_json() -> String {
    let h = NativeSha256;
    let items: Vec<String> = invalid_cases()
        .into_iter()
        .map(|b| {
            assert!(!verify(&h, &b.leaf, b.index, b.n, &b.siblings, &b.root), "{} must fail", b.name);
            format!(
                "    {{\n      \"name\": \"{}\",\n      \"n\": \"{}\",\n      \"index\": \"{}\",\n      \"leaf\": \"{}\",\n      \"siblings\": {},\n      \"root\": \"{}\",\n      \"error\": \"verify returns false\"\n    }}",
                b.name,
                b.n,
                b.index,
                hex(&b.leaf),
                hex_list(&b.siblings, "      "),
                hex(&b.root)
            )
        })
        .collect();
    format!("[\n{}\n  ]", items.join(",\n"))
}

/// The contents of `test-vectors/merkle.json`.
pub fn file() -> String {
    let vectors: Vec<String> = SIZES.iter().map(|n| tree_vector(*n)).collect();
    format!(
        "{{\n  \"format\": \"Merkle tree\",\n  \"spec\": \"§9.9\",\n  \"hash_rule\": \"hash = root. hex = the n leaves concatenated. node = H(0x01 || left || right); padding leaves are 32 zero bytes (not hashed); n = 0 gives the zero root; n = 1 gives the leaf itself.\",\n  \"context\": {{\n    \"leaf_rule\": \"leaf_i = H(\\\"caravel-merkle-vector\\\" || i u32 LE)\"\n  }},\n  \"vectors\": [\n{}\n  ],\n  \"invalid\": {}\n}}\n",
        vectors.join(",\n"),
        invalid_json()
    )
}
