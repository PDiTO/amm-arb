// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Test} from "forge-std/Test.sol";

import {ArbExecutor} from "../src/ArbExecutor.sol";
import {IERC20Minimal} from "../src/interfaces/IERC20Minimal.sol";
import {IUniswapV2Pair} from "../src/interfaces/IUniswapV2.sol";
import {PoolAddress} from "../src/libraries/PoolAddress.sol";

/// @notice Runs the executor against real mainnet pools at a pinned block,
///         after planting a mispricing by donating to a V2 pair and syncing.
/// @dev    Needs an archive RPC: `MAINNET_RPC_URL=https://eth.drpc.org forge test --mc Fork`.
///         Without `MAINNET_RPC_URL`, or if the endpoint cannot serve the block,
///         every test here is skipped.
contract ArbExecutorForkTest is Test {
    uint256 internal constant BLOCK = 23_700_000; // same block as fixtures/mainnet-23700000.json

    address internal constant V2_FACTORY = 0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f;
    address internal constant V3_FACTORY = 0x1F98431c8aD98523631AE4a59f267346ea31F984;
    bytes32 internal constant V2_INIT = 0x96e8ac4277198ff8b6f785478aa9a39f403cb768dd02cbee326c3e7da348845f;
    bytes32 internal constant V3_INIT = 0xe34f199b19b2b4f47f68442619d555527d244f78a3297ea89325f843f87b8b54;

    address internal constant WETH = 0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2;
    address internal constant USDC = 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48;
    address internal constant USDT = 0xdAC17F958D2ee523a2206206994597C13D831ec7;
    address internal constant V2_USDC_WETH = 0xB4e16d0168e52d35CaCD2c6185b44281Ec28C9Dc;
    address internal constant V2_WETH_USDT = 0x0d4a11d5EEaaC28EC3F61d100daF4d40471f1852;
    address internal constant V3_USDC_WETH_500 = 0x88e6A0c2dDD26FEEb64F039a2c41296FcB3f5640;
    address internal constant V3_WETH_USDT_500 = 0x11b815efB8f581194ae79006d24E0d814B7697F6;

    address internal owner = makeAddr("owner");
    ArbExecutor internal executor;
    bool internal forked;

    function setUp() public {
        string memory url = vm.envOr("MAINNET_RPC_URL", string(""));
        if (bytes(url).length == 0) return;
        try vm.createSelectFork(url, BLOCK) {
            forked = true;
        } catch {
            return;
        }
        executor = new ArbExecutor(owner, V2_FACTORY, V2_INIT, V3_FACTORY, V3_INIT);
    }

    modifier onlyFork() {
        if (!forked) vm.skip(true);
        _;
    }

    function test_fork_derivesRealPoolAddresses() public onlyFork {
        assertEq(PoolAddress.v2Pair(V2_FACTORY, V2_INIT, WETH, USDC), V2_USDC_WETH);
        assertEq(PoolAddress.v3Pool(V3_FACTORY, V3_INIT, WETH, USDC, 500), V3_USDC_WETH_500);
        assertEq(PoolAddress.v3Pool(V3_FACTORY, V3_INIT, USDT, WETH, 500), V3_WETH_USDT_500);
    }

    /// @dev Donate WETH to the V2 pair: WETH becomes cheap there. Buy it on V2
    ///      (flash leg) with USDC, sell on the 0.05% V3 pool. Profit in USDC.
    function test_fork_v2FlashArbAgainstV3() public onlyFork {
        _donateAndSync(V2_USDC_WETH, WETH, 30 ether);
        ArbExecutor.Hop[] memory hops = new ArbExecutor.Hop[](2);
        hops[0] = ArbExecutor.Hop(ArbExecutor.PoolKind.V2, USDC, WETH, 0);
        hops[1] = ArbExecutor.Hop(ArbExecutor.PoolKind.V3, WETH, USDC, 500);

        vm.prank(owner);
        uint256 profit = executor.execute(hops, 20_000e6, 0);

        assertGt(profit, 0);
        assertEq(IERC20Minimal(USDC).balanceOf(owner), profit);
        _assertEmpty();
    }

    /// @dev Same planted edge, flash from V3 first, through USDT, which does
    ///      not return a bool from transfer.
    function test_fork_v3FlashArbThroughUsdt() public onlyFork {
        _donateAndSync(V2_WETH_USDT, WETH, 300 ether);
        ArbExecutor.Hop[] memory hops = new ArbExecutor.Hop[](2);
        hops[0] = ArbExecutor.Hop(ArbExecutor.PoolKind.V3, WETH, USDT, 500);
        hops[1] = ArbExecutor.Hop(ArbExecutor.PoolKind.V2, USDT, WETH, 0);

        vm.prank(owner);
        uint256 profit = executor.execute(hops, 5 ether, 0);

        assertGt(profit, 0);
        assertEq(IERC20Minimal(WETH).balanceOf(owner), profit);
        _assertEmpty();
    }

    /// @dev Without a planted edge, real mainnet state at this block has no
    ///      two-hop arbitrage between these venues, and the executor refuses.
    function test_fork_revertsWithoutMispricing() public onlyFork {
        ArbExecutor.Hop[] memory hops = new ArbExecutor.Hop[](2);
        hops[0] = ArbExecutor.Hop(ArbExecutor.PoolKind.V2, USDC, WETH, 0);
        hops[1] = ArbExecutor.Hop(ArbExecutor.PoolKind.V3, WETH, USDC, 500);
        vm.expectPartialRevert(ArbExecutor.ProfitTooLow.selector);
        vm.prank(owner);
        executor.execute(hops, 20_000e6, 0);
    }

    function _donateAndSync(address pair, address token, uint256 amount) internal {
        deal(token, address(this), amount);
        IERC20Minimal(token).transfer(pair, amount);
        IUniswapV2Pair(pair).sync();
    }

    function _assertEmpty() internal view {
        assertEq(IERC20Minimal(WETH).balanceOf(address(executor)), 0);
        assertEq(IERC20Minimal(USDC).balanceOf(address(executor)), 0);
        assertEq(IERC20Minimal(USDT).balanceOf(address(executor)), 0);
    }
}
