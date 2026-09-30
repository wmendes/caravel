//! Checkpoint commitment, only on `CHECKPOINT_END` (spec §11.8).

use alloc::vec::Vec;

use caravel_types::fatal::BLOCK_LEVEL;
use caravel_types::preimage::{account_leaf_preimage, withdrawal_leaf_preimage};
use caravel_types::receipts::Event;
use caravel_types::state::CommitmentV1;

use crate::engine::{Engine, Hasher, OrFatal, Res};
use crate::margin::equity;
use crate::Crypto;

impl<C: Crypto> Engine<'_, C> {
    pub(crate) fn commitment(&mut self, block_height: u64) -> Res<()> {
        let e = BLOCK_LEVEL;
        let seq = self.st.checkpoint_seq.checked_add(1).or_fatal(e)?;
        let lane_id = self.st.lane_id;

        // Escape root: every account's equity at current prices, stale or not, floored at 0.
        let mut account_leaves = Vec::with_capacity(self.st.accounts.len());
        let mut escape_total: i128 = 0;
        for (j, a) in self.st.accounts.iter().enumerate() {
            let escape_equity = equity(&self.st, j).or_fatal(e)?.max(0);
            escape_total = escape_total.checked_add(escape_equity).or_fatal(e)?;
            let j = u32::try_from(j).or_fatal(e)?;
            account_leaves.push(self.c.sha256(&account_leaf_preimage(
                &lane_id,
                seq,
                j,
                &a.key,
                escape_equity,
            )));
        }
        let accounts_root = caravel_merkle::root(&Hasher(self.c), &account_leaves)
            .ok()
            .or_fatal(e)?;

        // Withdrawals root: this checkpoint's pending list.
        let mut withdrawal_leaves = Vec::with_capacity(self.st.pending.len());
        let mut withdrawals_total: i128 = 0;
        for (i, p) in self.st.pending.iter().enumerate() {
            withdrawals_total = withdrawals_total.checked_add(p.amount).or_fatal(e)?;
            let i = u32::try_from(i).or_fatal(e)?;
            withdrawal_leaves.push(self.c.sha256(&withdrawal_leaf_preimage(
                &lane_id, seq, i, &p.key, p.amount,
            )));
        }
        let withdrawals_root = caravel_merkle::root(&Hasher(self.c), &withdrawal_leaves)
            .ok()
            .or_fatal(e)?;

        self.st.last_commitment = CommitmentV1 {
            seq,
            last_block_height: block_height,
            accounts_root,
            account_count: u32::try_from(self.st.accounts.len()).or_fatal(e)?,
            escape_total,
            withdrawals_root,
            withdrawal_count: u32::try_from(self.st.pending.len()).or_fatal(e)?,
            withdrawals_total,
            inbox_through: self.st.inbox_through,
            inbox_acc: self.st.inbox_acc,
        };
        self.st.withdrawals_committed_total = self
            .st
            .withdrawals_committed_total
            .checked_add(withdrawals_total)
            .or_fatal(e)?;
        self.st.pending.clear();
        self.st.checkpoint_seq = seq;
        self.end_events.push(Event::Commitment {
            seq,
            withdrawals_total,
            escape_total,
        });
        Ok(())
    }
}
