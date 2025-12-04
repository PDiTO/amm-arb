//! End-to-end scan: graph, candidate cycles, sizing, gas, ranking.

use std::time::{Duration, Instant};

use alloy_primitives::{Address, I256, U256};

use crate::{
    cycles::{Cycle, enumerate_negative_cycles, find_negative_cycle},
    graph::TokenGraph,
    market::{Market, PoolState, u256_to_f64},
    optimize::{DEFAULT_MAX_BITS, optimize},
};

/// Gas used by `ArbExecutor.execute`, split into a fixed part and a per-hop part.
///
/// Calibrated from `forge test --isolate --gas-report` on the executor tests
/// (each call its own transaction, cold storage, 21k intrinsic included):
///
/// | cycle              | gas     |
/// |--------------------|---------|
/// | V2 -> V3           | 196,718 |
/// | V2 -> V2 -> V3     | 243,845 |
/// | V2 -> V3 -> V3     | 278,980 |
///
/// which gives ~47k per V2 hop, ~82k per V3 hop and ~67k fixed. Swaps that
/// cross initialized V3 ticks cost more and are not modelled separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GasModel {
    /// Intrinsic gas, calldata and executor overhead.
    pub base: u64,
    /// Marginal gas for a V2 hop.
    pub per_v2_hop: u64,
    /// Marginal gas for a V3 hop.
    pub per_v3_hop: u64,
}

impl Default for GasModel {
    fn default() -> Self {
        Self {
            base: 67_000,
            per_v2_hop: 47_000,
            per_v3_hop: 82_000,
        }
    }
}

/// Scanner settings.
#[derive(Debug, Clone, Copy)]
pub struct ScanConfig {
    /// Longest cycle to consider (2 or 3 in practice).
    pub max_hops: usize,
    /// Gas price in wei.
    pub gas_price_wei: u128,
    /// Gas model.
    pub gas: GasModel,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            max_hops: 3,
            gas_price_wei: 10_000_000_000,
            gas: GasModel::default(),
        }
    }
}

/// One executed hop of a sized opportunity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    /// Pool traded on.
    pub pool: Address,
    /// Token paid in.
    pub token_in: Address,
    /// Token received.
    pub token_out: Address,
    /// Exact input.
    pub amount_in: U256,
    /// Exact output.
    pub amount_out: U256,
}

/// A sized, gas-adjusted arbitrage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opportunity {
    /// Token the cycle starts and ends in; `gross_profit` is in this token.
    pub start_token: Address,
    /// Hops with exact amounts.
    pub hops: Vec<Hop>,
    /// Optimal input.
    pub amount_in: U256,
    /// Exact output of the last hop.
    pub amount_out: U256,
    /// `amount_out - amount_in`, exact.
    pub gross_profit: U256,
    /// Estimated gas units.
    pub gas_units: u64,
    /// Gas cost in wei of the gas token.
    pub gas_cost_wei: U256,
    /// Gross profit minus gas, in wei of the gas token. Exact when the cycle
    /// starts in the gas token, otherwise converted at the deepest pool's mid price.
    pub net_profit_wei: I256,
    /// Quote evaluations spent sizing this cycle.
    pub evaluations: u32,
}

/// Everything a scan produced, for reporting.
#[derive(Debug, Clone)]
pub struct ScanReport {
    /// Tokens with at least one tradable pool.
    pub tokens: usize,
    /// Directed edges in the graph.
    pub edges: usize,
    /// Whether Bellman-Ford found any negative cycle at all.
    pub any_negative_cycle: bool,
    /// Negative cycles of `2..=max_hops` hops before sizing.
    pub candidates: usize,
    /// Opportunities with positive gross profit, best net profit first.
    pub opportunities: Vec<Opportunity>,
    /// Time spent building the graph and finding cycles.
    pub search_time: Duration,
    /// Time spent sizing candidates with exact quotes.
    pub sizing_time: Duration,
}

/// Runs a full scan over `market`.
pub fn scan(market: &Market, config: &ScanConfig) -> ScanReport {
    let t0 = Instant::now();
    let graph = TokenGraph::build(market);
    let any_negative_cycle = find_negative_cycle(&graph).is_some();
    let candidates = if any_negative_cycle {
        enumerate_negative_cycles(&graph, config.max_hops)
    } else {
        Vec::new()
    };
    let search_time = t0.elapsed();

    let t1 = Instant::now();
    let mut opportunities: Vec<Opportunity> = candidates
        .iter()
        .filter_map(|c| best_rotation(market, &graph, c, config))
        .collect();
    opportunities.sort_by_key(|o| std::cmp::Reverse(o.net_profit_wei));
    let sizing_time = t1.elapsed();

    ScanReport {
        tokens: graph.tokens.len(),
        edges: graph.edges.len(),
        any_negative_cycle,
        candidates: candidates.len(),
        opportunities,
        search_time,
        sizing_time,
    }
}

