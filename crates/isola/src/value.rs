#[cfg(feature = "serde")]
use std::io::{self, Write};
#[cfg(feature = "serde")]
use std::{cell::Cell, convert::Infallible};

use bytes::Bytes;
#[cfg(feature = "serde")]
use bytes::BytesMut;
#[cfg(feature = "serde")]
use serde::{Serialize, de::DeserializeOwned, ser::SerializeMap, ser::SerializeSeq};

/// Opaque value exchanged with a guest as CBOR bytes.
///
/// Raw construction with [`Value::from_cbor`] does not validate the bytes.
/// Use `Value::from_serde` or `Value::from_json_str` when the `serde` feature
/// is enabled to construct validated values from Rust data.
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Value(Bytes);

impl Value {
    /// Wrap CBOR bytes without parsing or validating them.
    ///
    /// Malformed data is accepted here and will fail when a consumer attempts
    /// to decode it.
    #[must_use]
    pub fn from_cbor(value: impl Into<Bytes>) -> Self {
        Self(value.into())
    }

    /// Borrow the encoded CBOR bytes.
    #[must_use]
    pub fn as_cbor(&self) -> &[u8] {
        self.0.as_ref()
    }

    /// Consume this value and return its encoded CBOR bytes.
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
    /// This uses the same CBOR-to-JSON mapping as [`Value::to_json_str`].
    ///
    /// # Errors
    /// Returns an error if CBOR parsing or JSON parsing fails.
    pub fn to_json_value(&self) -> Result<serde_json::Value, Error> {
        serde_json::from_slice(&self.to_json_bytes()?).map_err(Error::from)
    }

    /// Alias for [`Value::from_json_str`].
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

    /// Alias for [`Value::to_json_str`].
    ///
    /// # Errors
    /// Returns an error if CBOR parsing or JSON serialization fails.
    pub fn to_json(&self) -> Result<String, Error> {
        self.to_json_str()
    }

    /// Convert a runtime `Value` into a JSON string.
    ///
    /// CBOR byte strings and recognized CBOR typed arrays become base64-encoded
    /// JSON strings. CBOR `undefined` becomes JSON `null`. Unsupported CBOR
    /// tags, malformed or trailing data, and values nested more than 128 levels
    /// return an error.
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
        TaggedCbor::new(self.as_ref())
            .serialize(&mut serde_json::Serializer::with_formatter(
                &mut o,
                Base64Formatter,
            ))
            .map_err(|error| Error::Transcode(error.to_string()))?;
        Ok(o)
    }
}

#[cfg(feature = "serde")]
const MAX_JSON_TRANSCODE_DEPTH: usize = 128;

#[cfg(feature = "serde")]
struct TaggedCbor<'a> {
    input: &'a [u8],
    position: Cell<usize>,
}

#[cfg(feature = "serde")]
impl<'a> TaggedCbor<'a> {
    const fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            position: Cell::new(0),
        }
    }

    fn datatype(&self) -> Result<minicbor::data::Type, minicbor::decode::Error> {
        minicbor::Decoder::new(&self.input[self.position.get()..]).datatype()
    }

    const fn is_finished(&self) -> bool {
        self.position.get() == self.input.len()
    }

    fn decode<T>(
        &self,
        f: impl FnOnce(&mut minicbor::Decoder<'a>) -> Result<T, minicbor::decode::Error>,
    ) -> Result<T, minicbor::decode::Error> {
        let mut decoder = minicbor::Decoder::new(&self.input[self.position.get()..]);
        let value = f(&mut decoder)?;
        self.position.set(self.position.get() + decoder.position());
        Ok(value)
    }
}

#[cfg(feature = "serde")]
impl Serialize for TaggedCbor<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;

        let result = serialize_cbor_value(self, serializer, 0)?;
        if !self.is_finished() {
            return Err(S::Error::custom("trailing data after root CBOR value"));
        }
        Ok(result)
    }
}

#[cfg(feature = "serde")]
const fn typed_array_element_width(tag: u64) -> Option<usize> {
    match tag {
        64 | 73 => Some(1),
        65 | 69 | 74 | 77 => Some(2),
        66 | 70 | 75 | 78 | 81 | 84 => Some(4),
        67 | 71 | 76 | 79 | 82 | 85 => Some(8),
        _ => None,
    }
}

