//! Property tests: every codec round-trips, and every decoder rejects garbage
//! without panicking (spec §8.1 INV-D6, §19.4).

use caravel_types::batch::BatchV1;
use caravel_types::block::{BlockInputV1, BlockRecordV1, Entry};
use caravel_types::checkpoint::CheckpointHeaderV1;
use caravel_types::codes;
use caravel_types::config::{AccessMode, GenesisConfigV1, MarketParamsV1};
use caravel_types::inbox::{InboxKind, InboxMsgV1};
use caravel_types::oracle::OracleUpdateV1;
use caravel_types::receipts::{CancelReason, DepositOutcome, Event, Receipt, Receipts};
use caravel_types::state::{
    AccountV1, CommitmentV1, MarketStateV1, OrderV1, PendingWithdrawalV1, PositionV1, SessionKeyV1,
    StateV1,
};
use caravel_types::step::StepEnvelope;
use caravel_types::tx::{LaneTxV1, PlaceOrder, Side, SigScheme, Tif, TxBody};
use proptest::collection::vec;
use proptest::prelude::*;

fn b32() -> impl Strategy<Value = [u8; 32]> {
    any::<[u8; 32]>()
}

fn side() -> impl Strategy<Value = Side> {
    prop_oneof![Just(Side::Buy), Just(Side::Sell)]
}

fn body() -> impl Strategy<Value = TxBody> {
    prop_oneof![
        (
            any::<u16>(),
            side(),
            prop_oneof![Just(Tif::Gtc), Just(Tif::Ioc), Just(Tif::PostOnly)],
            any::<bool>(),
            any::<i64>(),
            any::<i64>(),
            any::<u64>()
        )
            .prop_map(
                |(market_id, side, tif, reduce_only, price, lots, client_order_id)| {
                    TxBody::PlaceOrder(PlaceOrder {
                        market_id,
                        side,
                        tif,
                        reduce_only,
                        price,
                        lots,
                        client_order_id,
                    })
                }
            ),
        (any::<u16>(), any::<u64>()).prop_map(|(market_id, order_id)| TxBody::CancelOrder {
            market_id,
            order_id
        }),
        any::<u16>().prop_map(|market_id| TxBody::CancelAll { market_id }),
        any::<i128>().prop_map(|amount| TxBody::Withdraw { amount }),
        (b32(), any::<u64>(), any::<u8>()).prop_map(|(session_key, expires_at_ms, permissions)| {
            TxBody::AddSessionKey {
                session_key,
                expires_at_ms,
                permissions,
            }
        }),
        b32().prop_map(|session_key| TxBody::RevokeSessionKey { session_key }),
    ]
}

fn lane_tx() -> impl Strategy<Value = LaneTxV1> {
    (
        b32(),
        b32(),
        b32(),
        any::<u64>(),
        any::<u64>(),
        any::<bool>(),
        body(),
        any::<[u8; 64]>(),
    )
        .prop_map(
            |(lane_id, account, signer, nonce, expiry_ms, sep53, body, signature)| LaneTxV1 {
                lane_id,
                account,
                signer,
                nonce,
                expiry_ms,
                sig_scheme: if sep53 {
                    SigScheme::Sep53
                } else {
                    SigScheme::RawEd25519
                },
                body,
                signature,
            },
        )
}

fn inbox() -> impl Strategy<Value = InboxMsgV1> {
    (
        any::<bool>(),
        any::<u64>(),
        b32(),
        any::<i128>(),
        any::<u64>(),
    )
        .prop_map(
            |(fw, index, lane_account, amount, enqueued_at)| InboxMsgV1 {
                kind: if fw {
                    InboxKind::ForcedWithdrawal
                } else {
                    InboxKind::Deposit
                },
                index,
                lane_account,
                amount,
                enqueued_at,
            },
        )
}

