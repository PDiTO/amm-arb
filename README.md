<p align="center">
  <img src="docs/logo.png" alt="amm-arb logo" width="200">
</p>

# amm-arb

A Rust searcher that prices Uniswap V2 and V3 swaps off-chain to the wei and finds cyclic arbitrage, plus a Solidity contract that executes a cycle atomically with a flash swap.

```
$ amm-arb scan --fixtures fixtures/planted.json --top 2
source      fixtures/planted.json
graph       9 pools, 4 tokens, 18 directed edges
search      Bellman-Ford: negative cycle found; 6 negative cycles of <= 3 hops; 10.0µs
sizing      6 cycles sized with 510 exact quotes of the full cycle; 1.9ms
gas         10 gwei

4 of 6 cycles are profitable after gas. Top 2:

#1  net +0.009952 WETH   gross 30.020111 USDC   gas 196k = 0.00196 WETH
    7,111.870971 USDC
      -> [V3 0.05% 0xb000…0002] 2.81878 WETH
      -> [V2 0.3% 0xb000…0001] 7,141.891082 USDC

#2  net +0.002646 WETH   gross 11.609446 USDC   gas 196k = 0.00196 WETH
    3,916.07597 USDC
      -> [V3 0.3% 0xb000…0003] 1.547742 WETH
      -> [V2 0.3% 0xb000…0001] 3,927.685416 USDC
```

Every amount in that output is an exact integer quote. Run the same scan on the real mainnet snapshot and it finds nothing, which is what you'd expect from deep pools at a random block.

## Why I built this

I wanted to know exactly how a searcher prices a Uniswap V3 swap off-chain. Not approximately. To the wei, the way the pool will actually settle it, including the rounding at every tick boundary. Floating-point price math gets you close, and close loses money: a quote that is one wei optimistic becomes a reverted bundle.

The other half of the question was the on-chain side. Once you have a cycle, how do you execute it with no inventory, in one transaction, and make sure nobody else can drive your contract's callbacks? I had a rough idea. I wanted working code and tests that would tell me if I was wrong.

So this repo splits the work the way searchers do. Rust finds and sizes opportunities. Solidity executes them.

## What I looked at

The Rust math is a line-by-line port of `v3-core` at the `v1.0.0` tag and of `getAmountOut` and `getAmountIn` from `UniswapV2Library`. On the V3 side that means TickMath, SqrtPriceMath, SwapMath, TickBitmap, BitMath, LiquidityMath, FullMath and the loop inside `UniswapV3Pool.swap`. Each Rust module maps to one Solidity library and keeps its function names, so you can read the two side by side.

Two ideas come from Uniswap's periphery. Quoting by reverting inside the swap callback is how QuoterV2 works, and I use the same trick to pull expected values out of real pools in the tests. Deriving pool addresses with CREATE2 instead of trusting them is how the periphery's callback validation works, and the executor does the same.

## How it fits together

```
                      off-chain (Rust)                                   on-chain (Solidity)

  RPC (alloy + Multicall3) ─┐
                            ├─> Market ─> TokenGraph ─> cycles ─> optimize ─> scan ──> ArbExecutor.execute(hops, amountIn, minProfit)
  JSON fixture ─────────────┘   (pools)   -ln(rate)     SPFA gate  golden-      ranked       │
                                          edges         + 2-3 hop  section on   by net       ├─ flash leg: pool pays out first
                                                        DFS        exact quotes profit       ├─ callback: run remaining hops
                                                                                             ├─ repay flash pool
                                                                                             └─ revert unless profit >= minProfit
```

The workspace has three crates and a Foundry project:

| path | what it is |
|---|---|
| `crates/amm-math` | V2 and V3 math with Solidity integer semantics, plus the V3 swap simulator |
| `crates/amm-arb` | market model, token graph, cycle detection, input sizing, gas-aware ranking, JSON fixtures |
| `crates/amm-arb-cli` | the `amm-arb` binary (`scan`, `quote`, `snapshot`) and the RPC loader |
| `contracts/` | `ArbExecutor`, its tests, and the script that generates differential vectors |
| `testdata/` | 5,292 vectors produced by running real Uniswap bytecode |
| `fixtures/` | a mainnet snapshot at block 23,700,000 and a synthetic market with a planted mispricing |

