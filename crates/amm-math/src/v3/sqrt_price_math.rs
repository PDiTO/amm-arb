//! Port of `SqrtPriceMath.sol`: price movement for a given amount, and the
//! token amounts between two prices at constant liquidity.
//!
//! Within one tick range a V3 pool behaves like a V2 pool with virtual reserves
//! `x = L / sqrtP` and `y = L * sqrtP`. Everything here follows from that:
//!
//! - `Δy = L * (sqrtP_b - sqrtP_a)`
//! - `Δx = L * (sqrtP_b - sqrtP_a) / (sqrtP_a * sqrtP_b)`
//!
//! Rounding always favours the pool: amounts in round up, amounts out and
//! price moves in the trader's favour round down.

use alloy_primitives::{I256, U256, aliases::U160};

use super::{Q96, wide};
use crate::{
    MathError,
    full_math::{div_rounding_up, mul_div, mul_div_rounding_up},
};

/// Largest `uint160`, as a `uint256`.
const MAX_U160: U256 = U256::from_limbs([u64::MAX, u64::MAX, u32::MAX as u64, 0]);

/// Next sqrt price after adding or removing `amount` of token0, rounded up.
pub fn get_next_sqrt_price_from_amount0_rounding_up(
    sqrt_px96: U160,
    liquidity: u128,
    amount: U256,
    add: bool,
) -> Result<U160, MathError> {
    // Short-circuit: the formula is not guaranteed to return the input price.
    if amount.is_zero() {
        return Ok(sqrt_px96);
    }
    let sqrt_p = wide(sqrt_px96);
    let numerator1: U256 = U256::from(liquidity) << 96;

    if add {
        // `(product = amount * sqrtP) / amount == sqrtP` is exactly "did not wrap".
        let (product, wrapped) = amount.overflowing_mul(sqrt_p);
        if !wrapped {
            // `denominator >= numerator1` is exactly "did not wrap".
            let (denominator, wrapped) = numerator1.overflowing_add(product);
            if !wrapped {
                // Solidity downcasts with `uint160(...)`; it always fits.
                return Ok(U160::wrapping_from(mul_div_rounding_up(
                    numerator1,
                    sqrt_p,
                    denominator,
                )?));
            }
        }
        // Fallback: L / (L / sqrtP + amount), less precise but cannot overflow.
        let quotient = numerator1
            .checked_div(sqrt_p)
            .ok_or(MathError::DivisionByZero)?;
        let denominator = quotient.checked_add(amount).ok_or(MathError::Overflow)?;
        Ok(U160::wrapping_from(div_rounding_up(
            numerator1,
            denominator,
        )))
    } else {
        let (product, wrapped) = amount.overflowing_mul(sqrt_p);
        if wrapped || numerator1 <= product {
            return Err(MathError::NextPriceOutOfRange);
        }
        let denominator = numerator1 - product;
        let next = mul_div_rounding_up(numerator1, sqrt_p, denominator)?;
        U160::checked_from_limbs_slice(next.as_limbs()).ok_or(MathError::Overflow)
    }
}

/// Next sqrt price after adding or removing `amount` of token1, rounded down.
pub fn get_next_sqrt_price_from_amount1_rounding_down(
    sqrt_px96: U160,
    liquidity: u128,
    amount: U256,
    add: bool,
) -> Result<U160, MathError> {
    let sqrt_p = wide(sqrt_px96);
    let liquidity: U256 = U256::from(liquidity);
    if add {
        let quotient: U256 = if amount <= MAX_U160 {
            (amount << 96usize)
                .checked_div(liquidity)
                .ok_or(MathError::DivisionByZero)?
        } else {
            mul_div(amount, Q96, liquidity)?
        };
        let next = sqrt_p.checked_add(quotient).ok_or(MathError::Overflow)?;
        U160::checked_from_limbs_slice(next.as_limbs()).ok_or(MathError::Overflow)
    } else {
        let quotient: U256 = if amount <= MAX_U160 {
            div_rounding_up(amount << 96usize, liquidity)
        } else {
            mul_div_rounding_up(amount, Q96, liquidity)?
        };
        if sqrt_p <= quotient {
            return Err(MathError::NextPriceOutOfRange);
        }
        Ok(U160::wrapping_from(sqrt_p - quotient))
    }
}

