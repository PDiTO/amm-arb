//! Pools and the set of pools a search runs over.

use alloy_primitives::{Address, U256};
use amm_math::{
    MathError,
    v2::V2Pair,
    v3::{V3Pool, swap_math::FEE_DENOMINATOR},
};

/// Token metadata, used for display only; all math is in raw units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Contract address.
    pub address: Address,
    /// Ticker, if known.
    pub symbol: String,
    /// ERC-20 decimals.
    pub decimals: u8,
}

/// AMM-specific state of a pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolState {
    /// Uniswap V2 constant product pair (0.3% fee).
    V2(V2Pair),
    /// Uniswap V3 concentrated liquidity pool.
    V3(V3Pool),
}

/// A pool between `token0 < token1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pool {
    /// Pool contract address.
    pub address: Address,
    /// Lower-sorted token.
    pub token0: Address,
    /// Higher-sorted token.
    pub token1: Address,
    /// Reserves or V3 state.
    pub state: PoolState,
}

impl Pool {
    /// Whether swapping `token_in` goes token0 -> token1.
    ///
    /// Panics if `token_in` is not one of the pool's tokens.
    pub fn zero_for_one(&self, token_in: Address) -> bool {
        if token_in == self.token0 {
            true
        } else {
            assert_eq!(token_in, self.token1, "token not in pool");
            false
        }
    }

    /// The other token of the pair.
    pub fn other(&self, token: Address) -> Address {
        if token == self.token0 {
            self.token1
        } else {
            self.token0
        }
    }

    /// Exact output for `amount_in` of `token_in`, as the pool contract would pay.
    pub fn quote(&self, token_in: Address, amount_in: U256) -> Result<U256, MathError> {
        let zero_for_one = self.zero_for_one(token_in);
        match &self.state {
            PoolState::V2(pair) => pair.amount_out(zero_for_one, amount_in),
            PoolState::V3(pool) => pool.quote_exact_input(zero_for_one, amount_in),
        }
    }

    /// Marginal output per unit input at the current price, after fees.
    ///
    /// Returns `None` when the pool cannot trade at the margin (empty reserves
    /// or no active V3 liquidity at the current tick).
    pub fn marginal_rate(&self, token_in: Address) -> Option<f64> {
        let zero_for_one = self.zero_for_one(token_in);
        match &self.state {
            PoolState::V2(pair) => {
                if pair.reserve0 == 0 || pair.reserve1 == 0 {
                    return None;
                }
                let (r_in, r_out) = if zero_for_one {
                    (pair.reserve0, pair.reserve1)
                } else {
                    (pair.reserve1, pair.reserve0)
                };
                Some(r_out as f64 / r_in as f64 * 0.997)
            }
            PoolState::V3(pool) => {
                if pool.liquidity == 0 {
                    return None;
                }
                let price1_per_0 = mid_price_v3(pool);
                let fee = 1.0 - f64::from(pool.fee) / f64::from(FEE_DENOMINATOR);
                Some(if zero_for_one {
                    price1_per_0 * fee
                } else {
                    fee / price1_per_0
                })
            }
        }
    }

    /// Mid price of token1 in units of token0 (raw units, no fee).
    pub fn mid_price(&self) -> f64 {
        match &self.state {
            PoolState::V2(pair) => pair.reserve1 as f64 / pair.reserve0 as f64,
            PoolState::V3(pool) => mid_price_v3(pool),
        }
    }

    /// Rough depth of the pool in units of `token`, for picking reference prices.
    pub fn depth(&self, token: Address) -> f64 {
        match &self.state {
            PoolState::V2(pair) => {
                if token == self.token0 {
                    pair.reserve0 as f64
                } else {
                    pair.reserve1 as f64
                }
            }
            PoolState::V3(pool) => {
                // Virtual reserves at the current price: x = L / sqrtP, y = L * sqrtP.
                let sqrt_p = u256_to_f64(U256::from(pool.sqrt_price_x96)) / 2f64.powi(96);
                let l = pool.liquidity as f64;
                if token == self.token0 {
                    l / sqrt_p
                } else {
                    l * sqrt_p
                }
            }
        }
    }

    /// Short human label, e.g. `V3 0.05%` or `V2 0.3%`.
    pub fn label(&self) -> String {
        match &self.state {
            PoolState::V2(_) => "V2 0.3%".to_owned(),
            PoolState::V3(p) => format!("V3 {}%", f64::from(p.fee) / 10_000.0),
        }
    }
}

fn mid_price_v3(pool: &V3Pool) -> f64 {
    let sqrt_p = u256_to_f64(U256::from(pool.sqrt_price_x96)) / 2f64.powi(96);
    sqrt_p * sqrt_p
}

/// Lossy conversion for weights and display. Never used for amounts.
pub fn u256_to_f64(x: U256) -> f64 {
    let (hi, exp) = x.most_significant_bits();
    hi as f64 * 2f64.powi(exp as i32)
}

/// Tokens and pools a search runs over, plus the token gas is paid in.
#[derive(Debug, Clone)]
pub struct Market {
    /// Known tokens.
    pub tokens: Vec<Token>,
    /// All pools.
    pub pools: Vec<Pool>,
    /// Token gas costs are denominated in (WETH on mainnet).
    pub gas_token: Address,
}

impl Market {
    /// Pool by address.
    pub fn pool(&self, address: Address) -> Option<&Pool> {
        self.pools.iter().find(|p| p.address == address)
    }

    /// Token metadata by address.
    pub fn token(&self, address: Address) -> Option<&Token> {
        self.tokens.iter().find(|t| t.address == address)
    }

    /// Symbol for display, falling back to a shortened address.
    pub fn symbol(&self, address: Address) -> String {
        self.token(address)
            .map_or_else(|| format!("{address:#}"), |t| t.symbol.clone())
    }

    /// Value of one raw unit of `token` in raw units of the gas token, taken
    /// from the mid price of the deepest pool pairing the two.
    pub fn price_in_gas_token(&self, token: Address) -> Option<f64> {
        if token == self.gas_token {
            return Some(1.0);
        }
        self.pools
            .iter()
            .filter(|p| {
                (p.token0 == token && p.token1 == self.gas_token)
                    || (p.token1 == token && p.token0 == self.gas_token)
            })
            .filter(|p| p.marginal_rate(token).is_some())
            .max_by(|a, b| a.depth(self.gas_token).total_cmp(&b.depth(self.gas_token)))
            .map(|p| {
                let token1_per_token0 = p.mid_price();
                if p.token0 == token {
                    token1_per_token0
                } else {
                    1.0 / token1_per_token0
                }
            })
    }
}
