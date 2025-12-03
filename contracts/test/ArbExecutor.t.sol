// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {ArbExecutor} from "../src/ArbExecutor.sol";
import {IUniswapV2Pair} from "../src/interfaces/IUniswapV2.sol";
import {IUniswapV3Pool} from "../src/interfaces/IUniswapV3.sol";
import {SafeTransferLib} from "../src/libraries/SafeTransferLib.sol";
import {ArbSetup} from "./utils/ArbSetup.sol";
import {MockERC20} from "./utils/MockERC20.sol";

contract ArbExecutorTest is ArbSetup {
    event ArbExecuted(address indexed token, uint256 amountIn, uint256 profit);

    // ---------------------------------------------------------------------
    // Happy paths
    // ---------------------------------------------------------------------

    function test_v2FlashCycle_sendsExactProfitToOwner() public {
        ArbExecutor.Hop[] memory hops = _cycleV2Flash();
        uint256 amountIn = 5e18;
        (uint256 expectedOut, bool ok) = _quoteCycle(hops, amountIn);
        assertTrue(ok);
        uint256 expectedProfit = expectedOut - amountIn;
        assertGt(expectedProfit, 0);

        vm.expectEmit(address(executor));
        emit ArbExecuted(address(tokenA), amountIn, expectedProfit);
        vm.prank(owner);
        uint256 profit = executor.execute(hops, amountIn, 0);

        assertEq(profit, expectedProfit);
        assertEq(tokenA.balanceOf(owner), expectedProfit);
        _assertExecutorEmpty();
    }

    function test_v3FlashCycle_sendsExactProfitToOwner() public {
        ArbExecutor.Hop[] memory hops = _cycleV3Flash();
        uint256 amountIn = 5e18;
        (uint256 expectedOut, bool ok) = _quoteCycle(hops, amountIn);
        assertTrue(ok);

        vm.prank(owner);
        uint256 profit = executor.execute(hops, amountIn, 0);

        assertEq(profit, expectedOut - amountIn);
        assertEq(tokenB.balanceOf(owner), profit);
        _assertExecutorEmpty();
    }

    function test_threeHopCycle_sendsExactProfitToOwner() public {
        ArbExecutor.Hop[] memory hops = _cycleThreeHop();
        uint256 amountIn = 5e18;
        (uint256 expectedOut, bool ok) = _quoteCycle(hops, amountIn);
        assertTrue(ok);

        vm.prank(owner);
        uint256 profit = executor.execute(hops, amountIn, 0);

        assertEq(profit, expectedOut - amountIn);
        assertEq(tokenA.balanceOf(owner), profit);
        _assertExecutorEmpty();
    }

    function test_threeHopCycleWithTwoV2Hops() public {
        ArbExecutor.Hop[] memory hops = _cycleTwoV2Hops();
        (uint256 expectedOut, bool ok) = _quoteCycle(hops, 5e18);
        assertTrue(ok);

        vm.prank(owner);
        uint256 profit = executor.execute(hops, 5e18, 0);

        assertEq(profit, expectedOut - 5e18);
        _assertExecutorEmpty();
    }

    function test_minProfitIsInclusive() public {
        ArbExecutor.Hop[] memory hops = _cycleV2Flash();
        (uint256 out,) = _quoteCycle(hops, 5e18);
        vm.prank(owner);
        executor.execute(hops, 5e18, out - 5e18);
        assertEq(tokenA.balanceOf(owner), out - 5e18);
    }

    // ---------------------------------------------------------------------
    // Profit guard
    // ---------------------------------------------------------------------

    function test_revertsWhenProfitBelowMinimum() public {
        ArbExecutor.Hop[] memory hops = _cycleV2Flash();
        (uint256 out,) = _quoteCycle(hops, 5e18);
        uint256 minProfit = out - 5e18 + 1;
        vm.expectRevert(abi.encodeWithSelector(ArbExecutor.ProfitTooLow.selector, out, 5e18 + minProfit));
        vm.prank(owner);
        executor.execute(hops, 5e18, minProfit);
    }

    function test_revertsWhenCycleLosesMoney() public {
        ArbExecutor.Hop[] memory hops = _cycleBackwards();
        (uint256 out, bool ok) = _quoteCycle(hops, 1e18);
        assertTrue(ok);
        assertLt(out, 1e18);
        vm.expectRevert(abi.encodeWithSelector(ArbExecutor.ProfitTooLow.selector, out, 1e18));
        vm.prank(owner);
        executor.execute(hops, 1e18, 0);
    }

    function test_revertsWhenTradeIsTooLargeForTheMispricing() public {
        // Far past the optimum: price impact eats the edge.
        ArbExecutor.Hop[] memory hops = _cycleV2Flash();
        vm.expectPartialRevert(ArbExecutor.ProfitTooLow.selector);
        vm.prank(owner);
        executor.execute(hops, 200e18, 0);
    }

    // ---------------------------------------------------------------------
    // Access control
    // ---------------------------------------------------------------------

    function test_onlyOwnerCanExecute() public {
        ArbExecutor.Hop[] memory hops = _cycleV2Flash();
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        executor.execute(hops, 5e18, 0);
    }

    function testFuzz_onlyOwnerCanExecute(address caller) public {
        vm.assume(caller != owner);
        ArbExecutor.Hop[] memory hops = _cycleV2Flash();
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        vm.prank(caller);
        executor.execute(hops, 5e18, 0);
    }

    function test_onlyOwnerCanSweep() public {
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        executor.sweep(address(tokenA), address(this));
    }

    function test_sweepRecoversStrayTokensAndEth() public {
        tokenA.mint(address(executor), 123);
        vm.deal(address(executor), 1 ether);
        vm.startPrank(owner);
        executor.sweep(address(tokenA), owner);
        executor.sweep(address(0), owner);
        vm.stopPrank();
        assertEq(tokenA.balanceOf(owner), 123);
        assertEq(owner.balance, 1 ether);
        _assertExecutorEmpty();
    }

    // ---------------------------------------------------------------------
    // Callback validation
    // ---------------------------------------------------------------------

    function test_rejectsV3CallbackFromNonPool() public {
        vm.expectRevert(abi.encodeWithSelector(ArbExecutor.UnauthorizedCallback.selector, address(this)));
        executor.uniswapV3SwapCallback(1e18, -1e18, "");
    }

    function test_rejectsV2CallbackFromNonPool() public {
        vm.expectRevert(abi.encodeWithSelector(ArbExecutor.UnauthorizedCallback.selector, address(this)));
        executor.uniswapV2Call(address(executor), 1, 0, "");
    }

    function test_rejectsV3CallbackFromRealPoolWhenIdle() public {
        // Even the genuine pool address is refused unless the executor is
        // mid-swap with that exact pool.
        vm.prank(address(poolAB));
        vm.expectRevert(abi.encodeWithSelector(ArbExecutor.UnauthorizedCallback.selector, address(poolAB)));
        executor.uniswapV3SwapCallback(1e18, -1e18, "");
    }

    function test_rejectsV2FlashSwapAimedAtExecutor() public {
        // Classic V2 attack: anyone can call pair.swap(to = victim, data) and
        // make the genuine pair call the victim's uniswapV2Call.
        tokenB.mint(address(executor), 10e18); // something to steal
        (uint256 out0, uint256 out1) =
            address(tokenA) < address(tokenB) ? (uint256(0), uint256(1e18)) : (uint256(1e18), uint256(0));
        vm.expectRevert(abi.encodeWithSelector(ArbExecutor.UnauthorizedCallback.selector, address(pairAB)));
        pairAB.swap(out0, out1, address(executor), abi.encode(_cycleV2Flash(), 1e18, 0));
        assertEq(tokenB.balanceOf(address(executor)), 10e18);
    }

    function testFuzz_rejectsCallbacksFromAnyone(address caller, int256 d0, int256 d1, bytes calldata data) public {
        vm.prank(caller);
        vm.expectRevert(abi.encodeWithSelector(ArbExecutor.UnauthorizedCallback.selector, caller));
        executor.uniswapV3SwapCallback(d0, d1, data);
    }

    // ---------------------------------------------------------------------
    // Input validation
    // ---------------------------------------------------------------------

    function test_rejectsMalformedPaths() public {
        ArbExecutor.Hop[] memory hops = new ArbExecutor.Hop[](1);
        hops[0] = _hop(ArbExecutor.PoolKind.V2, tokenA, tokenB, 0);
        _expectInvalidPath(hops);

        // Does not return to the start token.
        hops = new ArbExecutor.Hop[](2);
        hops[0] = _hop(ArbExecutor.PoolKind.V2, tokenA, tokenB, 0);
        hops[1] = _hop(ArbExecutor.PoolKind.V3, tokenB, tokenC, 500);
        _expectInvalidPath(hops);

        // Broken chain.
        hops[1] = _hop(ArbExecutor.PoolKind.V3, tokenC, tokenA, 500);
        _expectInvalidPath(hops);

        // V2 hop with a fee set.
        hops[0] = _hop(ArbExecutor.PoolKind.V2, tokenA, tokenB, 3000);
        hops[1] = _hop(ArbExecutor.PoolKind.V3, tokenB, tokenA, 3000);
        _expectInvalidPath(hops);

        // Same token in and out.
        hops[0] = _hop(ArbExecutor.PoolKind.V2, tokenA, tokenA, 0);
        hops[1] = _hop(ArbExecutor.PoolKind.V3, tokenA, tokenA, 3000);
        _expectInvalidPath(hops);

        // Too long.
        hops = new ArbExecutor.Hop[](5);
        for (uint256 i; i < 5; ++i) {
            hops[i] = _hop(ArbExecutor.PoolKind.V3, i % 2 == 0 ? tokenA : tokenB, i % 2 == 0 ? tokenB : tokenA, 3000);
        }
        _expectInvalidPath(hops);
    }

    function test_rejectsZeroAmount() public {
        ArbExecutor.Hop[] memory hops = _cycleV2Flash();
        vm.expectRevert(ArbExecutor.ZeroAmount.selector);
        vm.prank(owner);
        executor.execute(hops, 0, 0);
    }

    function test_rejectsAmountAboveInt256() public {
        ArbExecutor.Hop[] memory hops = _cycleV3Flash();
        vm.expectRevert(ArbExecutor.AmountTooLarge.selector);
        vm.prank(owner);
        executor.execute(hops, uint256(1) << 255, 0);
    }

    function test_rejectsZeroAddresses() public {
        vm.expectRevert(ArbExecutor.ZeroAddress.selector);
        new ArbExecutor(address(0), address(v2Factory), bytes32(0), address(v3Factory), bytes32(0));
        vm.expectRevert(ArbExecutor.ZeroAddress.selector);
        vm.prank(owner);
        executor.sweep(address(tokenA), address(0));
    }

    function test_revertsOnPoolThatDoesNotExist() public {
        // No 1% A/B pool was created: the derived address has no code.
        ArbExecutor.Hop[] memory hops = _cycleV2Flash();
        hops[1].fee = 10_000;
        vm.expectRevert();
        vm.prank(owner);
        executor.execute(hops, 5e18, 0);
    }

    function _expectInvalidPath(ArbExecutor.Hop[] memory hops) internal {
        vm.expectRevert(ArbExecutor.InvalidPath.selector);
        vm.prank(owner);
        executor.execute(hops, 1e18, 0);
    }

    // ---------------------------------------------------------------------
    // Fuzz: outcome always matches an independent quote
    // ---------------------------------------------------------------------

    /// @dev Moves the V2 A/B price by donating to the pair and syncing, then
    ///      runs a randomly sized cycle. The executor must succeed exactly when
    ///      the independent quote says the cycle clears `minProfit`, and the
    ///      owner must receive exactly the quoted profit.
    function testFuzz_outcomeMatchesQuote(
        uint256 amountIn,
        uint256 skewBps,
        bool donateB,
        uint256 minProfit,
        uint8 which
    ) public {
        amountIn = bound(amountIn, 1e9, 150e18);
        skewBps = bound(skewBps, 0, 1000);
        minProfit = bound(minProfit, 0, 1e18);
        _skewV2(skewBps, donateB);

        ArbExecutor.Hop[] memory hops =
            which % 3 == 0 ? _cycleV2Flash() : which % 3 == 1 ? _cycleV3Flash() : _cycleThreeHop();
        MockERC20 start = MockERC20(hops[0].tokenIn);

        (uint256 out, bool ok) = _quoteCycle(hops, amountIn);
        vm.prank(owner);
        if (ok && out >= amountIn + minProfit) {
            uint256 profit = executor.execute(hops, amountIn, minProfit);
            assertEq(profit, out - amountIn);
            assertEq(start.balanceOf(owner), out - amountIn);
        } else {
            vm.expectRevert();
            executor.execute(hops, amountIn, minProfit);
        }
        _assertExecutorEmpty();
    }

    function _skewV2(uint256 skewBps, bool donateB) internal {
        MockERC20 token = donateB ? tokenB : tokenA;
        token.mint(address(pairAB), V2_DEPTH * skewBps / 10_000);
        pairAB.sync();
    }
}
