//! Port of `LiquidityMath.sol`.

use crate::MathError;

/// Adds a signed liquidity delta, failing on underflow (`LS`) or overflow (`LA`).
#[inline]
pub fn add_delta(x: u128, y: i128) -> Result<u128, MathError> {
    if y < 0 {
        x.checked_sub(y.unsigned_abs())
            .ok_or(MathError::LiquidityUnderflow)
    } else {
        x.checked_add(y as u128).ok_or(MathError::LiquidityOverflow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_delta_bounds() {
        assert_eq!(add_delta(5, -5), Ok(0));
        assert_eq!(add_delta(5, -6), Err(MathError::LiquidityUnderflow));
        assert_eq!(add_delta(u128::MAX, 1), Err(MathError::LiquidityOverflow));
        assert_eq!(add_delta(1, i128::MAX), Ok(1u128 << 127));
    }
}