fn oracle() -> impl Strategy<Value = OracleUpdateV1> {
    (
        any::<u16>(),
        any::<i64>(),
        any::<u64>(),
        b32(),
        any::<[u8; 64]>(),
    )
        .prop_map(
            |(market_id, price, publish_time_ms, oracle_key, signature)| OracleUpdateV1 {
                market_id,
                price,
                publish_time_ms,
                oracle_key,
                signature,
            },
        )
}

fn entry() -> impl Strategy<Value = Entry> {
    prop_oneof![
        inbox().prop_map(Entry::Inbox),
        oracle().prop_map(Entry::Oracle),
        lane_tx().prop_map(Entry::User)
    ]
}

fn block() -> impl Strategy<Value = BlockInputV1> {
    (
        b32(),
        any::<u64>(),
        any::<u64>(),
        b32(),
        any::<bool>(),
        vec(entry(), 0..8),
    )
        .prop_map(
            |(lane_id, height, timestamp_ms, prev_block_hash, checkpoint_end, entries)| {
                BlockInputV1 {
                    lane_id,
                    height,
                    timestamp_ms,
                    prev_block_hash,
                    checkpoint_end,
                    entries,
                }
            },
        )
}

fn market_params() -> impl Strategy<Value = MarketParamsV1> {
    (
        any::<u16>(),
        any::<[u8; 16]>(),
        any::<i64>(),
        any::<[u16; 6]>(),
        any::<[i64; 4]>(),
        any::<u8>(),
    )
        .prop_map(|(market_id, symbol, tick, bps, lots, dec)| MarketParamsV1 {
            market_id,
            symbol,
            tick,
            imf_bps: bps[0],
            mmf_bps: bps[1],
            taker_fee_bps: bps[2],
            maker_fee_bps: bps[3],
            liq_fee_bps: bps[4],
            band_bps: bps[5],
            max_position_lots: lots[0],
            max_oi_lots: lots[1],
            impact_lots: lots[2],
            display_lot_base_units: lots[3],
            display_base_decimals: dec,
        })
}

prop_compose! {
    fn config()(
        keys in any::<[[u8; 32]; 3]>(),
        allow in any::<bool>(),
        allowlist in vec(b32(), 0..4),
        oracle_keys in vec(b32(), 0..4),
        u64s in any::<[u64; 5]>(),
        u16s in any::<[u16; 7]>(),
        u32s in any::<[u32; 7]>(),
        mins in any::<[i128; 2]>(),
        max_session_keys in any::<u8>(),
        markets in vec(market_params(), 0..5),
    ) -> GenesisConfigV1 {
        GenesisConfigV1 {
            lane_id: keys[0],
            backstop_key: keys[1],
            treasury_key: keys[2],
            access_mode: if allow { AccessMode::Allowlist } else { AccessMode::Open },
            allowlist,
            oracle_keys,
            oracle_max_staleness_ms: u64s[0],
            oracle_max_future_ms: u64s[1],
            oracle_circuit_breaker_bps: u16s[0],
            oracle_breaker_bps_per_sec: u16s[1],
            funding_interval_ms: u64s[2],
            funding_damping: u16s[2],
            funding_max_rate_ppm: u32s[0],
            insurance_fee_share_bps: u16s[3],
            min_deposit: mins[0],
            min_withdrawal: mins[1],
            max_accounts: u32s[1],
            max_orders_per_side: u32s[2],
            max_open_orders_per_account: u16s[4],
            max_session_keys,
            max_txs_per_account_per_block: u16s[5],
            max_entries_per_block: u32s[3],
            max_block_bytes: u32s[4],
            max_pending_withdrawals: u32s[5],
            exec_cpu_limit: u64s[3],
            exec_mem_limit: u64s[4],
            markets,
        }
    }
}

fn order() -> impl Strategy<Value = OrderV1> {
    (
        any::<u64>(),
        any::<u32>(),
        any::<i64>(),
        any::<i64>(),
        any::<u64>(),
    )
        .prop_map(
            |(order_id, account_index, price, lots_remaining, client_order_id)| OrderV1 {
                order_id,
                account_index,
                price,
                lots_remaining,
                client_order_id,
            },
        )
}

