//! Withdrawal claims (spec §13.5).

use super::*;

/// A and B deposit; A withdraws 30 USDC and B 20 USDC; checkpoint 1 is accepted.
fn two_withdrawals() -> (Harness, Checkpoint) {
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    h.deposit(B, 50 * USDC);
    h.sync_inbox();
    h.lane.block();
    h.lane.withdraw(A, 30 * USDC);
    h.lane.withdraw(B, 20 * USDC);
    h.lane.block();
    h.lane.all_ok();
    let cp = h.checkpoint();
    assert_eq!(cp.withdrawals.len(), 2);
    h.accept(&cp);
    (h, cp)
}

fn claim(
    h: &Harness,
    cp: &Checkpoint,
    recipient: &Address,
    seed: u8,
    index: u32,
    amount: i128,
    proof: &Vec<BytesN<32>>,
) -> Result<(), soroban_sdk::Error> {
    let r = h.c().try_claim_withdrawal(
        recipient,
        &key32(&h.env, seed),
        &cp.header.seq,
        &index,
        &amount,
        proof,
    );
    match r {
        Ok(_) => Ok(()),
        Err(Ok(e)) => Err(e),
        Err(Err(e)) => panic!("invoke error {e:?}"),
    }
}

fn index_of(cp: &Checkpoint, seed: u8) -> u32 {
    cp.withdrawals
        .iter()
        .position(|(k, _)| *k == caravel_harness::pk(seed))
        .unwrap() as u32
}

#[test]
fn both_claims_pay_their_owners() {
    let (h, cp) = two_withdrawals();
    for (seed, amount) in [(A, 30 * USDC), (B, 20 * USDC)] {
        let i = index_of(&cp, seed);
        let who = h.user(seed, 0);
        claim(&h, &cp, &who, seed, i, amount, &h.withdrawal_proof(&cp, i)).unwrap();
        assert_eq!(h.token().balance(&who), amount);
    }
    assert_eq!(h.token().balance(&h.id), 100 * USDC);
    assert_eq!(h.c().totals(), (50 * USDC, 50 * USDC));
}

#[test]
fn anyone_may_submit_a_claim() {
    let (h, cp) = two_withdrawals();
    let i = index_of(&cp, A);
    let who = h.user(A, 0);
    // No authorization at all: the funds can only go to A.
    h.env.set_auths(&[]);
    claim(&h, &cp, &who, A, i, 30 * USDC, &h.withdrawal_proof(&cp, i)).unwrap();
    assert_eq!(h.token().balance(&who), 30 * USDC);
}

#[test]
fn double_claim_rejected() {
    let (h, cp) = two_withdrawals();
    let i = index_of(&cp, A);
    let who = h.user(A, 0);
    let proof = h.withdrawal_proof(&cp, i);
    claim(&h, &cp, &who, A, i, 30 * USDC, &proof).unwrap();
    assert_eq!(
        claim(&h, &cp, &who, A, i, 30 * USDC, &proof),
        Err(contract_error(Error::AlreadyClaimed))
    );
    assert_eq!(h.token().balance(&who), 30 * USDC);
}

#[test]
fn wrong_recipient_rejected() {
    let (h, cp) = two_withdrawals();
    let i = index_of(&cp, A);
    let proof = h.withdrawal_proof(&cp, i);
    // B's account, and a contract address.
    let b = h.user(B, 0);
    assert_eq!(
        claim(&h, &cp, &b, A, i, 30 * USDC, &proof),
        Err(contract_error(Error::WrongRecipient))
    );
    let contract = Address::generate(&h.env);
    assert_eq!(
        claim(&h, &cp, &contract, A, i, 30 * USDC, &proof),
        Err(contract_error(Error::WrongRecipient))
    );
}

#[test]
fn proof_of_wrong_length_rejected() {
    let (h, cp) = two_withdrawals();
    let i = index_of(&cp, A);
    let who = h.user(A, 0);
    let proof = h.withdrawal_proof(&cp, i);
    assert_eq!(proof.len(), 1);
    let mut long = proof.clone();
    long.push_back(BytesN::from_array(&h.env, &[0; 32]));
    assert_eq!(
        claim(&h, &cp, &who, A, i, 30 * USDC, &long),
        Err(contract_error(Error::BadProof))
    );
    assert_eq!(
        claim(&h, &cp, &who, A, i, 30 * USDC, &Vec::new(&h.env)),
        Err(contract_error(Error::BadProof))
    );
}

#[test]
fn wrong_leaf_rejected() {
    let (h, cp) = two_withdrawals();
    let i = index_of(&cp, A);
    let who = h.user(A, 0);
    let proof = h.withdrawal_proof(&cp, i);
    // Amount, index or sibling changed.
    assert_eq!(
        claim(&h, &cp, &who, A, i, 30 * USDC + 1, &proof),
        Err(contract_error(Error::BadProof))
    );
    assert_eq!(
        claim(&h, &cp, &who, A, 1 - i, 30 * USDC, &proof),
        Err(contract_error(Error::BadProof))
    );
    let mut tampered = proof.clone();
    let mut s = tampered.get(0).unwrap().to_array();
    s[0] ^= 1;
    tampered.set(0, BytesN::from_array(&h.env, &s));
    assert_eq!(
        claim(&h, &cp, &who, A, i, 30 * USDC, &tampered),
        Err(contract_error(Error::BadProof))
    );
    // B's leaf, sent to A.
    let j = index_of(&cp, B);
    assert_eq!(
        claim(&h, &cp, &who, A, j, 20 * USDC, &h.withdrawal_proof(&cp, j)),
        Err(contract_error(Error::BadProof))
    );
}

#[test]
fn unknown_checkpoint_and_index_rejected() {
    let (h, cp) = two_withdrawals();
    let who = h.user(A, 0);
    let proof = h.withdrawal_proof(&cp, 0);
    let r = h
        .c()
        .try_claim_withdrawal(&who, &key32(&h.env, A), &2, &0, &(30 * USDC), &proof);
    expect_err(r, Error::UnknownCheckpoint);
    assert_eq!(
        claim(&h, &cp, &who, A, 2, 30 * USDC, &proof),
        Err(contract_error(Error::BadIndex))
    );
}

#[test]
fn older_checkpoints_stay_claimable() {
    let (mut h, cp1) = two_withdrawals();
    h.lane.block();
    let cp2 = h.checkpoint();
    h.accept(&cp2);
    assert_eq!(h.c().last_checkpoint().seq, 2);
    let i = index_of(&cp1, A);
    let who = h.user(A, 0);
    claim(
        &h,
        &cp1,
        &who,
        A,
        i,
        30 * USDC,
        &h.withdrawal_proof(&cp1, i),
    )
    .unwrap();
    assert_eq!(h.token().balance(&who), 30 * USDC);
}
