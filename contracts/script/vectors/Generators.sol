// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IUniswapV2Pair} from "../../src/interfaces/IUniswapV2.sol";
import {IUniswapV3Pool} from "../../src/interfaces/IUniswapV3.sol";
import {IV3MathHarness} from "../../test/utils/IV3MathHarness.sol";
import {MockERC20} from "../../test/utils/MockERC20.sol";
import {UniswapFixture} from "../../test/utils/UniswapFixture.sol";
import {V2Probe, V3Quoter} from "../../test/utils/UniswapHelpers.sol";

/// @notice Shared randomness, JSON and file helpers.
abstract contract VectorBase is UniswapFixture {
    string internal constant DIR = "../testdata/";
    string internal constant MATH_FILE = "v3_math.jsonl";
    string internal constant V2_FILE = "v2_quotes.jsonl";
    string internal constant POOLS_FILE = "v3_pools.jsonl";
    string internal constant SWAPS_FILE = "v3_swaps.jsonl";

    IV3MathHarness internal harness;
    uint256 internal seed;

    constructor(uint256 seed_) {
        seed = seed_;
        harness = IV3MathHarness(_deployArtifact("V3MathHarness", ""));
    }

    // ---------------------------------------------------------------------
    // Randomness and helpers
    // ---------------------------------------------------------------------

    function _rand() internal returns (uint256) {
        seed = uint256(keccak256(abi.encode(seed)));
        return seed;
    }

    /// @dev A value with a uniformly random bit length in [0, maxBits].
    function _randBits(uint256 maxBits) internal returns (uint256) {
        uint256 bits = _rand() % (maxBits + 1);
        return bits == 0 ? 0 : _rand() >> (256 - bits);
    }

    function _randTick() internal returns (int24) {
        return int24(int256(_rand() % uint256(int256(2 * MAX_TICK + 1)))) - MAX_TICK;
    }

    /// @dev A valid sqrt price: either a tick price with noise, or uniform in bit length.
    function _randPrice() internal returns (uint160) {
        if (_rand() % 2 == 0) {
            return harness.getSqrtRatioAtTick(_randTick());
        }
        uint256 p = _randBits(160);
        if (p < MIN_SQRT_RATIO) p = MIN_SQRT_RATIO + p % 1000;
        if (p >= MAX_SQRT_RATIO) p = MAX_SQRT_RATIO - 1;
        return uint160(p);
    }

    function _nearbyPrice(uint160 p) internal returns (uint160) {
        int24 t = harness.getTickAtSqrtRatio(p);
        int24 d = int24(int256(_rand() % 2000)) - 1000;
        int24 u = t + d;
        if (u < MIN_TICK) u = MIN_TICK;
        if (u > MAX_TICK) u = MAX_TICK;
        return harness.getSqrtRatioAtTick(u);
    }

    function _align(int24 tick, int24 spacing) internal pure returns (int24) {
        int24 c = tick / spacing;
        if (tick < 0 && tick % spacing != 0) c--;
        return c * spacing;
    }

    function _clampTick(int24 tick, int24 spacing) internal pure returns (int24) {
        if (tick < _minUsableTick(spacing)) return _minUsableTick(spacing);
        if (tick >= _maxUsableTick(spacing)) return _maxUsableTick(spacing) - spacing;
        return tick;
    }

    function _seen(int24[] memory ticks, uint256 i) internal pure returns (bool) {
        for (uint256 j; j < i; ++j) {
            if (ticks[j] == ticks[i]) return true;
        }
        return false;
    }

    function _wordSeen(int24[] memory ticks, uint256 i, int24 spacing) internal pure returns (bool) {
        int16 word = int16(ticks[i] / spacing >> 8);
        for (uint256 j; j < i; ++j) {
            if (int16(ticks[j] / spacing >> 8) == word) return true;
        }
        return false;
    }

    function _reset(string memory file) internal {
        string memory path = string.concat(DIR, file);
        if (vm.exists(path)) vm.removeFile(path);
    }

    function _write(string memory file, string memory line) internal {
        vm.writeLine(string.concat(DIR, file), line);
    }

    function _math(string memory fn, string memory args, string memory outs) internal {
        _write(MATH_FILE, string.concat('{"fn":"', fn, '","args":[', args, '],"ok":true,"out":', outs, "}"));
    }

    function _mathErr(string memory fn, string memory args) internal {
        _write(MATH_FILE, string.concat('{"fn":"', fn, '","args":[', args, '],"ok":false}'));
    }

    function _args1(string memory a) internal pure returns (string memory) {
        return a;
    }

    function _outs1(string memory a) internal pure returns (string memory) {
        return string.concat("[", a, "]");
    }

    function _u(uint256 x) internal pure returns (string memory) {
        return string.concat('"', vm.toString(x), '"');
    }

    function _i(int256 x) internal pure returns (string memory) {
        return string.concat('"', vm.toString(x), '"');
    }

    function _iRaw(int256 x) internal pure returns (string memory) {
        return vm.toString(x);
    }

    function _b(bool x) internal pure returns (string memory) {
        return x ? '"true"' : '"false"';
    }
}