#[cfg(feature = "serde")]
fn serialize_cbor_value<S: serde::Serializer>(
    decoder: &TaggedCbor<'_>,
    serializer: S,
    depth: usize,
) -> Result<S::Ok, S::Error> {
    use minicbor::data::Type;
    use serde::ser::Error as _;

    if depth > MAX_JSON_TRANSCODE_DEPTH {
        return Err(S::Error::custom(format!(
            "maximum CBOR nesting depth of {MAX_JSON_TRANSCODE_DEPTH} exceeded"
        )));
    }

    let datatype = decoder.datatype().map_err(S::Error::custom)?;
    match datatype {
        Type::Bool => serializer.serialize_bool(
            decoder
                .decode(minicbor::Decoder::bool)
                .map_err(S::Error::custom)?,
        ),
        Type::Null => {
            decoder
                .decode(minicbor::Decoder::null)
                .map_err(S::Error::custom)?;
            serializer.serialize_none()
        }
        Type::Undefined => {
            decoder
                .decode(minicbor::Decoder::undefined)
                .map_err(S::Error::custom)?;
            serializer.serialize_none()
        }
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => serializer.serialize_u64(
            decoder
                .decode(minicbor::Decoder::u64)
                .map_err(S::Error::custom)?,
        ),
        Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Int => serializer.serialize_i64(
            decoder
                .decode(minicbor::Decoder::i64)
                .map_err(S::Error::custom)?,
        ),
        Type::F16 | Type::F32 => serializer.serialize_f32(
            decoder
                .decode(minicbor::Decoder::f32)
                .map_err(S::Error::custom)?,
        ),
        Type::F64 => serializer.serialize_f64(
            decoder
                .decode(minicbor::Decoder::f64)
                .map_err(S::Error::custom)?,
        ),
        Type::Bytes => serializer.serialize_bytes(
            decoder
                .decode(minicbor::Decoder::bytes)
                .map_err(S::Error::custom)?,
        ),
        Type::BytesIndef => {
            let bytes = decode_indefinite_bytes(decoder).map_err(S::Error::custom)?;
            serializer.serialize_bytes(&bytes)
        }
        Type::String => serializer.serialize_str(
            decoder
                .decode(minicbor::Decoder::str)
                .map_err(S::Error::custom)?,
        ),
        Type::StringIndef => {
            let text = decoder
                .decode(|decoder| decoder.str_iter()?.collect::<Result<String, _>>())
                .map_err(S::Error::custom)?;
            serializer.serialize_str(&text)
        }
        Type::Array | Type::ArrayIndef => serialize_cbor_array(decoder, serializer, depth),
        Type::Map | Type::MapIndef => serialize_cbor_map(decoder, serializer, depth),
        Type::Tag => {
            let tag = decoder
                .decode(minicbor::Decoder::tag)
                .map_err(S::Error::custom)?
                .as_u64();
            let Some(element_width) = typed_array_element_width(tag) else {
                return Err(S::Error::custom(format!("unsupported CBOR tag {tag}")));
            };
            serialize_typed_array(decoder, serializer, tag, element_width)
        }
        Type::Simple | Type::Break | Type::Unknown(_) => Err(S::Error::custom(format!(
            "unsupported CBOR type {datatype}"
        ))),
    }
}

#[cfg(feature = "serde")]
fn decode_indefinite_bytes(decoder: &TaggedCbor<'_>) -> Result<Vec<u8>, minicbor::decode::Error> {
    decoder.decode(|decoder| {
        let mut output = Vec::new();
        for chunk in decoder.bytes_iter()? {
            output.extend_from_slice(chunk?);
        }
        Ok(output)
    })
}

#[cfg(feature = "serde")]
fn serialize_typed_array<S: serde::Serializer>(
    decoder: &TaggedCbor<'_>,
    serializer: S,
    tag: u64,
    element_width: usize,
) -> Result<S::Ok, S::Error> {
    use minicbor::data::Type;
    use serde::ser::Error as _;

    match decoder.datatype().map_err(S::Error::custom)? {
        Type::Bytes => {
            let bytes = decoder
                .decode(minicbor::Decoder::bytes)
                .map_err(S::Error::custom)?;
            validate_typed_array_length(tag, element_width, bytes.len())
                .map_err(S::Error::custom)?;
            serializer.serialize_bytes(bytes)
        }
        Type::BytesIndef => {
            let bytes = decode_indefinite_bytes(decoder).map_err(S::Error::custom)?;
            validate_typed_array_length(tag, element_width, bytes.len())
                .map_err(S::Error::custom)?;
            serializer.serialize_bytes(&bytes)
        }
        _ => Err(S::Error::custom(format!(
            "CBOR typed-array tag {tag} must contain bytes"
        ))),
    }
}

#[cfg(feature = "serde")]
fn validate_typed_array_length(
    tag: u64,
    element_width: usize,
    byte_len: usize,
) -> Result<(), String> {
    if byte_len.is_multiple_of(element_width) {
        Ok(())
    } else {
        Err(format!(
            "CBOR typed-array tag {tag} has {byte_len} bytes, not a multiple of {element_width}"
        ))
    }
}

#[cfg(feature = "serde")]
fn serialize_cbor_array<S: serde::Serializer>(
    decoder: &TaggedCbor<'_>,
    serializer: S,
    depth: usize,
) -> Result<S::Ok, S::Error> {
    use serde::ser::Error as _;

    let len = decoder
        .decode(minicbor::Decoder::array)
        .map_err(S::Error::custom)?;
    let mut seq = serializer.serialize_seq(len.and_then(|n| usize::try_from(n).ok()))?;
    let mut remaining = len;
    while remaining.is_none_or(|n| n > 0) {
        if remaining.is_none()
            && decoder.datatype().map_err(S::Error::custom)? == minicbor::data::Type::Break
        {
            decoder
                .decode(minicbor::Decoder::skip)
                .map_err(S::Error::custom)?;
            break;
        }
        seq.serialize_element(&TaggedCborValue(decoder, depth + 1))?;
        remaining = remaining.map(|n| n - 1);
    }
    seq.end()
}

