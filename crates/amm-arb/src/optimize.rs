//! Input sizing for a cycle using exact integer quotes.
//!
//! For constant-product and concentrated-liquidity pools the output of a swap
//! is concave in its input, and so is a composition of swaps. Profit
//! `out(x) - x` is therefore concave, with a single peak. Integer rounding adds
//! a few wei of noise but does not move the peak meaningfully.
//!
//! The search runs in two phases:
//!
//! 1. Evaluate powers of two until profit falls away (or quotes start failing
//!    because a pool runs dry). The best power `2^k` brackets the optimum in
//!    `[2^(k-1), 2^(k+1)]`.
//! 2. Golden-section search inside the bracket, reusing one evaluation per
//!    step, then a brute-force pass once the bracket is a handful of units wide.

use alloy_primitives::{I256, U256};

/// Result of sizing one cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizing {
    /// Optimal input.
    pub amount_in: U256,
    /// Exact output for that input.
    pub amount_out: U256,
    /// Number of cycle evaluations spent.
    pub evaluations: u32,
}

impl Sizing {
    /// `amount_out - amount_in`; positive by construction.
    pub fn profit(&self) -> U256 {
        self.amount_out - self.amount_in
    }
}

/// `1 / φ` as a fraction, for integer golden-section steps.
const INV_PHI_NUM: u64 = 618_033_988_749;
const INV_PHI_DEN: u64 = 1_000_000_000_000;

/// Largest input tried by default: `2^DEFAULT_MAX_BITS`.
pub const DEFAULT_MAX_BITS: usize = 128;

struct Evaluator<F> {
    output: F,
    evaluations: u32,
    best: Option<(U256, U256, I256)>,
}

impl<F: Fn(U256) -> Option<U256>> Evaluator<F> {
    /// Profit at `x`, or `None` if the cycle cannot be executed at that size.
    fn profit(&mut self, x: U256) -> Option<I256> {
        self.evaluations += 1;
        let out = (self.output)(x)?;
        let p = I256::from_raw(out).wrapping_sub(I256::from_raw(x));
        if self.best.is_none_or(|(_, _, bp)| p > bp) {
            self.best = Some((x, out, p));
        }
        Some(p)
    }
}

/// Finds the input that maximises `output(x) - x`.
///
/// `output` returns the exact amount the cycle returns for input `x`, or
/// `None` if any hop would revert. Returns `None` if no size is profitable.
pub fn optimize<F: Fn(U256) -> Option<U256>>(output: F, max_bits: usize) -> Option<Sizing> {
    let mut ev = Evaluator {
        output,
        evaluations: 0,
        best: None,
    };

    // Phase 1: bracket with powers of two.
    let mut best_k: Option<(usize, I256)> = None;
    let mut seen_valid = false;
    for k in 0..=max_bits {
        let x = U256::from(1) << k;
        match ev.profit(x) {
            None if seen_valid => break, // past the point where a pool runs dry
            None => {}
            Some(p) => {
                seen_valid = true;
                if best_k.is_none_or(|(_, bp)| p > bp) {
                    best_k = Some((k, p));
                } else if let Some((bk, bp)) = best_k
                    && bp.is_positive()
                    && k > bk + 1
                {
                    // Concave: once past the peak it only goes down.
                    break;
                }
            }
        }
    }
    let (k, p) = best_k?;
    if !p.is_positive() {
        return None;
    }

    // Phase 2: golden-section search in [2^(k-1), 2^(k+1)].
    let mut a = if k == 0 {
        U256::from(1)
    } else {
        U256::from(1) << (k - 1)
    };
    let mut b = U256::from(1) << (k + 1).min(255);
    let step = |a: U256, b: U256| (b - a) * U256::from(INV_PHI_NUM) / U256::from(INV_PHI_DEN);
    let score = |p: Option<I256>| p.unwrap_or(I256::MIN);

    let mut c = b - step(a, b);
    let mut d = a + step(a, b);
    let mut fc = score(ev.profit(c));
    let mut fd = score(ev.profit(d));
    while b - a > U256::from(8) {
        if fc >= fd {
            b = d;
            d = c;
            fd = fc;
            c = b - step(a, b);
            fc = score(ev.profit(c));
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + step(a, b);
            fd = score(ev.profit(d));
        }
        if c >= d {
            // Integer rounding collapsed the interior points; re-seed them.
            c = b - step(a, b);
            d = a + step(a, b);
            if c >= d {
                break;
            }
            fc = score(ev.profit(c));
            fd = score(ev.profit(d));
        }
    }
    let mut x = a;
    while x <= b {
        ev.profit(x);
        x += U256::from(1);
    }

    let (amount_in, amount_out, profit) = ev.best?;
    profit.is_positive().then_some(Sizing {
        amount_in,
        amount_out,
        evaluations: ev.evaluations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concave toy: out(x) = 2x - x^2 / 1e12 (optimum at x = 5e11).
    fn toy(x: U256) -> Option<U256> {
        let x = x.to::<u128>();
        if x > 2_000_000_000_000 {
            return None;
        }
        Some(U256::from(2 * x - x * x / 1_000_000_000_000))
    }

    #[test]
    fn finds_the_peak_of_a_concave_function() {
        let s = optimize(toy, 64).unwrap();
        // Peak profit is x - x^2/1e12 at x = 5e11, i.e. 2.5e11. Integer
        // flooring makes the top flat for about +-1e6 around the peak, so
        // check the profit rather than the exact argmax.
        assert_eq!(s.profit(), U256::from(250_000_000_000u128));
        assert!(s.amount_in.to::<u128>().abs_diff(500_000_000_000) < 1_000_000);
        assert!(s.evaluations < 200, "took {} evaluations", s.evaluations);
    }

    #[test]
    fn unprofitable_returns_none() {
        assert!(optimize(|x| Some(x * U256::from(99) / U256::from(100)), 64).is_none());
    }
}
