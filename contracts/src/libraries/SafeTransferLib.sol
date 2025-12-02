// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @notice ERC-20 transfer that tolerates tokens returning nothing (USDT) and
///         refuses to "succeed" against an address with no code.
library SafeTransferLib {
    error TransferFailed(address token);

    function safeTransfer(address token, address to, uint256 amount) internal {
        (bool ok, bytes memory ret) = token.call(abi.encodeWithSelector(0xa9059cbb, to, amount));
        if (!ok || (ret.length == 0 ? token.code.length == 0 : !abi.decode(ret, (bool)))) {
            revert TransferFailed(token);
        }
    }
}