fn state() -> impl Strategy<Value = StateV1> {
    config()
        .prop_flat_map(|cfg| {
            let n = cfg.markets.len();
            let account = (
                b32(),
                any::<bool>(),
                any::<u64>(),
                any::<i128>(),
                any::<u16>(),
                vec((b32(), any::<u64>(), any::<u8>()), 0..3),
                vec((any::<i64>(), any::<i128>()), n),
                any::<u16>(),
            )
                .prop_map(
                    |(
                        key,
                        system,
                        next_nonce,
                        collateral,
                        open_order_count,
                        sks,
                        pos,
                        txs_this_block,
                    )| AccountV1 {
                        key,
                        system,
                        next_nonce,
                        collateral,
                        open_order_count,
                        session_keys: sks
                            .into_iter()
                            .map(|(key, expires_at_ms, permissions)| SessionKeyV1 {
                                key,
                                expires_at_ms,
                                permissions,
                            })
                            .collect(),
                        positions: pos
                            .into_iter()
                            .map(|(lots, cost_basis)| PositionV1 { lots, cost_basis })
                            .collect(),
                        txs_this_block,
                    },
                );
            let market = (
                any::<i64>(),
                any::<u64>(),
                any::<u64>(),
                any::<i128>(),
                any::<i64>(),
                vec(order(), 0..3),
                vec(order(), 0..3),
            )
                .prop_map(
                    |(
                        oracle_price,
                        oracle_time_ms,
                        last_funding_time_ms,
                        cumulative_funding_per_lot,
                        open_interest_lots,
                        bids,
                        asks,
                    )| MarketStateV1 {
                        oracle_price,
                        oracle_time_ms,
                        last_funding_time_ms,
                        cumulative_funding_per_lot,
                        open_interest_lots,
                        bids,
                        asks,
                    },
                );
            (
                Just(cfg),
                any::<[[u8; 32]; 4]>(),
                any::<[u64; 5]>(),
                any::<[i128; 2]>(),
                any::<bool>(),
                vec(account, 0..4),
                vec(market, n),
                vec((b32(), any::<i128>()), 0..3),
                any::<(u64, u64, u32, i128, u32, i128, u64)>(),
            )
        })
        .prop_map(
            |(config, hashes, u64s, totals, backstop_deficit, accounts, markets, pending, c)| {
                StateV1 {
                    lane_id: config.lane_id,
                    config_hash: hashes[0],
                    height: u64s[0],
                    last_block_input_hash: hashes[1],
                    last_timestamp_ms: u64s[1],
                    checkpoint_seq: u64s[2],
                    inbox_through: u64s[3],
                    inbox_acc: hashes[2],
                    next_order_id: u64s[4],
                    deposits_credited_total: totals[0],
                    withdrawals_committed_total: totals[1],
                    backstop_deficit,
                    accounts,
                    markets,
                    pending: pending
                        .into_iter()
                        .map(|(key, amount)| PendingWithdrawalV1 { key, amount })
                        .collect(),
                    last_commitment: CommitmentV1 {
                        seq: c.0,
                        last_block_height: c.1,
                        accounts_root: hashes[3],
                        account_count: c.2,
                        escape_total: c.3,
                        withdrawals_root: hashes[0],
                        withdrawal_count: c.4,
                        withdrawals_total: c.5,
                        inbox_through: c.6,
                        inbox_acc: hashes[1],
                    },
                    config,
                }
            },
        )
}

