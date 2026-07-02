//! CBOR diagnostic notation (RFC 8949 §8): a human-readable, lossless textual
//! rendering of a [`CborValue`](crate::CborValue), plus a parser for the same
//! canonical form.
//!
//! The parser accepts exactly what [`to_diagnostic`] emits, with lenient
//! whitespace and optional trailing commas — it is not a full parser for every
//! diagnostic-notation dialect (e.g. base64 byte strings or comments).

mod parse;
mod print;

pub use parse::from_diagnostic;
pub use print::to_diagnostic;
