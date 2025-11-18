//! Error type shared by the V2 and V3 math.

use thiserror::Error;

/// A failure that the equivalent Solidity would surface as a revert.
///
/// Variants map onto the `require` statements in the Uniswap contracts. The
/// short revert string used on-chain is noted on each variant where one exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MathError {
    /// A checked multiplication or addition overflowed, or a result did not fit
    /// its destination type (`SafeMath`, `SafeCast`, `FullMath`).
    #[error("arithmetic overflow")]
    Overflow,
    /// Division by zero in checked Solidity arithmetic.
    #[error("division by zero")]
    DivisionByZero,
    /// `TickMath.getSqrtRatioAtTick` was given a tick outside `[MIN_TICK, MAX_TICK]` (`T`).
    #[error("tick out of range")]
    TickOutOfBounds,
    /// `TickMath.getTickAtSqrtRatio` was given a price outside `[MIN_SQRT_RATIO, MAX_SQRT_RATIO)` (`R`).
    #[error("sqrt price out of range")]
    SqrtPriceOutOfBounds,
    /// `SqrtPriceMath` was called with a zero price or zero liquidity.
    #[error("sqrt price or liquidity is zero")]
    ZeroPriceOrLiquidity,
    /// Removing the requested amount would move the price past zero, or the
    /// resulting price does not fit in `uint160`.
    #[error("next sqrt price out of range")]
    NextPriceOutOfRange,
    /// `LiquidityMath.addDelta` underflowed (`LS`).
    #[error("liquidity underflow")]
    LiquidityUnderflow,
    /// `LiquidityMath.addDelta` overflowed (`LA`).
    #[error("liquidity overflow")]
    LiquidityOverflow,
    /// `swap` was called with `amountSpecified == 0` (`AS`).
    #[error("amount specified is zero")]
    ZeroAmountSpecified,
    /// The sqrt price limit is on the wrong side of the current price or outside
    /// the valid range (`SPL`).
    #[error("invalid sqrt price limit")]
    InvalidPriceLimit,
    /// A V2 quote was requested with a zero input (`INSUFFICIENT_INPUT_AMOUNT`).
    #[error("insufficient input amount")]
    InsufficientInputAmount,
    /// A V2 quote was requested with a zero output (`INSUFFICIENT_OUTPUT_AMOUNT`).
    #[error("insufficient output amount")]
    InsufficientOutputAmount,
    /// A pool cannot fill the request: empty reserves, output at or above the
    /// reserve, or a V3 swap that ran out of liquidity before filling.
    #[error("insufficient liquidity")]
    InsufficientLiquidity,
    /// A V2 swap would push a reserve past `uint112` (`UniswapV2: OVERFLOW`).
    #[error("reserve overflow")]
    ReserveOverflow,
    /// A tick is not a multiple of the pool's tick spacing, or is out of range.
    #[error("tick {0} is not usable with this tick spacing")]
    InvalidTick(i32),
    /// The swap needed a tick bitmap word that was not loaded from chain.
    #[error("tick bitmap word {0} was not loaded")]
    TickWordNotLoaded(i16),
    /// The bitmap marks a tick as initialized but no liquidity data was loaded for it.
    #[error("missing liquidity data for initialized tick {0}")]
    MissingTick(i32),
}
