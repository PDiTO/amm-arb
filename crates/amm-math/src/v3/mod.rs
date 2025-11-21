//! Uniswap V3 concentrated-liquidity math and a swap simulator.
//!
//! Module layout follows `v3-core/contracts/libraries` so each file can be read
//! side by side with its Solidity counterpart.

pub mod bit_math;
pub mod liquidity_math;
pub mod pool;
pub mod sqrt_price_math;
pub mod swap_math;
pub mod tick_bitmap;
pub mod tick_math;

pub use pool::{SwapResult, V3Pool};

use alloy_primitives::{U256, aliases::U160};

/// `2^96`, the fixed-point scale of `sqrtPriceX96` (`FixedPoint96.Q96`).
pub const Q96: U256 = U256::from_limbs([0, 1 << 32, 0, 0]);

/// Widens a `uint160` to `uint256`.
#[inline]
pub(crate) fn wide(x: U160) -> U256 {
    U256::from(x)
}
