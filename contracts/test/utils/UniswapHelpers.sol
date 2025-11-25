// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IUniswapV2Pair} from "../../src/interfaces/IUniswapV2.sol";
import {IUniswapV3MintCallback, IUniswapV3Pool, IUniswapV3SwapCallback} from "../../src/interfaces/IUniswapV3.sol";
import {MockERC20} from "./MockERC20.sol";

/// @notice Adds V3 liquidity, paying the pool by minting mock tokens on demand.
contract V3LiquidityProvider is IUniswapV3MintCallback {
    function mint(address pool, int24 tickLower, int24 tickUpper, uint128 liquidity) external {
        IUniswapV3Pool(pool).mint(address(this), tickLower, tickUpper, liquidity, abi.encode(pool));
    }

    function uniswapV3MintCallback(uint256 amount0Owed, uint256 amount1Owed, bytes calldata data) external {
        require(msg.sender == abi.decode(data, (address)), "not pool");
        if (amount0Owed > 0) MockERC20(IUniswapV3Pool(msg.sender).token0()).mint(msg.sender, amount0Owed);
        if (amount1Owed > 0) MockERC20(IUniswapV3Pool(msg.sender).token1()).mint(msg.sender, amount1Owed);
    }
}

/// @notice Quotes V3 swaps against the real pool by reverting inside the swap
///         callback, the same trick Uniswap's QuoterV2 uses. Can also execute
///         swaps for real, paying with freshly minted mock tokens.
contract V3Quoter is IUniswapV3SwapCallback {
    struct Quote {
        bool ok;
        int256 amount0;
        int256 amount1;
        uint160 sqrtPriceX96;
        int24 tick;
        uint128 liquidity;
    }

    error QuoteResult(int256 amount0, int256 amount1, uint160 sqrtPriceX96, int24 tick, uint128 liquidity);

    function quote(address pool, bool zeroForOne, int256 amountSpecified, uint160 sqrtPriceLimitX96)
        external
        returns (Quote memory q)
    {
        try IUniswapV3Pool(pool).swap(address(this), zeroForOne, amountSpecified, sqrtPriceLimitX96, abi.encode(true)) {
            revert("quote did not revert");
        } catch (bytes memory err) {
            if (err.length == 164 && bytes4(err) == QuoteResult.selector) {
                q.ok = true;
                assembly {
                    err := add(err, 4)
                }
                (q.amount0, q.amount1, q.sqrtPriceX96, q.tick, q.liquidity) =
                    abi.decode(err, (int256, int256, uint160, int24, uint128));
            }
        }
    }

    /// @notice Executes a real swap. Returns false instead of reverting on failure.
    function swap(address pool, bool zeroForOne, int256 amountSpecified, uint160 sqrtPriceLimitX96)
        external
        returns (bool ok, int256 amount0, int256 amount1)
    {
        try IUniswapV3Pool(pool).swap(address(this), zeroForOne, amountSpecified, sqrtPriceLimitX96, abi.encode(false))
        returns (int256 a0, int256 a1) {
            return (true, a0, a1);
        } catch {
            return (false, 0, 0);
        }
    }

    function uniswapV3SwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata data) external {
        IUniswapV3Pool pool = IUniswapV3Pool(msg.sender);
        if (abi.decode(data, (bool))) {
            (uint160 sqrtPriceX96, int24 tick,,,,,) = pool.slot0();
            revert QuoteResult(amount0Delta, amount1Delta, sqrtPriceX96, tick, pool.liquidity());
        }
        if (amount0Delta > 0) MockERC20(pool.token0()).mint(msg.sender, uint256(amount0Delta));
        if (amount1Delta > 0) MockERC20(pool.token1()).mint(msg.sender, uint256(amount1Delta));
    }
}

/// @notice Asks a real V2 pair whether it accepts a given swap, without keeping
///         any state change: the probe always reverts and reports the outcome.
contract V2Probe {
    error Accepted();

    function accepts(address pair, address tokenIn, uint256 amountIn, uint256 amount0Out, uint256 amount1Out)
        external
        returns (bool)
    {
        try this.attempt(pair, tokenIn, amountIn, amount0Out, amount1Out) {
            return false;
        } catch (bytes memory err) {
            return err.length == 4 && bytes4(err) == Accepted.selector;
        }
    }

    function attempt(address pair, address tokenIn, uint256 amountIn, uint256 amount0Out, uint256 amount1Out)
        external
    {
        require(msg.sender == address(this), "internal");
        MockERC20(tokenIn).mint(pair, amountIn);
        IUniswapV2Pair(pair).swap(amount0Out, amount1Out, address(this), "");
        revert Accepted();
    }
}
