//! Exact integer Uniswap V2 and V3 swap math.
//!
//! Every function in this crate is a port of the corresponding Solidity with the
//! same integer widths, the same rounding direction and the same failure points.
//! Where the Solidity relies on unchecked (wrapping) arithmetic, the port wraps
//! too; where the Solidity reverts, the port returns a [`MathError`].
//!
//! The crate is checked against the real Uniswap bytecode by the differential
//! tests in `tests/differential.rs`, which replay vectors produced by a Foundry
//! script running the canonical V2 and V3 core contracts.

pub mod error;
pub mod full_math;
pub mod v2;

pub use alloy_primitives::{I256, U256, aliases::U160};
pub use error::MathError;
