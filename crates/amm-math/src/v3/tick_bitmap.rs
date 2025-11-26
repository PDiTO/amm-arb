//! Port of `TickBitmap.sol`: one bit per usable tick, 256 ticks per word.
//!
//! The bitmap indexes *compressed* ticks (`tick / tickSpacing`). Word `w` bit
//! `b` is set when compressed tick `w * 256 + b` has liquidity referencing it.

use std::collections::HashMap;
use std::ops::RangeInclusive;

use alloy_primitives::U256;

use super::bit_math::{least_significant_bit, most_significant_bit};
use crate::MathError;

/// Sparse tick bitmap, optionally restricted to the words that were loaded.
///
/// Pools loaded from chain usually only fetch the words around the current
/// price. Reading outside `loaded_words` returns
/// [`MathError::TickWordNotLoaded`] instead of silently treating the word as
/// empty, which would produce a wrong quote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickBitmap {
    words: HashMap<i16, U256>,
    loaded_words: Option<RangeInclusive<i16>>,
}

/// Word and bit position of a compressed tick (`TickBitmap.position`).
#[inline]
pub fn position(compressed: i32) -> (i16, u8) {
    // `int16(tick >> 8)` and `uint8(tick % 256)`; the latter is the low byte.
    ((compressed >> 8) as i16, (compressed & 0xff) as u8)
}

impl TickBitmap {
    /// Empty bitmap with every word considered loaded.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts reads to `range`; other words fail with `TickWordNotLoaded`.
    pub fn set_loaded_words(&mut self, range: Option<RangeInclusive<i16>>) {
        self.loaded_words = range;
    }

    /// The word range this bitmap was loaded for, if restricted.
    pub fn loaded_words(&self) -> Option<&RangeInclusive<i16>> {
        self.loaded_words.as_ref()
    }

    /// Raw word, zero if unset.
    pub fn word(&self, word_pos: i16) -> Result<U256, MathError> {
        if let Some(range) = &self.loaded_words
            && !range.contains(&word_pos)
        {
            return Err(MathError::TickWordNotLoaded(word_pos));
        }
        Ok(self.words.get(&word_pos).copied().unwrap_or_default())
    }