fn event() -> impl Strategy<Value = Event> {
    prop_oneof![
        (
            any::<u16>(),
            any::<u64>(),
            any::<u32>(),
            any::<u32>(),
            any::<i64>(),
            any::<i64>(),
            side()
        )
            .prop_map(
                |(market, maker_order_id, maker_idx, taker_idx, price, lots, taker_side)| {
                    Event::Fill {
                        market,
                        maker_order_id,
                        maker_idx,
                        taker_idx,
                        price,
                        lots,
                        taker_side,
                    }
                }
            ),
        (
            any::<u16>(),
            any::<u64>(),
            any::<u32>(),
            side(),
            any::<i64>(),
            any::<i64>()
        )
            .prop_map(|(market, order_id, account_idx, side, price, lots)| {
                Event::OrderRested {
                    market,
                    order_id,
                    account_idx,
                    side,
                    price,
                    lots,
                }
            }),
        (any::<u16>(), any::<u64>(), 0u8..5).prop_map(|(market, order_id, r)| {
            Event::OrderCanceled {
                market,
                order_id,
                reason: [
                    CancelReason::User,
                    CancelReason::SelfTrade,
                    CancelReason::IocRemainder,
                    CancelReason::Liquidation,
                    CancelReason::ForcedWithdrawal,
                ][usize::from(r)],
            }
        }),
        (any::<u16>(), any::<i32>(), any::<i64>()).prop_map(|(market, rate_ppm, fpl)| {
            Event::Funding {
                market,
                rate_ppm,
                fpl,
            }
        }),
        (any::<u32>(), any::<i128>(), any::<i128>()).prop_map(|(account_idx, fee, deficit)| {
            Event::Liquidation {
                account_idx,
                fee,
                deficit,
            }
        }),
        (b32(), any::<i128>(), 0u8..3).prop_map(|(key, amount, o)| Event::Deposit {
            key,
            amount,
            outcome: [
                DepositOutcome::Credited,
                DepositOutcome::Created,
                DepositOutcome::Bounced
            ][usize::from(o)],
        }),
        (b32(), any::<i128>())
            .prop_map(|(key, amount)| Event::ForcedWithdrawalProcessed { key, amount }),
        (any::<u16>(), any::<i64>(), any::<bool>()).prop_map(|(market, price, accepted)| {
            Event::Oracle {
                market,
                price,
                accepted,
            }
        }),
        (any::<bool>(), any::<i128>()).prop_map(|(active, backstop_equity)| {
            Event::BackstopDeficit {
                active,
                backstop_equity,
            }
        }),
        (any::<u64>(), any::<i128>(), any::<i128>()).prop_map(
            |(seq, withdrawals_total, escape_total)| Event::Commitment {
                seq,
                withdrawals_total,
                escape_total
            }
        ),
    ]
}

