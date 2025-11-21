//! Port of `BitMath.sol`.

use alloy_primitives::U256;

/// Index of the most significant set bit. `x` must be non-zero.
///
/// The Solidity version is a hand-rolled binary search; the result is the same
/// as `bit_len - 1`, which is what this computes.
#[inline]
pub fn most_significant_bit(x: U256) -> u8 {
    debug_assert!(!x.is_zero(), "BitMath: zero has no msb");
    (x.bit_len() - 1) as u8
}

/// Index of the least significant set bit. `x` must be non-zero.
#[inline]
pub fn least_significant_bit(x: U256) -> u8 {
    debug_assert!(!x.is_zero(), "BitMath: zero has no lsb");
    x.trailing_zeros() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msb_and_lsb_at_the_edges() {
        assert_eq!(most_significant_bit(U256::from(1)), 0);
        assert_eq!(least_significant_bit(U256::from(1)), 0);
        assert_eq!(most_significant_bit(U256::MAX), 255);
        assert_eq!(least_significant_bit(U256::MAX), 0);
        let top = U256::from(1) << 255;
        assert_eq!(most_significant_bit(top), 255);
        assert_eq!(least_significant_bit(top), 255);
        assert_eq!(most_significant_bit(U256::from(0b1011_0000)), 7);
        assert_eq!(least_significant_bit(U256::from(0b1011_0000)), 4);
    }
}
