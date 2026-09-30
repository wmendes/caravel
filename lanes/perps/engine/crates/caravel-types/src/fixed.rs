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

        #[test]
        fn mul_div_matches_div_of_product(a in any::<i64>(), b in any::<i32>(), c in any::<i64>().prop_filter("nonzero", |c| *c != 0)) {
            let (a, b, c) = (i128::from(a), i128::from(b), i128::from(c));
            prop_assert_eq!(mul_div_floor(a, b, c), div_floor(a * b, c));
            prop_assert_eq!(mul_div_ceil(a, b, c), div_ceil(a * b, c));
            prop_assert_eq!(mul_div_trunc(a, b, c), div_trunc(a * b, c));
        }
    }
}