## The maths

### V2

A V2 pair holds reserves `x` and `y` and keeps `x * y` from falling, after charging 0.3% on the input. For an input `a` the output is

```
out = floor(a * 997 * y / (x * 1000 + a * 997))
```

and that floor is the largest output the pair's `k` check will accept. The differential tests check this against the real pair. It never accepts `out + 1`, and it accepts `out` in exactly the cases where the Rust says the swap is valid. `getAmountIn` has a quirk worth knowing: it always adds one after dividing, so when the division is exact it overpays by one wei. The port keeps that.

### V3

A V3 pool stores `sqrtPriceX96 = sqrt(price) * 2^96` as a `uint160`, and liquidity `L` is only active between the ticks of the positions that provide it. Within one range the pool behaves like a V2 pool with virtual reserves `x = L / sqrtP` and `y = L * sqrtP`, which gives

```
Δy = L * (sqrtP_b - sqrtP_a)
Δx = L * (sqrtP_b - sqrtP_a) / (sqrtP_a * sqrtP_b)
```

A swap is a loop. Find the next initialized tick in the current bitmap word and step toward that tick's price, stopping early if the input runs out. When the price lands exactly on the tick, cross it by adding its `liquidityNet`, negated when moving left. Rounding always favours the pool. Input amounts round up and output amounts round down, and `mulDiv` needs a 512-bit intermediate because `L << 96` times a price difference overflows 256 bits.

The details that decide whether you match to the wei:

- `getSqrtRatioAtTick` chains up to 19 multiplications by precomputed Q128.128 constants, one per bit of the tick, and rounds up at the end. `getTickAtSqrtRatio` computes a fixed-point log2 in 14 squaring rounds, then fixes a possible off-by-one with one forward evaluation.
- After a swap that ends exactly on a tick boundary moving left, the pool stores `tick - 1`, not the tick whose price it sits on. If your simulator recomputes the tick from the price, you drift.
- An initialized tick whose `liquidityNet` is zero still splits the swap into two steps, and that changes the rounding. You can't drop those ticks when loading state.
- `SwapMath` wraps in places. `uint256(-amountRemaining)` and `amountRemaining - amountIn` are unchecked in 0.7.6, so the port uses wrapping arithmetic exactly there and checked arithmetic where the Solidity uses SafeMath.

### Finding cycles

Every pool becomes two directed edges weighted `-ln(marginal rate after fees)`. A cycle whose weights sum below zero has rates that multiply to more than one, which is an arbitrage at the margin.

Bellman-Ford, run as SPFA from a virtual source connected to every token, proves there is no arbitrage anywhere in `O(V·E)`. Otherwise it hands back one negative cycle. It can't list every cycle, and the one it finds may be long. In practice searchers only care about two and three hops, because each extra hop costs 47k to 82k gas. So the scanner uses SPFA as a gate and then enumerates every simple negative cycle of up to three hops with a bounded DFS. On 17 mainnet pools the whole search takes microseconds.

### Sizing the trade

For both pool types the output is concave in the input, so profit `out(x) - x` has a single peak. The optimiser evaluates the exact integer quote at powers of two to bracket the peak in `[2^(k-1), 2^(k+1)]`, then runs golden-section search inside the bracket and brute-forces the last few units. Every evaluation runs the whole cycle through the same integer math the chain uses, so the sized amounts are what the pools would pay at that state.

One caveat. Each hop floors its output, so integer profit is only concave up to about a wei per hop. A property test compares golden-section against brute force on small pools and allows exactly that much slack. It found cases where the search lands one wei short of the true maximum, so I made the tolerance explicit instead of pretending it's exact.

Gas comes off at the end. The default model charges 67k fixed, 47k per V2 hop and 82k per V3 hop, fitted to the executor's measured gas below. Profit in other tokens converts to WETH at the mid price of the deepest direct pool.

## The executor

`ArbExecutor.execute(hops, amountIn, minProfit)` takes a list of hops. Each hop names a pool kind, the input and output tokens, and a V3 fee tier. There are no pool addresses in the call.

