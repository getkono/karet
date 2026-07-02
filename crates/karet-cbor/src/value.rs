//! The decoded CBOR data model.

use ciborium::value::Integer as CiboriumInteger;
use ciborium::value::Value as CiboriumValue;

use crate::error::CborError;

/// A decoded CBOR value: a dynamic tree mirroring the CBOR data model.
///
/// This is karet-cbor's stable public value type; the `ciborium` wire codec is an
/// internal implementation detail. Obtain one by [`decode`](crate::decode)-ing
/// CBOR bytes, render it with [`to_diagnostic`](crate::to_diagnostic), and
/// re-encode with [`encode`](crate::encode).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum CborValue {
    /// An integer (major types 0 and 1). CBOR integers span −2^64 ..= 2^64−1,
    /// which fits in an `i128`.
    Integer(i128),
    /// A byte string (major type 2).
    Bytes(Vec<u8>),
    /// A floating-point number (major type 7).
    Float(f64),
    /// A UTF-8 text string (major type 3).
    Text(String),
    /// A boolean.
    Bool(bool),
    /// The `null` value.
    Null,
    /// A tagged value (major type 6): a tag number and the value it encloses.
    Tag(u64, Box<CborValue>),
    /// An array (major type 4).
    Array(Vec<CborValue>),
    /// A map (major type 5): an ordered list of key/value pairs. CBOR keys may be
    /// any value, and their order is preserved.
    Map(Vec<(CborValue, CborValue)>),
}

impl CborValue {
    /// Convert a decoded `ciborium` value into a [`CborValue`].
    pub(crate) fn from_ciborium(value: CiboriumValue) -> Result<Self, CborError> {
        Ok(match value {
            CiboriumValue::Integer(int) => CborValue::Integer(i128::from(int)),
            CiboriumValue::Bytes(bytes) => CborValue::Bytes(bytes),
            CiboriumValue::Float(f) => CborValue::Float(f),
            CiboriumValue::Text(s) => CborValue::Text(s),
            CiboriumValue::Bool(b) => CborValue::Bool(b),
            CiboriumValue::Null => CborValue::Null,
            CiboriumValue::Tag(tag, inner) => {
                CborValue::Tag(tag, Box::new(CborValue::from_ciborium(*inner)?))
            },
            CiboriumValue::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(CborValue::from_ciborium(item)?);
                }
                CborValue::Array(out)
            },
            CiboriumValue::Map(pairs) => {
                let mut out = Vec::with_capacity(pairs.len());
                for (key, value) in pairs {
                    out.push((
                        CborValue::from_ciborium(key)?,
                        CborValue::from_ciborium(value)?,
                    ));
                }
                CborValue::Map(out)
            },
            // `ciborium::value::Value` is `#[non_exhaustive]`; a future variant is
            // one this crate does not yet model.
            other => return Err(CborError::Unsupported(format!("{other:?}"))),
        })
    }

    /// Convert a [`CborValue`] into a `ciborium` value for encoding.
    pub(crate) fn into_ciborium(self) -> Result<CiboriumValue, CborError> {
        Ok(match self {
            CborValue::Integer(i) => {
                let int = CiboriumInteger::try_from(i).map_err(|_| CborError::IntegerRange(i))?;
                CiboriumValue::Integer(int)
            },
            CborValue::Bytes(bytes) => CiboriumValue::Bytes(bytes),
            CborValue::Float(f) => CiboriumValue::Float(f),
            CborValue::Text(s) => CiboriumValue::Text(s),
            CborValue::Bool(b) => CiboriumValue::Bool(b),
            CborValue::Null => CiboriumValue::Null,
            CborValue::Tag(tag, inner) => {
                CiboriumValue::Tag(tag, Box::new((*inner).into_ciborium()?))
            },
            CborValue::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(item.into_ciborium()?);
                }
                CiboriumValue::Array(out)
            },
            CborValue::Map(pairs) => {
                let mut out = Vec::with_capacity(pairs.len());
                for (key, value) in pairs {
                    out.push((key.into_ciborium()?, value.into_ciborium()?));
                }
                CiboriumValue::Map(out)
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_ciborium() {
        let value = CborValue::Map(vec![
            (CborValue::Text("n".to_string()), CborValue::Integer(-7)),
            (
                CborValue::Integer(1),
                CborValue::Array(vec![CborValue::Bool(true), CborValue::Null]),
            ),
        ]);
        let ciborium = value.clone().into_ciborium().ok();
        assert!(ciborium.is_some(), "conversion to ciborium should succeed");
        if let Some(ciborium) = ciborium {
            assert_eq!(CborValue::from_ciborium(ciborium).ok(), Some(value));
        }
    }

    #[test]
    fn integer_out_of_range_is_rejected() {
        let too_big = CborValue::Integer(i128::MAX);
        assert!(matches!(
            too_big.into_ciborium(),
            Err(CborError::IntegerRange(_))
        ));
    }
}