/// Sizes every rotation of `cycle` whose start token can be priced in the gas
/// token, and keeps the one with the best net profit.
fn best_rotation(
    market: &Market,
    graph: &TokenGraph,
    cycle: &Cycle,
    config: &ScanConfig,
) -> Option<Opportunity> {
    (0..cycle.len())
        .filter_map(|k| size_cycle(market, graph, &cycle.rotated(k), config))
        .max_by(|a, b| a.net_profit_wei.cmp(&b.net_profit_wei))
}

/// Pools and input tokens along a cycle.
fn legs(graph: &TokenGraph, cycle: &Cycle) -> Vec<(usize, Address)> {
    cycle
        .edges
        .iter()
        .map(|&e| {
            let edge = graph.edges[e];
            (edge.pool, graph.tokens[edge.from])
        })
        .collect()
}

/// Exact output of running `amount_in` through `legs`, or `None` if any hop reverts.
pub fn cycle_output(market: &Market, legs: &[(usize, Address)], amount_in: U256) -> Option<U256> {
    legs.iter()
        .try_fold(amount_in, |amount, &(pool, token_in)| {
            market.pools[pool].quote(token_in, amount).ok()
        })
}

/// Sizes one rotation of a cycle and prices it in the gas token.
pub fn size_cycle(
    market: &Market,
    graph: &TokenGraph,
    cycle: &Cycle,
    config: &ScanConfig,
) -> Option<Opportunity> {
    let legs = legs(graph, cycle);
    let start_token = legs[0].1;
    let price = market.price_in_gas_token(start_token)?;

    // The optimum is a small fraction of the first pool's depth (pushing twice
    // the reserve through a pool moves its price ~9x), so there is no point
    // bracketing beyond that. This also stops huge V3 quotes from walking the
    // bitmap all the way to MIN_TICK or MAX_TICK.
    let depth = market.pools[legs[0].0].depth(start_token) * 2.0;
    let max_bits = if depth.is_finite() && depth >= 1.0 {
        (depth.log2().ceil() as usize).clamp(8, DEFAULT_MAX_BITS)
    } else {
        DEFAULT_MAX_BITS
    };
    let sizing = optimize(|x| cycle_output(market, &legs, x), max_bits)?;

    let mut hops = Vec::with_capacity(legs.len());
    let mut amount = sizing.amount_in;
    for &(pool_idx, token_in) in &legs {
        let pool = &market.pools[pool_idx];
        let out = pool.quote(token_in, amount).ok()?;
        hops.push(Hop {
            pool: pool.address,
            token_in,
            token_out: pool.other(token_in),
            amount_in: amount,
            amount_out: out,
        });
        amount = out;
    }

    let gas_units = config.gas.base
        + legs
            .iter()
            .map(|&(p, _)| match market.pools[p].state {
                PoolState::V2(_) => config.gas.per_v2_hop,
                PoolState::V3(_) => config.gas.per_v3_hop,
            })
            .sum::<u64>();
    let gas_cost_wei = U256::from(gas_units) * U256::from(config.gas_price_wei);
    let gross_profit = sizing.profit();
    let gross_in_gas_token = if start_token == market.gas_token {
        I256::from_raw(gross_profit)
    } else {
        f64_to_i256(u256_to_f64(gross_profit) * price)
    };
    let net_profit_wei = gross_in_gas_token.saturating_sub(I256::from_raw(gas_cost_wei));

    Some(Opportunity {
        start_token,
        hops,
        amount_in: sizing.amount_in,
        amount_out: sizing.amount_out,
        gross_profit,
        gas_units,
        gas_cost_wei,
        net_profit_wei,
        evaluations: sizing.evaluations,
    })
}

fn f64_to_i256(x: f64) -> I256 {
    if !x.is_finite() || x.abs() < 1.0 {
        return I256::ZERO;
    }
    let magnitude = format!("{:.0}", x.abs());
    let v = I256::from_dec_str(&magnitude).unwrap_or(I256::MAX);
    if x < 0.0 { -v } else { v }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::{consistent_market, planted_market};

    #[test]
    fn planted_market_yields_a_profitable_opportunity() {
        let market = planted_market();
        let report = scan(&market, &ScanConfig::default());
        assert!(report.any_negative_cycle);
        let best = report.opportunities.first().expect("an opportunity");
        assert!(best.net_profit_wei.is_positive());
        // The hops chain together and the amounts are exact quotes.
        assert_eq!(best.hops.first().unwrap().token_in, best.start_token);
        assert_eq!(best.hops.last().unwrap().token_out, best.start_token);
        assert_eq!(best.hops.last().unwrap().amount_out, best.amount_out);
        assert_eq!(best.gross_profit, best.amount_out - best.amount_in);
    }

    #[test]
    fn consistent_market_yields_nothing() {
        let market = consistent_market(&[0, 600, -1200, 2400], 1);
        let report = scan(&market, &ScanConfig::default());
        assert!(!report.any_negative_cycle);
        assert_eq!(report.opportunities.len(), 0);
    }
}
