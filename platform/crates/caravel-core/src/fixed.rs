//! Integer division helpers with a named rounding direction (spec §8.1 INV-D5).
//!
//! Consensus code never uses a bare `/` or `%` on signed values: it calls one of
//! these. Intermediates are `i128` and overflow is an `Err`, never a wrap.

/// Why an arithmetic helper could not produce a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithError {
    Overflow,
    DivByZero,
}

/// `a / b` rounded toward zero.
pub fn div_trunc(a: i128, b: i128) -> Result<i128, ArithError> {
    if b == 0 {
        return Err(ArithError::DivByZero);
    }
    a.checked_div(b).ok_or(ArithError::Overflow)
}

/// `a / b` rounded toward negative infinity.
pub fn div_floor(a: i128, b: i128) -> Result<i128, ArithError> {
    let q = div_trunc(a, b)?;
    let r = a.checked_rem(b).ok_or(ArithError::Overflow)?;
    if r != 0 && ((r < 0) != (b < 0)) {
        q.checked_sub(1).ok_or(ArithError::Overflow)
    } else {
        Ok(q)
    }
}

/// `a / b` rounded toward positive infinity.
pub fn div_ceil(a: i128, b: i128) -> Result<i128, ArithError> {
    let q = div_trunc(a, b)?;
    let r = a.checked_rem(b).ok_or(ArithError::Overflow)?;
    if r != 0 && ((r < 0) == (b < 0)) {
        q.checked_add(1).ok_or(ArithError::Overflow)
    } else {
        Ok(q)
    }
}

/// `a × b / c` rounded toward negative infinity.
pub fn mul_div_floor(a: i128, b: i128, c: i128) -> Result<i128, ArithError> {
    div_floor(a.checked_mul(b).ok_or(ArithError::Overflow)?, c)
}