1. The first hop is the flash leg. For V2 it computes `getAmountOut` from live reserves and calls `pair.swap` with non-empty data. For V3 it calls `pool.swap` for an exact input. Either way the pool sends its output before it gets paid.
2. Inside the callback the remaining hops run as ordinary swaps. V2 hops transfer in, then call `swap`. V3 hops call `swap` and pay from a nested callback.
3. The last hop returns the start token. The executor repays the flash pool, and if what's left is below `minProfit` it reverts the whole transaction with `ProfitTooLow(amountOut, required)`.
4. `execute` sends the profit to the owner.

### Security considerations

- **Pool addresses are derived, never supplied.** The executor computes every pool with CREATE2 from an immutable factory address and init code hash. Nobody can point it at a fake pool through calldata.
- **Callbacks are accepted from one address, once.** Before each swap the executor writes the pool it's calling into EIP-1153 transient storage. The callback reverts unless `msg.sender` equals that address, then clears it. Outside `execute` the slot is empty, so every callback reverts. That covers the classic V2 attack where someone calls `pair.swap(to = victim, data)` so a genuine pair calls the victim's `uniswapV2Call`. There's a test for exactly that.
- **The fuzzer found a real edge.** When idle the slot holds `address(0)`, and my first version compared `msg.sender == _expectedCaller` directly, so a call "from" `address(0)` passed. No transaction can come from the zero address on mainnet, so it wasn't exploitable, but the guard shouldn't depend on that. It now rejects an empty slot explicitly.
- **Owner-only.** `execute` and `sweep` revert with `NotOwner` for anyone else. The owner is immutable. I chose not to support ownership transfer rather than add a two-step handover.
- **No stuck funds.** The contract never holds inventory. V3 hops that can't absorb the full input revert with `PartialFill` instead of leaving intermediate tokens behind, and an invariant test checks the balance is zero after every call sequence. `sweep` exists for tokens someone sends by mistake.
- **Non-standard tokens.** `SafeTransferLib` accepts tokens like USDT that return nothing from `transfer`, and refuses to treat a call to an address with no code as success. A fork test runs a cycle through USDT. Fee-on-transfer and rebasing tokens aren't supported.

## Testing

### Differential tests against real Uniswap bytecode

This is the part I cared most about. The expected values come from executing Uniswap's own contracts, not from a second implementation of the maths.

Uniswap V2 pins solc 0.5.16 and V3 pins 0.7.6, and neither compiler has a native Apple Silicon build. `contracts/vendor/build.mjs` compiles the `v2-core` and `v3-core` submodules with solc-js using Uniswap's original settings. The build fails unless the pair and pool init code hashes equal the mainnet ones, `0x96e8ac42…845f` and `0xe34f199b…8b54`. They match, so the vendored artifacts are byte-for-byte the bytecode deployed on mainnet. A CI job rebuilds them and diffs against the committed files, and a Foundry test re-checks the hashes.

`contracts/script/GenerateVectors.s.sol` deploys those cores inside Foundry and writes JSONL:

| file | vectors | how the expected value is produced |
|---|---|---|
| `v3_math.jsonl` | 3,292 | TickMath, SqrtPriceMath, SwapMath and FullMath called through a harness compiled with the pool's exact settings. Includes reverting inputs. |
| `v2_quotes.jsonl` | 600 | Reserves from 2 wei to the `uint112` limit. The real pair must reject one wei more than `getAmountOut`, and accept or reject `getAmountOut` exactly as the Rust does. |
| `v3_pools.jsonl` | 10 pools | Every fee tier, pools near `MIN_TICK` and `MAX_TICK`, a tick with zero `liquidityNet`, a price exactly on a tick boundary. Exported with the on-chain tick bitmap. |
| `v3_swaps.jsonl` | 1,400 | Exact-in and exact-out, both directions, 1 wei up to amounts that drain the pool, random price limits, invalid limits. 1,100 are quoted on a fixed state. 300 are executed in sequence so the pool state evolves. |

