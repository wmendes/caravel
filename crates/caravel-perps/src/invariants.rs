//! State invariants (spec §11.9), checked after every block by the property
//! tests and the scenario runner. INV-P4 (the inbox fold) needs the processed
//! messages, so the test harness checks it; INV-P5/P6 are checked elsewhere.

use alloc::collections::BTreeSet;

use caravel_types::state::StateV1;

/// The first invariant a state breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    /// INV-P1: `Σ (collateral − Σ C) + Σ pending ≠ deposits − withdrawals_committed`.
    Conservation,
    /// INV-P2: `Σ s ≠ 0` on market `m`.
    ZeroSum { market: usize },
    /// INV-P2: `open_interest_lots ≠ Σ max(s, 0)` on market `m`.
    OpenInterest { market: usize },
    /// INV-P3: a book is out of order.
    BookOrder { market: usize },
    /// INV-P3: the book is crossed at rest.
    CrossedBook { market: usize },
    /// INV-P3: an order has no lots left or points at no account.
    BadOrder { market: usize },
    /// INV-P3: an account's `open_order_count` does not match the books.
    OpenOrderCount { account: usize },
    /// INV-P7: `Σ pending > deposits − withdrawals_committed`.
    LaneLiquidity,
    /// INV-P8: two accounts share a key.
    DuplicateAccountKey,
    /// INV-P8: an account's session keys are not strictly ascending.
    SessionKeys { account: usize },
    /// An arithmetic overflow while checking.
    Overflow,
}

fn sum(it: impl Iterator<Item = i128>) -> Result<i128, Violation> {
    it.into_iter()
        .try_fold(0i128, |a, b| a.checked_add(b))
        .ok_or(Violation::Overflow)
}

/// Checks INV-P1, P2, P3, P7 and P8.
pub fn check(st: &StateV1) -> Result<(), Violation> {
    // INV-P1 and INV-P7.
    let pending = sum(st.pending.iter().map(|p| p.amount))?;
    let mut held: i128 = pending;
    for a in &st.accounts {
        let basis = sum(a.positions.iter().map(|p| p.cost_basis))?;
        held = held
            .checked_add(a.collateral.checked_sub(basis).ok_or(Violation::Overflow)?)
            .ok_or(Violation::Overflow)?;
    }
    let net = st
        .deposits_credited_total
        .checked_sub(st.withdrawals_committed_total)
        .ok_or(Violation::Overflow)?;
    if held != net {
        return Err(Violation::Conservation);
    }
    if pending > net {
        return Err(Violation::LaneLiquidity);
    }

    // INV-P2.
    for m in 0..st.markets.len() {
        if sum(st.accounts.iter().map(|a| i128::from(a.positions[m].lots)))? != 0 {
            return Err(Violation::ZeroSum { market: m });
        }
        let oi = sum(st
            .accounts
            .iter()
            .map(|a| i128::from(a.positions[m].lots.max(0))))?;
        if oi != i128::from(st.markets[m].open_interest_lots) {
            return Err(Violation::OpenInterest { market: m });
        }
    }

    // INV-P3.
    let mut counts = alloc::vec![0u32; st.accounts.len()];
    for (m, mk) in st.markets.iter().enumerate() {
        let bids_ok = mk.bids.windows(2).all(|w| {
            w[0].price > w[1].price || (w[0].price == w[1].price && w[0].order_id < w[1].order_id)
        });
        let asks_ok = mk.asks.windows(2).all(|w| {
            w[0].price < w[1].price || (w[0].price == w[1].price && w[0].order_id < w[1].order_id)
        });
        if !bids_ok || !asks_ok {
            return Err(Violation::BookOrder { market: m });
        }
        if let (Some(b), Some(a)) = (mk.bids.first(), mk.asks.first()) {
            if b.price >= a.price {
                return Err(Violation::CrossedBook { market: m });
            }
        }
        for o in mk.bids.iter().chain(&mk.asks) {
            let slot = counts
                .get_mut(o.account_index as usize)
                .ok_or(Violation::BadOrder { market: m })?;
            if o.lots_remaining <= 0 {
                return Err(Violation::BadOrder { market: m });
            }
            *slot += 1;
        }
    }
    for (i, a) in st.accounts.iter().enumerate() {
        if u32::from(a.open_order_count) != counts[i] {
            return Err(Violation::OpenOrderCount { account: i });
        }
    }

    // INV-P8.
    let mut keys = BTreeSet::new();
    for (i, a) in st.accounts.iter().enumerate() {
        if !keys.insert(a.key) {
            return Err(Violation::DuplicateAccountKey);
        }
        if !a.session_keys.windows(2).all(|w| w[0].key < w[1].key) {
            return Err(Violation::SessionKeys { account: i });
        }
    }
    Ok(())
}
