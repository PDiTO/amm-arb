// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @notice Minimal Uniswap V2 pair interface.
interface IUniswapV2Pair {
    function token0() external view returns (address);
    function token1() external view returns (address);
    function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    function swap(uint256 amount0Out, uint256 amount1Out, address to, bytes calldata data) external;
    function mint(address to) external returns (uint256 liquidity);
    function sync() external;
}

/// @notice Minimal Uniswap V2 factory interface.
interface IUniswapV2Factory {
    function getPair(address tokenA, address tokenB) external view returns (address pair);
    function createPair(address tokenA, address tokenB) external returns (address pair);
}

/// @notice Flash swap callback invoked by a V2 pair when `swap` is called with data.
interface IUniswapV2Callee {
    function uniswapV2Call(address sender, uint256 amount0, uint256 amount1, bytes calldata data) external;
}
