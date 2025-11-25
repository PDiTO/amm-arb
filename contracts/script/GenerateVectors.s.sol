// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Script} from "forge-std/Script.sol";

import {AmmVectors, MathVectors} from "./vectors/Generators.sol";

/// @notice Generates the differential test vectors consumed by
///         crates/amm-math/tests/differential.rs.
/// @dev Every expected value comes from executing the canonical Uniswap
///      bytecode: the V2 pair's own `k` check, the V3 pool's `swap`, and the
///      v3-core math libraries through a harness. Randomness is a fixed keccak
///      chain, so rerunning produces byte-identical files; CI relies on that.
///
///      forge script script/GenerateVectors.s.sol
///
///      Script contracts may not rely on `address(this)`, and the generators
///      need a stable address (they own the factories and pay pools), so the
///      work happens in ordinary contracts deployed from here.
contract GenerateVectors is Script {
    function run() external {
        new MathVectors().generate();
        new AmmVectors().generate();
    }
}

