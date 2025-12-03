// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Test} from "forge-std/Test.sol";

import {ArbExecutor} from "../../src/ArbExecutor.sol";
import {IUniswapV2Pair} from "../../src/interfaces/IUniswapV2.sol";
import {IUniswapV3Pool} from "../../src/interfaces/IUniswapV3.sol";
import {PoolAddress} from "../../src/libraries/PoolAddress.sol";
import {MockERC20} from "./MockERC20.sol";
import {UniswapFixture} from "./UniswapFixture.sol";
import {V3Quoter} from "./UniswapHelpers.sol";

/// @notice Canonical Uniswap V2/V3 cores deployed locally, three tokens, and a
///         market where the V2 A/B pair sells B too cheaply.
///
///         A/B:  V2 pair selling B 2% cheap, V3 0.3% pool at price 1
///         B/C:  V2 pair and V3 0.05% pool, both at price 1
///         C/A:  V3 0.05% pool at price 1
abstract contract ArbSetup is Test, UniswapFixture {
    uint256 internal constant V2_DEPTH = 1_000e18;
    uint128 internal constant V3_LIQUIDITY = 1e23;
    uint160 internal constant PRICE_ONE = 2 ** 96;

    address internal owner = makeAddr("owner");
    ArbExecutor internal executor;
    V3Quoter internal quoter;

    MockERC20 internal tokenA;
    MockERC20 internal tokenB;
    MockERC20 internal tokenC;

    IUniswapV2Pair internal pairAB;
    IUniswapV2Pair internal pairBC;
    IUniswapV3Pool internal poolAB;
    IUniswapV3Pool internal poolBC;
    IUniswapV3Pool internal poolCA;

    function setUp() public virtual {
        _deployUniswap();
        quoter = new V3Quoter();
        tokenA = _newToken();
        tokenB = _newToken();
        tokenC = _newToken();

        // The vendored bytecode is the mainnet bytecode, so the mainnet init
        // code hashes are the right ones here too.
        executor = new ArbExecutor(
            owner, address(v2Factory), V2_PAIR_INIT_CODE_HASH, address(v3Factory), V3_POOL_INIT_CODE_HASH
        );

        pairAB = _createV2Pair(address(tokenA), address(tokenB), V2_DEPTH, V2_DEPTH * 10_200 / 10_000);
        pairBC = _createV2Pair(address(tokenB), address(tokenC), V2_DEPTH * 10, V2_DEPTH * 10);
        poolAB = _v3PoolAtPriceOne(tokenA, tokenB, 3000, 60);
        poolBC = _v3PoolAtPriceOne(tokenB, tokenC, 500, 10);
        poolCA = _v3PoolAtPriceOne(tokenC, tokenA, 500, 10);
    }

    function _v3PoolAtPriceOne(MockERC20 x, MockERC20 y, uint24 fee, int24 spacing)
        internal
        returns (IUniswapV3Pool pool)
    {
        pool = _createV3Pool(address(x), address(y), fee, PRICE_ONE);
        int24 width = (6000 / spacing) * spacing;
        _addLiquidity(pool, -width, width, V3_LIQUIDITY);
    }

    // ---------------------------------------------------------------------
    // Cycles
    // ---------------------------------------------------------------------

    function _hop(ArbExecutor.PoolKind kind, MockERC20 tokenIn, MockERC20 tokenOut, uint24 fee)
        internal
        pure
        returns (ArbExecutor.Hop memory)
    {
        return ArbExecutor.Hop(kind, address(tokenIn), address(tokenOut), fee);
    }

    /// @dev Buy cheap B on V2 (flash leg), sell it on V3. Profit in A.
    function _cycleV2Flash() internal view returns (ArbExecutor.Hop[] memory hops) {
        hops = new ArbExecutor.Hop[](2);
        hops[0] = _hop(ArbExecutor.PoolKind.V2, tokenA, tokenB, 0);
        hops[1] = _hop(ArbExecutor.PoolKind.V3, tokenB, tokenA, 3000);
    }

    /// @dev Same trade rotated to start with B: V3 flash leg, profit in B.
    function _cycleV3Flash() internal view returns (ArbExecutor.Hop[] memory hops) {
        hops = new ArbExecutor.Hop[](2);
        hops[0] = _hop(ArbExecutor.PoolKind.V3, tokenB, tokenA, 3000);
        hops[1] = _hop(ArbExecutor.PoolKind.V2, tokenA, tokenB, 0);
    }

    /// @dev A -> B on V2, B -> C -> A on the 0.05% V3 pools.
    function _cycleThreeHop() internal view returns (ArbExecutor.Hop[] memory hops) {
        hops = new ArbExecutor.Hop[](3);
        hops[0] = _hop(ArbExecutor.PoolKind.V2, tokenA, tokenB, 0);
        hops[1] = _hop(ArbExecutor.PoolKind.V3, tokenB, tokenC, 500);
        hops[2] = _hop(ArbExecutor.PoolKind.V3, tokenC, tokenA, 500);
    }

    /// @dev A -> B -> C on V2 pairs, C -> A on V3.
    function _cycleTwoV2Hops() internal view returns (ArbExecutor.Hop[] memory hops) {
        hops = new ArbExecutor.Hop[](3);
        hops[0] = _hop(ArbExecutor.PoolKind.V2, tokenA, tokenB, 0);
        hops[1] = _hop(ArbExecutor.PoolKind.V2, tokenB, tokenC, 0);
        hops[2] = _hop(ArbExecutor.PoolKind.V3, tokenC, tokenA, 500);
    }

    /// @dev The losing direction: sells B where it is cheap.
    function _cycleBackwards() internal view returns (ArbExecutor.Hop[] memory hops) {
        hops = new ArbExecutor.Hop[](2);
        hops[0] = _hop(ArbExecutor.PoolKind.V3, tokenA, tokenB, 3000);
        hops[1] = _hop(ArbExecutor.PoolKind.V2, tokenB, tokenA, 0);
    }

    // ---------------------------------------------------------------------
    // Independent quoting, to predict what the executor should earn
    // ---------------------------------------------------------------------

    /// @dev Output of running the cycle, from the V2 formula on live reserves
    ///      and the real V3 pools via a revert quoter. `ok` is false if any
    ///      hop would revert or partially fill.
    function _quoteCycle(ArbExecutor.Hop[] memory hops, uint256 amountIn) internal returns (uint256 amount, bool ok) {
        amount = amountIn;
        for (uint256 i; i < hops.length; ++i) {
            (amount, ok) = _quoteHop(hops[i], amount);
            if (!ok) return (0, false);
        }
        return (amount, true);
    }

    function _quoteHop(ArbExecutor.Hop memory hop, uint256 amountIn) internal returns (uint256, bool) {
        bool zeroForOne = hop.tokenIn < hop.tokenOut;
        if (hop.kind == ArbExecutor.PoolKind.V2) {
            address pair = PoolAddress.v2Pair(address(v2Factory), V2_PAIR_INIT_CODE_HASH, hop.tokenIn, hop.tokenOut);
            (uint112 r0, uint112 r1,) = IUniswapV2Pair(pair).getReserves();
            (uint256 rIn, uint256 rOut) = zeroForOne ? (uint256(r0), uint256(r1)) : (uint256(r1), uint256(r0));
            uint256 withFee = amountIn * 997;
            uint256 out = withFee * rOut / (rIn * 1000 + withFee);
            return (out, out > 0 && out < rOut);
        }
        address pool =
            PoolAddress.v3Pool(address(v3Factory), V3_POOL_INIT_CODE_HASH, hop.tokenIn, hop.tokenOut, hop.fee);
        if (amountIn == 0 || amountIn > uint256(type(int256).max)) return (0, false);
        V3Quoter.Quote memory q =
            quoter.quote(pool, zeroForOne, int256(amountIn), zeroForOne ? MIN_SQRT_RATIO + 1 : MAX_SQRT_RATIO - 1);
        if (!q.ok) return (0, false);
        (int256 paid, int256 got) = zeroForOne ? (q.amount0, q.amount1) : (q.amount1, q.amount0);
        return (uint256(-got), uint256(paid) == amountIn);
    }

    function _balances(address who) internal view returns (uint256 a, uint256 b, uint256 c) {
        return (tokenA.balanceOf(who), tokenB.balanceOf(who), tokenC.balanceOf(who));
    }

    function _assertExecutorEmpty() internal view {
        (uint256 a, uint256 b, uint256 c) = _balances(address(executor));
        assertEq(a + b + c, 0, "executor holds tokens");
    }
}
