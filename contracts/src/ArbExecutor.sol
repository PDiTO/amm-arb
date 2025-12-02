// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20Minimal} from "./interfaces/IERC20Minimal.sol";
import {IUniswapV2Callee, IUniswapV2Pair} from "./interfaces/IUniswapV2.sol";
import {IUniswapV3Pool, IUniswapV3SwapCallback} from "./interfaces/IUniswapV3.sol";
import {PoolAddress} from "./libraries/PoolAddress.sol";
import {SafeTransferLib} from "./libraries/SafeTransferLib.sol";

/// @title ArbExecutor
/// @author Paul Davis
/// @notice Executes a cyclic arbitrage across Uniswap V2 pairs and V3 pools in a
///         single transaction without holding inventory.
/// @dev The first hop is a flash swap: the pool sends its output before it is
///      paid. Inside that callback the remaining hops run, the first pool is
///      repaid, and whatever is left over is the profit. If the leftover is
///      below `minProfit` the whole transaction reverts.
///
///      Pools are never supplied by the caller. Each hop names its tokens (and
///      fee tier for V3), and the pool address is derived with CREATE2 from the
///      configured factory and init code hash. A callback is accepted only from
///      the exact pool this contract is calling at that moment, which is tracked
///      in transient storage and cleared on use.
contract ArbExecutor is IUniswapV2Callee, IUniswapV3SwapCallback {
    using SafeTransferLib for address;

    /// @notice Which AMM a hop trades on.
    enum PoolKind {
        V2,
        V3
    }

    /// @notice One leg of a cycle.
    /// @param kind V2 pair or V3 pool.
    /// @param tokenIn Token paid into the pool.
    /// @param tokenOut Token received from the pool.
    /// @param fee V3 fee tier in hundredths of a bip; must be zero for V2.
    struct Hop {
        PoolKind kind;
        address tokenIn;
        address tokenOut;
        uint24 fee;
    }

    /// @notice Longest cycle accepted. Searchers rarely see profit beyond three hops after gas.
    uint256 public constant MAX_HOPS = 4;

    /// @dev `TickMath.MIN_SQRT_RATIO + 1` and `TickMath.MAX_SQRT_RATIO - 1`: no price limit.
    uint160 internal constant MIN_SQRT_RATIO_PLUS_ONE = 4295128740;
    uint160 internal constant MAX_SQRT_RATIO_MINUS_ONE = 1461446703485210103287273052203988822378723970341;

    /// @notice Receives all profit and is the only account allowed to execute or sweep.
    address public immutable owner;
    /// @notice Uniswap V2 factory whose pairs may be traded.
    address public immutable v2Factory;
    /// @notice keccak256 of the V2 pair creation code, used to derive pair addresses.
    bytes32 public immutable v2InitCodeHash;
    /// @notice Uniswap V3 factory whose pools may be traded.
    address public immutable v3Factory;
    /// @notice keccak256 of the V3 pool creation code, used to derive pool addresses.
    bytes32 public immutable v3InitCodeHash;

    /// @dev The only address allowed to call back into this contract right now.
    address private transient _expectedCaller;
    /// @dev True between starting the flash leg and receiving its callback.
    bool private transient _flashPending;
    /// @dev Token owed to the V3 pool currently being paid.
    address private transient _payToken;
    /// @dev Profit computed inside the flash callback, read back by `execute`.
    uint256 private transient _profit;

    /// @notice Emitted after a profitable cycle settles.
    /// @param token Token the cycle starts and ends in; profit is denominated in it.
    /// @param amountIn Amount borrowed through the flash leg.
    /// @param profit Amount sent to the owner.
    event ArbExecuted(address indexed token, uint256 amountIn, uint256 profit);

    /// @notice Emitted when the owner recovers tokens sent here by mistake.
    event Swept(address indexed token, address indexed to, uint256 amount);

    /// @notice Caller is not the owner.
    error NotOwner();
    /// @notice The hops do not form a closed cycle of a supported length.
    error InvalidPath();
    /// @notice `amountIn` was zero.
    error ZeroAmount();
    /// @notice A constructor or sweep address argument was zero.
    error ZeroAddress();
    /// @notice `amountIn` does not fit in `int256`.
    error AmountTooLarge();
    /// @notice The flash pool returned without calling back. Unreachable with genuine pools.
    error FlashNotSettled();
    /// @notice A callback came from something other than the pool being called.
    error UnauthorizedCallback(address caller);
    /// @notice A V3 pool could not absorb the full input before hitting its price limit.
    error PartialFill();
    /// @notice The cycle returned less than `amountOwed + minProfit`.
    /// @param amountOut What the final hop returned.
    /// @param required What was needed to repay the flash leg plus `minProfit`.
    error ProfitTooLow(uint256 amountOut, uint256 required);

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    /// @param owner_ Account that may execute and receives profit.
    /// @param v2Factory_ Uniswap V2 factory.
    /// @param v2InitCodeHash_ V2 pair init code hash (mainnet: 0x96e8ac42...845f).
    /// @param v3Factory_ Uniswap V3 factory.
    /// @param v3InitCodeHash_ V3 pool init code hash (mainnet: 0xe34f199b...8b54).
    constructor(
        address owner_,
        address v2Factory_,
        bytes32 v2InitCodeHash_,
        address v3Factory_,
        bytes32 v3InitCodeHash_
    ) {
        if (owner_ == address(0) || v2Factory_ == address(0) || v3Factory_ == address(0)) {
            revert ZeroAddress();
        }
        owner = owner_;
        v2Factory = v2Factory_;
        v2InitCodeHash = v2InitCodeHash_;
        v3Factory = v3Factory_;
        v3InitCodeHash = v3InitCodeHash_;
    }

    // ---------------------------------------------------------------------
    // Entry points
    // ---------------------------------------------------------------------

    /// @notice Runs `hops` as a cycle starting with `amountIn` of `hops[0].tokenIn`.
    /// @dev Reverts unless the cycle returns at least `amountIn + minProfit`
    ///      (for V2 the flash leg is repaid exactly `amountIn`; for V3 the pool's
    ///      reported input, which equals `amountIn` or the call reverts).
    /// @param hops The cycle. `hops[i].tokenOut` must equal `hops[i + 1].tokenIn`,
    ///        and the last hop must return to `hops[0].tokenIn`.
    /// @param amountIn Input to the first hop.
    /// @param minProfit Minimum profit, in `hops[0].tokenIn`, for the call to succeed.
    /// @return profit Amount of `hops[0].tokenIn` transferred to the owner.
    function execute(Hop[] calldata hops, uint256 amountIn, uint256 minProfit)
        external
        onlyOwner
        returns (uint256 profit)
    {
        if (amountIn == 0) revert ZeroAmount();
        _validatePath(hops);

        Hop calldata first = hops[0];
        bool zeroForOne = first.tokenIn < first.tokenOut;
        bytes memory data = abi.encode(hops, amountIn, minProfit);
        _flashPending = true;

        if (first.kind == PoolKind.V2) {
            address pair = PoolAddress.v2Pair(v2Factory, v2InitCodeHash, first.tokenIn, first.tokenOut);
            uint256 amountOut = _v2AmountOut(pair, amountIn, zeroForOne);
            _expectedCaller = pair;
            IUniswapV2Pair(pair).swap(zeroForOne ? 0 : amountOut, zeroForOne ? amountOut : 0, address(this), data);
        } else {
            address pool = PoolAddress.v3Pool(v3Factory, v3InitCodeHash, first.tokenIn, first.tokenOut, first.fee);
            _expectedCaller = pool;
            // Amounts are settled and checked inside the callback.
            // forge-lint: disable-next-item(unused-return)
            IUniswapV3Pool(pool)
                .swap(
                    address(this),
                    zeroForOne,
                    _toInt256(amountIn),
                    zeroForOne ? MIN_SQRT_RATIO_PLUS_ONE : MAX_SQRT_RATIO_MINUS_ONE,
                    data
                );
        }

        if (_flashPending) revert FlashNotSettled();

        profit = _profit;
        emit ArbExecuted(first.tokenIn, amountIn, profit);
        first.tokenIn.safeTransfer(owner, profit);
    }

    /// @notice Sends this contract's entire balance of `token` to `to`.
    /// @dev The executor never holds funds between transactions; this exists for
    ///      tokens sent here by mistake. `token == address(0)` sweeps ETH.
    function sweep(address token, address to) external onlyOwner {
        if (to == address(0)) revert ZeroAddress();
        if (token == address(0)) {
            uint256 amount = address(this).balance;
            emit Swept(token, to, amount);
            (bool ok,) = to.call{value: amount}("");
            if (!ok) revert SafeTransferLib.TransferFailed(address(0));
        } else {
            uint256 amount = IERC20Minimal(token).balanceOf(address(this));
            emit Swept(token, to, amount);
            token.safeTransfer(to, amount);
        }
    }

    // ---------------------------------------------------------------------
    // Callbacks
    // ---------------------------------------------------------------------

    /// @inheritdoc IUniswapV2Callee
    /// @dev Only reached for a V2 flash leg; V2 payment hops pass empty data.
    function uniswapV2Call(address sender, uint256 amount0, uint256 amount1, bytes calldata data) external {
        _authorizeCallback();
        if (sender != address(this) || !_flashPending) revert UnauthorizedCallback(msg.sender);
        _flashPending = false;

        (Hop[] memory hops, uint256 amountIn, uint256 minProfit) = abi.decode(data, (Hop[], uint256, uint256));
        _settleFlash(hops, amount0 + amount1, amountIn, minProfit);
    }

    /// @inheritdoc IUniswapV3SwapCallback
    function uniswapV3SwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata data) external {
        _authorizeCallback();
        // For an exact-input swap the pool reports the input as positive (owed)
        // and the output as negative (received), so both casts are sign-safe.
        // forge-lint: disable-next-item(unsafe-typecast)
        (uint256 owed, uint256 received) = amount0Delta > 0
            ? (uint256(amount0Delta), uint256(-amount1Delta))
            : (uint256(amount1Delta), uint256(-amount0Delta));

        if (_flashPending) {
            _flashPending = false;
            (Hop[] memory hops, uint256 amountIn, uint256 minProfit) = abi.decode(data, (Hop[], uint256, uint256));
            if (owed != amountIn) revert PartialFill();
            _settleFlash(hops, received, owed, minProfit);
        } else {
            _payToken.safeTransfer(msg.sender, owed);
        }
    }

    // ---------------------------------------------------------------------
    // Internals
    // ---------------------------------------------------------------------

    /// @dev Runs hops 1..n-1 with the flash proceeds, checks profit, repays the flash pool.
    function _settleFlash(Hop[] memory hops, uint256 received, uint256 owed, uint256 minProfit) private {
        address flashPool = msg.sender;
        uint256 amount = received;
        for (uint256 i = 1; i < hops.length; ++i) {
            amount = _swap(hops[i], amount);
        }

        uint256 required = owed + minProfit;
        if (amount < required) revert ProfitTooLow(amount, required);
        _profit = amount - owed;

        hops[0].tokenIn.safeTransfer(flashPool, owed);
    }

    /// @dev Executes one non-flash hop, returning the output received by this contract.
    function _swap(Hop memory hop, uint256 amountIn) private returns (uint256 amountOut) {
        bool zeroForOne = hop.tokenIn < hop.tokenOut;
        if (hop.kind == PoolKind.V2) {
            address pair = PoolAddress.v2Pair(v2Factory, v2InitCodeHash, hop.tokenIn, hop.tokenOut);
            amountOut = _v2AmountOut(pair, amountIn, zeroForOne);
            hop.tokenIn.safeTransfer(pair, amountIn);
            IUniswapV2Pair(pair).swap(zeroForOne ? 0 : amountOut, zeroForOne ? amountOut : 0, address(this), "");
        } else {
            address pool = PoolAddress.v3Pool(v3Factory, v3InitCodeHash, hop.tokenIn, hop.tokenOut, hop.fee);
            _expectedCaller = pool;
            _payToken = hop.tokenIn;
            (int256 amount0, int256 amount1) = IUniswapV3Pool(pool)
                .swap(
                    address(this),
                    zeroForOne,
                    _toInt256(amountIn),
                    zeroForOne ? MIN_SQRT_RATIO_PLUS_ONE : MAX_SQRT_RATIO_MINUS_ONE,
                    ""
                );
            (int256 paid, int256 got) = zeroForOne ? (amount0, amount1) : (amount1, amount0);
            // Exact input: `paid` > 0 and `got` <= 0, so both casts are sign-safe.
            // forge-lint: disable-next-line(unsafe-typecast)
            if (uint256(paid) != amountIn) revert PartialFill();
            // forge-lint: disable-next-line(unsafe-typecast)
            amountOut = uint256(-got);
        }
    }

    /// @dev `UniswapV2Library.getAmountOut` against live reserves.
    function _v2AmountOut(address pair, uint256 amountIn, bool zeroForOne) private view returns (uint256) {
        // forge-lint: disable-next-line(unused-return)
        (uint112 reserve0, uint112 reserve1,) = IUniswapV2Pair(pair).getReserves();
        (uint256 reserveIn, uint256 reserveOut) = zeroForOne ? (reserve0, reserve1) : (reserve1, reserve0);
        uint256 amountInWithFee = amountIn * 997;
        return (amountInWithFee * reserveOut) / (reserveIn * 1000 + amountInWithFee);
    }

    /// @dev Accepts a callback only from the pool currently being called, once.
    ///      The zero check matters: when idle the slot holds address(0), and a
    ///      bare equality test would treat that as "expected". No transaction
    ///      can come from address(0) on mainnet, but the guard should not
    ///      depend on that (the fuzzer found this case).
    function _authorizeCallback() private {
        address expected = _expectedCaller;
        if (expected == address(0) || msg.sender != expected) revert UnauthorizedCallback(msg.sender);
        _expectedCaller = address(0);
    }

    function _validatePath(Hop[] calldata hops) private pure {
        uint256 n = hops.length;
        if (n < 2 || n > MAX_HOPS) revert InvalidPath();
        for (uint256 i; i < n; ++i) {
            Hop calldata hop = hops[i];
            if (hop.tokenIn == hop.tokenOut) revert InvalidPath();
            if (hop.kind == PoolKind.V2 && hop.fee != 0) revert InvalidPath();
            if (hop.tokenOut != hops[(i + 1) % n].tokenIn) revert InvalidPath();
        }
    }

    function _toInt256(uint256 x) private pure returns (int256) {
        if (x >> 255 != 0) revert AmountTooLarge();
        // forge-lint: disable-next-line(unsafe-typecast)
        return int256(x);
    }
}
