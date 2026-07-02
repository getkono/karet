//! The CBOR wire codec: bytes ↔ [`CborValue`], backed by `ciborium`.

use ciborium::value::Value as CiboriumValue;

use crate::error::CborError;
use crate::value::CborValue;

/// Decode CBOR `bytes` into a [`CborValue`].
///
/// # Errors
/// Returns [`CborError::Decode`] if `bytes` is not valid CBOR, or
/// [`CborError::Unsupported`] for a value this crate does not model.
pub fn decode(bytes: &[u8]) -> Result<CborValue, CborError> {
    let value: CiboriumValue =
        ciborium::from_reader(bytes).map_err(|e| CborError::Decode(e.to_string()))?;
    CborValue::from_ciborium(value)
}

/// Encode a [`CborValue`] to CBOR bytes.
///
/// # Errors
/// Returns [`CborError::Encode`] on a serializer error, or
/// [`CborError::IntegerRange`] for an integer outside the CBOR range.
pub fn encode(value: &CborValue) -> Result<Vec<u8>, CborError> {
    let ciborium = value.clone().into_ciborium()?;
    let mut out = Vec::new();
    ciborium::into_writer(&ciborium, &mut out).map_err(|e| CborError::Encode(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_known_bytes() {
        // Major type 0, value 1 encodes as the single byte 0x01.
        assert_eq!(decode(&[0x01]).ok(), Some(CborValue::Integer(1)));
    }

    #[test]
    fn encodes_known_bytes() {
        assert_eq!(encode(&CborValue::Integer(1)).ok(), Some(vec![0x01]));
    }

    #[test]
    fn rejects_invalid_cbor() {
        // Empty input is not a valid CBOR item.
        assert!(matches!(decode(&[]), Err(CborError::Decode(_))));
    }

    #[test]
    fn matches_rfc8949_vectors() {
        // Canonical encodings straight from RFC 8949 Appendix A — an external
        // oracle proving we read and write standard CBOR, not just our own bytes.
        let cases: &[(CborValue, &[u8])] = &[
            (CborValue::Integer(-1), &[0x20]),
            (CborValue::Bool(false), &[0xf4]),
            (CborValue::Bool(true), &[0xf5]),
            (CborValue::Null, &[0xf6]),
            (
                CborValue::Text("IETF".to_string()),
                &[0x64, 0x49, 0x45, 0x54, 0x46],
            ),
            (CborValue::Bytes(vec![0x01, 0x02]), &[0x42, 0x01, 0x02]),
            (
                CborValue::Array(vec![
                    CborValue::Integer(1),
                    CborValue::Integer(2),
                    CborValue::Integer(3),
                ]),
                &[0x83, 0x01, 0x02, 0x03],
            ),
            (
                CborValue::Tag(1, Box::new(CborValue::Integer(1363896240))),
                &[0xc1, 0x1a, 0x51, 0x4b, 0x67, 0xb0],
            ),
            (
                CborValue::Map(vec![
                    (CborValue::Text("a".to_string()), CborValue::Integer(1)),
                    (
                        CborValue::Text("b".to_string()),
                        CborValue::Array(vec![CborValue::Integer(2), CborValue::Integer(3)]),
                    ),
                ]),
                &[0xa2, 0x61, 0x61, 0x01, 0x61, 0x62, 0x82, 0x02, 0x03],
            ),
        ];
        for (value, bytes) in cases {
            assert_eq!(
                encode(value).ok().as_deref(),
                Some(*bytes),
                "encode {value:?}"
            );
            assert_eq!(
                decode(bytes).ok().as_ref(),
                Some(value),
                "decode {bytes:02x?}"
            );
        }
    }

    #[test]
    fn byte_round_trips() {
        let value = CborValue::Map(vec![
            (
                CborValue::Text("bytes".to_string()),
                CborValue::Bytes(vec![0, 1, 255]),
            ),
            (
                CborValue::Text("tag".to_string()),
                CborValue::Tag(0, Box::new(CborValue::Text("t".to_string()))),
            ),
            (
                CborValue::Integer(-3),
                CborValue::Array(vec![
                    CborValue::Float(1.5),
                    CborValue::Bool(false),
                    CborValue::Null,
                ]),
            ),
        ]);
        let bytes = encode(&value).ok();
        assert!(bytes.is_some());
        if let Some(bytes) = bytes {
            assert_eq!(decode(&bytes).ok(), Some(value));
        }
    }
}