`crates/amm-math/tests/differential.rs` replays every line. Amounts, final price, tick and liquidity must match exactly, and the Rust must fail wherever the Solidity reverts. The swap vectors cross 6,346 initialized ticks between them. Randomness is a fixed keccak chain, so regenerating produces identical files, and CI checks that. As a sanity check I broke the fee rounding in `compute_swap_step` on purpose, and both V3 suites failed.

### Property tests (proptest)

- TickMath round trips for every tick, is strictly monotonic, and brackets any price.
- V2: `getAmountIn(getAmountOut(a)) <= a`, and the output is monotonic and below the reserve.
- V3 on random multi-position pools: exact output for `out` never costs more than the exact input that produced it, price moves the right way, the stored tick always matches the stored price, and output is monotonic in input.
- A market priced from one global price vector has no negative cycles by either algorithm and no opportunities.
- Plant a 1% to 8% mispricing on a random pool: SPFA finds a cycle, every enumerated cycle uses the skewed pool, and the scan reports positive gross profit.
- Profit along a planted cycle is midpoint-concave within rounding noise.
- Golden-section search agrees with brute force within the rounding bound.

### Executor tests (Foundry)

The local tests deploy the vendored V2 and V3 cores and build a market where the V2 A/B pair sells B 2% cheap.

- The happy paths cover a V2 flash leg, a V3 flash leg, and three-hop cycles with either two V3 hops or two V2 hops. Each one predicts the profit with an independent quote, using the V2 formula on live reserves and the revert quoter on the real V3 pools, and asserts the owner receives exactly that.
- Guards covered: `minProfit` (inclusive), losing cycles, oversized trades, malformed paths, zero amounts, amounts above `int256`, zero addresses, and a pool that doesn't exist.
- Callback attacks covered: direct calls to both callbacks, a genuine pool address calling while the executor is idle, and the V2 `pair.swap(to = executor)` trick.
- Fuzz tests run 1,000 times each. One fuzz test donates to the V2 pair to move its price either way, picks a cycle shape, an amount and a `minProfit`, and asserts that `execute` succeeds exactly when the independent quote clears `minProfit`, paying exactly the quoted profit. Others fuzz access control, callback callers, and CREATE2 derivation against both factories.
- The invariant suite runs 256 sequences of 64 calls, 16,384 calls in total. A handler mixes executions of six cycle shapes, trader swaps on every pool, and callback attacks. The invariants are that the executor holds zero tokens, the owner's balance equals the sum of reported profits, and no attack ever succeeds.

### Mainnet cross-checks (opt-in)

Both need an archive endpoint in `MAINNET_RPC_URL` and skip cleanly without one. `https://eth.drpc.org` worked for me without a key. `ethereum-rpc.publicnode.com` answers, but it refuses historical state without a token.

- `crates/amm-arb-cli/tests/mainnet_quoter.rs` takes the committed snapshot at block 23,700,000 and compares its quotes with Uniswap's deployed QuoterV2 and Router02 at the same block. 96 quotes across all 17 pools matched to the wei. It skipped 6 because the trade would run past the tick range the snapshot loaded, and the simulator returns an error in that case rather than guessing.
- `contracts/test/ArbExecutor.fork.t.sol` forks the same block, plants a mispricing by donating WETH to a V2 pair and calling `sync`, and executes real cycles against the real 0.05% pools, one of them through USDT.

## Numbers

### Gas

`forge test --isolate --gas-report` with Foundry v1.4.3. With `--isolate` each call is its own transaction, so these include the 21k intrinsic cost and cold storage access. None of these swaps cross an initialized tick.

| `execute` path | gas |
|---|---|
| 2 hops, V2 flash then V3 | 197,489 |
| 2 hops, V3 flash then V2 | 195,039 |
| 3 hops, V2 flash, V2, V3 | 244,664 |
| 3 hops, V2 flash, V3, V3 | 280,066 |
| 2 hops, reverts with `ProfitTooLow` | 192,426 |

The runtime bytecode is 7,759 bytes. One obvious saving I didn't take: a V2 hop could send its output straight to the next V2 pair instead of back to the executor, which skips a transfer.

### Rust timings

`cargo bench -p amm-arb --bench quotes` on an AMD Ryzen Threadripper 1920X, release build. The V3 swaps use the real USDC/WETH 0.05% pool at block 23,700,000 (31 October 2025). These are Criterion's point estimates from a short run with 10 samples, 100 ms warm-up and one second of measurement. Other work was running on the machine, so treat them as rough.

