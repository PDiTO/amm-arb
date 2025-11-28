//! Property tests for cycle detection and input sizing.

use amm_arb::{
    U256,
    cycles::{enumerate_negative_cycles, find_negative_cycle},
    graph::TokenGraph,
    market::{Market, PoolState, Token},
    optimize::optimize,
    scan::{ScanConfig, cycle_output, scan},
    synthetic::{Kind, Listing, build_market, consistent_market, token_address},
};
use amm_math::v2::V2Pair;
use proptest::prelude::*;

/// Token values as ticks relative to token 0, multiples of 600 so every fee tier is exact.
fn values() -> impl Strategy<Value = Vec<i32>> {
    prop::collection::vec(-80i32..80, 2..5).prop_map(|mut v| {
        v.insert(0, 0);
        v.iter().map(|x| x * 600).collect()
    })
}

/// Consistent market with one pool's price pushed off by `skew`.
fn planted(values: &[i32], which: usize, skew: f64) -> (Market, usize) {
    let mut market = consistent_market(values, 3);
    let n = market.pools.len();
    let which = which % n;
    let skewed = {
        let tokens: Vec<Token> = market.tokens.clone();
        let t0 = tokens
            .iter()
            .position(|t| t.address == market.pools[which].token0)
            .unwrap();
        let t1 = tokens
            .iter()
            .position(|t| t.address == market.pools[which].token1)
            .unwrap();
        let kind = match &market.pools[which].state {
            PoolState::V2(_) => Kind::V2,
            PoolState::V3(p) => Kind::V3(p.fee),
        };
        build_market(
            tokens,
            values,
            &[Listing {
                a: t0,
                b: t1,
                kind,
                skew,
                depth: 5e21,
            }],
        )
        .pools
        .remove(0)
    };
    market.pools[which].state = skewed.state;
    (market, which)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn arbitrage_free_markets_have_no_negative_cycles(values in values(), seed in 0u64..100) {
        let market = consistent_market(&values, seed);
        let graph = TokenGraph::build(&market);
        prop_assert!(find_negative_cycle(&graph).is_none());
        prop_assert!(enumerate_negative_cycles(&graph, 3).is_empty());
        prop_assert!(scan(&market, &ScanConfig::default()).opportunities.is_empty());
    }

    #[test]
    fn planted_mispricing_is_found_and_every_cycle_uses_it(
        values in values(),
        which in 0usize..1000,
        bump in 0.01f64..0.08,
        cheaper in any::<bool>(),
    ) {
        // 1% or more beats the worst two-hop fee stack (0.3% + 0.3% plus tick rounding).
        let skew = if cheaper { 1.0 / (1.0 + bump) } else { 1.0 + bump };
        let (market, skewed) = planted(&values, which, skew);
        let graph = TokenGraph::build(&market);

        let cycle = find_negative_cycle(&graph);
        prop_assert!(cycle.is_some(), "SPFA missed the planted arbitrage");

        let cycles = enumerate_negative_cycles(&graph, 3);
        prop_assert!(!cycles.is_empty());
        for c in &cycles {
            // All other pools agree on price, so any arbitrage must touch the skewed one.
            prop_assert!(c.edges.iter().any(|&e| graph.edges[e].pool == skewed));
        }

        let report = scan(&market, &ScanConfig { gas_price_wei: 0, ..ScanConfig::default() });
        prop_assert!(!report.opportunities.is_empty());
        prop_assert!(report.opportunities[0].gross_profit > U256::ZERO);
    }

    #[test]
    fn profit_is_concave_along_a_planted_cycle(
        values in values(),
        which in 0usize..1000,
        bump in 0.01f64..0.08,
        x in 1_000u128..(1u128 << 70),
    ) {
        let (market, _) = planted(&values, which, 1.0 + bump);
        let graph = TokenGraph::build(&market);
        let cycle = enumerate_negative_cycles(&graph, 3).into_iter().next().unwrap();
        let legs: Vec<_> = cycle
            .edges
            .iter()
            .map(|&e| (graph.edges[e].pool, graph.tokens[graph.edges[e].from]))
            .collect();
        let profit = |x: u128| {
            cycle_output(&market, &legs, U256::from(x))
                .map(|out| out.to::<u128>() as i128 - x as i128)
        };
        // Midpoint concavity: p(x) >= (p(x/2) + p(3x/2)) / 2, up to rounding noise.
        if let (Some(lo), Some(mid), Some(hi)) = (profit(x / 2), profit(x), profit(x + x / 2)) {
            let noise = 8 + (mid.unsigned_abs() >> 40) as i128;
            prop_assert!(2 * mid + noise >= lo + hi, "lo {lo} mid {mid} hi {hi}");
        }
    }
}

/// Two V2 pairs on the same tokens with small reserves, so brute force is cheap.
fn small_v2_cycle(r: [u128; 4]) -> Market {
    let tokens = (0..2)
        .map(|i| Token {
            address: token_address(i),
            symbol: format!("T{i}"),
            decimals: 18,
        })
        .collect();
    let mut market = build_market(
        tokens,
        &[0, 0],
        &[
            Listing {
                a: 0,
                b: 1,
                kind: Kind::V2,
                skew: 1.0,
                depth: 1e6,
            },
            Listing {
                a: 0,
                b: 1,
                kind: Kind::V2,
                skew: 1.0,
                depth: 1e6,
            },
        ],
    );
    market.pools[0].state = PoolState::V2(V2Pair::new(r[0], r[1]).unwrap());
    market.pools[1].state = PoolState::V2(V2Pair::new(r[2], r[3]).unwrap());
    market
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(40))]

    #[test]
    fn golden_section_matches_brute_force(
        base in 20_000u128..60_000,
        skew_bp in 150u128..1500,
        depth2 in 1u128..4,
    ) {
        // Pool A sells token1 cheap, pool B buys it back at the fair price.
        let r = [base, base * (10_000 + skew_bp) / 10_000, base * depth2, base * depth2];
        let market = small_v2_cycle(r);
        let token0 = market.pools[0].token0;
        let legs = [(0usize, token0), (1usize, market.pools[0].token1)];
        let output = |x: U256| cycle_output(&market, &legs, x);

        let brute = (1..=base as u64)
            .filter_map(|x| output(U256::from(x)).map(|o| o.to::<u128>() as i128 - x as i128))
            .max()
            .unwrap();
        let found = optimize(output, 64).map_or(0, |s| s.profit().to::<u128>() as i128);
        // Each hop floors its output, so integer profit is concave only up to
        // about one unit per hop. The search is exact up to that noise.
        prop_assert!(found <= brute);
        prop_assert!(found + 2 >= brute, "found {found}, brute force {brute}");
    }
}
