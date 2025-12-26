use std::convert::Infallible;
use std::io::{self, Write};

use bytes::{Bytes, BytesMut};
use serde::Serialize;
use thiserror::Error;

#[cfg(feature = "prost")]
mod prost;

#[derive(Debug, Error)]
pub enum Error {
    #[error("JSON serialization error")]
    Json(#[from] serde_json::Error),
    #[error("CBOR decode error")]
    CborDecode(#[from] minicbor_serde::error::DecodeError),
    #[error("CBOR encode error")]
    CborEncode(#[from] minicbor_serde::error::EncodeError<std::convert::Infallible>),
    #[error("UTF-8 encoding error")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("I/O error")]
    Io(#[from] io::Error),
    #[error("Prost serialization error")]
    ProstSerialization(#[from] serde::de::value::Error),
}

struct Base64Formatter;

impl serde_json::ser::Formatter for Base64Formatter {
    fn write_byte_array<W>(&mut self, mut writer: &mut W, value: &[u8]) -> io::Result<()>
    where
        W: io::Write + ?Sized,
    {
        writer.write_all(b"\"")?;
        base64::write::EncoderWriter::new(&mut writer, &base64::engine::general_purpose::STANDARD)
            .write_all(value)?;
        writer.write_all(b"\"")
    }
}

/// Convert JSON string to CBOR bytes.
///
/// # Errors
/// Returns error if JSON parsing or CBOR serialization fails.
pub fn json_to_cbor(json: &str) -> Result<Bytes, Error> {
    let mut serializer = minicbor_serde::Serializer::new(CborBytesMut::default());
    serde_transcode::Transcoder::new(&mut serde_json::Deserializer::from_str(json))
        .serialize(serializer.serialize_unit_as_null(true))?;
    Ok(serializer.into_encoder().into_writer().freeze())
}

/// Convert CBOR bytes to JSON string.
///
/// # Errors
/// Returns error if CBOR parsing or JSON serialization fails.
pub fn cbor_to_json(cbor: &[u8]) -> Result<String, Error> {
    let mut o = vec![];
    serde_transcode::Transcoder::new(&mut minicbor_serde::Deserializer::new(cbor)).serialize(
        &mut serde_json::Serializer::with_formatter(&mut o, Base64Formatter),
    )?;
    Ok(String::from_utf8(o)?)
}

#[cfg(feature = "prost")]
/// Convert prost Value to CBOR bytes.
///
/// # Errors
/// Returns error if CBOR serialization fails.
pub fn prost_to_cbor(prost: &prost_types::Value) -> Result<Bytes, Error> {
    let mut o = CborBytesMut::default();
    serde_transcode::Transcoder::new(prost::ProstValue::new(prost))
        .serialize(&mut minicbor_serde::Serializer::new(&mut o))?;
    Ok(o.freeze())
}

#[cfg(feature = "prost")]
/// Convert CBOR bytes to prost Value.
///
/// # Errors
/// Returns error if CBOR parsing fails.
pub fn cbor_to_prost(cbor: &[u8]) -> Result<prost_types::Value, Error> {
    serde_transcode::Transcoder::new(&mut minicbor_serde::Deserializer::new(cbor))
        .serialize(prost::ProstValue::serializer())
        .map_err(Error::ProstSerialization)
}

/// Serialize any serializable value to CBOR bytes.
///
/// # Errors
/// Returns error if serialization fails.
pub fn to_cbor<T: Serialize>(value: &T) -> Result<Bytes, Error> {
    let mut serializer = minicbor_serde::Serializer::new(CborBytesMut::default());
    value.serialize(serializer.serialize_unit_as_null(true))?;
    Ok(serializer.into_encoder().into_writer().freeze())
}

/// Deserialize CBOR bytes to any deserializable type.
///
/// # Errors
/// Returns error if deserialization fails.
pub fn from_cbor<T: serde::de::DeserializeOwned>(cbor: &[u8]) -> Result<T, Error> {
    let mut deserializer = minicbor_serde::Deserializer::new(cbor);
    Ok(T::deserialize(&mut deserializer)?)
}

struct CborBytesMut(BytesMut);

impl CborBytesMut {
    fn freeze(self) -> Bytes {
        self.0.freeze()
    }
}

impl Default for CborBytesMut {
    fn default() -> Self {
        Self(BytesMut::new())
    }
}

impl minicbor::encode::Write for CborBytesMut {
    type Error = Infallible;

    fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        self.0.extend_from_slice(buf);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use base64::Engine;
    #[cfg(feature = "prost")]
    use prost_types::Value;

    fn json_roundtrip(json: &str) {
        let cbor = json_to_cbor(json).unwrap();
        let json2 = cbor_to_json(&cbor).unwrap();
        let v1: serde_json::Value = serde_json::from_str(json).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&json2).unwrap();
        assert_eq!(v1, v2);
    }

    #[cfg(feature = "prost")]
    fn prost_roundtrip(prost: &Value) {
        let cbor = prost_to_cbor(prost).unwrap();
        let prost2 = cbor_to_prost(&cbor).unwrap();
        assert_eq!(*prost, prost2);
    }

    #[cfg(feature = "prost")]
    fn make_prost_string(s: &str) -> Value {
        Value {
            kind: Some(prost_types::value::Kind::StringValue(s.to_string())),
        }
    }

    #[cfg(feature = "prost")]
    fn make_prost_number(n: f64) -> Value {
        Value {
            kind: Some(prost_types::value::Kind::NumberValue(n)),
        }
    }

    #[cfg(feature = "prost")]
    fn make_prost_bool(b: bool) -> Value {
        Value {
            kind: Some(prost_types::value::Kind::BoolValue(b)),
        }
    }

    #[test]
    fn test_json_roundtrips() {
        let test_cases = [
            r#"{"key": "value", "num": 42}"#,
            "{}",
            "null",
            r#"{"nullfield": null}"#,
            r#"{"true_val": true, "false_val": false}"#,
            r#"{"array": [1, 2, 3], "nested": [[1, 2], [3, 4]]}"#,
            r#"{"outer": {"inner": {"deep": "value"}}, "another": {"data": 42}}"#,
            r#"{"unicode": "🚀", "special": "quotes\"and\\backslash", "newline": "line1\nline2"}"#,
            r#"{"large_int": 9223372036854775807, "large_float": 1.7976931348623157e+308, "small_float": 2.2250738585072014e-308}"#,
        ];

        for json in test_cases {
            json_roundtrip(json);
        }
    }

    #[test]
    #[cfg(feature = "prost")]
    fn test_prost_roundtrips() {
        let test_cases = [
            Value {
                kind: Some(prost_types::value::Kind::NullValue(
                    prost_types::NullValue::NullValue as i32,
                )),
            },
            make_prost_bool(true),
            make_prost_bool(false),
            Value {
                kind: Some(prost_types::value::Kind::ListValue(
                    prost_types::ListValue {
                        values: vec![
                            make_prost_string("item1"),
                            make_prost_number(123.0),
                            make_prost_bool(true),
                        ],
                    },
                )),
            },
            Value {
                kind: Some(prost_types::value::Kind::StructValue(prost_types::Struct {
                    fields: std::collections::BTreeMap::new(),
                })),
            },
            Value {
                kind: Some(prost_types::value::Kind::ListValue(
                    prost_types::ListValue { values: vec![] },
                )),
            },
        ];

        for prost in &test_cases {
            prost_roundtrip(prost);
        }
    }

    #[test]
    fn test_invalid_inputs() {
        assert!(cbor_to_json(b"notcbor").is_err());
        assert!(json_to_cbor("{not json}").is_err());

        #[cfg(feature = "prost")]
        {
            assert!(cbor_to_prost(b"notcbor").is_err());
            assert!(prost_to_cbor(&Value { kind: None }).is_ok());
        }
    }

    #[test]
    fn test_base64_bytes_encoding() {
        use serde::Serializer;

        let test_bytes = b"Hello, World!";
        let mut cbor_serializer = minicbor_serde::Serializer::new(vec![]);
        cbor_serializer.serialize_bytes(test_bytes).unwrap();
        let cbor_data = cbor_serializer.into_encoder().into_writer();

        let json_result = cbor_to_json(&cbor_data).unwrap();

        let expected_base64 = base64::prelude::BASE64_STANDARD.encode(test_bytes);
        assert_eq!(json_result, format!("\"{expected_base64}\""));
        assert_eq!(json_result, "\"SGVsbG8sIFdvcmxkIQ==\"");
    }

    #[test]
    #[cfg(feature = "prost")]
    fn test_prost_base64_bytes_encoding() {
        use serde::Serializer;

        let test_bytes = b"Hello, World!";
        let mut cbor_serializer = minicbor_serde::Serializer::new(vec![]);
        cbor_serializer.serialize_bytes(test_bytes).unwrap();
        let cbor_data = cbor_serializer.into_encoder().into_writer();

        let prost_value = cbor_to_prost(&cbor_data).unwrap();
        let json_result = serde_json::to_string(&prost::ProstValue::new(&prost_value)).unwrap();

        let expected_base64 = base64::prelude::BASE64_STANDARD.encode(test_bytes);
        assert_eq!(json_result, format!("\"{expected_base64}\""));
        assert_eq!(json_result, "\"SGVsbG8sIFdvcmxkIQ==\"");
    }

    #[test]
    #[cfg(feature = "prost")]
    fn test_cross_format_consistency() {
        let json =
            r#"{"mixed": {"string": "value", "number": 42, "bool": true, "array": [1, 2, 3]}}"#;

        let cbor = json_to_cbor(json).unwrap();
        let json2 = cbor_to_json(&cbor).unwrap();

        let prost = cbor_to_prost(&cbor).unwrap();
        let cbor2 = prost_to_cbor(&prost).unwrap();
        let json3 = cbor_to_json(&cbor2).unwrap();

        let v1: serde_json::Value = serde_json::from_str(json).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&json2).unwrap();
        let v3: serde_json::Value = serde_json::from_str(&json3).unwrap();

        assert_eq!(v1, v2);
        assert_eq!(v2, v3);
    }

    #[test]
    #[cfg(feature = "prost")]
    fn test_prost_integer_conversion_precision() {
        let test_cases = [
            (42.0, "42"),
            (2000.0, "2000"),
            (-42.0, "-42"),
            (42.1, "42.1"),
            (1e30, "1e+30"),
            (-1e30, "-1e+30"),
        ];

        for (input, expected) in test_cases {
            let prost_value = make_prost_number(input);
            let json = cbor_to_json(&prost_to_cbor(&prost_value).unwrap()).unwrap();
            assert_eq!(json, expected, "Failed for input: {input}");
        }
    }
}
