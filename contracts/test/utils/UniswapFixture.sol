// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {CommonBase} from "forge-std/Base.sol";

import {IUniswapV2Factory, IUniswapV2Pair} from "../../src/interfaces/IUniswapV2.sol";
import {IUniswapV3Factory, IUniswapV3Pool} from "../../src/interfaces/IUniswapV3.sol";
import {MockERC20} from "./MockERC20.sol";
import {V3LiquidityProvider} from "./UniswapHelpers.sol";

/// @notice Deploys the canonical Uniswap V2 and V3 core bytecode from
///         `vendor/artifacts` (rebuilt from the submodules by vendor/build.mjs)
///         and offers helpers to create and fund pools.
abstract contract UniswapFixture is CommonBase {
    /// @dev Mainnet init code hashes; the vendored bytecode must reproduce them.
    bytes32 internal constant V2_PAIR_INIT_CODE_HASH =
        0x96e8ac4277198ff8b6f785478aa9a39f403cb768dd02cbee326c3e7da348845f;
    bytes32 internal constant V3_POOL_INIT_CODE_HASH =
        0xe34f199b19b2b4f47f68442619d555527d244f78a3297ea89325f843f87b8b54;

    int24 internal constant MIN_TICK = -887272;
    int24 internal constant MAX_TICK = 887272;
    uint160 internal constant MIN_SQRT_RATIO = 4295128739;
    uint160 internal constant MAX_SQRT_RATIO = 1461446703485210103287273052203988822378723970342;

    IUniswapV2Factory internal v2Factory;
    IUniswapV3Factory internal v3Factory;
    V3LiquidityProvider internal lp;

    uint256 private _tokenNonce;

    function _deployUniswap() internal {
        v2Factory = IUniswapV2Factory(_deployArtifact("UniswapV2Factory", abi.encode(address(this))));
        v3Factory = IUniswapV3Factory(_deployArtifact("UniswapV3Factory", ""));
        v3Factory.enableFeeAmount(100, 1);
        lp = new V3LiquidityProvider();
    }

    function _artifactCode(string memory name) internal view returns (bytes memory) {
        string memory json = vm.readFile(string.concat(vm.projectRoot(), "/vendor/artifacts/", name, ".json"));
        return vm.parseJsonBytes(json, ".bytecode");
    }

    function _deployArtifact(string memory name, bytes memory args) internal returns (address addr) {
        bytes memory code = bytes.concat(_artifactCode(name), args);
        assembly {
            addr := create(0, add(code, 0x20), mload(code))
        }
        require(addr != address(0), string.concat("deploy failed: ", name));
    }

    /// @dev Two fresh tokens, returned sorted so token0 < token1.
    function _newTokenPair() internal returns (MockERC20 token0, MockERC20 token1) {
        MockERC20 a = _newToken();
        MockERC20 b = _newToken();
        (token0, token1) = address(a) < address(b) ? (a, b) : (b, a);
    }

    function _newToken() internal returns (MockERC20) {
        string memory id = vm.toString(++_tokenNonce);
        return new MockERC20(string.concat("Token ", id), string.concat("T", id), 18);
    }

    /// @dev Creates a pair and sets its reserves directly with `sync`, which
    ///      allows reserves too small for the first `mint`.
    function _createV2Pair(address tokenA, address tokenB, uint256 reserveA, uint256 reserveB)
        internal
        returns (IUniswapV2Pair pair)
    {
        pair = IUniswapV2Pair(v2Factory.createPair(tokenA, tokenB));
        MockERC20(tokenA).mint(address(pair), reserveA);
        MockERC20(tokenB).mint(address(pair), reserveB);
        pair.sync();
    }

    function _createV3Pool(address tokenA, address tokenB, uint24 fee, uint160 sqrtPriceX96)
        internal
        returns (IUniswapV3Pool pool)
    {
        pool = IUniswapV3Pool(v3Factory.createPool(tokenA, tokenB, fee));
        pool.initialize(sqrtPriceX96);
    }

    function _addLiquidity(IUniswapV3Pool pool, int24 tickLower, int24 tickUpper, uint128 liquidity) internal {
        lp.mint(address(pool), tickLower, tickUpper, liquidity);
    }

    function _minUsableTick(int24 spacing) internal pure returns (int24) {
        return (MIN_TICK / spacing) * spacing;
    }

    function _maxUsableTick(int24 spacing) internal pure returns (int24) {
        return (MAX_TICK / spacing) * spacing;
    }
}