/// @notice TickMath, SqrtPriceMath, SwapMath and FullMath vectors from the harness.
contract MathVectors is VectorBase {
    constructor() VectorBase(0x6d617468) {}

    function generate() external {
        _reset(MATH_FILE);
        _tickMathVectors();
        _sqrtPriceMathVectors();
        _swapStepVectors();
        _fullMathVectors();
    }

    // ---------------------------------------------------------------------
    // TickMath
    // ---------------------------------------------------------------------

    function _tickMathVectors() internal {
        int24[12] memory edges =
            [MIN_TICK - 1, MIN_TICK, MIN_TICK + 1, -1, 0, 1, MAX_TICK - 1, MAX_TICK, MAX_TICK + 1, -524288, 524288, 50];
        for (uint256 i; i < edges.length; ++i) {
            _sqrtRatioAtTick(edges[i]);
        }
        for (uint256 i; i < 400; ++i) {
            // Mostly in range, sometimes out of range to check the error path.
            int256 span = _rand() % 10 == 0 ? int256(8_388_607) : int256(MAX_TICK);
            _sqrtRatioAtTick(int24(int256(_rand() % uint256(2 * span + 1)) - span));
        }

        uint160[4] memory priceEdges = [MIN_SQRT_RATIO - 1, MIN_SQRT_RATIO, MAX_SQRT_RATIO - 1, MAX_SQRT_RATIO];
        for (uint256 i; i < priceEdges.length; ++i) {
            _tickAtSqrtRatio(priceEdges[i]);
        }
        for (uint256 i; i < 400; ++i) {
            uint256 kind = _rand() % 3;
            if (kind == 0) {
                // Exact tick prices and their neighbours probe the tickLow/tickHi resolution.
                uint160 p = harness.getSqrtRatioAtTick(_randTick());
                _tickAtSqrtRatio(p);
                _tickAtSqrtRatio(p - 1);
                _tickAtSqrtRatio(p + 1);
            } else {
                _tickAtSqrtRatio(uint160(_randBits(160)));
            }
        }
    }

    function _sqrtRatioAtTick(int24 tick) internal {
        string memory args = _args1(_i(tick));
        try harness.getSqrtRatioAtTick(tick) returns (uint160 r) {
            _math("getSqrtRatioAtTick", args, _outs1(_u(r)));
        } catch {
            _mathErr("getSqrtRatioAtTick", args);
        }
    }

    function _tickAtSqrtRatio(uint160 price) internal {
        string memory args = _args1(_u(price));
        try harness.getTickAtSqrtRatio(price) returns (int24 t) {
            _math("getTickAtSqrtRatio", args, _outs1(_i(t)));
        } catch {
            _mathErr("getTickAtSqrtRatio", args);
        }
    }

    // ---------------------------------------------------------------------
    // SqrtPriceMath
    // ---------------------------------------------------------------------

    function _sqrtPriceMathVectors() internal {
        for (uint256 i; i < 300; ++i) {
            uint160 p = _randPrice();
            uint128 l = uint128(_randBits(128));
            uint256 amount = _randBits(256);
            bool zeroForOne = _rand() % 2 == 0;
            string memory args = string.concat(_u(p), ",", _u(l), ",", _u(amount), ",", _b(zeroForOne));

            try harness.getNextSqrtPriceFromInput(p, l, amount, zeroForOne) returns (uint160 r) {
                _math("getNextSqrtPriceFromInput", args, _outs1(_u(r)));
            } catch {
                _mathErr("getNextSqrtPriceFromInput", args);
            }
            try harness.getNextSqrtPriceFromOutput(p, l, amount, zeroForOne) returns (uint160 r) {
                _math("getNextSqrtPriceFromOutput", args, _outs1(_u(r)));
            } catch {
                _mathErr("getNextSqrtPriceFromOutput", args);
            }
        }
        for (uint256 i; i < 300; ++i) {
            uint160 a = _rand() % 20 == 0 ? 0 : _randPrice();
            uint160 b = _randPrice();
            uint128 l = uint128(_randBits(128));
            bool roundUp = _rand() % 2 == 0;
            string memory args = string.concat(_u(a), ",", _u(b), ",", _u(l), ",", _b(roundUp));

            try harness.getAmount0Delta(a, b, l, roundUp) returns (uint256 r) {
                _math("getAmount0Delta", args, _outs1(_u(r)));
            } catch {
                _mathErr("getAmount0Delta", args);
            }
            try harness.getAmount1Delta(a, b, l, roundUp) returns (uint256 r) {
                _math("getAmount1Delta", args, _outs1(_u(r)));
            } catch {
                _mathErr("getAmount1Delta", args);
            }
        }
    }

    // ---------------------------------------------------------------------
    // SwapMath
    // ---------------------------------------------------------------------

    function _swapStepVectors() internal {
        uint24[6] memory fees = [uint24(100), 500, 3000, 10000, 0, 999_999];
        for (uint256 i; i < 600; ++i) {
            uint160 current = _randPrice();
            // Targets are usually near the current price, like real tick steps.
            uint160 target = _rand() % 3 == 0 ? _randPrice() : _nearbyPrice(current);
            uint128 l = uint128(_randBits(128));
            int256 remaining = int256(_randBits(255));
            if (_rand() % 2 == 0) remaining = -remaining;
            if (remaining == 0) remaining = 1;
            uint24 fee = _rand() % 4 == 0 ? uint24(_rand() % 1_000_000) : fees[_rand() % fees.length];
            _swapStep(current, target, l, remaining, fee);
        }
    }

    function _swapStep(uint160 current, uint160 target, uint128 l, int256 remaining, uint24 fee) internal {
        string memory args = string.concat(_u(current), ",", _u(target), ",", _u(l), ",", _i(remaining), ",", _u(fee));
        try harness.computeSwapStep(current, target, l, remaining, fee) returns (
            uint160 next, uint256 amountIn, uint256 amountOut, uint256 feeAmount
        ) {
            _math(
                "computeSwapStep",
                args,
                string.concat("[", _u(next), ",", _u(amountIn), ",", _u(amountOut), ",", _u(feeAmount), "]")
            );
        } catch {
            _mathErr("computeSwapStep", args);
        }
    }

    // ---------------------------------------------------------------------
    // FullMath
    // ---------------------------------------------------------------------

    function _fullMathVectors() internal {
        for (uint256 i; i < 200; ++i) {
            uint256 a = _randBits(256);
            uint256 b = _randBits(256);
            uint256 d = _rand() % 20 == 0 ? 0 : _randBits(256);
            string memory args = string.concat(_u(a), ",", _u(b), ",", _u(d));
            try harness.mulDiv(a, b, d) returns (uint256 r) {
                _math("mulDiv", args, _outs1(_u(r)));
            } catch {
                _mathErr("mulDiv", args);
            }
            try harness.mulDivRoundingUp(a, b, d) returns (uint256 r) {
                _math("mulDivRoundingUp", args, _outs1(_u(r)));
            } catch {
                _mathErr("mulDivRoundingUp", args);
            }
        }
    }
}