#[cfg(feature = "serde")]
fn serialize_cbor_map<S: serde::Serializer>(
    decoder: &TaggedCbor<'_>,
    serializer: S,
    depth: usize,
) -> Result<S::Ok, S::Error> {
    use serde::ser::Error as _;

    let len = decoder
        .decode(minicbor::Decoder::map)
        .map_err(S::Error::custom)?;
    let mut map = serializer.serialize_map(len.and_then(|n| usize::try_from(n).ok()))?;
    let mut remaining = len;
    while remaining.is_none_or(|n| n > 0) {
        if remaining.is_none()
            && decoder.datatype().map_err(S::Error::custom)? == minicbor::data::Type::Break
        {
            decoder
                .decode(minicbor::Decoder::skip)
                .map_err(S::Error::custom)?;
            break;
        }
        map.serialize_entry(
            &TaggedCborValue(decoder, depth + 1),
            &TaggedCborValue(decoder, depth + 1),
        )?;
        remaining = remaining.map(|n| n - 1);
    }
    map.end()
}

#[cfg(feature = "serde")]
struct TaggedCborValue<'a, 'b>(&'a TaggedCbor<'b>, usize);

#[cfg(feature = "serde")]
impl Serialize for TaggedCborValue<'_, '_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_cbor_value(self.0, serializer, self.1)
    }
}

#[cfg(feature = "serde")]
/// Error converting a [`Value`] to or from serde and JSON representations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// JSON input could not be parsed or a JSON value could not be produced.
    #[error("JSON serialization error")]
    Json(#[from] serde_json::Error),
    /// CBOR bytes could not be deserialized into the requested Rust type.
    #[error("CBOR decode error")]
    CborDecode(#[from] minicbor_serde::error::DecodeError),
    /// A Rust value could not be serialized as CBOR.
    #[error("CBOR encode error")]
    CborEncode(#[from] minicbor_serde::error::EncodeError<std::convert::Infallible>),
    /// CBOR could not be represented as JSON.
    #[error("CBOR to JSON transcoding error: {0}")]
    Transcode(String),
    /// Generated JSON bytes were not valid UTF-8.
    #[error("UTF-8 encoding error")]
    Utf8(#[from] std::string::FromUtf8Error),
    /// Writing the converted representation failed.
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

    #[test]
    fn value_nested_typed_arrays_encode_as_base64_json() {
        let mut cbor = Vec::new();
        let mut encoder = minicbor::Encoder::new(&mut cbor);
        encoder
            .map(1)
            .and_then(|e| e.str("arrays"))
            .and_then(|e| e.array(2))
            .and_then(|e| e.tag(minicbor::data::Tag::new(84)))
            .and_then(|e| e.bytes(&[0, 0, 192, 63]))
            .and_then(|e| e.tag(minicbor::data::Tag::new(73)))
            .and_then(|e| e.bytes(&[255]))
            .expect("encode nested typed arrays");

        let value = Value::from_cbor(cbor);
        assert_eq!(
            value.to_json_str().expect("to json"),
            r#"{"arrays":["AADAPw==","/w=="]}"#
        );
    }

    #[test]
    fn value_json_rejects_trailing_cbor_data() {
        let value = Value::from_cbor([0xf5, 0xf4].as_slice());
        assert!(value.to_json_str().is_err());
    }

    #[test]
    fn value_json_rejects_excessive_nesting() {
        let mut cbor = vec![0x81; super::MAX_JSON_TRANSCODE_DEPTH + 1];
        cbor.push(0xf6);
        let error = Value::from_cbor(cbor)
            .to_json_str()
            .expect_err("depth limit");
        assert!(error.to_string().contains("maximum CBOR nesting depth"));
    }

    #[test]
    fn value_json_rejects_misaligned_typed_array() {
        let mut cbor = Vec::new();
        minicbor::Encoder::new(&mut cbor)
            .tag(minicbor::data::Tag::new(84))
            .and_then(|encoder| encoder.bytes(&[1, 2, 3]))
            .expect("encode malformed typed array");

        let error = Value::from_cbor(cbor)
            .to_json_str()
            .expect_err("misaligned typed array");
        assert!(error.to_string().contains("not a multiple of 4"));
    }

    #[test]
    fn value_json_combines_indefinite_typed_array_chunks() {
        let cbor = [0xd8, 84, 0x5f, 0x42, 0, 0, 0x42, 0xc0, 0x3f, 0xff];
        assert_eq!(
            Value::from_cbor(cbor.to_vec())
                .to_json_str()
                .expect("indefinite typed array"),
            r#""AADAPw==""#
        );
    }
}
