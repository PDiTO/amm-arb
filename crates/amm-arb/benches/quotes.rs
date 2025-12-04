//! Quote and scan timings on real pool state.
//!
//! `cargo bench -p amm-arb --bench quotes`

// Criterion generates public benchmark harness functions.
#![allow(missing_docs)]

use std::hint::black_box;
use std::path::PathBuf;

use alloy_primitives::{Address, I256, U256, address};
use amm_arb::{
    fixture::Fixture,
    market::{Market, PoolState},
    scan::{ScanConfig, scan},
};
use amm_math::{
    U160,
    v2::get_amount_out,
    v3::tick_math::{get_sqrt_ratio_at_tick, get_tick_at_sqrt_ratio},
};
use criterion::{Criterion, criterion_group, criterion_main};

const USDC_WETH_500: Address = address!("88e6A0c2dDD26FEEb64F039a2c41296FcB3f5640");
const WETH: Address = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");

fn load(name: &str) -> Market {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    Fixture::load(path).unwrap().to_market().unwrap()
}

fn math(c: &mut Criterion) {
    let r_in = U256::from(10u128.pow(22));
    let r_out = U256::from(25u128 * 10u128.pow(12));
    c.bench_function("v2 getAmountOut", |b| {
        b.iter(|| get_amount_out(black_box(U256::from(10u128.pow(18))), r_in, r_out));
    });
    c.bench_function("v3 getSqrtRatioAtTick", |b| {
        b.iter(|| get_sqrt_ratio_at_tick(black_box(-197_351)));
    });
    let p = get_sqrt_ratio_at_tick(197_351).unwrap() + U160::from(12_345);
    c.bench_function("v3 getTickAtSqrtRatio", |b| {
        b.iter(|| get_tick_at_sqrt_ratio(black_box(p)));
    });
}

fn v3_swaps(c: &mut Criterion) {
    let market = load("mainnet-23700000.json");
    let pool = market.pool(USDC_WETH_500).unwrap();
    let PoolState::V3(v3) = &pool.state else {
        unreachable!()
    };
    let zero_for_one = pool.zero_for_one(WETH);
    for (label, weth) in [("1 WETH", 1u128), ("100 WETH", 100), ("1000 WETH", 1000)] {
        let amount = I256::try_from(weth * 10u128.pow(18)).unwrap();
        let crossed = v3.swap(zero_for_one, amount, None).unwrap().ticks_crossed;
        c.bench_function(
            &format!("v3 swap USDC/WETH 0.05%, {label} in ({crossed} ticks crossed)"),
            |b| b.iter(|| v3.swap(zero_for_one, black_box(amount), None)),
        );
    }
}

fn scans(c: &mut Criterion) {
    let config = ScanConfig::default();
    let planted = load("planted.json");
    let r = scan(&planted, &config);
    c.bench_function(
        &format!(
            "scan planted fixture ({} pools, {} cycles sized)",
            planted.pools.len(),
            r.opportunities.len()
        ),
        |b| b.iter(|| scan(black_box(&planted), &config)),
    );
    let mainnet = load("mainnet-23700000.json");
    c.bench_function(
        &format!(
            "scan mainnet snapshot ({} pools, no cycle)",
            mainnet.pools.len()
        ),
        |b| b.iter(|| scan(black_box(&mainnet), &config)),
    );
}

criterion_group!(benches, math, v3_swaps, scans);
criterion_main!(benches);
