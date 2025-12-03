// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {Test} from "forge-std/Test.sol";

import {PoolAddress} from "../src/libraries/PoolAddress.sol";
import {MockERC20} from "./utils/MockERC20.sol";
import {UniswapFixture} from "./utils/UniswapFixture.sol";

/// @notice The locally deployed cores are the mainnet bytecode, and the
///         executor's CREATE2 derivation agrees with the factories.
contract VendoredBytecodeTest is Test, UniswapFixture {
    function setUp() public {
        _deployUniswap();
    }

    function test_pairBytecodeMatchesMainnetInitCodeHash() public view {
        assertEq(keccak256(_artifactCode("UniswapV2Pair")), V2_PAIR_INIT_CODE_HASH);
    }

    function test_poolBytecodeMatchesMainnetInitCodeHash() public view {
        assertEq(keccak256(_artifactCode("UniswapV3Pool")), V3_POOL_INIT_CODE_HASH);
    }

    function testFuzz_v2PairAddressDerivation(uint8 salt) public {
        (MockERC20 a, MockERC20 b) = _newTokenPair();
        salt; // fresh tokens each run
        address pair = v2Factory.createPair(address(b), address(a));
        assertEq(PoolAddress.v2Pair(address(v2Factory), V2_PAIR_INIT_CODE_HASH, address(a), address(b)), pair);
        assertEq(PoolAddress.v2Pair(address(v2Factory), V2_PAIR_INIT_CODE_HASH, address(b), address(a)), pair);
    }

    function testFuzz_v3PoolAddressDerivation(uint8 feeIndex) public {
        uint24[4] memory fees = [uint24(100), 500, 3000, 10_000];
        uint24 fee = fees[feeIndex % 4];
        (MockERC20 a, MockERC20 b) = _newTokenPair();
        address pool = v3Factory.createPool(address(b), address(a), fee);
        assertEq(PoolAddress.v3Pool(address(v3Factory), V3_POOL_INIT_CODE_HASH, address(a), address(b), fee), pool);
        assertEq(PoolAddress.v3Pool(address(v3Factory), V3_POOL_INIT_CODE_HASH, address(b), address(a), fee), pool);
    }
}
