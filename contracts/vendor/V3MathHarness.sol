// SPDX-License-Identifier: GPL-2.0-or-later
pragma solidity =0.7.6;
pragma abicoder v2;

import {FullMath} from "../lib/v3-core/contracts/libraries/FullMath.sol";
import {SqrtPriceMath} from "../lib/v3-core/contracts/libraries/SqrtPriceMath.sol";
import {SwapMath} from "../lib/v3-core/contracts/libraries/SwapMath.sol";
import {TickMath} from "../lib/v3-core/contracts/libraries/TickMath.sol";

/// @notice Exposes the v3-core math libraries, compiled exactly as the pool is,
///         so the differential vectors come from the real library bytecode.
contract V3MathHarness {
    function getSqrtRatioAtTick(int24 tick) external pure returns (uint160) {
        return TickMath.getSqrtRatioAtTick(tick);
    }

    function getTickAtSqrtRatio(uint160 sqrtPriceX96) external pure returns (int24) {
        return TickMath.getTickAtSqrtRatio(sqrtPriceX96);
    }

    function getNextSqrtPriceFromInput(uint160 sqrtP, uint128 liquidity, uint256 amountIn, bool zeroForOne)
        external
        pure
        returns (uint160)
    {
        return SqrtPriceMath.getNextSqrtPriceFromInput(sqrtP, liquidity, amountIn, zeroForOne);
    }

    function getNextSqrtPriceFromOutput(uint160 sqrtP, uint128 liquidity, uint256 amountOut, bool zeroForOne)
        external
        pure
        returns (uint160)
    {
        return SqrtPriceMath.getNextSqrtPriceFromOutput(sqrtP, liquidity, amountOut, zeroForOne);
    }

    function getAmount0Delta(uint160 sqrtA, uint160 sqrtB, uint128 liquidity, bool roundUp)
        external
        pure
        returns (uint256)
    {
        return SqrtPriceMath.getAmount0Delta(sqrtA, sqrtB, liquidity, roundUp);
    }

    function getAmount1Delta(uint160 sqrtA, uint160 sqrtB, uint128 liquidity, bool roundUp)
        external
        pure
        returns (uint256)
    {
        return SqrtPriceMath.getAmount1Delta(sqrtA, sqrtB, liquidity, roundUp);
    }

    function computeSwapStep(
        uint160 sqrtCurrent,
        uint160 sqrtTarget,
        uint128 liquidity,
        int256 amountRemaining,
        uint24 feePips
    ) external pure returns (uint160 sqrtNext, uint256 amountIn, uint256 amountOut, uint256 feeAmount) {
        return SwapMath.computeSwapStep(sqrtCurrent, sqrtTarget, liquidity, amountRemaining, feePips);
    }

    function mulDiv(uint256 a, uint256 b, uint256 denominator) external pure returns (uint256) {
        return FullMath.mulDiv(a, b, denominator);
    }

    function mulDivRoundingUp(uint256 a, uint256 b, uint256 denominator) external pure returns (uint256) {
        return FullMath.mulDivRoundingUp(a, b, denominator);
    }
}
