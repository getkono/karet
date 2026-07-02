//! `karet-cbor` — CBOR support for karet editors.
//!
//! CBOR (Concise Binary Object Representation, RFC 8949) is a compact binary
//! serialization format. This crate decodes CBOR bytes into a [`CborValue`] tree
//! (via the `ciborium` wire codec, kept as an internal detail) and renders that
//! tree as CBOR **diagnostic notation** — the standard, lossless textual form —
//! so an editor can show and edit CBOR as text and re-encode it on save.
//!
//! # Layers
//! - [`decode`] / [`encode`] — CBOR bytes ↔ [`CborValue`].
//! - [`to_diagnostic`] / [`from_diagnostic`] — [`CborValue`] ↔ diagnostic text.
//! - [`decode_to_text`] / [`encode_from_text`] — the bytes ↔ text seam an editor
//!   uses at file open/save.

mod codec;
mod diag;
mod error;
mod value;

pub use codec::decode;
pub use codec::encode;
pub use diag::from_diagnostic;
pub use diag::to_diagnostic;
pub use error::CborError;
pub use value::CborValue;

/// Decode CBOR `bytes` and render them as pretty diagnostic-notation text.
///
/// The seam for opening a `.cbor` file as an editable text document.
///
/// # Errors
/// Returns [`CborError`] if `bytes` is not valid or representable CBOR.
pub fn decode_to_text(bytes: &[u8]) -> Result<String, CborError> {
    Ok(to_diagnostic(&decode(bytes)?))
}

/// Parse diagnostic-notation `text` and encode it back to CBOR bytes.
///
/// The seam for saving an edited `.cbor` document.
///
/// # Errors
/// Returns [`CborError`] if `text` is not valid diagnostic notation, or the
/// parsed value cannot be encoded.
pub fn encode_from_text(text: &str) -> Result<Vec<u8>, CborError> {
    encode(&from_diagnostic(text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_to_text_and_back_preserves_the_value() {
        let value = CborValue::Map(vec![
            (CborValue::Text("id".to_string()), CborValue::Integer(7)),
            (
                CborValue::Text("blob".to_string()),
                CborValue::Bytes(vec![1, 2, 3]),
            ),
            (CborValue::Text("ok".to_string()), CborValue::Bool(true)),
        ]);
        let bytes = encode(&value).ok();
        assert!(bytes.is_some());
        let Some(bytes) = bytes else { return };

        let text = decode_to_text(&bytes).ok();
        assert!(text.is_some());
        let Some(text) = text else { return };

        // The edited text re-encodes to CBOR that decodes back to the same value.
        let reencoded = encode_from_text(&text).ok();
        assert!(reencoded.is_some());
        if let Some(reencoded) = reencoded {
            assert_eq!(decode(&reencoded).ok(), Some(value));
        }
    }

    #[test]
    fn malformed_text_fails_to_encode() {
        assert!(encode_from_text("{ this is not cbor").is_err());
    }
}
