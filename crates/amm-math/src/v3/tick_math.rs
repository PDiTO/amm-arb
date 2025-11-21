//! Port of `TickMath.sol`: conversion between ticks and Q64.96 sqrt prices.
//!
//! `price(tick) = 1.0001^tick`, so `sqrtPriceX96(tick) = sqrt(1.0001)^tick * 2^96`.
//! The Solidity computes this with a chain of precomputed Q128.128 multipliers,
//! one per bit of `|tick|`, and inverts for positive ticks. The inverse uses a
//! fixed-point `log2` followed by a change of base, and resolves the final
//! off-by-one with a single forward evaluation. Both are reproduced verbatim.

use alloy_primitives::{I256, U256, aliases::U160, uint};

use crate::MathError;

/// Smallest tick usable by any pool: `log_{1.0001}(2^-128)`.
pub const MIN_TICK: i32 = -887_272;
/// Largest tick usable by any pool.
pub const MAX_TICK: i32 = -MIN_TICK;

/// `getSqrtRatioAtTick(MIN_TICK)`.
pub const MIN_SQRT_RATIO: U160 = uint!(4295128739_U160);
/// `getSqrtRatioAtTick(MAX_TICK)`.
pub const MAX_SQRT_RATIO: U160 = uint!(1461446703485210103287273052203988822378723970342_U160);

/// `log_{sqrt(1.0001)}(2)` in Q64.64, so `log_2 (Q64.64) * this` is Q128.128.
const LOG_SQRT10001_OF_2: I256 = I256::from_raw(uint!(255738958999603826347141_U256));
/// Lower error bound of the log approximation (`0.010000497...` in Q128.128).
const TICK_LOW_ERR: I256 = I256::from_raw(uint!(3402992956809132418596140100660247210_U256));
/// Upper error bound of the log approximation (`0.856...` in Q128.128).
const TICK_HI_ERR: I256 = I256::from_raw(uint!(291339464771989622907027621153398088495_U256));

/// Q128.128 values of `1 / sqrt(1.0001)^(2^i)` for `i = 1..=19`, keyed by the
/// bit of `|tick|` they apply to. Bit 0 is the starting value, handled separately.
const MULTIPLIERS: [(u32, U256); 19] = [
    (0x2, uint!(0xfff97272373d413259a46990580e213a_U256)),
    (0x4, uint!(0xfff2e50f5f656932ef12357cf3c7fdcc_U256)),
    (0x8, uint!(0xffe5caca7e10e4e61c3624eaa0941cd0_U256)),
    (0x10, uint!(0xffcb9843d60f6159c9db58835c926644_U256)),
    (0x20, uint!(0xff973b41fa98c081472e6896dfb254c0_U256)),
    (0x40, uint!(0xff2ea16466c96a3843ec78b326b52861_U256)),
    (0x80, uint!(0xfe5dee046a99a2a811c461f1969c3053_U256)),
    (0x100, uint!(0xfcbe86c7900a88aedcffc83b479aa3a4_U256)),
    (0x200, uint!(0xf987a7253ac413176f2b074cf7815e54_U256)),
    (0x400, uint!(0xf3392b0822b70005940c7a398e4b70f3_U256)),
    (0x800, uint!(0xe7159475a2c29b7443b29c7fa6e889d9_U256)),
    (0x1000, uint!(0xd097f3bdfd2022b8845ad8f792aa5825_U256)),
    (0x2000, uint!(0xa9f746462d870fdf8a65dc1f90e061e5_U256)),
    (0x4000, uint!(0x70d869a156d2a1b890bb3df62baf32f7_U256)),
    (0x8000, uint!(0x31be135f97d08fd981231505542fcfa6_U256)),
    (0x10000, uint!(0x9aa508b5b7a84e1c677de54f3e99bc9_U256)),
    (0x20000, uint!(0x5d6af8dedb81196699c329225ee604_U256)),
    (0x40000, uint!(0x2216e584f5fa1ea926041bedfe98_U256)),
    (0x80000, uint!(0x48a170391f7dc42444e8fa2_U256)),
];

/// `sqrt(1.0001^tick) * 2^96`, rounded up, as a `uint160`.
///
/// Fails with [`MathError::TickOutOfBounds`] outside `[MIN_TICK, MAX_TICK]`.
pub fn get_sqrt_ratio_at_tick(tick: i32) -> Result<U160, MathError> {
    let abs_tick = tick.unsigned_abs();
    if abs_tick > MAX_TICK as u32 {
        return Err(MathError::TickOutOfBounds);
    }

    let mut ratio = if abs_tick & 0x1 != 0 {
        uint!(0xfffcb933bd6fad37aa2d162d1a594001_U256)
    } else {
        uint!(0x100000000000000000000000000000000_U256)
    };
    for (bit, multiplier) in MULTIPLIERS {
        if abs_tick & bit != 0 {
            // ratio <= 2^128 and multiplier < 2^128, so this never wraps.
            ratio = ratio.wrapping_mul(multiplier) >> 128;
        }
    }

    if tick > 0 {
        ratio = U256::MAX / ratio;
    }

    // Q128.128 -> Q128.96, rounding up so getTickAtSqrtRatio stays consistent.
    let round_up = if (ratio & U256::from(u32::MAX)).is_zero() {
        U256::ZERO
    } else {
        U256::from(1)
    };
    Ok(U160::wrapping_from((ratio >> 32) + round_up))
}

