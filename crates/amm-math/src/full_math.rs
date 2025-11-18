//! 512-bit intermediate multiply-divide, ported from `FullMath.sol` and `UnsafeMath.sol`.

use alloy_primitives::{U256, U512};

use crate::MathError;

/// `floor(a * b / denominator)` with a full 512-bit intermediate product.
///
/// Mirrors `FullMath.mulDiv`: fails if `denominator == 0` or if the result does
/// not fit in 256 bits.
#[inline]
pub fn mul_div(a: U256, b: U256, denominator: U256) -> Result<U256, MathError> {
    mul_div_rem(a, b, denominator).map(|(q, _)| q)
}

/// `ceil(a * b / denominator)`, mirroring `FullMath.mulDivRoundingUp`.
#[inline]
pub fn mul_div_rounding_up(a: U256, b: U256, denominator: U256) -> Result<U256, MathError> {
    let (q, rem_nonzero) = mul_div_rem(a, b, denominator)?;
    if rem_nonzero {
        // `require(result < type(uint256).max); result++;`
        q.checked_add(U256::from(1)).ok_or(MathError::Overflow)
    } else {
        Ok(q)
    }
}

/// `x / y` rounded up, mirroring `UnsafeMath.divRoundingUp`.
///
/// This is the assembly version: division by zero yields zero, exactly as the
/// EVM `div` and `mod` opcodes do.
#[inline]
pub fn div_rounding_up(x: U256, y: U256) -> U256 {
    if y.is_zero() {
        return U256::ZERO;
    }
    let (q, r) = x.div_rem(y);
    if r.is_zero() { q } else { q + U256::from(1) }
}

/// Shared core: quotient and whether the remainder is non-zero.
#[inline]
fn mul_div_rem(a: U256, b: U256, denominator: U256) -> Result<(U256, bool), MathError> {
    if denominator.is_zero() {
        return Err(MathError::DivisionByZero);
    }
    // Fast path: the product fits in 256 bits, so a plain division is exact.
    let (prod, overflow) = a.overflowing_mul(b);
    if !overflow {
        let (q, r) = prod.div_rem(denominator);
        return Ok((q, !r.is_zero()));
    }
    let wide: U512 = a.widening_mul(b);
    let (q, r) = wide.div_rem(U512::from(denominator));
    let q = U256::checked_from_limbs_slice(q.as_limbs()).ok_or(MathError::Overflow)?;
    Ok((q, !r.is_zero()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_div_handles_phantom_overflow() {
        // (2^255 * 4) / 8 = 2^254 even though the product needs 258 bits.
        let a = U256::from(1) << 255;
        assert_eq!(
            mul_div(a, U256::from(4), U256::from(8)).unwrap(),
            U256::from(1) << 254
        );
    }

    #[test]
    fn mul_div_rejects_zero_denominator_and_overflow() {
        assert_eq!(
            mul_div(U256::from(1), U256::from(1), U256::ZERO),
            Err(MathError::DivisionByZero)
        );
        assert_eq!(
            mul_div(U256::MAX, U256::MAX, U256::from(1)),
            Err(MathError::Overflow)
        );
    }

    #[test]
    fn mul_div_rounding_up_rounds_and_guards_max() {
        assert_eq!(
            mul_div_rounding_up(U256::from(7), U256::from(3), U256::from(2)).unwrap(),
            U256::from(11)
        );
        assert_eq!(
            mul_div_rounding_up(U256::MAX, U256::MAX, U256::MAX).unwrap(),
            U256::MAX
        );
    }

    #[test]
    fn div_rounding_up_matches_evm_on_zero() {
        assert_eq!(div_rounding_up(U256::from(5), U256::ZERO), U256::ZERO);
        assert_eq!(div_rounding_up(U256::from(5), U256::from(2)), U256::from(3));
        assert_eq!(div_rounding_up(U256::from(6), U256::from(2)), U256::from(3));
    }
}
