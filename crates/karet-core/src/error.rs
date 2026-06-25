//! Error types for `karet-core`.

/// Errors produced when constructing or validating core vocabulary types.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// A [`Range`](crate::coord::Range)'s start was ordered after its end.
    #[error("range start must not exceed range end")]
    InvalidRange,
    /// A [`Span`](crate::coord::Span)'s start byte was greater than its end byte.
    #[error("span start must not exceed span end")]
    InvalidSpan,
    /// A position or index fell outside the valid bounds of its container.
    #[error("position out of bounds")]
    OutOfBounds,
}