/// @notice V2 quotes checked by the real pair, and V3 swaps run on real pools.
contract AmmVectors is VectorBase {
    V2Probe internal probe;
    V3Quoter internal quoter;

    struct PoolSpec {
        uint24 fee;
        int24 spacing;
        int24 centerTick;
        uint256 positions;
        int24 maxWidth;
        int24 spread;
        bool fullRange;
        bool onBoundary;
    }

    constructor() VectorBase(0x616d6d) {
        _deployUniswap();
        probe = new V2Probe();
        quoter = new V3Quoter();
    }

    function generate() external {
        _reset(V2_FILE);
        _reset(POOLS_FILE);
        _reset(SWAPS_FILE);
        _v2Vectors();
        _v3PoolVectors();
    }

    // ---------------------------------------------------------------------
    // Uniswap V2
    // ---------------------------------------------------------------------

    function _v2Vectors() internal {
        for (uint256 i; i < 120; ++i) {
            (MockERC20 t0, MockERC20 t1) = _newTokenPair();
            uint256 r0 = _randReserve();
            uint256 r1 = _randReserve();
            IUniswapV2Pair pair = _createV2Pair(address(t0), address(t1), r0, r1);

            for (uint256 j; j < 5; ++j) {
                bool zeroForOne = _rand() % 2 == 0;
                (uint256 rIn, uint256 rOut) = zeroForOne ? (r0, r1) : (r1, r0);
                address tokenIn = zeroForOne ? address(t0) : address(t1);
                if (j % 2 == 0) {
                    _v2Out(pair, tokenIn, zeroForOne, rIn, rOut);
                } else {
                    _v2In(pair, tokenIn, zeroForOne, rIn, rOut);
                }
            }
        }
    }

    function _v2Out(IUniswapV2Pair pair, address tokenIn, bool zeroForOne, uint256 rIn, uint256 rOut) internal {
        uint256 amountIn = _rand() % 8 == 0 ? 1 + _rand() % 1000 : 1 + _randBits(112);
        uint256 withFee = amountIn * 997;
        uint256 amountOut = (withFee * rOut) / (rIn * 1000 + withFee);
        bool accepted = _v2Accepts(pair, tokenIn, zeroForOne, amountIn, amountOut);
        bool acceptedPlusOne = _v2Accepts(pair, tokenIn, zeroForOne, amountIn, amountOut + 1);
        _write(
            V2_FILE,
            string.concat(
                '{"kind":"out","zeroForOne":',
                zeroForOne ? "true" : "false",
                ',"reserveIn":',
                _u(rIn),
                ',"reserveOut":',
                _u(rOut),
                ',"amountIn":',
                _u(amountIn),
                ',"amountOut":',
                _u(amountOut),
                ',"accepted":',
                accepted ? "true" : "false",
                ',"acceptedPlusOne":',
                acceptedPlusOne ? "true" : "false",
                "}"
            )
        );
    }

    function _v2In(IUniswapV2Pair pair, address tokenIn, bool zeroForOne, uint256 rIn, uint256 rOut) internal {
        uint256 amountOut = 1 + _rand() % (rOut - 1);
        uint256 amountIn = (rIn * amountOut * 1000) / ((rOut - amountOut) * 997) + 1;
        bool accepted = _v2Accepts(pair, tokenIn, zeroForOne, amountIn, amountOut);
        _write(
            V2_FILE,
            string.concat(
                '{"kind":"in","zeroForOne":',
                zeroForOne ? "true" : "false",
                ',"reserveIn":',
                _u(rIn),
                ',"reserveOut":',
                _u(rOut),
                ',"amountIn":',
                _u(amountIn),
                ',"amountOut":',
                _u(amountOut),
                ',"accepted":',
                accepted ? "true" : "false",
                "}"
            )
        );
    }

    function _v2Accepts(IUniswapV2Pair pair, address tokenIn, bool zeroForOne, uint256 amountIn, uint256 amountOut)
        internal
        returns (bool)
    {
        (uint256 out0, uint256 out1) = zeroForOne ? (uint256(0), amountOut) : (amountOut, uint256(0));
        return probe.accepts(address(pair), tokenIn, amountIn, out0, out1);
    }

    /// @dev Reserves from a few wei up to the uint112 ceiling, log-uniform.
    function _randReserve() internal returns (uint256) {
        uint256 r = _rand() % 10 == 0 ? 2 + _rand() % 10_000 : _randBits(112);
        return r < 2 ? 2 : r;
    }

    // ---------------------------------------------------------------------
    // Uniswap V3 pools and swaps
    // ---------------------------------------------------------------------

    function _v3PoolVectors() internal {
        PoolSpec[10] memory specs = [
            // fee, spacing, center, positions, maxWidth (in spacings), spread (ticks), fullRange, onBoundary
            PoolSpec(500, 10, 0, 30, 60, 5000, false, true),
            PoolSpec(3000, 60, -201_000, 40, 80, 20_000, true, false),
            PoolSpec(10_000, 200, 50_000, 20, 5, 40_000, false, false),
            PoolSpec(100, 1, 3, 40, 40, 60, false, false),
            PoolSpec(3000, 60, MIN_TICK + 600, 10, 30, 3000, true, false),
            PoolSpec(3000, 60, MAX_TICK - 600, 10, 30, 3000, true, false),
            PoolSpec(3000, 60, 12_345, 0, 1, 1, true, false),
            PoolSpec(500, 10, 0, 12, 50, 2000, false, true),
            PoolSpec(10_000, 200, -30_000, 25, 40, 30_000, true, false),
            PoolSpec(500, 10, 100, 30, 20, 1000, false, true)
        ];
        for (uint256 id; id < specs.length; ++id) {
            _v3Pool(id, specs[id]);
        }
    }

    function _v3Pool(uint256 id, PoolSpec memory spec) internal {
        (MockERC20 t0, MockERC20 t1) = _newTokenPair();
        int24 start = _align(spec.centerTick, spec.spacing);
        uint160 price = harness.getSqrtRatioAtTick(start);
        if (!spec.onBoundary) {
            // Somewhere strictly inside the tick above `start`.
            uint160 next = harness.getSqrtRatioAtTick(start + 1);
            price += uint160(_rand() % (next - price));
        }
        IUniswapV3Pool pool = _createV3Pool(address(t0), address(t1), spec.fee, price);

        int24[] memory ticks = new int24[](2 * spec.positions + 6);
        uint256 n;
        if (spec.fullRange) {
            ticks[n++] = _minUsableTick(spec.spacing);
            ticks[n++] = _maxUsableTick(spec.spacing);
            _addLiquidity(pool, ticks[n - 2], ticks[n - 1], uint128(10 ** (15 + _rand() % 8)));
        }
        if (id == 7) {
            // Two positions sharing tick 0 with equal liquidity: tick 0 stays
            // initialized with liquidityNet == 0, which still splits swap steps.
            _addLiquidity(pool, -1000, 0, 1e21);
            _addLiquidity(pool, 0, 1000, 1e21);
            ticks[n++] = -1000;
            ticks[n++] = 0;
            ticks[n++] = 1000;
        }
        for (uint256 i; i < spec.positions; ++i) {
            int24 offset = int24(int256(_rand() % uint256(int256(2 * spec.spread + 1)))) - spec.spread;
            int24 lower = _clampTick(_align(start + offset, spec.spacing), spec.spacing);
            int24 width = int24(int256(1 + _rand() % uint256(int256(spec.maxWidth)))) * spec.spacing;
            int24 upper = lower + width;
            if (upper > _maxUsableTick(spec.spacing)) {
                upper = _maxUsableTick(spec.spacing);
                lower = upper - width;
            }
            uint128 l = id == 8 ? uint128(10 ** (28 + _rand() % 5)) : uint128(10 ** (12 + _rand() % 12));
            _addLiquidity(pool, lower, upper, l);
            ticks[n++] = lower;
            ticks[n++] = upper;
        }

        _writePool(id, pool, spec, ticks, n);
        for (uint256 i; i < 110; ++i) {
            _v3Swap(id, pool, false);
        }
        for (uint256 i; i < 30; ++i) {
            _v3Swap(id, pool, true);
        }
    }

    function _writePool(uint256 id, IUniswapV3Pool pool, PoolSpec memory spec, int24[] memory ticks, uint256 n)
        internal
    {
        (uint160 price, int24 tick,,,,,) = pool.slot0();
        string memory tickJson;
        string memory wordJson;
        for (uint256 i; i < n; ++i) {
            if (_seen(ticks, i)) continue;
            (,,,,,,, bool initialized) = pool.ticks(ticks[i]);
            if (!initialized) continue;
            (, int128 net,,,,,,) = pool.ticks(ticks[i]);
            tickJson =
                string.concat(tickJson, bytes(tickJson).length == 0 ? "" : ",", "[", _i(ticks[i]), ",", _i(net), "]");
            int16 word = int16(ticks[i] / spec.spacing >> 8);
            if (!_wordSeen(ticks, i, spec.spacing)) {
                wordJson = string.concat(
                    wordJson, bytes(wordJson).length == 0 ? "" : ",", "[", _i(word), ",", _u(pool.tickBitmap(word)), "]"
                );
            }
        }
        _write(
            POOLS_FILE,
            string.concat(
                '{"id":',
                vm.toString(id),
                ',"fee":',
                vm.toString(spec.fee),
                ',"tickSpacing":',
                _iRaw(spec.spacing),
                ',"sqrtPriceX96":',
                _u(price),
                ',"tick":',
                _iRaw(tick),
                ',"liquidity":',
                _u(pool.liquidity()),
                ',"ticks":[',
                tickJson,
                '],"words":[',
                wordJson,
                "]}"
            )
        );
    }

    function _v3Swap(uint256 id, IUniswapV3Pool pool, bool sequential) internal {
        (uint160 price,,,,,,) = pool.slot0();
        bool zeroForOne = _rand() % 2 == 0;
        int256 amount = _randSwapAmount();
        if (_rand() % 2 == 0) amount = -amount;
        uint160 limit = _randLimit(price, zeroForOne, !sequential);

        V3Quoter.Quote memory q;
        if (sequential) {
            (bool ok, int256 a0, int256 a1) = quoter.swap(address(pool), zeroForOne, amount, limit);
            if (ok) {
                (uint160 p, int24 t,,,,,) = pool.slot0();
                q = V3Quoter.Quote(true, a0, a1, p, t, pool.liquidity());
            }
        } else {
            q = quoter.quote(address(pool), zeroForOne, amount, limit);
        }

        string memory head = string.concat(
            '{"pool":',
            vm.toString(id),
            ',"seq":',
            sequential ? "true" : "false",
            ',"zeroForOne":',
            zeroForOne ? "true" : "false",
            ',"amountSpecified":',
            _i(amount),
            ',"sqrtPriceLimitX96":',
            _u(limit),
            ',"ok":',
            q.ok ? "true" : "false"
        );
        if (!q.ok) {
            _write(SWAPS_FILE, string.concat(head, "}"));
            return;
        }
        _write(
            SWAPS_FILE,
            string.concat(
                head,
                ',"amount0":',
                _i(q.amount0),
                ',"amount1":',
                _i(q.amount1),
                ',"sqrtPriceX96":',
                _u(q.sqrtPriceX96),
                ',"tick":',
                _iRaw(q.tick),
                ',"liquidity":',
                _u(q.liquidity),
                "}"
            )
        );
    }

    /// @dev Dust, typical, and far-beyond-liquidity sizes.
    function _randSwapAmount() internal returns (int256) {
        uint256 kind = _rand() % 4;
        uint256 a;
        if (kind == 0) a = 1 + _rand() % 1000;
        else if (kind == 3) a = 1 + (_rand() >> (256 - 80 - _rand() % 50));
        else a = 1 + (_rand() >> (256 - 30 - _rand() % 50));
        return int256(a);
    }

    /// @dev Extreme limit, a limit some ticks away, or (rarely) an invalid one.
    function _randLimit(uint160 price, bool zeroForOne, bool allowInvalid) internal returns (uint160) {
        uint256 kind = _rand() % 10;
        if (allowInvalid && kind == 0) {
            return zeroForOne ? price + uint160(_rand() % 1000) : price - uint160(_rand() % 1000);
        }
        if (kind < 5) return zeroForOne ? MIN_SQRT_RATIO + 1 : MAX_SQRT_RATIO - 1;
        int24 current = harness.getTickAtSqrtRatio(price);
        int24 distance = int24(int256(1 + _rand() % 5000));
        int24 t = zeroForOne ? current - distance : current + distance;
        if (t <= MIN_TICK) return MIN_SQRT_RATIO + 1;
        if (t >= MAX_TICK) return MAX_SQRT_RATIO - 1;
        uint160 limit = harness.getSqrtRatioAtTick(t);
        if (zeroForOne ? limit >= price : limit <= price) return zeroForOne ? MIN_SQRT_RATIO + 1 : MAX_SQRT_RATIO - 1;
        return limit;
    }
}
