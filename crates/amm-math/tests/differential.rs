//! Differential tests against the real Uniswap bytecode.
//!
//! The vectors in `testdata/` are produced by `contracts/script/GenerateVectors.s.sol`,
//! which runs the canonical V2 pair, the canonical V3 pool, and the v3-core
//! math libraries (compiled exactly as Uniswap compiled them) inside Foundry.
//! Every expected value here came out of that bytecode. The Rust must agree to
//! the wei, and must fail exactly where the Solidity reverts.

#![allow(clippy::semicolon_if_nothing_returned)]

use std::fs;
use std::path::PathBuf;

use amm_math::{
    I256, U160, U256,
    full_math::{mul_div, mul_div_rounding_up},
    v2::{V2Pair, get_amount_in, get_amount_out},
    v3::{
        V3Pool,
        sqrt_price_math::{
            get_amount0_delta, get_amount1_delta, get_next_sqrt_price_from_input,
            get_next_sqrt_price_from_output,
        },
        swap_math::compute_swap_step,
        tick_math::{get_sqrt_ratio_at_tick, get_tick_at_sqrt_ratio},
    },
};
use serde_json::Value;

fn lines(file: &str) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(file);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}. Run `forge script script/GenerateVectors.s.sol` in contracts/",
            path.display()
        )
    });
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid json line"))
        .collect()
}

fn s(v: &Value) -> &str {
    v.as_str()
        .unwrap_or_else(|| panic!("expected string, got {v}"))
}
fn u256(v: &Value) -> U256 {
    s(v).parse().expect("u256")
}
fn u160(v: &Value) -> U160 {
    s(v).parse().expect("u160")
}
fn u128_(v: &Value) -> u128 {
    s(v).parse().expect("u128")
}
fn i256(v: &Value) -> I256 {
    I256::from_dec_str(s(v)).expect("i256")
}
fn i32_(v: &Value) -> i32 {
    match v {
        Value::String(x) => x.parse().expect("i32"),
        _ => v.as_i64().expect("i32") as i32,
    }
}
fn boolean(v: &Value) -> bool {
    match v {
        Value::String(x) => x == "true",
        _ => v.as_bool().expect("bool"),
    }
}

/// Compares a Rust result against a vector: same success/failure, same outputs.
fn check<T: PartialEq + std::fmt::Debug, E: std::fmt::Debug>(
    line: &Value,
    got: Result<T, E>,
    expected: impl FnOnce(&[Value]) -> T,
) {
    let ok = line["ok"].as_bool().unwrap();
    match (ok, got) {
        (true, Ok(g)) => {
            let want = expected(line["out"].as_array().unwrap());
            assert_eq!(g, want, "mismatch on {line}");
        }
        (false, Err(_)) => {}
        (true, Err(e)) => panic!("Solidity succeeded but Rust failed with {e:?} on {line}"),
        (false, Ok(g)) => panic!("Solidity reverted but Rust returned {g:?} on {line}"),
    }
}

#[test]
fn v3_math_libraries_match_bytecode() {
    let vectors = lines("v3_math.jsonl");
    assert!(vectors.len() > 3000, "expected the full vector set");
    let mut by_fn = std::collections::BTreeMap::<String, usize>::new();

    for line in &vectors {
        let f = line["fn"].as_str().unwrap();
        *by_fn.entry(f.to_owned()).or_default() += 1;
        let a = line["args"].as_array().unwrap();
        match f {
            "getSqrtRatioAtTick" => {
                check(line, get_sqrt_ratio_at_tick(i32_(&a[0])), |o| u160(&o[0]))
            }
            "getTickAtSqrtRatio" => {
                check(line, get_tick_at_sqrt_ratio(u160(&a[0])), |o| i32_(&o[0]))
            }
            "getNextSqrtPriceFromInput" => check(
                line,
                get_next_sqrt_price_from_input(
                    u160(&a[0]),
                    u128_(&a[1]),
                    u256(&a[2]),
                    boolean(&a[3]),
                ),
                |o| u160(&o[0]),
            ),
            "getNextSqrtPriceFromOutput" => check(
                line,
                get_next_sqrt_price_from_output(
                    u160(&a[0]),
                    u128_(&a[1]),
                    u256(&a[2]),
                    boolean(&a[3]),
                ),
                |o| u160(&o[0]),
            ),
            "getAmount0Delta" => check(
                line,
                get_amount0_delta(u160(&a[0]), u160(&a[1]), u128_(&a[2]), boolean(&a[3])),
                |o| u256(&o[0]),
            ),
            "getAmount1Delta" => check(
                line,
                get_amount1_delta(u160(&a[0]), u160(&a[1]), u128_(&a[2]), boolean(&a[3])),
                |o| u256(&o[0]),
            ),
            "computeSwapStep" => check(
                line,
                compute_swap_step(
                    u160(&a[0]),
                    u160(&a[1]),
                    u128_(&a[2]),
                    i256(&a[3]),
                    s(&a[4]).parse().unwrap(),
                )
                .map(|st| {
                    (
                        st.sqrt_ratio_next_x96,
                        st.amount_in,
                        st.amount_out,
                        st.fee_amount,
                    )
                }),
                |o| (u160(&o[0]), u256(&o[1]), u256(&o[2]), u256(&o[3])),
            ),
            "mulDiv" => check(line, mul_div(u256(&a[0]), u256(&a[1]), u256(&a[2])), |o| {
                u256(&o[0])
            }),
            "mulDivRoundingUp" => check(
                line,
                mul_div_rounding_up(u256(&a[0]), u256(&a[1]), u256(&a[2])),
                |o| u256(&o[0]),
            ),
            other => panic!("unknown fn {other}"),
        }
    }
    println!("v3 math vectors: {} {by_fn:?}", vectors.len());
}

