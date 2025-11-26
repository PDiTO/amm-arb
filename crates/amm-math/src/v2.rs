//! Uniswap V2 constant-product quotes.
//!
//! [`get_amount_out`] and [`get_amount_in`] are line-for-line ports of
//! `UniswapV2Library` from v2-periphery, including its `SafeMath` overflow
//! checks. [`V2Pair`] adds the checks the pair contract itself makes on top of
//! the library (output strictly below the reserve, reserves stay in `uint112`).

use alloy_primitives::U256;
use serde::{Deserialize, Serialize};

use crate::MathError;

/// Fee numerator: 0.3% fee means 997 of every 1000 input units count.
const FEE_NUMERATOR: u64 = 997;
const FEE_DENOMINATOR: u64 = 1000;

/// Largest value a V2 reserve can hold (`uint112`).
pub const MAX_RESERVE: u128 = (1u128 << 112) - 1;

/// Output amount for an exact input, identical to `UniswapV2Library.getAmountOut`.
///
/// `amountOut = amountIn * 997 * reserveOut / (reserveIn * 1000 + amountIn * 997)`,
/// rounded down. This is the largest output the pair's `k` check accepts for
/// `amount_in`, which the differential tests confirm against the real pair.
pub fn get_amount_out(
    amount_in: U256,
    reserve_in: U256,
    reserve_out: U256,
) -> Result<U256, MathError> {
    if amount_in.is_zero() {
        return Err(MathError::InsufficientInputAmount);
    }
    if reserve_in.is_zero() || reserve_out.is_zero() {
        return Err(MathError::InsufficientLiquidity);
    }
    let amount_in_with_fee = checked_mul(amount_in, U256::from(FEE_NUMERATOR))?;
    let numerator = checked_mul(amount_in_with_fee, reserve_out)?;
    let denominator = checked_mul(reserve_in, U256::from(FEE_DENOMINATOR))?
        .checked_add(amount_in_with_fee)
        .ok_or(MathError::Overflow)?;
    Ok(numerator / denominator)
}

/// Input amount for an exact output, identical to `UniswapV2Library.getAmountIn`.
///
/// `amountIn = reserveIn * amountOut * 1000 / ((reserveOut - amountOut) * 997) + 1`.
/// Note the unconditional `+ 1`: when the division is exact this overpays by one
/// wei, and the port keeps that behaviour.
pub fn get_amount_in(
    amount_out: U256,
    reserve_in: U256,
    reserve_out: U256,
) -> Result<U256, MathError> {
    if amount_out.is_zero() {
        return Err(MathError::InsufficientOutputAmount);
    }
    if reserve_in.is_zero() || reserve_out.is_zero() {
        return Err(MathError::InsufficientLiquidity);
    }
    let numerator = checked_mul(
        checked_mul(reserve_in, amount_out)?,
        U256::from(FEE_DENOMINATOR),
    )?;
    let remaining = reserve_out
        .checked_sub(amount_out)
        .ok_or(MathError::Overflow)?;
    let denominator = checked_mul(remaining, U256::from(FEE_NUMERATOR))?;
    if denominator.is_zero() {
        return Err(MathError::DivisionByZero);
    }
    (numerator / denominator)
        .checked_add(U256::from(1))
        .ok_or(MathError::Overflow)
}

fn checked_mul(a: U256, b: U256) -> Result<U256, MathError> {
    a.checked_mul(b).ok_or(MathError::Overflow)
}

/// Reserves of a Uniswap V2 pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct V2Pair {
    /// Reserve of `token0` (`uint112` on chain).
    pub reserve0: u128,
    /// Reserve of `token1` (`uint112` on chain).
    pub reserve1: u128,
}

impl V2Pair {
    /// Builds a pair, rejecting reserves that do not fit in `uint112`.
    pub fn new(reserve0: u128, reserve1: u128) -> Result<Self, MathError> {
        if reserve0 > MAX_RESERVE || reserve1 > MAX_RESERVE {
            return Err(MathError::ReserveOverflow);
        }
        Ok(Self { reserve0, reserve1 })
    }

    /// `(reserve_in, reserve_out)` for a swap direction.
    fn reserves(&self, zero_for_one: bool) -> (U256, U256) {
        if zero_for_one {
            (U256::from(self.reserve0), U256::from(self.reserve1))
        } else {
            (U256::from(self.reserve1), U256::from(self.reserve0))
        }
    }