    /// Non-zero words in the bitmap.
    pub fn words(&self) -> impl Iterator<Item = (i16, U256)> + '_ {
        self.words
            .iter()
            .map(|(k, v)| (*k, *v))
            .filter(|(_, v)| !v.is_zero())
    }

    /// Replaces a whole word, as when loading `tickBitmap(int16)` from chain.
    pub fn set_word(&mut self, word_pos: i16, word: U256) {
        if word.is_zero() {
            self.words.remove(&word_pos);
        } else {
            self.words.insert(word_pos, word);
        }
    }

    /// Flips the bit for `tick` (`TickBitmap.flipTick`).
    pub fn flip_tick(&mut self, tick: i32, tick_spacing: i32) -> Result<(), MathError> {
        if tick % tick_spacing != 0 {
            return Err(MathError::InvalidTick(tick));
        }
        let (word_pos, bit_pos) = position(tick / tick_spacing);
        let word = self.words.entry(word_pos).or_default();
        *word ^= U256::from(1) << bit_pos;
        Ok(())
    }

    /// Whether `tick`'s bit is set.
    pub fn is_initialized(&self, tick: i32, tick_spacing: i32) -> bool {
        if tick % tick_spacing != 0 {
            return false;
        }
        let (word_pos, bit_pos) = position(tick / tick_spacing);
        self.words
            .get(&word_pos)
            .is_some_and(|w| w.bit(bit_pos as usize))
    }

    /// Next initialized tick in the same word as `tick`, searching left
    /// (`lte = true`, inclusive) or right (exclusive).
    ///
    /// Returns the word boundary with `initialized = false` if none is found,
    /// so a swap never jumps more than 256 compressed ticks per step.
    pub fn next_initialized_tick_within_one_word(
        &self,
        tick: i32,
        tick_spacing: i32,
        lte: bool,
    ) -> Result<(i32, bool), MathError> {
        let mut compressed = tick / tick_spacing;
        // Round towards negative infinity.
        if tick < 0 && tick % tick_spacing != 0 {
            compressed -= 1;
        }

        if lte {
            let (word_pos, bit_pos) = position(compressed);
            // All the 1s at or to the right of bit_pos.
            let mask = (U256::from(1) << bit_pos) - U256::from(1) + (U256::from(1) << bit_pos);
            let masked = self.word(word_pos)? & mask;
            let initialized = !masked.is_zero();
            let next = if initialized {
                (compressed - i32::from(bit_pos - most_significant_bit(masked))) * tick_spacing
            } else {
                (compressed - i32::from(bit_pos)) * tick_spacing
            };
            Ok((next, initialized))
        } else {
            // Start from the next tick's word; the current tick does not matter.
            let (word_pos, bit_pos) = position(compressed + 1);
            // All the 1s at or to the left of bit_pos.
            let mask = !((U256::from(1) << bit_pos) - U256::from(1));
            let masked = self.word(word_pos)? & mask;
            let initialized = !masked.is_zero();
            let next = if initialized {
                (compressed + 1 + i32::from(least_significant_bit(masked) - bit_pos)) * tick_spacing
            } else {
                (compressed + 1 + i32::from(u8::MAX - bit_pos)) * tick_spacing
            };
            Ok((next, initialized))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(ticks: &[i32]) -> TickBitmap {
        let mut b = TickBitmap::new();
        for &t in ticks {
            b.flip_tick(t, 1).unwrap();
        }
        b
    }

    // Cases from v3-core/test/TickBitmap.spec.ts, tick spacing 1.
    const INITIALIZED: [i32; 7] = [-200, -55, -4, 70, 78, 84, 139];

    #[test]
    fn position_uses_floor_semantics() {
        assert_eq!(position(-1), (-1, 255));
        assert_eq!(position(-256), (-1, 0));
        assert_eq!(position(-257), (-2, 255));
        assert_eq!(position(255), (0, 255));
        assert_eq!(position(256), (1, 0));
    }

    #[test]
    fn search_right() {
        let b = bitmap(&INITIALIZED);
        let next = |t| {
            b.next_initialized_tick_within_one_word(t, 1, false)
                .unwrap()
        };
        assert_eq!(next(78), (84, true));
        assert_eq!(next(-55), (-4, true));
        assert_eq!(next(77), (78, true));
        assert_eq!(next(-56), (-55, true));
        assert_eq!(next(255), (511, false));
        assert_eq!(next(-257), (-200, true));
    }

    #[test]
    fn search_left() {
        let b = bitmap(&INITIALIZED);
        let next = |t| b.next_initialized_tick_within_one_word(t, 1, true).unwrap();
        assert_eq!(next(78), (78, true));
        assert_eq!(next(79), (78, true));
        assert_eq!(next(258), (256, false));
        assert_eq!(next(256), (256, false));
        assert_eq!(next(72), (70, true));
        assert_eq!(next(-257), (-512, false));
        assert_eq!(next(1023), (768, false));
    }

    #[test]
    fn flip_twice_clears() {
        let mut b = bitmap(&[-230]);
        assert!(b.is_initialized(-230, 1));
        b.flip_tick(-230, 1).unwrap();
        assert!(!b.is_initialized(-230, 1));
    }

    #[test]
    fn unloaded_words_fail_loudly() {
        let mut b = bitmap(&INITIALIZED);
        b.set_loaded_words(Some(0..=0));
        assert!(
            b.next_initialized_tick_within_one_word(78, 1, false)
                .is_ok()
        );
        assert_eq!(
            b.next_initialized_tick_within_one_word(-3, 1, true),
            Err(MathError::TickWordNotLoaded(-1))
        );
    }
}
