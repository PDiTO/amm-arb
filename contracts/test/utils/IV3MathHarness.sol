// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

/// @notice ABI of vendor/V3MathHarness.sol (compiled with solc 0.7.6 against v3-core).
interface IV3MathHarness {
    function getSqrtRatioAtTick(int24 tick) external pure returns (uint160);
    function getTickAtSqrtRatio(uint160 sqrtPriceX96) external pure returns (int24);
    function getNextSqrtPriceFromInput(uint160 sqrtP, uint128 liquidity, uint256 amountIn, bool zeroForOne)
        external
        pure
        returns (uint160);
    function getNextSqrtPriceFromOutput(uint160 sqrtP, uint128 liquidity, uint256 amountOut, bool zeroForOne)
        external
        pure
        returns (uint160);
    function getAmount0Delta(uint160 sqrtA, uint160 sqrtB, uint128 liquidity, bool roundUp)
        external
        pure
        returns (uint256);
    function getAmount1Delta(uint160 sqrtA, uint160 sqrtB, uint128 liquidity, bool roundUp)
        external
        pure
        returns (uint256);
    function computeSwapStep(
        uint160 sqrtCurrent,
        uint160 sqrtTarget,
        uint128 liquidity,
        int256 amountRemaining,
        uint24 feePips
    ) external pure returns (uint160 sqrtNext, uint256 amountIn, uint256 amountOut, uint256 feeAmount);
    function mulDiv(uint256 a, uint256 b, uint256 denominator) external pure returns (uint256);
    function mulDivRoundingUp(uint256 a, uint256 b, uint256 denominator) external pure returns (uint256);
}