/// Next sqrt price after swapping `amount_in` into the pool.
pub fn get_next_sqrt_price_from_input(
    sqrt_px96: U160,
    liquidity: u128,
    amount_in: U256,
    zero_for_one: bool,
) -> Result<U160, MathError> {
    if sqrt_px96.is_zero() || liquidity == 0 {
        return Err(MathError::ZeroPriceOrLiquidity);
    }
    // Round so we never pass the target price.
    if zero_for_one {
        get_next_sqrt_price_from_amount0_rounding_up(sqrt_px96, liquidity, amount_in, true)
    } else {
        get_next_sqrt_price_from_amount1_rounding_down(sqrt_px96, liquidity, amount_in, true)
    }
}

/// Next sqrt price after taking `amount_out` out of the pool.
pub fn get_next_sqrt_price_from_output(
    sqrt_px96: U160,
    liquidity: u128,
    amount_out: U256,
    zero_for_one: bool,
) -> Result<U160, MathError> {
    if sqrt_px96.is_zero() || liquidity == 0 {
        return Err(MathError::ZeroPriceOrLiquidity);
    }
    // Round so we always reach the target price.
    if zero_for_one {
        get_next_sqrt_price_from_amount1_rounding_down(sqrt_px96, liquidity, amount_out, false)
    } else {
        get_next_sqrt_price_from_amount0_rounding_up(sqrt_px96, liquidity, amount_out, false)
    }
}

/// Token0 between two prices: `L * (sqrtB - sqrtA) / (sqrtA * sqrtB)`.
pub fn get_amount0_delta(
    sqrt_ratio_a_x96: U160,
    sqrt_ratio_b_x96: U160,
    liquidity: u128,
    round_up: bool,
) -> Result<U256, MathError> {
    let (a, b) = sorted(sqrt_ratio_a_x96, sqrt_ratio_b_x96);
    let numerator1: U256 = U256::from(liquidity) << 96;
    let numerator2 = b - a;
    if a.is_zero() {
        return Err(MathError::ZeroPriceOrLiquidity);
    }
    if round_up {
        Ok(div_rounding_up(
            mul_div_rounding_up(numerator1, numerator2, b)?,
            a,
        ))
    } else {
        Ok(mul_div(numerator1, numerator2, b)? / a)
    }
}

/// Token1 between two prices: `L * (sqrtB - sqrtA)`.
pub fn get_amount1_delta(
    sqrt_ratio_a_x96: U160,
    sqrt_ratio_b_x96: U160,
    liquidity: u128,
    round_up: bool,
) -> Result<U256, MathError> {
    let (a, b) = sorted(sqrt_ratio_a_x96, sqrt_ratio_b_x96);
    let liquidity = U256::from(liquidity);
    if round_up {
        mul_div_rounding_up(liquidity, b - a, Q96)
    } else {
        mul_div(liquidity, b - a, Q96)
    }
}

/// Signed token0 delta for a signed liquidity change (mint rounds up, burn down).
pub fn get_amount0_delta_signed(
    sqrt_ratio_a_x96: U160,
    sqrt_ratio_b_x96: U160,
    liquidity: i128,
) -> Result<I256, MathError> {
    signed(liquidity, |l, up| {
        get_amount0_delta(sqrt_ratio_a_x96, sqrt_ratio_b_x96, l, up)
    })
}

/// Signed token1 delta for a signed liquidity change (mint rounds up, burn down).
pub fn get_amount1_delta_signed(
    sqrt_ratio_a_x96: U160,
    sqrt_ratio_b_x96: U160,
    liquidity: i128,
) -> Result<I256, MathError> {
    signed(liquidity, |l, up| {
        get_amount1_delta(sqrt_ratio_a_x96, sqrt_ratio_b_x96, l, up)
    })
}

