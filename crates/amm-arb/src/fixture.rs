//! JSON fixture format: a snapshot of pool state that can be scanned offline.
//!
//! Large integers are decimal strings so the files stay readable and survive
//! tools that parse JSON numbers as doubles.

use std::{fs, path::Path};

use alloy_primitives::{Address, aliases::U160};
use amm_math::{MathError, v2::V2Pair, v3::V3Pool};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::market::{Market, Pool, PoolState, Token};

/// Errors loading or validating a fixture.
#[derive(Debug, Error)]
pub enum FixtureError {
    /// File could not be read or written.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// File is not valid fixture JSON.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// A pool's state is inconsistent.
    #[error("pool {pool}: {source}")]
    Pool {
        /// Pool address.
        pool: Address,
        /// Underlying math error.
        source: MathError,
    },
    /// token0 must sort below token1.
    #[error("pool {0}: token0 must be < token1")]
    TokenOrder(Address),
}

/// A market snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    /// Free text: where the data came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Chain the snapshot was taken on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<u64>,
    /// Block the snapshot was taken at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<u64>,
    /// Token gas is paid in.
    pub gas_token: Address,
    /// Token metadata.
    pub tokens: Vec<TokenJson>,
    /// Pool state.
    pub pools: Vec<PoolJson>,
}

/// Token entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenJson {
    /// Address.
    pub address: Address,
    /// Symbol.
    pub symbol: String,
    /// Decimals.
    pub decimals: u8,
}

/// Initialized tick entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickJson {
    /// Tick index.
    pub tick: i32,
    /// `liquidityNet` at that tick.
    #[serde(with = "dec")]
    pub liquidity_net: i128,
}

/// Pool entry, tagged by `kind`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PoolJson {
    /// Uniswap V2 pair.
    V2 {
        /// Pair address.
        address: Address,
        /// token0.
        token0: Address,
        /// token1.
        token1: Address,
        /// reserve0.
        #[serde(with = "dec")]
        reserve0: u128,
        /// reserve1.
        #[serde(with = "dec")]
        reserve1: u128,
    },
    /// Uniswap V3 pool.
    V3 {
        /// Pool address.
        address: Address,
        /// token0.
        token0: Address,
        /// token1.
        token1: Address,
        /// Fee in hundredths of a bip.
        fee: u32,
        /// Tick spacing.
        tick_spacing: i32,
        /// `slot0.sqrtPriceX96`.
        #[serde(with = "dec")]
        sqrt_price_x96: U160,
        /// `slot0.tick`.
        tick: i32,
        /// Active liquidity.
        #[serde(with = "dec")]
        liquidity: u128,
        /// Every initialized tick in the loaded words.
        ticks: Vec<TickJson>,
        /// Inclusive range of bitmap words that were loaded; `None` means all.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        loaded_words: Option<(i16, i16)>,
    },
}

impl Fixture {
    /// Reads a fixture file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, FixtureError> {
        Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
    }

    /// Writes a fixture file, pretty-printed.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), FixtureError> {
        fs::write(path, serde_json::to_string_pretty(self)? + "\n")?;
        Ok(())
    }

    /// Validates and converts into a [`Market`].
    pub fn to_market(&self) -> Result<Market, FixtureError> {
        let tokens = self
            .tokens
            .iter()
            .map(|t| Token {
                address: t.address,
                symbol: t.symbol.clone(),
                decimals: t.decimals,
            })
            .collect();
        let pools = self
            .pools
            .iter()
            .map(pool_from_json)
            .collect::<Result<_, _>>()?;
        Ok(Market {
            tokens,
            pools,
            gas_token: self.gas_token,
        })
    }

    /// Snapshot of a [`Market`].
    pub fn from_market(market: &Market) -> Self {
        Self {
            description: None,
            chain_id: None,
            block: None,
            gas_token: market.gas_token,
            tokens: market
                .tokens
                .iter()
                .map(|t| TokenJson {
                    address: t.address,
                    symbol: t.symbol.clone(),
                    decimals: t.decimals,
                })
                .collect(),
            pools: market.pools.iter().map(pool_to_json).collect(),
        }
    }
}

fn pool_from_json(p: &PoolJson) -> Result<Pool, FixtureError> {
    let (address, token0, token1, state) = match p {
        PoolJson::V2 {
            address,
            token0,
            token1,
            reserve0,
            reserve1,
        } => {
            let pair = V2Pair::new(*reserve0, *reserve1).map_err(|source| FixtureError::Pool {
                pool: *address,
                source,
            })?;
            (*address, *token0, *token1, PoolState::V2(pair))
        }
        PoolJson::V3 {
            address,
            token0,
            token1,
            fee,
            tick_spacing,
            sqrt_price_x96,
            tick,
            liquidity,
            ticks,
            loaded_words,
        } => {
            let err = |source| FixtureError::Pool {
                pool: *address,
                source,
            };
            let mut pool = V3Pool::new(*sqrt_price_x96, *tick, *liquidity, *fee, *tick_spacing)
                .map_err(err)?;
            for t in ticks {
                pool.insert_tick(t.tick, t.liquidity_net).map_err(err)?;
            }
            pool.set_loaded_words(loaded_words.map(|(lo, hi)| lo..=hi));
            (*address, *token0, *token1, PoolState::V3(pool))
        }
    };
    if token0 >= token1 {
        return Err(FixtureError::TokenOrder(address));
    }
    Ok(Pool {
        address,
        token0,
        token1,
        state,
    })
}

fn pool_to_json(p: &Pool) -> PoolJson {
    match &p.state {
        PoolState::V2(pair) => PoolJson::V2 {
            address: p.address,
            token0: p.token0,
            token1: p.token1,
            reserve0: pair.reserve0,
            reserve1: pair.reserve1,
        },
        PoolState::V3(pool) => PoolJson::V3 {
            address: p.address,
            token0: p.token0,
            token1: p.token1,
            fee: pool.fee,
            tick_spacing: pool.tick_spacing,
            sqrt_price_x96: pool.sqrt_price_x96,
            tick: pool.tick,
            liquidity: pool.liquidity,
            ticks: pool
                .ticks()
                .into_iter()
                .map(|(tick, liquidity_net)| TickJson {
                    tick,
                    liquidity_net,
                })
                .collect(),
            loaded_words: pool.bitmap().loaded_words().map(|r| (*r.start(), *r.end())),
        },
    }
}

/// Serde helpers for integers stored as decimal strings.
mod dec {
    use std::{fmt::Display, str::FromStr};

    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<T: Display, S: Serializer>(v: &T, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(v)
    }

    pub fn deserialize<'de, T, D>(d: D) -> Result<T, D::Error>
    where
        T: FromStr,
        T::Err: Display,
        D: Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        s.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::planted_market;

    #[test]
    fn round_trips_through_json() {
        let market = planted_market();
        let json = serde_json::to_string(&Fixture::from_market(&market)).unwrap();
        let back: Fixture = serde_json::from_str(&json).unwrap();
        let again = back.to_market().unwrap();
        assert_eq!(again.pools, market.pools);
        assert_eq!(again.tokens, market.tokens);
    }
}