/// The greatest tick whose sqrt ratio is at or below `sqrt_price_x96`.
///
/// Fails with [`MathError::SqrtPriceOutOfBounds`] outside
/// `[MIN_SQRT_RATIO, MAX_SQRT_RATIO)`.
pub fn get_tick_at_sqrt_ratio(sqrt_price_x96: U160) -> Result<i32, MathError> {
    if sqrt_price_x96 < MIN_SQRT_RATIO || sqrt_price_x96 >= MAX_SQRT_RATIO {
        return Err(MathError::SqrtPriceOutOfBounds);
    }
    let ratio: U256 = U256::from(sqrt_price_x96) << 32;

    // Integer part of log2: the index of the most significant bit.
    let msb = ratio.bit_len() - 1;

    // Normalise so r sits in [2^127, 2^128).
    let mut r = if msb >= 128 {
        ratio >> (msb - 127)
    } else {
        ratio << (127 - msb)
    };

    // log_2 is a signed Q64.64 value; fractional bits are ORed into the raw
    // two's-complement word exactly as the assembly does.
    let mut log_2 = I256::try_from(msb as i64 - 128)
        .expect("small")
        .wrapping_shl(64)
        .into_raw();

    // 14 rounds of square-and-compare give 14 fractional bits (63 down to 50).
    for shift in (50..=63).rev() {
        r = r.wrapping_mul(r) >> 127;
        let f: U256 = r >> 128;
        log_2 |= f << shift;
        r >>= f.to::<usize>();
    }

    let log_2 = I256::from_raw(log_2);
    let log_sqrt10001 = log_2.wrapping_mul(LOG_SQRT10001_OF_2);
    let tick_low = log_sqrt10001.wrapping_sub(TICK_LOW_ERR).asr(128).low_i32();
    let tick_hi = log_sqrt10001.wrapping_add(TICK_HI_ERR).asr(128).low_i32();

    // Solidity truncates both to int24; they already fit.
    Ok(if tick_low == tick_hi {
        tick_low
    } else if get_sqrt_ratio_at_tick(tick_hi)? <= sqrt_price_x96 {
        tick_hi
    } else {
        tick_low
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_match_the_constants() {
        assert_eq!(get_sqrt_ratio_at_tick(MIN_TICK).unwrap(), MIN_SQRT_RATIO);
        assert_eq!(get_sqrt_ratio_at_tick(MAX_TICK).unwrap(), MAX_SQRT_RATIO);
        assert_eq!(get_sqrt_ratio_at_tick(MIN_TICK - 1), Err(MathError::TickOutOfBounds));
        assert_eq!(get_sqrt_ratio_at_tick(MAX_TICK + 1), Err(MathError::TickOutOfBounds));
    }

    #[test]
    fn tick_zero_is_price_one() {
        assert_eq!(get_sqrt_ratio_at_tick(0).unwrap(), U160::from(1u128 << 96));
        assert_eq!(get_tick_at_sqrt_ratio(U160::from(1u128 << 96)).unwrap(), 0);
    }

    #[test]
    fn known_values_from_the_uniswap_test_suite() {
        // From v3-core/test/TickMath.spec.ts.
        assert_eq!(get_sqrt_ratio_at_tick(MIN_TICK + 1).unwrap(), U160::from(4295343490_u64));
        assert_eq!(
            get_sqrt_ratio_at_tick(MAX_TICK - 1).unwrap(),
            uint!(1461373636630004318706518188784493106690254656249_U160)
        );
    }

    #[test]
    fn inverse_at_the_edges() {
        assert_eq!(get_tick_at_sqrt_ratio(MIN_SQRT_RATIO).unwrap(), MIN_TICK);
        assert_eq!(
            get_tick_at_sqrt_ratio(MAX_SQRT_RATIO - U160::from(1)).unwrap(),
            MAX_TICK - 1
        );
        assert_eq!(
            get_tick_at_sqrt_ratio(MIN_SQRT_RATIO - U160::from(1)),
            Err(MathError::SqrtPriceOutOfBounds)
        );
        assert_eq!(get_tick_at_sqrt_ratio(MAX_SQRT_RATIO), Err(MathError::SqrtPriceOutOfBounds));
    }
}
