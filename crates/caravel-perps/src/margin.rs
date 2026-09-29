//! Margin formulas (spec §11.3.4), shared by order checks, withdrawals, forced
//! withdrawals, liquidations and commitments.
//!
//! For account `a` and market `m`, with `s` signed lots, `C` cost basis, `M` the
//! oracle price and `B`/`A` the account's resting bid/ask lots:
//!
//! ```text
//! upnl_m      = s × M − C
//! equity E    = collateral + Σ_m upnl_m
//! exposure_m  = max(|s + B|, |s − A|)
//! IM_m        = mul_div_ceil(exposure_m × M, imf_bps, 10_000)
//! MM_m        = mul_div_ceil(|s| × M, mmf_bps, 10_000)
//! free_collat = E − Σ_m IM_m
//! ```

use caravel_types::fixed::{mul_div_ceil, ArithError};
use caravel_types::state::StateV1;
use caravel_types::tx::Side;

/// Equity and margin requirements of one account, in USDC stroops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Margin {
    pub equity: i128,
    /// Σ IM, including resting orders.
    pub initial: i128,
    /// Σ MM, positions only.
    pub maintenance: i128,
}

impl Margin {
    pub fn free_collateral(&self) -> Result<i128, ArithError> {
        self.equity
            .checked_sub(self.initial)
            .ok_or(ArithError::Overflow)
    }
}

/// A new order added to the worst-case exposure (§11.3.4 step 1).
#[derive(Clone, Copy, Debug)]
pub struct ExtraOrder {
    pub market: usize,
    pub side: Side,
    pub lots: i64,
}

const OVERFLOW: ArithError = ArithError::Overflow;

fn add(a: i128, b: i128) -> Result<i128, ArithError> {
    a.checked_add(b).ok_or(OVERFLOW)
}

fn sub(a: i128, b: i128) -> Result<i128, ArithError> {
    a.checked_sub(b).ok_or(OVERFLOW)
}

fn mul(a: i128, b: i128) -> Result<i128, ArithError> {
    a.checked_mul(b).ok_or(OVERFLOW)
}

fn abs(a: i128) -> Result<i128, ArithError> {
    a.checked_abs().ok_or(OVERFLOW)
}

/// `(B, A)`: the account's resting bid and ask lots on market `m`.
pub fn resting_lots(
    st: &StateV1,
    m: usize,
    account_index: u32,
) -> Result<(i128, i128), ArithError> {
    let market = &st.markets[m];
    let sum = |orders: &[caravel_types::state::OrderV1]| {
        orders
            .iter()
            .filter(|o| o.account_index == account_index)
            .try_fold(0i128, |acc, o| add(acc, i128::from(o.lots_remaining)))
    };
    Ok((sum(&market.bids)?, sum(&market.asks)?))
}

/// `upnl_m = s × M − C` for account `a` on market `m`, at the current oracle price.
pub fn upnl(st: &StateV1, a: usize, m: usize) -> Result<i128, ArithError> {
    let p = &st.accounts[a].positions[m];
    sub(
        mul(i128::from(p.lots), i128::from(st.markets[m].oracle_price))?,
        p.cost_basis,
    )
}

/// `E = collateral + Σ upnl` at the current oracle prices, stale or not.
pub fn equity(st: &StateV1, a: usize) -> Result<i128, ArithError> {
    (0..st.markets.len()).try_fold(st.accounts[a].collateral, |e, m| add(e, upnl(st, a, m)?))
}

/// Equity, IM (with resting orders and an optional new order) and MM for account `a`.
pub fn account_margin(
    st: &StateV1,
    a: usize,
    extra: Option<ExtraOrder>,
) -> Result<Margin, ArithError> {
    let account_index = u32::try_from(a).map_err(|_| OVERFLOW)?;
    let mut out = Margin {
        equity: st.accounts[a].collateral,
        initial: 0,
        maintenance: 0,
    };
    for (m, params) in st.config.markets.iter().enumerate() {
        let s = i128::from(st.accounts[a].positions[m].lots);
        let price = i128::from(st.markets[m].oracle_price);
        let (mut bids, mut asks) = resting_lots(st, m, account_index)?;
        if let Some(x) = extra.filter(|x| x.market == m) {
            match x.side {
                Side::Buy => bids = add(bids, i128::from(x.lots))?,
                Side::Sell => asks = add(asks, i128::from(x.lots))?,
            }
        }
        out.equity = add(out.equity, upnl(st, a, m)?)?;
        let exposure = abs(add(s, bids)?)?.max(abs(sub(s, asks)?)?);
        out.initial = add(
            out.initial,
            mul_div_ceil(mul(exposure, price)?, i128::from(params.imf_bps), 10_000)?,
        )?;
        out.maintenance = add(
            out.maintenance,
            mul_div_ceil(mul(abs(s)?, price)?, i128::from(params.mmf_bps), 10_000)?,
        )?;
    }
    Ok(out)
}
