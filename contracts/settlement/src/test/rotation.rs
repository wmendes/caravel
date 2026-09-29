//! Signer sets and rotation (spec §13.4), including the testnet-only admin
//! rotation (spec §4.3).

use caravel_types::preimage::{
    rotate_message_preimage, signers_hash_preimage, WeightedSigner as RawSigner,
};
use soroban_sdk::testutils::Events as _;
use soroban_sdk::Event as _;

use super::*;

/// Validators `3k .. 3k + 3`, as a 2-of-3 set.
fn set(h: &Harness, k: u8) -> (WeightedSigners, StdVec<SigningKey>) {
    let keys: StdVec<SigningKey> = (3 * k..3 * k + 3).map(validator).collect();
    signer_set(&h.env, &keys, 2)
}

/// Computed here, independently of the contract.
fn signers_hash(set: &WeightedSigners) -> [u8; 32] {
    let raw: StdVec<RawSigner> = set
        .signers
        .iter()
        .map(|s| RawSigner {
            key: s.key.to_array(),
            weight: s.weight,
        })
        .collect();
    sha256(&signers_hash_preimage(&raw, set.threshold).unwrap())
}

fn rotate_msg(h: &Harness, new_epoch: u64, new: &WeightedSigners) -> StdVec<u8> {
    let addr_hash = sha256(&h.id.clone().to_xdr(&h.env).to_alloc_vec());
    let network = h.env.ledger().network_id().to_array();
    rotate_message_preimage(
        &caravel_testkit::lane::config().lane_id,
        &network,
        &addr_hash,
        new_epoch,
        &signers_hash(new),
    )
    .to_vec()
}

/// Rotates from `epoch` (signed by `keys`) to `new`. `Err(None)` is a trap
/// that is not a contract error, such as a failed signature check.
fn rotate(
    h: &Harness,
    epoch: u64,
    keys: &[SigningKey],
    new: &WeightedSigners,
) -> Result<(), Option<soroban_sdk::Error>> {
    let sigs = sign_with(&h.env, keys, &rotate_msg(h, epoch + 1, new), &[0, 2]);
    h.c()
        .try_rotate_signers(new, &epoch, &sigs)
        .map(|_| ())
        .map_err(|e| code_of::<()>(Err(e)))
}

/// The next checkpoint on a lane with one more block.
fn next_checkpoint(h: &mut Harness) -> Checkpoint {
    h.lane.block();
    h.checkpoint()
}

#[test]
fn rotation_needs_the_delay() {
    let h = Harness::new();
    let (set2, _) = set(&h, 1);
    h.advance(PARAMS.min_rotation_delay_secs - 1);
    assert_eq!(
        rotate(&h, 1, &h.validators, &set2),
        Err(Some(contract_error(Error::RotationTooSoon)))
    );
    h.advance(1);
    rotate(&h, 1, &h.validators, &set2).unwrap();
    let event = SignersRotatedEvent {
        epoch: 2,
        signers_hash: BytesN::from_array(&h.env, &signers_hash(&set2)),
    };
    assert_eq!(
        h.env
            .events()
            .all()
            .filter_by_contract(&h.id)
            .events()
            .last(),
        Some(&event.to_xdr(&h.env, &h.id))
    );
    assert_eq!(h.c().epoch(), 2);
    assert_eq!(h.c().signers(&2), Some(set2.clone()));
    // The delay starts again from this rotation.
    let (set3, _) = set(&h, 2);
    let (_, keys2) = set(&h, 1);
    h.advance(PARAMS.min_rotation_delay_secs - 1);
    assert_eq!(
        rotate(&h, 2, &keys2, &set3),
        Err(Some(contract_error(Error::RotationTooSoon)))
    );
}

#[test]
fn rotation_wrong_epoch_rejected() {
    let h = Harness::new();
    let (set2, _) = set(&h, 1);
    h.advance(PARAMS.min_rotation_delay_secs);
    assert_eq!(
        rotate(&h, 0, &h.validators, &set2),
        Err(Some(contract_error(Error::BadEpoch)))
    );
    assert_eq!(
        rotate(&h, 2, &h.validators, &set2),
        Err(Some(contract_error(Error::BadEpoch)))
    );
}

#[test]
fn rotation_needs_the_current_set() {
    let h = Harness::new();
    let (set2, keys2) = set(&h, 1);
    h.advance(PARAMS.min_rotation_delay_secs);
    // The new set cannot sign itself in: its signatures fail verification.
    assert_eq!(rotate(&h, 1, &keys2, &set2), Err(None));
    // One signature is below the threshold.
    let one = sign_with(&h.env, &h.validators, &rotate_msg(&h, 2, &set2), &[1]);
    expect_err(
        h.c().try_rotate_signers(&set2, &1, &one),
        Error::BelowThreshold,
    );
    // A signature over the message for another epoch does not verify.
    let wrong = sign_with(&h.env, &h.validators, &rotate_msg(&h, 3, &set2), &[0, 2]);
    let r = h.c().try_rotate_signers(&set2, &1, &wrong);
    assert!(r.is_err() && code_of(r).is_none());
    assert_eq!(h.c().epoch(), 1);
}

#[test]
fn a_set_used_before_cannot_come_back() {
    let h = Harness::new();
    let (set1, _) = signer_set(&h.env, &h.validators, 2);
    let (set2, keys2) = set(&h, 1);
    h.advance(PARAMS.min_rotation_delay_secs);
    rotate(&h, 1, &h.validators, &set2).unwrap();
    h.advance(PARAMS.min_rotation_delay_secs);
    assert_eq!(
        rotate(&h, 2, &keys2, &set1),
        Err(Some(contract_error(Error::SignersReused)))
    );
    expect_err(h.c().try_admin_rotate_signers(&set1), Error::SignersReused);
    expect_err(h.c().try_admin_rotate_signers(&set2), Error::SignersReused);
}

