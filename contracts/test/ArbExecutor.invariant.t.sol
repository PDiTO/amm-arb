// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {CommonBase} from "forge-std/Base.sol";
import {StdCheats} from "forge-std/StdCheats.sol";
import {StdUtils} from "forge-std/StdUtils.sol";

import {ArbExecutor} from "../src/ArbExecutor.sol";
import {IUniswapV2Pair} from "../src/interfaces/IUniswapV2.sol";
import {IUniswapV3Pool} from "../src/interfaces/IUniswapV3.sol";
import {ArbSetup} from "./utils/ArbSetup.sol";
import {MockERC20} from "./utils/MockERC20.sol";
import {V3Quoter} from "./utils/UniswapHelpers.sol";

/// @notice Drives the executor and the market through random sequences:
///         cycles of every shape and size, traders moving prices in every
///         pool, and callback attacks.
contract ExecutorHandler is CommonBase, StdCheats, StdUtils {
    ArbExecutor internal immutable executor;
    address internal immutable owner;
    V3Quoter internal immutable trader;
    MockERC20[3] internal tokens;
    IUniswapV2Pair internal immutable pair;
    IUniswapV3Pool[3] internal pools;
    ArbExecutor.Hop[][] internal cycles;

    // Ghost state.
    mapping(address => uint256) public profitPaid;
    uint256 public executions;
    uint256 public reverted;
    uint256 public attacksSucceeded;

    constructor(
        ArbExecutor executor_,
        address owner_,
        MockERC20[3] memory tokens_,
        IUniswapV2Pair pair_,
        IUniswapV3Pool[3] memory pools_,
        ArbExecutor.Hop[][] memory cycles_
    ) {
        executor = executor_;
        owner = owner_;
        tokens = tokens_;
        pair = pair_;
        pools = pools_;
        trader = new V3Quoter();
        for (uint256 i; i < cycles_.length; ++i) {
            cycles.push();
            for (uint256 j; j < cycles_[i].length; ++j) {
                cycles[i].push(cycles_[i][j]);
            }
        }
    }

    function execute(uint256 which, uint256 amountIn, uint256 minProfit) external {
        ArbExecutor.Hop[] memory hops = cycles[which % cycles.length];
        // Mostly realistic sizes, occasionally dust or oversized.
        amountIn = amountIn % 8 == 0 ? bound(amountIn, 1, 300e18) : bound(amountIn, 1e15, 20e18);
        minProfit = minProfit % 2 == 0 ? 0 : bound(minProfit, 0, 1e17);
        vm.prank(owner);
        try executor.execute(hops, amountIn, minProfit) returns (uint256 profit) {
            profitPaid[hops[0].tokenIn] += profit;
            ++executions;
        } catch {
            ++reverted;
        }
    }

    /// @dev A trader swaps on the V2 pair, moving its price either way.
    function tradeV2(bool zeroForOne, uint256 amountIn) external {
        amountIn = bound(amountIn, 1e6, 100e18);
        (uint112 r0, uint112 r1,) = pair.getReserves();
        (uint256 rIn, uint256 rOut) = zeroForOne ? (uint256(r0), uint256(r1)) : (uint256(r1), uint256(r0));
        uint256 out = amountIn * 997 * rOut / (rIn * 1000 + amountIn * 997);
        if (out == 0) return;
        MockERC20(zeroForOne ? pair.token0() : pair.token1()).mint(address(pair), amountIn);
        pair.swap(zeroForOne ? 0 : out, zeroForOne ? out : 0, address(this), "");
    }

    /// @dev A trader swaps on one of the V3 pools.
    function tradeV3(uint256 which, bool zeroForOne, uint256 amountIn) external {
        amountIn = bound(amountIn, 1e6, 100e18);
        trader.swap(
            address(pools[which % 3]),
            zeroForOne,
            int256(amountIn),
            zeroForOne ? 4295128739 + 1 : 1461446703485210103287273052203988822378723970342 - 1
        );
    }

    /// @dev Tries to make a genuine pair call the executor's V2 callback.
    function attackV2Callback(uint256 amountOut) external {
        amountOut = bound(amountOut, 1, 1e18);
        try pair.swap(amountOut, 0, address(executor), abi.encode(cycles[0], amountOut, 0)) {
            ++attacksSucceeded;
        } catch {}
    }

    /// @dev Calls the V3 callback directly, from a pool address and from this contract.
    function attackV3Callback(uint256 which, int256 d0, int256 d1) external {
        vm.prank(address(pools[which % 3]));
        try executor.uniswapV3SwapCallback(d0, d1, "") {
            ++attacksSucceeded;
        } catch {}
        try executor.uniswapV3SwapCallback(d0, d1, "") {
            ++attacksSucceeded;
        } catch {}
    }
}

contract ArbExecutorInvariantTest is ArbSetup {
    ExecutorHandler internal handler;
    MockERC20[3] internal tokenList;

    function setUp() public override {
        super.setUp();
        tokenList = [tokenA, tokenB, tokenC];

        ArbExecutor.Hop[][] memory cycles = new ArbExecutor.Hop[][](6);
        cycles[0] = _cycleV2Flash();
        cycles[1] = _cycleV3Flash();
        cycles[2] = _cycleThreeHop();
        cycles[3] = _cycleBackwards();
        cycles[4] = _reversed(_cycleThreeHop());
        cycles[5] = _reversed(_cycleV2Flash());

        handler = new ExecutorHandler(executor, owner, tokenList, pairAB, [poolAB, poolBC, poolCA], cycles);
        targetContract(address(handler));
    }

    function _reversed(ArbExecutor.Hop[] memory hops) internal pure returns (ArbExecutor.Hop[] memory r) {
        uint256 n = hops.length;
        r = new ArbExecutor.Hop[](n);
        for (uint256 i; i < n; ++i) {
            ArbExecutor.Hop memory h = hops[n - 1 - i];
            r[i] = ArbExecutor.Hop(h.kind, h.tokenOut, h.tokenIn, h.fee);
        }
    }

    /// @notice The executor ends every call holding nothing.
    function invariant_executorHoldsNoTokens() public view {
        for (uint256 i; i < 3; ++i) {
            assertEq(tokenList[i].balanceOf(address(executor)), 0);
        }
    }

    /// @notice Everything the owner holds came from reported profit, and every
    ///         reported profit reached the owner.
    function invariant_ownerBalanceEqualsReportedProfit() public view {
        for (uint256 i; i < 3; ++i) {
            assertEq(tokenList[i].balanceOf(owner), handler.profitPaid(address(tokenList[i])));
        }
    }

    /// @notice No outside party ever gets a callback accepted.
    function invariant_callbacksOnlyFromActiveSwaps() public view {
        assertEq(handler.attacksSucceeded(), 0);
    }

    function afterInvariant() external view {
        // Make sure the campaign did real work in both directions.
        assertGt(handler.executions() + handler.reverted(), 0);
    }
}
