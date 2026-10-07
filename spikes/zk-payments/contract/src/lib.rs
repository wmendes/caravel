//! Spike: a checkpoint accepted on a validity proof instead of signatures.
//! The guest's journal is `H(prev_header) ‖ H(header)`; the contract knows
//! the first half (its last header hash) and computes the second, so a proof
//! for any other transition doesn't verify.
#![no_std]

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, panic_with_error, Address,
    Bytes, BytesN, Env,
};

const HEADER_LEN: u32 = 442;
const PREV_HEADER_HASH: u32 = 146; // offsets in CheckpointHeaderV1 (spec §9.8)
const BATCH_HASH: u32 = 234;

#[contractclient(name = "VerifierClient")]
pub trait Verifier {
    fn verify(env: Env, seal: Bytes, image_id: BytesN<32>, journal: BytesN<32>);
}

#[contracttype]
enum Key {
    Verifier,
    ImageId,
    Last,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    BadHeaderLength = 1,
    BadBatchHash = 2,
    BadPrevHeader = 3,
}

#[contract]
pub struct ProvenCheckpoint;

fn field(env: &Env, header: &Bytes, at: u32) -> BytesN<32> {
    let mut out = [0u8; 32];
    header.slice(at..at + 32).copy_into_slice(&mut out);
    BytesN::from_array(env, &out)
}

#[contractimpl]
impl ProvenCheckpoint {
    pub fn __constructor(env: Env, verifier: Address, image_id: BytesN<32>, last: BytesN<32>) {
        let s = env.storage().instance();
        s.set(&Key::Verifier, &verifier);
        s.set(&Key::ImageId, &image_id);
        s.set(&Key::Last, &last);
    }

    pub fn last(env: Env) -> BytesN<32> {
        env.storage().instance().get(&Key::Last).unwrap()
    }

    pub fn submit(env: Env, header: Bytes, batch: Bytes, seal: Bytes) -> BytesN<32> {
        if header.len() != HEADER_LEN {
            panic_with_error!(&env, Error::BadHeaderLength);
        }
        let s = env.storage().instance();
        let last: BytesN<32> = s.get(&Key::Last).unwrap();
        // Checks 4 and 5 as today: the chain link and the batch's hash.
        if field(&env, &header, PREV_HEADER_HASH) != last {
            panic_with_error!(&env, Error::BadPrevHeader);
        }
        let batch_hash: BytesN<32> = env.crypto().sha256(&batch).into();
        if field(&env, &header, BATCH_HASH) != batch_hash {
            panic_with_error!(&env, Error::BadBatchHash);
        }
        // Check 7, as a proof: the guest ran the engine from `last` to this header.
        let header_hash: BytesN<32> = env.crypto().sha256(&header).into();
        let mut journal = Bytes::from_array(&env, &last.to_array());
        journal.append(&Bytes::from_array(&env, &header_hash.to_array()));
        let digest: BytesN<32> = env.crypto().sha256(&journal).into();
        let verifier: Address = s.get(&Key::Verifier).unwrap();
        let image_id: BytesN<32> = s.get(&Key::ImageId).unwrap();
        VerifierClient::new(&env, &verifier).verify(&seal, &image_id, &digest);
        s.set(&Key::Last, &header_hash);
        header_hash
    }
}