#[test]
fn invalid_sets_rejected() {
    let h = Harness::new();
    let e = &h.env;
    let signer = |seed: u8, weight: u32| WeightedSigner {
        key: BytesN::from_array(e, &raw_pk(&validator(seed))),
        weight,
    };
    let sorted = |mut v: StdVec<WeightedSigner>| {
        v.sort_by_key(|s| s.key.to_array());
        v
    };
    let make = |signers: StdVec<WeightedSigner>, threshold: u32| WeightedSigners {
        signers: Vec::from_slice(e, &signers),
        threshold,
    };
    let good = sorted(std::vec![signer(20, 1), signer(21, 1), signer(22, 1)]);
    let mut reversed = good.clone();
    reversed.reverse();
    let many = sorted((0..33).map(|i| signer(100 + i, 1)).collect());
    for bad in [
        make(std::vec![], 1),
        make(reversed, 2),
        make(std::vec![good[0].clone(), good[0].clone()], 1),
        make(
            std::vec![
                good[0].clone(),
                WeightedSigner {
                    weight: 0,
                    ..good[1].clone()
                }
            ],
            1,
        ),
        make(good.clone(), 0),
        make(good.clone(), 4),
        make(many, 17),
    ] {
        expect_err(h.c().try_admin_rotate_signers(&bad), Error::BadSignerSet);
    }
    // 32 signers is the limit.
    let most = sorted((0..32).map(|i| signer(100 + i, 1)).collect());
    h.c().admin_rotate_signers(&make(most, 17));
}

#[test]
fn old_epochs_within_retention_are_accepted() {
    let mut h = Harness::new();
    let keys1 = h.validators.clone();
    let (set2, keys2) = set(&h, 1);
    let (set3, keys3) = set(&h, 2);
    let (set4, _) = set(&h, 3);
    for (epoch, keys, new) in [(1, &keys1, &set2), (2, &keys2, &set3)] {
        h.advance(PARAMS.min_rotation_delay_secs);
        rotate(&h, epoch, keys, new).unwrap();
    }
    assert_eq!(h.c().epoch(), 3);
    // Epoch 3 − 1 = 2 ≤ retention: the first set still signs checkpoints.
    let cp = next_checkpoint(&mut h);
    let sigs = sign_with(&h.env, &keys1, &cp.header.encode(), &[0, 2]);
    h.accept_with(&cp, 1, &sigs);

    h.advance(PARAMS.min_rotation_delay_secs);
    rotate(&h, 3, &keys3, &set4).unwrap();
    let cp = next_checkpoint(&mut h);
    let sigs = sign_with(&h.env, &keys1, &cp.header.encode(), &[0, 2]);
    expect_err(h.try_submit(&cp, 1, &sigs), Error::BadEpoch);
    // Signatures from one set under another epoch do not verify.
    let r = h.try_submit(&cp, 2, &sigs);
    assert!(r.is_err() && code_of(r).is_none());
    let sigs = sign_with(&h.env, &keys2, &cp.header.encode(), &[0, 2]);
    h.accept_with(&cp, 2, &sigs);
}

#[test]
fn admin_rotation_invalidates_older_sets_at_once() {
    let mut h = Harness::new();
    let (set2, keys2) = set(&h, 1);
    // No delay and no validator signatures: testnet only.
    h.c().admin_rotate_signers(&set2);
    let event = AdminRotateEvent {
        epoch: 2,
        signers_hash: BytesN::from_array(&h.env, &signers_hash(&set2)),
    };
    assert_eq!(
        h.env
            .events()
            .all()
            .filter_by_contract(&h.id)
            .events()
            .last(),
        Some(&event.to_xdr(&h.env, &h.id))
    );
    assert_eq!((h.c().epoch(), h.c().min_valid_epoch()), (2, 2));
    let cp = next_checkpoint(&mut h);
    expect_err(
        h.try_submit(&cp, 1, &h.sign(&cp.header, &[0, 2])),
        Error::BadEpoch,
    );
    let sigs = sign_with(&h.env, &keys2, &cp.header.encode(), &[0, 2]);
    h.accept_with(&cp, 2, &sigs);
}

#[test]
fn admin_rotation_needs_the_admin() {
    let h = Harness::new();
    let (set2, _) = set(&h, 1);
    h.env.set_auths(&[]);
    let r = h.c().try_admin_rotate_signers(&set2);
    assert!(r.is_err() && code_of(r).is_none(), "no admin authorization");
    assert_eq!(h.c().epoch(), 1);
    h.env.mock_all_auths();
    h.c().admin_rotate_signers(&set2);
    assert_eq!(
        h.env.auths().first().map(|(who, _)| who.clone()),
        Some(h.admin.clone())
    );
}

#[test]
fn weights_count_toward_the_threshold() {
    let mut h = Harness::new();
    let (mut weighted, keys) = set(&h, 1);
    let mut heavy = weighted.signers.get(0).unwrap();
    heavy.weight = 3;
    weighted.signers.set(0, heavy);
    weighted.threshold = 3;
    h.c().admin_rotate_signers(&weighted);
    let cp = next_checkpoint(&mut h);
    let encoded = cp.header.encode();
    expect_err(
        h.try_submit(&cp, 2, &sign_with(&h.env, &keys, &encoded, &[1, 2])),
        Error::BelowThreshold,
    );
    h.accept_with(&cp, 2, &sign_with(&h.env, &keys, &encoded, &[0]));
}
