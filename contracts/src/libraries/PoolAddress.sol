// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @notice CREATE2 address derivation for Uniswap V2 pairs and V3 pools.
/// @dev Deriving the address from (factory, tokens, fee, init code hash) instead
///      of trusting a caller-supplied address is what lets the executor accept
///      callbacks only from genuine pools.
library PoolAddress {
    function sortTokens(address a, address b) internal pure returns (address token0, address token1) {
        (token0, token1) = a < b ? (a, b) : (b, a);
    }

    function v2Pair(address factory, bytes32 initCodeHash, address tokenA, address tokenB)
        internal
        pure
        returns (address)
    {
        (address token0, address token1) = sortTokens(tokenA, tokenB);
        bytes32 salt = keccak256(abi.encodePacked(token0, token1));
        return _create2(factory, salt, initCodeHash);
    }

    function v3Pool(address factory, bytes32 initCodeHash, address tokenA, address tokenB, uint24 fee)
        internal
        pure
        returns (address)
    {
        (address token0, address token1) = sortTokens(tokenA, tokenB);
        bytes32 salt = keccak256(abi.encode(token0, token1, fee));
        return _create2(factory, salt, initCodeHash);
    }

    function _create2(address deployer, bytes32 salt, bytes32 initCodeHash) private pure returns (address) {
        return address(uint160(uint256(keccak256(abi.encodePacked(hex"ff", deployer, salt, initCodeHash)))));
    }
}
