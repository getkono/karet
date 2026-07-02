//! The error type produced by karet-cbor.

use thiserror::Error;

/// An error decoding, encoding, or parsing CBOR.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CborError {
    /// The input bytes are not valid CBOR.
    #[error("invalid CBOR: {0}")]
    Decode(String),
    /// The value could not be serialized to CBOR.
    #[error("could not encode CBOR: {0}")]
    Encode(String),
    /// The diagnostic-notation text was malformed.
    #[error("diagnostic notation parse error at line {line}, column {column}: {message}")]
    Parse {
        /// 1-based line number where parsing failed.
        line: usize,
        /// 1-based column number where parsing failed.
        column: usize,
        /// A human-readable description of the failure.
        message: String,
    },
    /// An integer outside the CBOR range (−2^64 ..= 2^64−1) was given to the encoder.
    #[error("integer {0} is out of the CBOR range")]
    IntegerRange(i128),
    /// A CBOR value karet-cbor does not model (e.g. a future `ciborium` variant).
    #[error("unsupported CBOR value: {0}")]
    Unsupported(String),
}