| operation | time |
|---|---|
| V2 `getAmountOut` | 102 ns |
| `getSqrtRatioAtTick` | 63 ns |
| `getTickAtSqrtRatio` | 246 ns |
| V3 swap, 1 WETH, no tick crossed | 0.88 µs |
| V3 swap, 100 WETH, 10 ticks crossed | 5.9 µs |
| V3 swap, 1,000 WETH, 136 ticks crossed | 73 µs |
| full scan of the mainnet snapshot, 17 pools, no cycle | 4.4 µs |
| full scan of the planted fixture, 9 pools, 6 cycles sized | 2.6 ms |

Almost all of the planted scan goes on sizing. It sizes every rotation of every candidate cycle, and the six cycles it reports used 510 full-cycle evaluations between them. Two speedups I haven't tried: a specialised 512-by-256-bit division for `mulDiv`, which falls back to ruint's generic `U512` division whenever the product overflows 256 bits, as it does in most `getAmount0Delta` calls, and caching tick prices across evaluations.

## Limitations

This is an educational project, not a production searcher.

- There's no mempool monitoring, no bundle building, and no submission to builders. The scanner works from a fixed snapshot or a single block.
- There's no MEV protection. Sending `execute` to the public mempool would get it copied or sandwiched. You'd want a private relay, and a real searcher would also bribe the builder out of the profit.
- Only canonical Uniswap V2 and V3 are supported, with one factory of each per executor. No Sushiswap, Curve, Balancer or V4.
- Cycles are capped at three hops in the scanner and four in the executor.
- The gas model is linear per hop and ignores tick crossings, which cost extra on V3.
- V3 state loaded over RPC covers a window of ticks around the current price. Quotes that would leave it fail instead of silently treating the rest as empty.
- Fee-on-transfer and rebasing tokens aren't supported.
- The optimiser is exact only up to about a wei per hop of rounding noise. That doesn't matter at real sizes, but it isn't exact.

## Quickstart

You need Rust 1.88 or newer and Foundry.

```sh
git clone --recurse-submodules <this repo> amm-arb
cd amm-arb

# Rust: math, search, CLI
cargo test
cargo install --path crates/amm-arb-cli    # puts `amm-arb` on your PATH
amm-arb scan --fixtures fixtures/planted.json
amm-arb scan --fixtures fixtures/mainnet-23700000.json
amm-arb quote --fixtures fixtures/mainnet-23700000.json \
    --pool 0x88e6A0c2dDD26FEEb64F039a2c41296FcB3f5640 --token-in WETH --amount 1000

# Solidity: executor tests, fuzz and invariants
cd contracts && forge test && cd ..

# Everything CI runs
make check
```

Live data needs an RPC URL:

```sh
amm-arb scan --rpc https://eth.drpc.org          # latest block, 17 default pools
amm-arb snapshot --rpc https://eth.drpc.org --block 23700000 --out fixtures/snap.json
MAINNET_RPC_URL=https://eth.drpc.org make fork   # fork tests and the QuoterV2 cross-check
```

Regenerating test inputs:

```sh
make vectors   # differential vectors from the vendored bytecode, deterministic
make vendor    # rebuild vendored bytecode from the submodules, needs Node 22
make bench
```

## Licensing

My code is MIT, see `LICENSE`.

The Uniswap sources are submodules under `contracts/lib` and are only used for testing. `v2-core` is GPL-3.0. `v3-core` was published under BUSL-1.1 for the core contracts, with a change date of 2023-04-01 after which it becomes GPL-2.0-or-later, and most of its libraries and interfaces are GPL-2.0-or-later already. The vendored bytecode in `contracts/vendor/artifacts` is compiled from those sources and keeps their licenses, as does `contracts/vendor/V3MathHarness.sol`, which links against the v3-core libraries.

`ArbExecutor` doesn't import any Uniswap code. It uses its own minimal MIT interfaces. The Rust crates are my port of the Uniswap maths, written against those sources. If you plan to use any of this for more than reading, check the upstream licenses yourself.
