//! Cross-checks the committed mainnet snapshot against Uniswap's own on-chain
//! quoters at the snapshot's block.
//!
//! Needs an archive-capable endpoint, so it only runs when `MAINNET_RPC_URL`
//! is set (for example `https://eth.drpc.org`). Otherwise it prints a note and
//! passes.

use std::path::PathBuf;

use alloy::{
    eips::BlockId,
    primitives::{
        Address, U256, address,
        aliases::{U24, U160},
    },
    providers::{Provider, ProviderBuilder},
    sol,
};
use amm_arb::{fixture::Fixture, market::PoolState};

sol! {
    struct QuoteExactInputSingleParams {
        address tokenIn;
        address tokenOut;
        uint256 amountIn;
        uint24 fee;
        uint160 sqrtPriceLimitX96;
    }

    #[sol(rpc)]
    interface IQuoterV2 {
        function quoteExactInputSingle(QuoteExactInputSingleParams memory params)
            external
            returns (uint256 amountOut, uint160 sqrtPriceX96After, uint32 initializedTicksCrossed, uint256 gasEstimate);
    }

    #[sol(rpc)]
    interface IUniswapV2Router02 {
        function getAmountsOut(uint256 amountIn, address[] calldata path) external view returns (uint256[] memory amounts);
    }
}

const QUOTER_V2: Address = address!("61fFE014bA17989E743c5F6cB21bF9697530B21e");
const ROUTER_02: Address = address!("7a250d5630B4cF539739dF2C5dAcb4c659F2488D");

#[tokio::test]
async fn snapshot_quotes_match_mainnet_quoters() {
    let Ok(url) = std::env::var("MAINNET_RPC_URL") else {
        eprintln!("MAINNET_RPC_URL not set; skipping the mainnet quoter cross-check");
        return;
    };
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mainnet-23700000.json");
    let fixture = Fixture::load(&path).unwrap();
    let block = BlockId::number(fixture.block.expect("fixture has a block"));
    let market = fixture.to_market().unwrap();
    let provider = ProviderBuilder::new().connect_http(url.parse().unwrap());
    let quoter = IQuoterV2::new(QUOTER_V2, &provider);
    let router = IUniswapV2Router02::new(ROUTER_02, &provider);

    let mut checked = 0;
    for pool in &market.pools {
        for token_in in [pool.token0, pool.token1] {
            let token_out = pool.other(token_in);
            let depth = pool.depth(token_in);
            // Fractions of the virtual reserve at the current price. For
            // concentrated pools that reserve is far larger than the real one,
            // so 1% is already a trade that crosses many ticks.
            for fraction in [1e-6, 1e-4, 1e-2] {
                let amount = U256::from((depth * fraction) as u128);
                if amount.is_zero() {
                    continue;
                }
                let ours = pool.quote(token_in, amount);
                let theirs = match &pool.state {
                    PoolState::V2(_) => router
                        .getAmountsOut(amount, vec![token_in, token_out])
                        .block(block)
                        .call()
                        .await
                        .map(|a| a[1]),
                    PoolState::V3(v3) => {
                        let params = QuoteExactInputSingleParams {
                            tokenIn: token_in,
                            tokenOut: token_out,
                            amountIn: amount,
                            fee: U24::from(v3.fee),
                            sqrtPriceLimitX96: U160::ZERO,
                        };
                        quoter
                            .quoteExactInputSingle(params)
                            .block(block)
                            .call()
                            .await
                            .map(|r| r.amountOut)
                    }
                };
                match (ours, theirs) {
                    (Ok(a), Ok(b)) => {
                        assert_eq!(a, b, "pool {} {} in {amount}", pool.address, pool.label());
                        checked += 1;
                    }
                    // Beyond the loaded bitmap words we refuse to quote; the
                    // chain still can. That is the intended trade-off.
                    (Err(e), Ok(_)) => eprintln!("skip {} {amount}: {e}", pool.address),
                    (_, Err(e)) => panic!("on-chain quote failed for {}: {e}", pool.address),
                }
            }
        }
    }
    eprintln!(
        "{checked} quotes matched mainnet to the wei at block {:?}",
        fixture.block
    );
    assert!(checked >= 60, "only {checked} quotes compared");
    let _ = provider.get_chain_id().await;
}