    /// Output for an exact input, rejecting swaps the pair contract would revert.
    pub fn amount_out(&self, zero_for_one: bool, amount_in: U256) -> Result<U256, MathError> {
        let (reserve_in, reserve_out) = self.reserves(zero_for_one);
        let out = get_amount_out(amount_in, reserve_in, reserve_out)?;
        Self::check_swap(reserve_in, reserve_out, amount_in, out)?;
        Ok(out)
    }

    /// Input for an exact output, rejecting swaps the pair contract would revert.
    pub fn amount_in(&self, zero_for_one: bool, amount_out: U256) -> Result<U256, MathError> {
        let (reserve_in, reserve_out) = self.reserves(zero_for_one);
        if amount_out >= reserve_out {
            return Err(MathError::InsufficientLiquidity);
        }
        let amount_in = get_amount_in(amount_out, reserve_in, reserve_out)?;
        Self::check_swap(reserve_in, reserve_out, amount_in, amount_out)?;
        Ok(amount_in)
    }

    /// Reserves after swapping, or an error where `UniswapV2Pair.swap` reverts.
    pub fn after_swap(&self, zero_for_one: bool, amount_in: U256) -> Result<Self, MathError> {
        let (reserve_in, reserve_out) = self.reserves(zero_for_one);
        let out = self.amount_out(zero_for_one, amount_in)?;
        let new_in = (reserve_in + amount_in).to::<u128>();
        let new_out = (reserve_out - out).to::<u128>();
        Ok(if zero_for_one {
            Self::new(new_in, new_out)?
        } else {
            Self::new(new_out, new_in)?
        })
    }

    /// The pair's own guards: output strictly below the reserve and the new
    /// input-side balance still fits in `uint112`.
    fn check_swap(
        reserve_in: U256,
        reserve_out: U256,
        amount_in: U256,
        amount_out: U256,
    ) -> Result<(), MathError> {
        if amount_out.is_zero() {
            return Err(MathError::InsufficientOutputAmount);
        }
        if amount_out >= reserve_out {
            return Err(MathError::InsufficientLiquidity);
        }
        let new_in = reserve_in
            .checked_add(amount_in)
            .ok_or(MathError::ReserveOverflow)?;
        if new_in > U256::from(MAX_RESERVE) {
            return Err(MathError::ReserveOverflow);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(x: u128) -> U256 {
        U256::from(x)
    }

    #[test]
    fn amount_out_matches_hand_computation() {
        // 1 ETH into a 100 ETH / 200k USDC pool.
        let out = get_amount_out(
            u(10u128.pow(18)),
            u(100 * 10u128.pow(18)),
            u(200_000 * 10u128.pow(6)),
        )
        .unwrap();
        // 1e18*997*2e11 / (1e20*1000 + 997e18) = 1974316068.1... -> 1974316068
        assert_eq!(out, u(1_974_316_068));
    }

    #[test]
    fn amount_in_overpays_by_one_on_exact_division() {
        // reserve_in * out * 1000 / ((reserve_out - out) * 997) with an exact quotient.
        let r_in = u(997);
        let r_out = u(2000);
        let out = u(1000);
        // 997 * 1000 * 1000 / (1000 * 997) = 1000 exactly, then + 1.
        assert_eq!(get_amount_in(out, r_in, r_out).unwrap(), u(1001));
    }

    #[test]
    fn zero_and_empty_inputs_fail_like_the_library() {
        assert_eq!(
            get_amount_out(U256::ZERO, u(1), u(1)),
            Err(MathError::InsufficientInputAmount)
        );
        assert_eq!(
            get_amount_out(u(1), U256::ZERO, u(1)),
            Err(MathError::InsufficientLiquidity)
        );
        assert_eq!(
            get_amount_in(U256::ZERO, u(1), u(1)),
            Err(MathError::InsufficientOutputAmount)
        );
        assert_eq!(
            get_amount_in(u(5), u(10), u(5)),
            Err(MathError::DivisionByZero)
        );
        assert_eq!(get_amount_in(u(6), u(10), u(5)), Err(MathError::Overflow));
    }

    #[test]
    fn pair_rejects_reserve_overflow() {
        let pair = V2Pair::new(MAX_RESERVE - 10, MAX_RESERVE).unwrap();
        assert_eq!(
            pair.amount_out(true, u(11)),
            Err(MathError::ReserveOverflow)
        );
        assert!(V2Pair::new(MAX_RESERVE + 1, 1).is_err());
    }

    #[test]
    fn pair_rejects_dust_that_rounds_to_zero() {
        let pair = V2Pair::new(10u128.pow(24), 1000).unwrap();
        assert_eq!(
            pair.amount_out(true, u(1)),
            Err(MathError::InsufficientOutputAmount)
        );
    }
}
