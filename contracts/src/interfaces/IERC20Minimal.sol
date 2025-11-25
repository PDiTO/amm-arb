// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @notice The subset of ERC-20 the executor touches.
interface IERC20Minimal {
    function balanceOf(address account) external view returns (uint256);
    function transfer(address to, uint256 amount) external returns (bool);
}
