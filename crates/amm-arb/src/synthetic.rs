//! Synthetic markets for tests, benchmarks and the demo fixture.
//!
//! Prices are expressed as ticks: token `i` is worth `1.0001^values[i]` raw
//! units of token 0. Using multiples of 600 (the least common multiple of the
//! tick spacings 1, 10, 60 and 200) keeps every pool price on a tick that every
//! fee tier can represent exactly, so a market built with no skew is
//! arbitrage-free up to float rounding in V2 reserves, far below any fee.

use alloy_primitives::{Address, U256};
use amm_math::{
    v2::V2Pair,
    v3::{V3Pool, tick_math::get_sqrt_ratio_at_tick},
};

use crate::market::{Market, Pool, PoolState, Token, u256_to_f64};

/// Deterministic token address; addresses increase with `i`.
pub fn token_address(i: usize) -> Address {
    let mut bytes = [0u8; 20];
    bytes[0] = 0x70;
    bytes[18..].copy_from_slice(&(i as u16 + 1).to_be_bytes());
    Address::from(bytes)
}

/// Deterministic pool address.
pub fn pool_address(i: usize) -> Address {
    let mut bytes = [0u8; 20];
    bytes[0] = 0xb0;
    bytes[18..].copy_from_slice(&(i as u16 + 1).to_be_bytes());
    Address::from(bytes)
}

/// Pool flavour in a listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Uniswap V2 pair.
    V2,
    /// Uniswap V3 pool with the given fee tier (100, 500, 3000 or 10000).
    V3(u32),
}

/// One pool to create between tokens `a` and `b`.
#[derive(Debug, Clone, Copy)]
pub struct Listing {
    /// First token index.
    pub a: usize,
    /// Second token index.
    pub b: usize,
    /// V2 or V3 fee tier.
    pub kind: Kind,
    /// Multiplier on the pool's token1-per-token0 price; 1.0 is fair.
    pub skew: f64,
    /// Liquidity, as the value of the token0 side in raw units of token 0.
    pub depth: f64,
}

fn spacing(fee: u32) -> i32 {
    match fee {
        100 => 1,
        500 => 10,
        3000 => 60,
        10_000 => 200,
        _ => panic!("unsupported fee tier {fee}"),
    }
}

/// Builds a market from token values and listings.
pub fn build_market(tokens: Vec<Token>, values: &[i32], listings: &[Listing]) -> Market {
    let pools = listings
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let (t0, t1) = if l.a < l.b { (l.a, l.b) } else { (l.b, l.a) };
            // Raw units of token1 per raw unit of token0.
            let price = 1.0001f64.powi(values[t0] - values[t1]) * l.skew;
            let value0 = 1.0001f64.powi(values[t0]);
            let reserve0 = l.depth / value0;
            let state = match l.kind {
                Kind::V2 => PoolState::V2(
                    V2Pair::new(reserve0 as u128, (reserve0 * price) as u128)
                        .expect("reserves fit uint112"),
                ),
                Kind::V3(fee) => {
                    let s = spacing(fee);
                    let raw_tick = (price.ln() / 1.0001f64.ln()).round() as i32;
                    let tick = raw_tick - raw_tick.rem_euclid(s);
                    let sqrt_p = get_sqrt_ratio_at_tick(tick).expect("tick in range");
                    let mut pool = V3Pool::new(sqrt_p, tick, 0, fee, s).expect("valid pool");
                    // Virtual reserve0 = L / sqrtP, so L = reserve0 * sqrtP.
                    let sqrt_f = u256_to_f64(U256::from(sqrt_p)) / 2f64.powi(96);
                    let liquidity = (reserve0 * sqrt_f) as u128;
                    let width = 4000 - 4000 % s;
                    pool.add_position(tick - width, tick + width, liquidity)
                        .expect("valid position");
                    PoolState::V3(pool)
                }
            };
            Pool {
                address: pool_address(i),
                token0: token_address(t0),
                token1: token_address(t1),
                state,
            }
        })
        .collect();
    Market {
        gas_token: tokens[0].address,
        tokens,
        pools,
    }
}

/// Arbitrage-free market: every pair gets a V2 pair and V3 pools at 0.05% and
/// 0.3%, all at the price implied by `values`. `seed` varies pool depths.
pub fn consistent_market(values: &[i32], seed: u64) -> Market {
    let tokens = (0..values.len())
        .map(|i| Token {
            address: token_address(i),
            symbol: format!("T{i}"),
            decimals: 18,
        })
        .collect();
    let mut listings = Vec::new();
    let mut depth = 1e22 * (1.0 + (seed % 7) as f64);
    for a in 0..values.len() {
        for b in a + 1..values.len() {
            for kind in [Kind::V2, Kind::V3(500), Kind::V3(3000)] {
                listings.push(Listing {
                    a,
                    b,
                    kind,
                    skew: 1.0,
                    depth,
                });
                depth *= 1.37;
                if depth > 1e24 {
                    depth /= 50.0;
                }
            }
        }
    }
    build_market(tokens, values, &listings)
}

/// Rough mainnet-like market with one planted mispricing: the V2 USDC/WETH
/// pair quotes WETH 1.2% cheaper than every other venue.
pub fn planted_market() -> Market {
    // (symbol, decimals, USD per whole token)
    let specs = [
        ("WETH", 18, 2500.0),
        ("USDC", 6, 1.0),
        ("DAI", 18, 1.0),
        ("WBTC", 8, 60_000.0),
    ];
    let usd_per_raw = |i: usize| specs[i].2 / 10f64.powi(specs[i].1);
    let values: Vec<i32> = (0..specs.len())
        .map(|i| {
            let t = ((usd_per_raw(i) / usd_per_raw(0)).ln() / 1.0001f64.ln()).round() as i32;
            t - t.rem_euclid(600)
        })
        .collect();
    let tokens = specs
        .iter()
        .enumerate()
        .map(|(i, (symbol, decimals, _))| Token {
            address: token_address(i),
            symbol: (*symbol).to_owned(),
            decimals: *decimals as u8,
        })
        .collect();

    // Depth in raw WETH units: 1e21 = 1,000 WETH on the token0 side.
    let (weth, usdc, dai, wbtc) = (0, 1, 2, 3);
    let listing = |a, b, kind, skew, weth_depth: f64| Listing {
        a,
        b,
        kind,
        skew,
        depth: weth_depth * 1e18,
    };
    // token0 of the WETH/USDC pair is WETH (lower address), so a skew above 1
    // means more USDC per WETH on this venue: WETH is expensive here.
    let listings = [
        listing(weth, usdc, Kind::V2, 1.012, 800.0),
        listing(weth, usdc, Kind::V3(500), 1.0, 4000.0),
        listing(weth, usdc, Kind::V3(3000), 1.0, 1500.0),
        listing(weth, dai, Kind::V2, 1.0, 600.0),
        listing(weth, dai, Kind::V3(3000), 1.0, 900.0),
        listing(usdc, dai, Kind::V3(100), 1.0, 2000.0),
        listing(weth, wbtc, Kind::V3(3000), 1.0, 3000.0),
        listing(weth, wbtc, Kind::V2, 1.0, 400.0),
        listing(usdc, wbtc, Kind::V3(3000), 1.0, 700.0),
    ];
    build_market(tokens, &values, &listings)
}
