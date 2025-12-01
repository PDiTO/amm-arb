//! Loads live pool state over JSON-RPC into a [`Market`].
//!
//! Everything is read at one pinned block through Multicall3, so the snapshot
//! is internally consistent and a busy V3 pool costs a handful of requests
//! rather than one per tick.

use std::collections::BTreeSet;

use alloy::{
    eips::BlockId,
    primitives::{Address, address},
    providers::{DynProvider, Provider, ProviderBuilder},
    sol,
};
use amm_arb::market::{Market, Pool, PoolState, Token};
use amm_math::{
    v2::V2Pair,
    v3::{
        V3Pool,
        tick_math::{MAX_TICK, MIN_TICK},
    },
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Canonical WETH, used as the gas token on mainnet.
pub const WETH: Address = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");

sol! {
    #[sol(rpc)]
    interface IUniswapV2Pair {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    }

    #[sol(rpc)]
    interface IUniswapV3Pool {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function fee() external view returns (uint24);
        function tickSpacing() external view returns (int24);
        function liquidity() external view returns (uint128);
        function slot0() external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint16 observationIndex,
            uint16 observationCardinality,
            uint16 observationCardinalityNext,
            uint8 feeProtocol,
            bool unlocked
        );
        function tickBitmap(int16 wordPosition) external view returns (uint256);
        function ticks(int24 tick) external view returns (
            uint128 liquidityGross,
            int128 liquidityNet,
            uint256 feeGrowthOutside0X128,
            uint256 feeGrowthOutside1X128,
            int56 tickCumulativeOutside,
            uint160 secondsPerLiquidityOutsideX128,
            uint32 secondsOutside,
            bool initialized
        );
    }

    #[sol(rpc)]
    interface IERC20Metadata {
        function symbol() external view returns (string);
        function decimals() external view returns (uint8);
    }
}

/// One entry of a pool list file.
#[derive(Debug, Clone, Deserialize)]
pub struct PoolRef {
    /// `v2` or `v3`.
    pub kind: String,
    /// Pool address.
    pub address: Address,
}

/// The default pool list: deep mainnet pools across WETH, USDC, USDT, DAI, WBTC.
pub fn default_pools() -> Vec<PoolRef> {
    serde_json::from_str(include_str!("mainnet_pools.json")).expect("embedded pool list is valid")
}

/// Connection plus the block every read is pinned to.
pub struct Loader {
    provider: DynProvider,
    /// Block number all state is read at.
    pub block: u64,
    /// Chain id reported by the node.
    pub chain_id: u64,
    /// Ticks of range to load on each side of the current V3 tick.
    pub tick_range: i32,
}

impl Loader {
    /// Connects and pins to `block`, or to the latest block if `None`.
    pub async fn connect(url: &str, block: Option<u64>, tick_range: i32) -> Result<Self> {
        let provider = ProviderBuilder::new()
            .connect_http(url.parse().context("invalid RPC URL")?)
            .erased();
        let chain_id = provider
            .get_chain_id()
            .await
            .context("eth_chainId failed")?;
        let block = match block {
            Some(b) => b,
            None => provider
                .get_block_number()
                .await
                .context("eth_blockNumber failed")?,
        };
        Ok(Self {
            provider,
            block,
            chain_id,
            tick_range,
        })
    }

    fn at(&self) -> BlockId {
        BlockId::number(self.block)
    }

    /// Loads every pool in `refs` plus token metadata.
    pub async fn load(&self, refs: &[PoolRef]) -> Result<Market> {
        let mut pools = Vec::with_capacity(refs.len());
        for r in refs {
            let pool = match r.kind.as_str() {
                "v2" => self.load_v2(r.address).await,
                "v3" => self.load_v3(r.address).await,
                other => bail!("unknown pool kind {other:?} for {}", r.address),
            }
            .with_context(|| format!("loading {} pool {}", r.kind, r.address))?;
            pools.push(pool);
        }
        let token_addresses: BTreeSet<Address> = pools
            .iter()
            .flat_map(|p: &Pool| [p.token0, p.token1])
            .collect();
        let tokens = self.load_tokens(&token_addresses).await?;
        Ok(Market {
            tokens,
            pools,
            gas_token: WETH,
        })
    }

