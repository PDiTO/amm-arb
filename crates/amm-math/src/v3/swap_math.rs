//! Port of `SwapMath.sol`: one step of a swap within a single initialized-tick range.

use alloy_primitives::{I256, U256, aliases::U160};

use super::sqrt_price_math::{
    get_amount0_delta, get_amount1_delta, get_next_sqrt_price_from_input,
    get_next_sqrt_price_from_output,
};
use crate::{
    MathError,
    full_math::{mul_div, mul_div_rounding_up},
};

/// Fee denominator: fees are expressed in hundredths of a basis point.
pub const FEE_DENOMINATOR: u32 = 1_000_000;

/// Result of [`compute_swap_step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapStep {
    /// Price after the step, never past the target.
    pub sqrt_ratio_next_x96: U160,
    /// Input consumed by the step, excluding the fee.
    pub amount_in: U256,
    /// Output produced by the step.
    pub amount_out: U256,
    /// Fee charged on the input.
    pub fee_amount: U256,
}

/// Swaps as far toward `sqrt_ratio_target_x96` as `amount_remaining` allows.
///
/// A positive `amount_remaining` is an exact input (fee included), a negative one
/// an exact output. The direction is inferred from the two prices.
pub fn compute_swap_step(
    sqrt_ratio_current_x96: U160,
    sqrt_ratio_target_x96: U160,
    liquidity: u128,
    amount_remaining: I256,
    fee_pips: u32,
) -> Result<SwapStep, MathError> {
    let zero_for_one = sqrt_ratio_current_x96 >= sqrt_ratio_target_x96;
    let exact_in = !amount_remaining.is_negative();
    let fee_complement = U256::from(
        FEE_DENOMINATOR
            .checked_sub(fee_pips)
            .ok_or(MathError::Overflow)?,
    );
    let fee_denominator = U256::from(FEE_DENOMINATOR);
    // `uint256(-amountRemaining)`; wrapping_neg mirrors the unchecked negation.
    let amount_remaining_abs = if exact_in {
        amount_remaining.into_raw()
    } else {
        amount_remaining.wrapping_neg().into_raw()
    };

    let mut amount_in = U256::ZERO;
    let mut amount_out = U256::ZERO;
    let sqrt_ratio_next_x96;

    if exact_in {
        let amount_remaining_less_fee =
            mul_div(amount_remaining_abs, fee_complement, fee_denominator)?;
        amount_in = if zero_for_one {
            get_amount0_delta(sqrt_ratio_target_x96, sqrt_ratio_current_x96, liquidity, true)?
        } else {
            get_amount1_delta(sqrt_ratio_current_x96, sqrt_ratio_target_x96, liquidity, true)?
        };
        sqrt_ratio_next_x96 = if amount_remaining_less_fee >= amount_in {
            sqrt_ratio_target_x96
        } else {
            get_next_sqrt_price_from_input(
                sqrt_ratio_current_x96,
                liquidity,
                amount_remaining_less_fee,
                zero_for_one,
            )?
        };
    } else {
        amount_out = if zero_for_one {
            get_amount1_delta(sqrt_ratio_target_x96, sqrt_ratio_current_x96, liquidity, false)?
        } else {
            get_amount0_delta(sqrt_ratio_current_x96, sqrt_ratio_target_x96, liquidity, false)?
        };
        sqrt_ratio_next_x96 = if amount_remaining_abs >= amount_out {
            sqrt_ratio_target_x96
        } else {
            get_next_sqrt_price_from_output(
                sqrt_ratio_current_x96,
                liquidity,
                amount_remaining_abs,
                zero_for_one,
            )?
        };
    }

    let max = sqrt_ratio_target_x96 == sqrt_ratio_next_x96;

    if zero_for_one {
        if !(max && exact_in) {
            amount_in =
                get_amount0_delta(sqrt_ratio_next_x96, sqrt_ratio_current_x96, liquidity, true)?;
        }
        if !(max && !exact_in) {
            amount_out =
                get_amount1_delta(sqrt_ratio_next_x96, sqrt_ratio_current_x96, liquidity, false)?;
        }
    } else {
        if !(max && exact_in) {
            amount_in =
                get_amount1_delta(sqrt_ratio_current_x96, sqrt_ratio_next_x96, liquidity, true)?;
        }
        if !(max && !exact_in) {
            amount_out =
                get_amount0_delta(sqrt_ratio_current_x96, sqrt_ratio_next_x96, liquidity, false)?;
        }
    }

    // Cap the output so an exact-output swap never pays out more than asked.
    if !exact_in && amount_out > amount_remaining_abs {
        amount_out = amount_remaining_abs;
    }

    let fee_amount = if exact_in && sqrt_ratio_next_x96 != sqrt_ratio_target_x96 {
        // Target not reached: the rest of the input is the fee (unchecked in Solidity).
        amount_remaining_abs.wrapping_sub(amount_in)
    } else {
        mul_div_rounding_up(amount_in, U256::from(fee_pips), fee_complement)?
    };

    Ok(SwapStep {
        sqrt_ratio_next_x96,
        amount_in,
        amount_out,
        fee_amount,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::uint;

    fn i(x: i128) -> I256 {
        I256::try_from(x).unwrap()
    }

    #[test]
    fn exact_in_capped_at_target_one_for_zero() {
        // SwapMath.spec.ts: "exact amount in that gets capped at price target in one for zero".
        let price = U160::from(1u128 << 96);
        let target = uint!(79623317895830914510639640423_U160); // encodePriceSqrt(101, 100)
        let step = compute_swap_step(price, target, 2 * 10u128.pow(18), i(10i128.pow(18)), 600)
            .unwrap();
        assert_eq!(step.amount_in, uint!(9975124224178055_U256));
        assert_eq!(step.fee_amount, uint!(5988667735148_U256));
        assert_eq!(step.amount_out, uint!(9925619580021728_U256));
        assert_eq!(step.sqrt_ratio_next_x96, target);
    }

    #[test]
    fn exact_in_uses_whole_amount_when_target_not_reached() {
        let price = U160::from(1u128 << 96);
        let target = uint!(250541448375047931186413801569_U160); // encodePriceSqrt(1000, 100)
        let step = compute_swap_step(price, target, 2 * 10u128.pow(18), i(10i128.pow(18)), 600)
            .unwrap();
        assert_eq!(step.amount_in, uint!(999400000000000000_U256));
        assert_eq!(step.fee_amount, uint!(600000000000000_U256));
        assert_eq!(step.amount_out, uint!(666399946655997866_U256));
        assert_eq!(
            step.amount_in + step.fee_amount,
            U256::from(10u128.pow(18))
        );
    }

    #[test]
    fn exact_out_is_capped_at_requested_amount() {
        // "amount out is capped at the desired amount out"
        let step = compute_swap_step(
            uint!(417332158212080721273783715441582_U160),
            uint!(1452870262520218020823638996_U160),
            159344665391607089467575320103,
            i(-1),
            1,
        )
        .unwrap();
        assert_eq!(step.amount_in, U256::from(1u8));
        assert_eq!(step.fee_amount, U256::from(1u8));
        assert_eq!(step.amount_out, U256::from(1u8));
        assert_eq!(
            step.sqrt_ratio_next_x96,
            uint!(417332158212080721273783715441581_U160)
        );
    }

    #[test]
    fn entire_input_taken_as_fee() {
        // "entire input amount taken as fee"
        let step = compute_swap_step(
            U160::from(2413_u64),
            uint!(79887613182836312_U160),
            1985041575832132834610021537970,
            i(10),
            1872,
        )
        .unwrap();
        assert_eq!(step.amount_in, U256::ZERO);
        assert_eq!(step.fee_amount, U256::from(10u8));
        assert_eq!(step.amount_out, U256::ZERO);
        assert_eq!(step.sqrt_ratio_next_x96, U160::from(2413_u64));
    }
}