fn signed(
    liquidity: i128,
    f: impl Fn(u128, bool) -> Result<U256, MathError>,
) -> Result<I256, MathError> {
    // `uint128(-liquidity)` for int128.min wraps to 2^127 in 0.7, as unsigned_abs does.
    let amount = f(liquidity.unsigned_abs(), liquidity >= 0)?;
    let amount = I256::try_from(amount).map_err(|_| MathError::Overflow)?;
    Ok(if liquidity < 0 { -amount } else { amount })
}

#[inline]
fn sorted(a: U160, b: U160) -> (U256, U256) {
    if a > b {
        (wide(b), wide(a))
    } else {
        (wide(a), wide(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::uint;

    fn price_one() -> U160 {
        U160::from(1u128 << 96)
    }

    #[test]
    fn zero_amount_returns_input_price() {
        let p = price_one();
        assert_eq!(
            get_next_sqrt_price_from_input(p, 1, U256::ZERO, true).unwrap(),
            p
        );
        assert_eq!(
            get_next_sqrt_price_from_input(p, 1, U256::ZERO, false).unwrap(),
            p
        );
    }

    #[test]
    fn input_of_one_tenth_token1_at_price_one() {
        // From SqrtPriceMath.spec.ts: 0.1 token1 into 1e18 liquidity at price 1.
        let next = get_next_sqrt_price_from_input(
            price_one(),
            10u128.pow(18),
            U256::from(10u128.pow(17)),
            false,
        )
        .unwrap();
        assert_eq!(next, uint!(87150978765690771352898345369_U160));
    }

    #[test]
    fn input_of_one_tenth_token0_at_price_one() {
        let next = get_next_sqrt_price_from_input(
            price_one(),
            10u128.pow(18),
            U256::from(10u128.pow(17)),
            true,
        )
        .unwrap();
        assert_eq!(next, uint!(72025602285694852357767227579_U160));
    }

    #[test]
    fn cannot_take_the_whole_reserve() {
        // Output of all of token1 at price 1 with liquidity 1 would need sqrtP -> 0.
        assert_eq!(
            get_next_sqrt_price_from_output(price_one(), 1, U256::from(1u8), true),
            Err(MathError::NextPriceOutOfRange)
        );
        assert_eq!(
            get_next_sqrt_price_from_output(price_one(), 1, U256::from(1u8), false),
            Err(MathError::NextPriceOutOfRange)
        );
    }

    #[test]
    fn amount_deltas_round_in_the_pools_favour() {
        let a = price_one();
        let b = uint!(87150978765690771352898345369_U160);
        let l = 10u128.pow(18);
        let up0 = get_amount0_delta(a, b, l, true).unwrap();
        let down0 = get_amount0_delta(a, b, l, false).unwrap();
        assert_eq!(up0, uint!(90909090909090910_U256));
        assert_eq!(up0 - down0, U256::from(1u8));
        let up1 = get_amount1_delta(a, b, l, true).unwrap();
        let down1 = get_amount1_delta(a, b, l, false).unwrap();
        assert_eq!(up1, uint!(100000000000000000_U256));
        assert_eq!(up1 - down1, U256::from(1u8));
        assert_eq!(
            get_amount0_delta(U160::ZERO, b, l, true),
            Err(MathError::ZeroPriceOrLiquidity)
        );
    }

    #[test]
    fn signed_helpers_flip_rounding() {
        let a = price_one();
        let b = uint!(87150978765690771352898345369_U160);
        let mint = get_amount0_delta_signed(a, b, 10i128.pow(18)).unwrap();
        let burn = get_amount0_delta_signed(a, b, -(10i128.pow(18))).unwrap();
        assert_eq!(mint + burn, I256::ONE);
    }
}
