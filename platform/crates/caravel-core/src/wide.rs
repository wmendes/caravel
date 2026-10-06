//! `a × b / c` with a 256-bit intermediate, for amounts whose product can
//! pass `i128::MAX` while the result fits: the settlement contract's escape
//! payout (issue #145, S-01, DEC-126). Not part of the engine's formats;
//! `fixed` stays the frozen engine's copy.

use crate::fixed::ArithError;

/// `a × b / c` rounded toward negative infinity, for `a, b ≥ 0` and `c > 0`,
/// with a 256-bit intermediate: `Overflow` only when the quotient itself
/// passes `i128::MAX`, never because `a × b` does (issue #145, S-01).
pub fn mul_div_floor(a: i128, b: i128, c: i128) -> Result<i128, ArithError> {
    if c == 0 {
        return Err(ArithError::DivByZero);
    }
    if a < 0 || b < 0 || c < 0 {
        return Err(ArithError::Overflow);
    }
    let (hi, lo) = mul_u128(a as u128, b as u128);
    let c = c as u128;
    if hi >= c {
        return Err(ArithError::Overflow);
    }
    // Long division of hi:lo by c, one bit at a time. The remainder stays
    // below c < 2^127, so shifting it left never overflows.
    let (mut rem, mut q) = (hi, 0u128);
    for i in (0..128).rev() {
        rem = (rem << 1) | ((lo >> i) & 1);
        q <<= 1;
        if rem >= c {
            rem -= c;
            q |= 1;
        }
    }
    i128::try_from(q).map_err(|_| ArithError::Overflow)
}

/// `a × b` as `(high, low)` 128-bit halves.
fn mul_u128(a: u128, b: u128) -> (u128, u128) {
    const M: u128 = u64::MAX as u128;
    let (a1, a0) = (a >> 64, a & M);
    let (b1, b0) = (b >> 64, b & M);
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let mid = (p00 >> 64) + (p01 & M) + (p10 & M);
    let lo = (p00 & M) | (mid << 64);
    let hi = p11 + (p01 >> 64) + (p10 >> 64) + (mid >> 64);
    (hi, lo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixed;
    use proptest::prelude::*;

    #[test]
    fn exact_where_the_product_passes_i128() {
        // Issue #145's case: the product passes i128::MAX, the result doesn't.
        let x: i128 = 14_000_000_000_000_000_000;
        assert_eq!(fixed::mul_div_floor(x, x, x), Err(ArithError::Overflow));
        assert_eq!(mul_div_floor(x, x, x), Ok(x));
        let m = i128::MAX;
        assert_eq!(mul_div_floor(m, m, m), Ok(m));
        assert_eq!(mul_div_floor(m, m - 1, m), Ok(m - 1));
        assert_eq!(mul_div_floor(m, 1, 2), Ok(m / 2));
        assert_eq!(mul_div_floor(m, 2, 2), Ok(m));
        assert_eq!(mul_div_floor(m - 1, m, m - 1), Ok(m));
        assert_eq!(mul_div_floor(7, 10, 3), Ok(23));
        assert_eq!(mul_div_floor(0, m, 1), Ok(0));
        assert_eq!(mul_div_floor(1 << 100, 1 << 100, 1 << 74), Ok(1 << 126));
        // 9e18 × 27e18 / 27e18: three equal accounts of a high-decimal token.
        let e: i128 = 9_000_000_000_000_000_000;
        assert_eq!(mul_div_floor(e, 3 * e, 3 * e), Ok(e));
        assert_eq!(
            mul_div_floor(e, 2 * e, 3 * e),
            Ok(6_000_000_000_000_000_000)
        );
        // Errors: a quotient past i128::MAX, c = 0, negative inputs.
        assert_eq!(mul_div_floor(m, 2, 1), Err(ArithError::Overflow));
        assert_eq!(mul_div_floor(m, m, m - 1), Err(ArithError::Overflow));
        assert_eq!(mul_div_floor(1, 1, 0), Err(ArithError::DivByZero));
        assert_eq!(mul_div_floor(-1, 1, 1), Err(ArithError::Overflow));
        assert_eq!(mul_div_floor(1, -1, 1), Err(ArithError::Overflow));
        assert_eq!(mul_div_floor(1, 1, -1), Err(ArithError::Overflow));
    }

    proptest! {
        // Wherever the product fits, the wide helper is the narrow one.
        #[test]
        fn wide_matches_narrow_where_the_product_fits(a in 0..=i128::MAX, b in 0..=i128::MAX, c in 1..=i128::MAX) {
            if let Some(p) = a.checked_mul(b) {
                prop_assert_eq!(mul_div_floor(a, b, c), fixed::div_floor(p, c));
            }
            let (a, b) = (a >> 64, b >> 63);
            prop_assert_eq!(mul_div_floor(a, b, c), fixed::div_floor(a * b, c));
        }

        // floor(a·b/c) = q iff q·c ≤ a·b < (q+1)·c, checked in 256 bits.
        #[test]
        fn wide_brackets_the_exact_quotient(a in 0..=i128::MAX, b in 0..=i128::MAX, c in 1..=i128::MAX) {
            let ab = mul_u128(a as u128, b as u128);
            match mul_div_floor(a, b, c) {
                Ok(q) => {
                    let qc = mul_u128(q as u128, c as u128);
                    let q1c = mul_u128(q as u128 + 1, c as u128);
                    prop_assert!(qc <= ab && ab < q1c);
                }
                Err(e) => {
                    prop_assert_eq!(e, ArithError::Overflow);
                    // The quotient passes i128::MAX: (MAX+1)·c ≤ a·b.
                    prop_assert!(mul_u128(1u128 << 127, c as u128) <= ab);
                }
            }
        }
    }
}
