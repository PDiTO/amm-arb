//! Cyclic arbitrage search over Uniswap V2 and V3 pools.
//!
//! The pipeline is the one most on-chain searchers use, minus the mempool:
//!
//! 1. [`market::Market`] holds pool state, loaded from JSON fixtures or RPC.
//! 2. [`graph::TokenGraph`] turns every pool into two directed edges weighted
//!    by `-ln(marginal rate after fees)`. A cycle whose weights sum below zero
//!    multiplies to a rate above one: an arbitrage at the margin.
//! 3. [`cycles`] finds those cycles: Bellman-Ford (SPFA) answers "is there any
//!    arbitrage at all", and a bounded enumeration lists every negative cycle
//!    of two or three hops.
//! 4. [`optimize`] sizes each candidate with exact integer quotes. Profit is
//!    concave in the input, so a bracketing scan plus golden-section search
//!    finds the optimum without derivatives.
//! 5. [`scan`] subtracts gas and ranks what is left.

pub mod cycles;
pub mod fixture;
pub mod graph;
pub mod market;
pub mod optimize;
pub mod scan;
pub mod synthetic;

pub use alloy_primitives::{Address, U256};
