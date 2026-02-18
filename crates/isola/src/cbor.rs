use std::convert::Infallible;
use std::io::{self, Write};

use bytes::{Bytes, BytesMut};
use serde::Serialize;
use thiserror::Error;

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

    fn json_roundtrip(json: &str) {
        let cbor = json_to_cbor(json).unwrap();
        let json2 = cbor_to_json(&cbor).unwrap();
        let v1: serde_json::Value = serde_json::from_str(json).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&json2).unwrap();
        assert_eq!(v1, v2);
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
    fn test_invalid_inputs() {
        assert!(cbor_to_json(b"notcbor").is_err());
        assert!(json_to_cbor("{not json}").is_err());
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
}