fn receipts() -> impl Strategy<Value = Receipts> {
    vec(
        (
            any::<u32>(),
            proptest::sample::select(codes::ALL),
            vec(event(), 0..4),
        ),
        0..6,
    )
    .prop_map(|rs| Receipts {
        receipts: rs
            .into_iter()
            .map(|(entry_index, code, events)| Receipt {
                entry_index,
                code,
                events,
            })
            .collect(),
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn lane_tx_round_trips(t in lane_tx()) {
        let bytes = t.encode();
        prop_assert_eq!(bytes.len(), t.encoded_len());
        prop_assert_eq!(LaneTxV1::decode(&bytes), Ok(t));
    }

    #[test]
    fn inbox_and_oracle_round_trip(m in inbox(), u in oracle()) {
        prop_assert_eq!(InboxMsgV1::decode(&m.encode()), Ok(m));
        prop_assert_eq!(OracleUpdateV1::decode(&u.encode()), Ok(u));
    }

    #[test]
    fn block_record_and_batch_round_trip(b in block(), hash in b32(), seq in any::<u64>()) {
        let input = b.encode().unwrap();
        prop_assert_eq!(input.len(), b.encoded_len());
        prop_assert_eq!(BlockInputV1::decode(&input), Ok(b.clone()));
        let rec = BlockRecordV1 { input, state_hash_after: hash };
        prop_assert_eq!(BlockRecordV1::decode(&rec.encode()), Ok(rec.clone()));
        let batch = BatchV1 { lane_id: b.lane_id, checkpoint_seq: seq, blocks: vec![rec.clone(), rec] };
        let bytes = batch.encode().unwrap();
        prop_assert_eq!(bytes.len(), batch.encoded_len());
        prop_assert_eq!(BatchV1::decode(&bytes), Ok(batch));
    }

    #[test]
    fn header_round_trips(fields in any::<[[u8; 32]; 12]>(), nums in any::<(u64, u64, u64, u64, u32, i128, u32, i128, u64)>()) {
        let h = CheckpointHeaderV1 {
            lane_id: fields[0], network_id: fields[1], settlement_addr_hash: fields[2], engine_wasm_hash: fields[3],
            seq: nums.0, prev_header_hash: fields[4], first_block_height: nums.1, last_block_height: nums.2,
            last_block_timestamp_ms: nums.3, last_block_hash: fields[5], batch_hash: fields[6], state_hash: fields[7],
            accounts_root: fields[8], account_count: nums.4, escape_total: nums.5, withdrawals_root: fields[9],
            withdrawal_count: nums.6, withdrawals_total: nums.7, inbox_through: nums.8, inbox_acc: fields[10],
        };
        prop_assert_eq!(CheckpointHeaderV1::decode(&h.encode()), Ok(h));
    }

    #[test]
    fn config_round_trips(c in config()) {
        prop_assert_eq!(GenesisConfigV1::decode(&c.encode().unwrap()), Ok(c));
    }

    #[test]
    fn state_round_trips(s in state()) {
        let bytes = s.encode().unwrap();
        prop_assert_eq!(bytes.len(), s.encoded_len());
        prop_assert_eq!(s.config.encode().unwrap().len(), s.config.encoded_len());
        prop_assert_eq!(StateV1::decode(&bytes), Ok(s));
    }

    #[test]
    fn receipts_and_step_round_trip(r in receipts(), state in vec(any::<u8>(), 0..64)) {
        let bytes = r.encode().unwrap();
        prop_assert_eq!(Receipts::decode(&bytes), Ok(r));
        let env = StepEnvelope { state, receipts: bytes };
        prop_assert_eq!(StepEnvelope::decode(&env.encode().unwrap()), Ok(env));
    }

    // Arbitrary bytes, with and without a valid magic, never panic a decoder.
    #[test]
    fn decoders_never_panic(mut bytes in vec(any::<u8>(), 0..600), magic in 0usize..7) {
        let magics: [&[u8; 8]; 7] = [b"CVBLKIN1", b"CVBATCH1", b"CVCKPT01", b"CVGENES1", b"CVSTATE1", b"CVRCPT01", b"CVSTEP01"];
        if bytes.len() >= 8 && magic < magics.len() {
            bytes[..8].copy_from_slice(magics[magic]);
        }
        let _ = LaneTxV1::decode(&bytes);
        let _ = InboxMsgV1::decode(&bytes);
        let _ = OracleUpdateV1::decode(&bytes);
        let _ = BlockInputV1::decode(&bytes);
        let _ = BlockRecordV1::decode(&bytes);
        let _ = BatchV1::decode(&bytes);
        let _ = CheckpointHeaderV1::decode(&bytes);
        let _ = GenesisConfigV1::decode(&bytes);
        let _ = StateV1::decode(&bytes);
        let _ = Receipts::decode(&bytes);
        let _ = StepEnvelope::decode(&bytes);
    }

    // A valid encoding with any single byte appended is rejected.
    #[test]
    fn one_trailing_byte_is_always_rejected(t in lane_tx(), b in block(), extra in any::<u8>()) {
        let mut tx = t.encode();
        tx.push(extra);
        prop_assert!(LaneTxV1::decode(&tx).is_err());
        let mut blk = b.encode().unwrap();
        blk.push(extra);
        prop_assert!(BlockInputV1::decode(&blk).is_err());
    }
}