#[test]
fn v2_quotes_match_the_pair_k_check() {
    let vectors = lines("v2_quotes.jsonl");
    assert_eq!(vectors.len(), 600);
    for line in &vectors {
        let zero_for_one = line["zeroForOne"].as_bool().unwrap();
        let r_in = u256(&line["reserveIn"]);
        let r_out = u256(&line["reserveOut"]);
        let amount_in = u256(&line["amountIn"]);
        let amount_out = u256(&line["amountOut"]);
        let accepted = line["accepted"].as_bool().unwrap();
        let (r0, r1) = if zero_for_one {
            (r_in, r_out)
        } else {
            (r_out, r_in)
        };
        let pair = V2Pair::new(r0.to(), r1.to()).unwrap();

        match line["kind"].as_str().unwrap() {
            "out" => {
                assert_eq!(
                    get_amount_out(amount_in, r_in, r_out).unwrap(),
                    amount_out,
                    "{line}"
                );
                // The real pair accepts exactly what the quote allows...
                assert_eq!(
                    pair.amount_out(zero_for_one, amount_in).is_ok(),
                    accepted,
                    "{line}"
                );
                // ...and never one wei more: getAmountOut is the maximum.
                assert!(!line["acceptedPlusOne"].as_bool().unwrap(), "{line}");
            }
            "in" => {
                assert_eq!(
                    get_amount_in(amount_out, r_in, r_out).unwrap(),
                    amount_in,
                    "{line}"
                );
                assert_eq!(
                    pair.amount_in(zero_for_one, amount_out).is_ok(),
                    accepted,
                    "{line}"
                );
            }
            other => panic!("unknown kind {other}"),
        }
    }
}

fn load_pools() -> Vec<V3Pool> {
    lines("v3_pools.jsonl")
        .iter()
        .enumerate()
        .map(|(i, p)| {
            assert_eq!(p["id"].as_u64().unwrap() as usize, i);
            let mut pool = V3Pool::new(
                u160(&p["sqrtPriceX96"]),
                p["tick"].as_i64().unwrap() as i32,
                u128_(&p["liquidity"]),
                p["fee"].as_u64().unwrap() as u32,
                p["tickSpacing"].as_i64().unwrap() as i32,
            )
            .unwrap();
            for t in p["ticks"].as_array().unwrap() {
                pool.insert_tick(i32_(&t[0]), s(&t[1]).parse().unwrap())
                    .unwrap();
            }
            // The bitmap derived from the ticks must equal the one on chain.
            for w in p["words"].as_array().unwrap() {
                let word: i16 = s(&w[0]).parse().unwrap();
                assert_eq!(
                    pool.bitmap().word(word).unwrap(),
                    u256(&w[1]),
                    "pool {i} word {word}"
                );
            }
            pool
        })
        .collect()
}

#[test]
fn v3_swaps_match_the_real_pool() {
    let mut pools = load_pools();
    assert_eq!(pools.len(), 10);
    let vectors = lines("v3_swaps.jsonl");
    let (mut ok, mut failed, mut crossed) = (0, 0, 0);

    for line in &vectors {
        let id = line["pool"].as_u64().unwrap() as usize;
        let zero_for_one = line["zeroForOne"].as_bool().unwrap();
        let amount = i256(&line["amountSpecified"]);
        let limit = u160(&line["sqrtPriceLimitX96"]);
        let got = pools[id].swap(zero_for_one, amount, Some(limit));

        if line["ok"].as_bool().unwrap() {
            let r = got.unwrap_or_else(|e| {
                panic!("pool swap succeeded on chain, Rust failed: {e:?} {line}")
            });
            assert_eq!(r.amount0, i256(&line["amount0"]), "amount0 {line}");
            assert_eq!(r.amount1, i256(&line["amount1"]), "amount1 {line}");
            assert_eq!(
                r.sqrt_price_x96,
                u160(&line["sqrtPriceX96"]),
                "price {line}"
            );
            assert_eq!(r.tick, line["tick"].as_i64().unwrap() as i32, "tick {line}");
            assert_eq!(r.liquidity, u128_(&line["liquidity"]), "liquidity {line}");
            crossed += r.ticks_crossed;
            ok += 1;
            if line["seq"].as_bool().unwrap() {
                pools[id] = pools[id].apply(&r);
            }
        } else {
            assert!(
                got.is_err(),
                "pool swap reverted on chain, Rust returned {got:?} for {line}"
            );
            failed += 1;
        }
    }
    assert_eq!(ok + failed, 1400);
    assert!(
        crossed > 1000,
        "vectors should exercise tick crossings, got {crossed}"
    );
    println!("v3 swaps: {ok} matched, {failed} reverts matched, {crossed} tick crossings");
}
