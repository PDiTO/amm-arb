//! Property tests for invariants the Uniswap math must satisfy regardless of input.

use amm_math::{
    I256, U160, U256,
    v2::{MAX_RESERVE, get_amount_in, get_amount_out},
    v3::{
        V3Pool,
        tick_math::{
            MAX_SQRT_RATIO, MAX_TICK, MIN_SQRT_RATIO, MIN_TICK, get_sqrt_ratio_at_tick,
            get_tick_at_sqrt_ratio,
        },
    },
};
use proptest::prelude::*;

fn tick() -> impl Strategy<Value = i32> {
    MIN_TICK..=MAX_TICK
}

/// A sqrt price anywhere in `[MIN_SQRT_RATIO, MAX_SQRT_RATIO)`, log-uniform-ish.
fn sqrt_price() -> impl Strategy<Value = U160> {
    (33u32..160, any::<[u64; 3]>()).prop_map(|(bits, limbs)| {
        let raw = U160::from_limbs([limbs[0], limbs[1], limbs[2] & 0xffff_ffff]);
        let p = raw >> (160 - bits as usize);
        p.clamp(MIN_SQRT_RATIO, MAX_SQRT_RATIO - U160::from(1))
    })
}

fn reserve() -> impl Strategy<Value = u128> {
    (1u32..=112).prop_flat_map(|bits| {
        let max = if bits == 112 {
            MAX_RESERVE
        } else {
            (1u128 << bits) - 1
        };
        (max / 2).max(1)..=max
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn tick_math_round_trips(t in MIN_TICK..MAX_TICK) {
        // MAX_TICK maps to MAX_SQRT_RATIO, which is outside getTickAtSqrtRatio's domain.
        let p = get_sqrt_ratio_at_tick(t).unwrap();
        prop_assert_eq!(get_tick_at_sqrt_ratio(p).unwrap(), t);
    }

    #[test]
    fn sqrt_ratio_is_strictly_increasing(a in tick(), b in tick()) {
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        prop_assume!(lo != hi);
        prop_assert!(get_sqrt_ratio_at_tick(lo).unwrap() < get_sqrt_ratio_at_tick(hi).unwrap());
    }

    #[test]
    fn tick_at_price_brackets_the_price(p in sqrt_price()) {
        let t = get_tick_at_sqrt_ratio(p).unwrap();
        prop_assert!(get_sqrt_ratio_at_tick(t).unwrap() <= p);
        if t < MAX_TICK {
            prop_assert!(p < get_sqrt_ratio_at_tick(t + 1).unwrap());
        }
    }

    #[test]
    fn v2_out_then_in_never_undercharges(r_in in reserve(), r_out in reserve(), x in 1u128..=(1u128 << 100)) {
        let amount_in = U256::from(x);
        let out = get_amount_out(amount_in, U256::from(r_in), U256::from(r_out)).unwrap();
        prop_assume!(!out.is_zero());
        // Buying back exactly `out` never costs less than what produced it.
        let needed = get_amount_in(out, U256::from(r_in), U256::from(r_out)).unwrap();
        prop_assert!(needed <= amount_in);
        // And paying `needed` yields at least `out`.
        prop_assert!(get_amount_out(needed, U256::from(r_in), U256::from(r_out)).unwrap() >= out);
    }

    #[test]
    fn v2_output_is_monotonic_and_bounded(r_in in reserve(), r_out in reserve(), x in 1u128..(1u128 << 100)) {
        let a = get_amount_out(U256::from(x), U256::from(r_in), U256::from(r_out)).unwrap();
        let b = get_amount_out(U256::from(x + 1), U256::from(r_in), U256::from(r_out)).unwrap();
        prop_assert!(a <= b);
        prop_assert!(b < U256::from(r_out));
    }
}

/// A pool with a handful of random positions around a random price.
fn v3_pool() -> impl Strategy<Value = V3Pool> {
    let spacing = prop_oneof![
        Just((100u32, 1i32)),
        Just((500, 10)),
        Just((3000, 60)),
        Just((10_000, 200))
    ];
    (
        spacing,
        -200_000i32..200_000,
        prop::collection::vec((-400i32..400, 1i32..200, 12u32..26), 1..12),
    )
        .prop_map(|((fee, spacing), center, positions)| {
            let center = center - center.rem_euclid(spacing);
            let price = get_sqrt_ratio_at_tick(center).unwrap();
            let mut pool = V3Pool::new(price, center, 0, fee, spacing).unwrap();
            // Always keep some liquidity at the current price.
            pool.add_position(center - 50 * spacing, center + 50 * spacing, 10u128.pow(18))
                .unwrap();
            for (offset, width, exp) in positions {
                let lower = center + offset * spacing;
                pool.add_position(lower, lower + width * spacing, 10u128.pow(exp))
                    .unwrap();
            }
            pool
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn v3_exact_output_is_consistent_with_exact_input(
        pool in v3_pool(),
        zero_for_one in any::<bool>(),
        x in 1_000u128..(1u128 << 70),
    ) {
        let amount_in = U256::from(x);
        let Ok(out) = pool.quote_exact_input(zero_for_one, amount_in) else {
            return Ok(()); // ran out of liquidity: nothing to compare
        };
        prop_assume!(!out.is_zero());
        // Asking for exactly `out` never costs more than the input that produced it.
        let needed = pool.quote_exact_output(zero_for_one, out).unwrap();
        prop_assert!(needed <= amount_in, "needed {} > amount_in {}", needed, amount_in);
        // And that input buys at least `out` back.
        prop_assert!(pool.quote_exact_input(zero_for_one, needed).unwrap() >= out);
    }

    #[test]
    fn v3_swap_moves_price_in_the_right_direction(
        pool in v3_pool(),
        zero_for_one in any::<bool>(),
        x in 1u128..(1u128 << 90),
        exact_in in any::<bool>(),
    ) {
        let amount = I256::try_from(x).unwrap();
        let amount = if exact_in { amount } else { -amount };
        let r = pool.swap(zero_for_one, amount, None).unwrap();
        if zero_for_one {
            prop_assert!(r.sqrt_price_x96 <= pool.sqrt_price_x96);
            prop_assert!(!r.amount0.is_negative() && !r.amount1.is_positive());
        } else {
            prop_assert!(r.sqrt_price_x96 >= pool.sqrt_price_x96);
            prop_assert!(!r.amount1.is_negative() && !r.amount0.is_positive());
        }
        // The stored tick is always consistent with the stored price.
        prop_assert!(get_sqrt_ratio_at_tick(r.tick).unwrap() <= r.sqrt_price_x96);
        prop_assert!(r.sqrt_price_x96 < get_sqrt_ratio_at_tick(r.tick + 1).unwrap());
    }

    #[test]
    fn v3_output_is_monotonic_in_input(
        pool in v3_pool(),
        zero_for_one in any::<bool>(),
        x in 1u128..(1u128 << 64),
    ) {
        let a = pool.quote_exact_input(zero_for_one, U256::from(x));
        let b = pool.quote_exact_input(zero_for_one, U256::from(x) * U256::from(2));
        if let (Ok(a), Ok(b)) = (a, b) {
            prop_assert!(a <= b);
        }
    }
}
