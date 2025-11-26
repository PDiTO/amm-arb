//! Off-chain replica of `UniswapV3Pool.swap`.
//!
//! The loop below is the Solidity loop with the oracle, fee-growth and
//! protocol-fee bookkeeping removed. None of those affect the amounts or the
//! resulting price: protocol fees are carved out of `feeAmount` after the
//! amounts are fixed, and fee growth is only accounting for LPs.

use std::collections::HashMap;
use std::ops::RangeInclusive;

use alloy_primitives::{I256, U256, aliases::U160};

use super::{
    liquidity_math::add_delta,
    swap_math::{FEE_DENOMINATOR, compute_swap_step},
    tick_bitmap::TickBitmap,
    tick_math::{
        MAX_SQRT_RATIO, MAX_TICK, MIN_SQRT_RATIO, MIN_TICK, get_sqrt_ratio_at_tick,
        get_tick_at_sqrt_ratio,
    },
};
use crate::MathError;

/// In-memory state of a V3 pool: everything `swap` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V3Pool {
    /// Current `slot0.sqrtPriceX96`.
    pub sqrt_price_x96: U160,
    /// Current `slot0.tick`.
    pub tick: i32,
    /// Liquidity active at the current tick.
    pub liquidity: u128,
    /// Fee in hundredths of a basis point (500 = 0.05%).
    pub fee: u32,
    /// Tick spacing for the fee tier.
    pub tick_spacing: i32,
    bitmap: TickBitmap,
    liquidity_net: HashMap<i32, i128>,
}

/// Outcome of a simulated swap, in the pool's sign convention: positive
/// amounts flow into the pool, negative amounts flow out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapResult {
    /// Token0 delta from the pool's point of view.
    pub amount0: I256,
    /// Token1 delta from the pool's point of view.
    pub amount1: I256,
    /// Price after the swap.
    pub sqrt_price_x96: U160,
    /// Tick after the swap.
    pub tick: i32,
    /// Active liquidity after the swap.
    pub liquidity: u128,
    /// Number of initialized ticks crossed.
    pub ticks_crossed: u32,
}

impl SwapResult {
    /// Amount paid into the pool (the positive side).
    pub fn amount_in(&self, zero_for_one: bool) -> U256 {
        let v = if zero_for_one {
            self.amount0
        } else {
            self.amount1
        };
        v.into_raw()
    }

    /// Amount paid out of the pool (the negated negative side).
    pub fn amount_out(&self, zero_for_one: bool) -> U256 {
        let v = if zero_for_one {
            self.amount1
        } else {
            self.amount0
        };
        v.wrapping_neg().into_raw()
    }
}

impl V3Pool {
    /// Creates a pool with no initialized ticks.
    ///
    /// `tick` must be the tick the chain reports in `slot0`; it is not always
    /// `getTickAtSqrtRatio(sqrtPrice)` (after a swap that ends exactly on a tick
    /// boundary moving left, the pool stores `tick - 1`).
    pub fn new(
        sqrt_price_x96: U160,
        tick: i32,
        liquidity: u128,
        fee: u32,
        tick_spacing: i32,
    ) -> Result<Self, MathError> {
        if fee >= FEE_DENOMINATOR || tick_spacing <= 0 || !(MIN_TICK..=MAX_TICK).contains(&tick) {
            return Err(MathError::InvalidTick(tick));
        }
        Ok(Self {
            sqrt_price_x96,
            tick,
            liquidity,
            fee,
            tick_spacing,
            bitmap: TickBitmap::new(),
            liquidity_net: HashMap::new(),
        })
    }

    /// Marks `tick` initialized with the given `liquidityNet`.
    ///
    /// Ticks with `liquidityNet == 0` still matter: the swap loop stops at every
    /// initialized tick, and step boundaries change rounding.
    pub fn insert_tick(&mut self, tick: i32, liquidity_net: i128) -> Result<(), MathError> {
        if tick % self.tick_spacing != 0 || !(MIN_TICK..=MAX_TICK).contains(&tick) {
            return Err(MathError::InvalidTick(tick));
        }
        if !self.bitmap.is_initialized(tick, self.tick_spacing) {
            self.bitmap.flip_tick(tick, self.tick_spacing)?;
        }
        self.liquidity_net.insert(tick, liquidity_net);
        Ok(())
    }

    /// Adds a liquidity position, doing the same tick and active-liquidity
    /// bookkeeping as `UniswapV3Pool.mint` (token amounts are not tracked).
    pub fn add_position(
        &mut self,
        tick_lower: i32,
        tick_upper: i32,
        liquidity: u128,
    ) -> Result<(), MathError> {
        if tick_lower >= tick_upper {
            return Err(MathError::InvalidTick(tick_lower));
        }
        let delta = i128::try_from(liquidity).map_err(|_| MathError::LiquidityOverflow)?;
        let lower_net = self.liquidity_net.get(&tick_lower).copied().unwrap_or(0);
        let upper_net = self.liquidity_net.get(&tick_upper).copied().unwrap_or(0);
        let lower_net = lower_net
            .checked_add(delta)
            .ok_or(MathError::LiquidityOverflow)?;
        let upper_net = upper_net
            .checked_sub(delta)
            .ok_or(MathError::LiquidityOverflow)?;
        self.insert_tick(tick_lower, lower_net)?;
        self.insert_tick(tick_upper, upper_net)?;
        if (tick_lower..tick_upper).contains(&self.tick) {
            self.liquidity = add_delta(self.liquidity, delta)?;
        }
        Ok(())
    }

