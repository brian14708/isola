#[cfg(feature = "serde")]
use std::convert::Infallible;
#[cfg(feature = "serde")]
use std::io::{self, Write};

use bytes::Bytes;
#[cfg(feature = "serde")]
use bytes::BytesMut;
#[cfg(feature = "serde")]
use serde::{Serialize, de::DeserializeOwned};

/// Opaque Isola runtime value represented as CBOR bytes.
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Value(Bytes);

impl Value {
    #[must_use]
    pub fn from_cbor(value: impl Into<Bytes>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_cbor(&self) -> &[u8] {
        self.0.as_ref()
    }

    #[must_use]
    pub fn into_cbor(self) -> Bytes {
        self.0
    }
}

#[cfg(feature = "serde")]
impl Value {
    /// Convert a JSON value into a runtime `Value`.
    ///
    /// # Errors
    /// Returns an error if CBOR serialization fails.
    pub fn from_json_value(value: &serde_json::Value) -> Result<Self, Error> {
        Self::from_serde(value)
    }

    /// Convert a runtime `Value` into a JSON value.
    ///
    /// # Errors
    /// Returns an error if CBOR parsing or JSON parsing fails.
    pub fn to_json_value(&self) -> Result<serde_json::Value, Error> {
        serde_json::from_slice(&self.to_json_bytes()?).map_err(Error::from)
    }

    /// Convert a JSON string into a runtime `Value`.
    ///
    /// # Errors
    /// Returns an error if JSON parsing or CBOR serialization fails.
    pub fn from_json(json: &str) -> Result<Self, Error> {
        Self::from_json_str(json)
    }

    /// Convert a JSON string into a runtime `Value`.
    ///
    /// # Errors
    /// Returns an error if JSON parsing or CBOR serialization fails.
    pub fn from_json_str(json: &str) -> Result<Self, Error> {
        let mut serializer = minicbor_serde::Serializer::new(CborBytesMut::default());
        serde_transcode::Transcoder::new(&mut serde_json::Deserializer::from_str(json))
            .serialize(serializer.serialize_unit_as_null(true))?;
        Ok(Self(serializer.into_encoder().into_writer().freeze()))
    }

    /// Convert a runtime `Value` into a JSON string.
    ///
    /// # Errors
    /// Returns an error if CBOR parsing or JSON serialization fails.
    pub fn to_json(&self) -> Result<String, Error> {
        self.to_json_str()
    }

    /// Convert a runtime `Value` into a JSON string.
    ///
    /// # Errors
    /// Returns an error if CBOR parsing or JSON serialization fails.
    pub fn to_json_str(&self) -> Result<String, Error> {
        Ok(String::from_utf8(self.to_json_bytes()?)?)
    }

    /// Serialize a serde value into a runtime `Value`.
    ///
    /// # Errors
    /// Returns an error if serialization fails.
    pub fn from_serde<T: Serialize>(value: &T) -> Result<Self, Error> {
        let mut serializer = minicbor_serde::Serializer::new(CborBytesMut::default());
        value.serialize(serializer.serialize_unit_as_null(true))?;
        Ok(Self(serializer.into_encoder().into_writer().freeze()))
    }

    /// Deserialize a runtime `Value` into a serde value.
    ///
    /// # Errors
    /// Returns an error if deserialization fails.
    pub fn to_serde<T: DeserializeOwned>(&self) -> Result<T, Error> {
        let mut deserializer = minicbor_serde::Deserializer::new(self.as_ref());
        Ok(T::deserialize(&mut deserializer)?)
    }

    fn to_json_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut o = vec![];
        serde_transcode::Transcoder::new(&mut minicbor_serde::Deserializer::new(self.as_ref()))
            .serialize(&mut serde_json::Serializer::with_formatter(
                &mut o,
                Base64Formatter,
            ))?;
        Ok(o)
    }
}

#[cfg(feature = "serde")]
#[derive(Debug, thiserror::Error)]
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

#[cfg(feature = "serde")]
struct Base64Formatter;

#[cfg(feature = "serde")]
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

#[cfg(feature = "serde")]
struct CborBytesMut(BytesMut);

#[cfg(feature = "serde")]
impl CborBytesMut {
    fn freeze(self) -> Bytes {
        self.0.freeze()
    }
}

#[cfg(feature = "serde")]
impl Default for CborBytesMut {
    fn default() -> Self {
        Self(BytesMut::new())
    }
}

#[cfg(feature = "serde")]
impl minicbor::encode::Write for CborBytesMut {
    type Error = Infallible;

    fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        self.0.extend_from_slice(buf);
        Ok(())
    }
}

impl From<Bytes> for Value {
    fn from(value: Bytes) -> Self {
        Self(value)
    }
}

impl From<Value> for Bytes {
    fn from(value: Value) -> Self {
        value.0
    }
}

impl AsRef<[u8]> for Value {
    fn as_ref(&self) -> &[u8] {
        self.as_cbor()
    }
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use base64::Engine;

    use super::Value;

    #[test]
    fn value_json_roundtrip_methods() {
        let value = Value::from_json(r#"{"a":1,"b":[true,false]}"#).expect("from json");
        let json = value.to_json().expect("to json");
        let got: serde_json::Value = serde_json::from_str(&json).expect("parse");
        let want: serde_json::Value =
            serde_json::from_str(r#"{"a":1,"b":[true,false]}"#).expect("parse");
        assert_eq!(got, want);
    }

    #[test]
    fn value_serde_roundtrip_methods() {
        let input = ("hello".to_string(), 42_i64);
        let value = Value::from_serde(&input).expect("from serde");
        let output: (String, i64) = value.to_serde().expect("to serde");
        assert_eq!(output, input);
    }

    #[test]
    fn value_invalid_inputs() {
        assert!(Value::from_cbor(b"notcbor".to_vec()).to_json().is_err());
        assert!(Value::from_json("{not json}").is_err());
    }

    #[test]
    fn value_json_value_roundtrip_methods() {
        let input: serde_json::Value =
            serde_json::from_str(r#"{"a":1,"b":[true,false]}"#).expect("parse");
        let value = Value::from_json_value(&input).expect("from json value");
        let output = value.to_json_value().expect("to json value");
        assert_eq!(output, input);
    }

    #[test]
    fn value_base64_bytes_encoding() {
        use serde::Serializer;

        let test_bytes = b"Hello, World!";
        let mut cbor_serializer = minicbor_serde::Serializer::new(vec![]);
        cbor_serializer
            .serialize_bytes(test_bytes)
            .expect("serialize");
        let cbor_data = cbor_serializer.into_encoder().into_writer();

        let json_result = Value::from_cbor(cbor_data).to_json().expect("to json");

        let expected_base64 = base64::prelude::BASE64_STANDARD.encode(test_bytes);
        assert_eq!(json_result, format!("\"{expected_base64}\""));
        assert_eq!(json_result, "\"SGVsbG8sIFdvcmxkIQ==\"");
    }

    #[test]
    fn value_json_value_base64_bytes_encoding() {
        use serde::Serializer;

        let test_bytes = b"Hello, World!";
        let mut cbor_serializer = minicbor_serde::Serializer::new(vec![]);
        cbor_serializer
            .serialize_bytes(test_bytes)
            .expect("serialize");
        let cbor_data = cbor_serializer.into_encoder().into_writer();

        let json_value = Value::from_cbor(cbor_data)
            .to_json_value()
            .expect("to json value");

        let expected_base64 = base64::prelude::BASE64_STANDARD.encode(test_bytes);
        assert_eq!(json_value, serde_json::Value::String(expected_base64));
    }
}