/// `a × b / c` rounded toward negative infinity, for `a, b ≥ 0` and `c > 0`,
/// with a 256-bit intermediate: `Overflow` only when the quotient itself
/// passes `i128::MAX`, never because `a × b` does (issue #145, S-01).
pub fn mul_div_floor_wide(a: i128, b: i128, c: i128) -> Result<i128, ArithError> {
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

/// `a × b / c` rounded toward positive infinity.
pub fn mul_div_ceil(a: i128, b: i128, c: i128) -> Result<i128, ArithError> {
    div_ceil(a.checked_mul(b).ok_or(ArithError::Overflow)?, c)
}

/// `a × b / c` rounded toward zero.
pub fn mul_div_trunc(a: i128, b: i128, c: i128) -> Result<i128, ArithError> {
    div_trunc(a.checked_mul(b).ok_or(ArithError::Overflow)?, c)
}

/// Whether `value` is a whole multiple of `step` (`rem_euclid`, spec INV-D5).
/// A non-positive `step` has no multiples.
pub fn is_multiple_of(value: i64, step: i64) -> bool {
    step > 0 && value.checked_rem_euclid(step) == Some(0)
}

/// `notional(lots, price) = |lots| × price` (spec §11.1). Never overflows:
/// `|i64| × |i64| < 2^126`.
pub fn notional(lots: i64, price: i64) -> i128 {
    i128::from(lots).abs() * i128::from(price)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rounding_directions() {
        assert_eq!(div_floor(7, 2), Ok(3));
        assert_eq!(div_floor(-7, 2), Ok(-4));
        assert_eq!(div_floor(7, -2), Ok(-4));
        assert_eq!(div_floor(-7, -2), Ok(3));
        assert_eq!(div_ceil(7, 2), Ok(4));
        assert_eq!(div_ceil(-7, 2), Ok(-3));
        assert_eq!(div_ceil(7, -2), Ok(-3));
        assert_eq!(div_ceil(-7, -2), Ok(4));
        assert_eq!(div_trunc(-7, 2), Ok(-3));
        assert_eq!(div_trunc(7, -2), Ok(-3));
        assert_eq!(div_floor(6, 3), Ok(2));
        assert_eq!(div_ceil(-6, 3), Ok(-2));
    }

    #[test]
    fn errors_instead_of_wrapping() {
        assert_eq!(div_floor(1, 0), Err(ArithError::DivByZero));
        assert_eq!(div_trunc(i128::MIN, -1), Err(ArithError::Overflow));
        assert_eq!(div_ceil(i128::MIN, -1), Err(ArithError::Overflow));
        assert_eq!(mul_div_floor(i128::MAX, 2, 3), Err(ArithError::Overflow));
        assert_eq!(
            mul_div_ceil(1 << 100, 1 << 30, 1),
            Err(ArithError::Overflow)
        );
    }

    #[test]
    fn wide_mul_div_floor() {
        // Issue #145's case: the product passes i128::MAX, the result doesn't.
        let x: i128 = 14_000_000_000_000_000_000;
        assert_eq!(mul_div_floor(x, x, x), Err(ArithError::Overflow));
        assert_eq!(mul_div_floor_wide(x, x, x), Ok(x));
        let m = i128::MAX;
        assert_eq!(mul_div_floor_wide(m, m, m), Ok(m));
        assert_eq!(mul_div_floor_wide(m, m - 1, m), Ok(m - 1));
        assert_eq!(mul_div_floor_wide(m, 1, 2), Ok(m / 2));
        assert_eq!(mul_div_floor_wide(m, 2, 2), Ok(m));
        assert_eq!(mul_div_floor_wide(m - 1, m, m - 1), Ok(m));
        assert_eq!(mul_div_floor_wide(7, 10, 3), Ok(23));
        assert_eq!(mul_div_floor_wide(0, m, 1), Ok(0));
        assert_eq!(
            mul_div_floor_wide(1 << 100, 1 << 100, 1 << 74),
            Ok(1 << 126)
        );
        // 9e18 × 27e18 / 27e18: three equal accounts of a high-decimal token.
        let e: i128 = 9_000_000_000_000_000_000;
        assert_eq!(mul_div_floor_wide(e, 3 * e, 3 * e), Ok(e));
        assert_eq!(
            mul_div_floor_wide(e, 2 * e, 3 * e),
            Ok(6_000_000_000_000_000_000)
        );
        // Errors: a quotient past i128::MAX, c = 0, negative inputs.
        assert_eq!(mul_div_floor_wide(m, 2, 1), Err(ArithError::Overflow));
        assert_eq!(mul_div_floor_wide(m, m, m - 1), Err(ArithError::Overflow));
        assert_eq!(mul_div_floor_wide(1, 1, 0), Err(ArithError::DivByZero));
        assert_eq!(mul_div_floor_wide(-1, 1, 1), Err(ArithError::Overflow));
        assert_eq!(mul_div_floor_wide(1, -1, 1), Err(ArithError::Overflow));
        assert_eq!(mul_div_floor_wide(1, 1, -1), Err(ArithError::Overflow));
    }

    #[test]
    fn multiples_and_notional() {
        assert!(is_multiple_of(3000, 1000));
        assert!(is_multiple_of(-2000, 1000));
        assert!(!is_multiple_of(1500, 1000));
        assert!(!is_multiple_of(1000, 0));
        assert!(!is_multiple_of(1000, -1000));
        assert_eq!(notional(-25, 4000), 100_000);
        assert_eq!(
            notional(i64::MIN, i64::MAX),
            i128::from(i64::MIN).abs() * i128::from(i64::MAX)
        );
    }

    proptest! {
        // q is the rounded quotient iff q*b and (q±1)*b bracket a on the right side.
        #[test]
        fn floor_ceil_trunc_bracket_the_exact_quotient(a in any::<i64>(), b in any::<i64>().prop_filter("nonzero", |b| *b != 0)) {
            let (a, b) = (i128::from(a), i128::from(b));
            let f = div_floor(a, b).unwrap();
            let c = div_ceil(a, b).unwrap();
            let t = div_trunc(a, b).unwrap();
            // Compare a/b with q via multiplication, flipping when b < 0.
            let le = |q: i128| if b > 0 { q * b <= a } else { q * b >= a };
            let ge = |q: i128| if b > 0 { q * b >= a } else { q * b <= a };
            prop_assert!(le(f) && !le(f + 1));
            prop_assert!(ge(c) && !ge(c - 1));
            prop_assert_eq!(t, if (a < 0) == (b < 0) { f } else { c });
            prop_assert!(c - f <= 1);
        }

        // Wherever the product fits, the wide helper is the narrow one.
        #[test]
        fn wide_matches_narrow_where_the_product_fits(a in 0..=i128::MAX, b in 0..=i128::MAX, c in 1..=i128::MAX) {
            if let Some(p) = a.checked_mul(b) {
                prop_assert_eq!(mul_div_floor_wide(a, b, c), div_floor(p, c));
            }
            let (a, b) = (a >> 64, b >> 63);
            prop_assert_eq!(mul_div_floor_wide(a, b, c), div_floor(a * b, c));
        }

        // floor(a·b/c) = q iff q·c ≤ a·b < (q+1)·c, checked in 256 bits.
        #[test]
        fn wide_brackets_the_exact_quotient(a in 0..=i128::MAX, b in 0..=i128::MAX, c in 1..=i128::MAX) {
            let ab = mul_u128(a as u128, b as u128);
            match mul_div_floor_wide(a, b, c) {
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

        #[test]
        fn mul_div_matches_div_of_product(a in any::<i64>(), b in any::<i32>(), c in any::<i64>().prop_filter("nonzero", |c| *c != 0)) {
            let (a, b, c) = (i128::from(a), i128::from(b), i128::from(c));
            prop_assert_eq!(mul_div_floor(a, b, c), div_floor(a * b, c));
            prop_assert_eq!(mul_div_ceil(a, b, c), div_ceil(a * b, c));
            prop_assert_eq!(mul_div_trunc(a, b, c), div_trunc(a * b, c));
        }
    }
}