    /// Restricts the simulator to the bitmap words that were actually loaded.
    pub fn set_loaded_words(&mut self, range: Option<RangeInclusive<i16>>) {
        self.bitmap.set_loaded_words(range);
    }

    /// The tick bitmap, for inspection and cross-checking against chain.
    pub fn bitmap(&self) -> &TickBitmap {
        &self.bitmap
    }

    /// Initialized ticks and their `liquidityNet`, sorted by tick.
    pub fn ticks(&self) -> Vec<(i32, i128)> {
        let mut v: Vec<_> = self.liquidity_net.iter().map(|(t, n)| (*t, *n)).collect();
        v.sort_unstable();
        v
    }

    /// Simulates `swap(recipient, zeroForOne, amountSpecified, sqrtPriceLimitX96, data)`.
    ///
    /// `amount_specified > 0` is exact input, `< 0` exact output. With
    /// `sqrt_price_limit_x96 = None` the extreme limit (`MIN_SQRT_RATIO + 1` or
    /// `MAX_SQRT_RATIO - 1`) is used, as routers do.
    pub fn swap(
        &self,
        zero_for_one: bool,
        amount_specified: I256,
        sqrt_price_limit_x96: Option<U160>,
    ) -> Result<SwapResult, MathError> {
        if amount_specified.is_zero() {
            return Err(MathError::ZeroAmountSpecified);
        }
        let limit = sqrt_price_limit_x96.unwrap_or(if zero_for_one {
            MIN_SQRT_RATIO + U160::from(1)
        } else {
            MAX_SQRT_RATIO - U160::from(1)
        });
        let valid_limit = if zero_for_one {
            limit < self.sqrt_price_x96 && limit > MIN_SQRT_RATIO
        } else {
            limit > self.sqrt_price_x96 && limit < MAX_SQRT_RATIO
        };
        if !valid_limit {
            return Err(MathError::InvalidPriceLimit);
        }

        let exact_input = amount_specified.is_positive();
        let mut remaining = amount_specified;
        let mut calculated = I256::ZERO;
        let mut sqrt_price = self.sqrt_price_x96;
        let mut tick = self.tick;
        let mut liquidity = self.liquidity;
        let mut ticks_crossed = 0;

        while !remaining.is_zero() && sqrt_price != limit {
            let sqrt_price_start = sqrt_price;
            let (next, initialized) = self.bitmap.next_initialized_tick_within_one_word(
                tick,
                self.tick_spacing,
                zero_for_one,
            )?;
            // The bitmap does not know about MIN_TICK / MAX_TICK.
            let tick_next = next.clamp(MIN_TICK, MAX_TICK);
            let sqrt_price_next = get_sqrt_ratio_at_tick(tick_next)?;

            let target = if (zero_for_one && sqrt_price_next < limit)
                || (!zero_for_one && sqrt_price_next > limit)
            {
                limit
            } else {
                sqrt_price_next
            };
            let step = compute_swap_step(sqrt_price, target, liquidity, remaining, self.fee)?;
            sqrt_price = step.sqrt_ratio_next_x96;

            // `amountIn + feeAmount` is an unchecked add, then SafeCast.toInt256.
            let in_plus_fee = to_int256(step.amount_in.wrapping_add(step.fee_amount))?;
            let out = to_int256(step.amount_out)?;
            if exact_input {
                remaining = remaining.wrapping_sub(in_plus_fee);
                calculated = calculated.checked_sub(out).ok_or(MathError::Overflow)?;
            } else {
                remaining = remaining.wrapping_add(out);
                calculated = calculated
                    .checked_add(in_plus_fee)
                    .ok_or(MathError::Overflow)?;
            }

            if sqrt_price == sqrt_price_next {
                if initialized {
                    let net = *self
                        .liquidity_net
                        .get(&tick_next)
                        .ok_or(MathError::MissingTick(tick_next))?;
                    // Moving left, liquidityNet applies with the opposite sign.
                    let net = if zero_for_one {
                        net.wrapping_neg()
                    } else {
                        net
                    };
                    liquidity = add_delta(liquidity, net)?;
                    ticks_crossed += 1;
                }
                tick = if zero_for_one {
                    tick_next - 1
                } else {
                    tick_next
                };
            } else if sqrt_price != sqrt_price_start {
                // Recompute unless we are on a lower tick boundary and have not moved.
                tick = get_tick_at_sqrt_ratio(sqrt_price)?;
            }
        }

        let specified_used = amount_specified.wrapping_sub(remaining);
        let (amount0, amount1) = if zero_for_one == exact_input {
            (specified_used, calculated)
        } else {
            (calculated, specified_used)
        };
        Ok(SwapResult {
            amount0,
            amount1,
            sqrt_price_x96: sqrt_price,
            tick,
            liquidity,
            ticks_crossed,
        })
    }