    async fn load_v2(&self, address: Address) -> Result<Pool> {
        let pair = IUniswapV2Pair::new(address, &self.provider);
        let (token0, token1, reserves) = self
            .provider
            .multicall()
            .add(pair.token0())
            .add(pair.token1())
            .add(pair.getReserves())
            .block(self.at())
            .aggregate()
            .await?;
        Ok(Pool {
            address,
            token0,
            token1,
            state: PoolState::V2(V2Pair::new(reserves.reserve0.to(), reserves.reserve1.to())?),
        })
    }

    async fn load_v3(&self, address: Address) -> Result<Pool> {
        let pool = IUniswapV3Pool::new(address, &self.provider);
        let (token0, token1, fee, spacing, slot0, liquidity) = self
            .provider
            .multicall()
            .add(pool.token0())
            .add(pool.token1())
            .add(pool.fee())
            .add(pool.tickSpacing())
            .add(pool.slot0())
            .add(pool.liquidity())
            .block(self.at())
            .aggregate()
            .await?;
        let spacing = spacing.as_i32();
        let tick = slot0.tick.as_i32();
        let mut state = V3Pool::new(
            slot0.sqrtPriceX96,
            tick,
            liquidity,
            fee.to::<u32>(),
            spacing,
        )?;

        // Bitmap words covering `tick_range` ticks either side of the current
        // tick. One word spans 256 * spacing ticks, so a 0.01% pool (spacing 1)
        // needs far more words than a 1% pool (spacing 200) for the same range.
        // Always take at least two words either side as well.
        let word_of = |t: i32| (t.div_euclid(spacing) >> 8) as i16;
        let lo = word_of((tick - self.tick_range).max(MIN_TICK)).min(word_of(tick) - 2);
        let hi = word_of((tick + self.tick_range).min(MAX_TICK)).max(word_of(tick) + 2);
        let mut words = self
            .provider
            .multicall()
            .dynamic::<IUniswapV3Pool::tickBitmapCall>();
        for w in lo..=hi {
            words = words.add_dynamic(pool.tickBitmap(w));
        }
        let words = words.block(self.at()).aggregate().await?;

        // Every set bit is an initialized tick; fetch its liquidityNet.
        let mut initialized = Vec::new();
        for (w, word) in (lo..=hi).zip(words) {
            for bit in 0..256usize {
                if word.bit(bit) {
                    initialized.push((i32::from(w) * 256 + bit as i32) * spacing);
                }
            }
        }
        // Keep archive calls small enough for public RPC execution limits.
        for chunk in initialized.chunks(64) {
            let mut calls = self
                .provider
                .multicall()
                .dynamic::<IUniswapV3Pool::ticksCall>();
            for &t in chunk {
                calls = calls.add_dynamic(pool.ticks(t.try_into()?));
            }
            for (&t, info) in chunk.iter().zip(calls.block(self.at()).aggregate().await?) {
                state.insert_tick(t, info.liquidityNet)?;
            }
        }
        state.set_loaded_words(Some(lo..=hi));

        Ok(Pool {
            address,
            token0,
            token1,
            state: PoolState::V3(state),
        })
    }

    async fn load_tokens(&self, addresses: &BTreeSet<Address>) -> Result<Vec<Token>> {
        let mut symbols = self
            .provider
            .multicall()
            .dynamic::<IERC20Metadata::symbolCall>();
        let mut decimals = self
            .provider
            .multicall()
            .dynamic::<IERC20Metadata::decimalsCall>();
        for &a in addresses {
            let token = IERC20Metadata::new(a, &self.provider);
            symbols = symbols.add_dynamic(token.symbol());
            decimals = decimals.add_dynamic(token.decimals());
        }
        let symbols = symbols.block(self.at()).aggregate().await?;
        let decimals = decimals.block(self.at()).aggregate().await?;
        Ok(addresses
            .iter()
            .zip(symbols.into_iter().zip(decimals))
            .map(|(&address, (symbol, decimals))| Token {
                address,
                symbol,
                decimals,
            })
            .collect())
    }
}
