use std::borrow::Cow;

use cbor4ii::core::utils::{BufWriter, SliceReader};
use promptkit_executor::ExecSource;
use serde::{
    de::Visitor,
    ser::{SerializeMap, SerializeSeq},
    Deserialize, Serialize,
};
use tonic::Status;

use crate::proto::script::v1::{
    self as script, argument::Marker, result, source::SourceType, ContentType, Source,
};

pub fn argument(s: script::Argument) -> Result<Result<Vec<u8>, Marker>, Status> {
    match s.argument_type {
        None => Err(Status::invalid_argument("argument type is not specified")),
        Some(arg) => match arg {
            script::argument::ArgumentType::Value(s) => Ok(Ok(cbor4ii::serde::to_vec(
                vec![],
                &ProstValueSerializer { value: &s },
            )
            .map_err(|_| Status::internal("failed to serialize argument to json"))?)),
            script::argument::ArgumentType::Json(j) => {
                let mut o = BufWriter::new(vec![]);
                let mut s = cbor4ii::serde::Serializer::new(&mut o);
                serde_transcode::Transcoder::new(&mut serde_json::Deserializer::from_str(&j))
                    .serialize(&mut s)
                    .unwrap();
                Ok(Ok(o.into_inner()))
            }
            script::argument::ArgumentType::Cbor(c) => Ok(Ok(c)),
            script::argument::ArgumentType::Marker(m) => Ok(Err(Marker::try_from(m)
                .map_err(|e| Status::invalid_argument(format!("invalid marker: {e}")))?)),
        },
    }
}

pub fn parse_source(source: &Option<Source>) -> Result<ExecSource<'_>, Status> {
    match source {
        Some(Source {
            source_type: Some(SourceType::ScriptInline(i)),
        }) => Ok(ExecSource::Script(&i.prelude, &i.script)),
        Some(Source {
            source_type: Some(SourceType::BundleInline(i)),
        }) => Ok(ExecSource::Bundle(i)),
        Some(Source { source_type: None }) | None => {
            Err(Status::invalid_argument("source type is not specified"))
        }
    }
}

pub fn result_type(
    s: Cow<'_, [u8]>,
    content_type: impl IntoIterator<Item = i32>,
) -> Result<script::Result, Status> {
    for c in content_type
        .into_iter()
        .filter_map(|e| ContentType::try_from(e).ok())
        .chain(Some(ContentType::ProtobufValue))
    {
        match c {
            ContentType::Unspecified => {}
            ContentType::Json => {
                let mut s = SliceReader::new(s.as_ref());
                let mut o = vec![];
                serde_transcode::Transcoder::new(&mut cbor4ii::serde::Deserializer::new(&mut s))
                    .serialize(&mut serde_json::Serializer::new(&mut o))
                    .map_err(|_| Status::internal("failed to serialize result to json"))?;
                return Ok(script::Result {
                    result_type: Some(result::ResultType::Json(String::from_utf8(o).unwrap())),
                });
            }
            ContentType::ProtobufValue => {
                return match ProstValueDeserializer::deserialize(
                    &mut cbor4ii::serde::Deserializer::new(&mut SliceReader::new(s.as_ref())),
                ) {
                    Ok(ProstValueDeserializer(v)) => Ok(script::Result {
                        result_type: Some(result::ResultType::Value(v)),
                    }),
                    Err(_) => Err(Status::invalid_argument(
                        "failed to serialize result to struct",
                    )),
                };
            }
            ContentType::Cbor => {
                return Ok(script::Result {
                    result_type: Some(result::ResultType::Cbor(s.into())),
                });
            }
        }
    }
    unreachable!()
}

struct ProstValueDeserializer(prost_types::Value);

impl<'de> Deserialize<'de> for ProstValueDeserializer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(Self(prost_types::Value { kind: None }))
    }
}

impl<'de> Visitor<'de> for ProstValueDeserializer {
    type Value = ProstValueDeserializer;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a type that can deserialize in pb::Struct")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ProstValueDeserializer(prost_types::Value {
            kind: Some(prost_types::value::Kind::BoolValue(v)),
        }))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ProstValueDeserializer(prost_types::Value {
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            kind: Some(prost_types::value::Kind::NumberValue(v as f64)),
        }))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ProstValueDeserializer(prost_types::Value {
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            kind: Some(prost_types::value::Kind::NumberValue(v as f64)),
        }))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ProstValueDeserializer(prost_types::Value {
            kind: Some(prost_types::value::Kind::NumberValue(v)),
        }))
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ProstValueDeserializer(prost_types::Value {
            kind: Some(prost_types::value::Kind::StringValue(v.to_owned())),
        }))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ProstValueDeserializer(prost_types::Value {
            kind: Some(prost_types::value::Kind::StringValue(v)),
        }))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ProstValueDeserializer(prost_types::Value {
            kind: Some(prost_types::value::Kind::NullValue(
                prost_types::NullValue::NullValue.into(),
            )),
        }))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        serde::Deserialize::deserialize(deserializer)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ProstValueDeserializer(prost_types::Value { kind: None }))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut elems = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(elem) = seq.next_element::<Self::Value>()? {
            elems.push(elem.0);
        }

        Ok(ProstValueDeserializer(prost_types::Value {
            kind: Some(prost_types::value::Kind::ListValue(
                prost_types::ListValue { values: elems },
            )),
        }))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut fields = prost_types::Struct {
            ..Default::default()
        };
        while let Some((key, value)) = map.next_entry::<_, Self::Value>()? {
            fields.fields.insert(key, value.0);
        }
        Ok(ProstValueDeserializer(prost_types::Value {
            kind: Some(prost_types::value::Kind::StructValue(fields)),
        }))
    }
}

struct ProstValueSerializer<'s> {
    value: &'s prost_types::Value,
}

impl<'s> serde::Serialize for ProstValueSerializer<'s> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self.value.kind {
            Some(kind) => match kind {
                prost_types::value::Kind::NullValue(_) => serializer.serialize_none(),
                prost_types::value::Kind::NumberValue(n) => {
                    if n.fract() == 0.0 {
                        #[allow(clippy::cast_possible_truncation)]
                        serializer.serialize_i64(*n as i64)
                    } else {
                        serializer.serialize_f64(*n)
                    }
                }
                prost_types::value::Kind::StringValue(s) => serializer.serialize_str(s),
                prost_types::value::Kind::BoolValue(b) => serializer.serialize_bool(*b),
                prost_types::value::Kind::StructValue(s) => {
                    let mut map = serializer.serialize_map(Some(s.fields.len()))?;
                    for (key, value) in &s.fields {
                        map.serialize_entry(&key, &Self { value })?;
                    }
                    map.end()
                }
                prost_types::value::Kind::ListValue(l) => {
                    let mut seq = serializer.serialize_seq(Some(l.values.len()))?;
                    for value in &l.values {
                        seq.serialize_element(&Self { value })?;
                    }
                    seq.end()
                }
            },
            None => serializer.serialize_unit(),
        }
    }
}