    /// Output for exactly `amount_in`, failing if the pool cannot absorb all of it.
    pub fn quote_exact_input(
        &self,
        zero_for_one: bool,
        amount_in: U256,
    ) -> Result<U256, MathError> {
        let specified = to_int256(amount_in)?;
        let r = self.swap(zero_for_one, specified, None)?;
        if r.amount_in(zero_for_one) != amount_in {
            return Err(MathError::InsufficientLiquidity);
        }
        Ok(r.amount_out(zero_for_one))
    }

    /// Input required for exactly `amount_out`, failing on a partial fill.
    pub fn quote_exact_output(
        &self,
        zero_for_one: bool,
        amount_out: U256,
    ) -> Result<U256, MathError> {
        let specified = to_int256(amount_out)?.wrapping_neg();
        let r = self.swap(zero_for_one, specified, None)?;
        if r.amount_out(zero_for_one) != amount_out {
            return Err(MathError::InsufficientLiquidity);
        }
        Ok(r.amount_in(zero_for_one))
    }

    /// Applies a swap result, producing the post-swap pool.
    pub fn apply(&self, result: &SwapResult) -> Self {
        let mut next = self.clone();
        next.sqrt_price_x96 = result.sqrt_price_x96;
        next.tick = result.tick;
        next.liquidity = result.liquidity;
        next
    }
}

/// `SafeCast.toInt256`.
#[inline]
fn to_int256(x: U256) -> Result<I256, MathError> {
    I256::try_from(x).map_err(|_| MathError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPACING: i32 = 60;

    /// Price 1, one position of 1e18 liquidity over [-600, 600].
    fn simple_pool() -> V3Pool {
        let mut p = V3Pool::new(U160::from(1u128 << 96), 0, 0, 3000, SPACING).unwrap();
        p.add_position(-600, 600, 10u128.pow(18)).unwrap();
        p
    }

    #[test]
    fn add_position_tracks_active_liquidity_and_net() {
        let mut p = simple_pool();
        assert_eq!(p.liquidity, 10u128.pow(18));
        p.add_position(600, 1200, 5).unwrap();
        p.add_position(-1200, -600, 7).unwrap();
        assert_eq!(p.liquidity, 10u128.pow(18));
        assert_eq!(
            p.ticks(),
            vec![
                (-1200, 7),
                (-600, 10i128.pow(18) - 7),
                (600, 5 - 10i128.pow(18)),
                (1200, -5)
            ]
        );
    }

    fn i(x: i128) -> I256 {
        I256::try_from(x).unwrap()
    }

    #[test]
    fn small_swap_stays_in_range() {
        let pool = simple_pool();
        let r = pool.swap(true, i(10i128.pow(15)), None).unwrap();
        assert_eq!(r.amount0, i(10i128.pow(15)));
        assert!(r.amount1.is_negative());
        assert_eq!(r.ticks_crossed, 0);
        assert_eq!(r.liquidity, 10u128.pow(18));
        assert!(r.sqrt_price_x96 < pool.sqrt_price_x96);
    }

    #[test]
    fn large_swap_crosses_out_of_range_and_stops_at_limit() {
        let pool = simple_pool();
        // Far more than the position holds: the price runs to the limit.
        let r = pool.swap(true, i(10i128.pow(24)), None).unwrap();
        assert_eq!(r.ticks_crossed, 1);
        assert_eq!(r.liquidity, 0);
        assert_eq!(r.sqrt_price_x96, MIN_SQRT_RATIO + U160::from(1));
        assert!(r.amount0 < i(10i128.pow(24)));
        assert_eq!(
            pool.quote_exact_input(true, U256::from(10u128.pow(24))),
            Err(MathError::InsufficientLiquidity)
        );
    }

    #[test]
    fn rejects_bad_limits_and_zero_amounts() {
        let pool = simple_pool();
        assert_eq!(
            pool.swap(true, I256::ZERO, None),
            Err(MathError::ZeroAmountSpecified)
        );
        assert_eq!(
            pool.swap(true, i(1), Some(pool.sqrt_price_x96)),
            Err(MathError::InvalidPriceLimit)
        );
        assert_eq!(
            pool.swap(false, i(1), Some(MAX_SQRT_RATIO)),
            Err(MathError::InvalidPriceLimit)
        );
    }

    #[test]
    fn exact_output_round_trip() {
        let pool = simple_pool();
        let out = pool
            .quote_exact_input(false, U256::from(10u128.pow(16)))
            .unwrap();
        let back = pool.quote_exact_output(false, out).unwrap();
        assert!(back <= U256::from(10u128.pow(16)));
    }

    #[test]
    fn unloaded_word_is_an_error_not_a_wrong_quote() {
        let mut pool = simple_pool();
        pool.set_loaded_words(Some(0..=0));
        assert!(matches!(
            pool.swap(true, i(10i128.pow(15)), None),
            Err(MathError::TickWordNotLoaded(-1))
        ));
    }
}
