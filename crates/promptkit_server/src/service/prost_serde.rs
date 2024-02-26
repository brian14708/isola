use std::{borrow::Cow, collections::HashMap, ops::Add};

use promptkit_executor::trace::TraceEvent;
use serde::{
    de::Visitor,
    ser::{SerializeMap, SerializeSeq},
    Deserialize,
};
use tonic::Status;

use crate::proto::script::{
    self, argument::Marker, result, source::SourceType, trace, ContentType, Source, Trace,
};

pub fn argument(s: &script::Argument) -> Result<Result<Cow<'_, str>, Marker>, Status> {
    match s.argument_type.as_ref() {
        None => Err(Status::invalid_argument("argument type is not specified")),
        Some(arg) => match arg {
            script::argument::ArgumentType::Value(s) => {
                Ok(Ok(serde_json::to_string(&ProstValueSerializer {
                    value: s,
                })
                .map_err(|_| Status::internal("failed to serialize argument to json"))?
                .into()))
            }
            script::argument::ArgumentType::Json(j) => Ok(Ok(j.into())),
            script::argument::ArgumentType::Marker(m) => Ok(Err(Marker::try_from(*m)
                .map_err(|e| Status::invalid_argument(format!("invalid marker: {e}")))?)),
        },
    }
}

pub fn parse_source(source: &Option<Source>) -> Result<(&str, &str), Status> {
    match source {
        Some(Source {
            source_type: Some(SourceType::Inline(i)),
        }) => Ok((&i.code, &i.method)),
        Some(Source { source_type: None }) | None => {
            Err(Status::invalid_argument("source type is not specified"))
        }
    }
}

pub fn result_type(
    s: Cow<'_, str>,
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
                return Ok(script::Result {
                    result_type: Some(result::ResultType::Json(s.into())),
                })
            }
            ContentType::ProtobufValue => match ProstValueDeserializer::deserialize(
                &mut serde_json::Deserializer::from_str(&s),
            ) {
                Ok(ProstValueDeserializer(v)) => {
                    return Ok(script::Result {
                        result_type: Some(result::ResultType::Value(v)),
                    })
                }
                _ => {
                    return Err(Status::invalid_argument(
                        "failed to serialize result to struct",
                    ))
                }
            },
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

pub fn trace_convert(event: TraceEvent, start: &std::time::Duration) -> Trace {
    let timestamp = start.add(std::time::Duration::from_micros(
        event.timestamp.as_micros(),
    ));
    Trace {
        id: i32::from(event.id),
        group: event.group.into(),
        timestamp: Some(prost_types::Timestamp {
            #[allow(clippy::cast_possible_wrap)]
            seconds: timestamp.as_secs() as i64,
            #[allow(clippy::cast_possible_wrap)]
            nanos: timestamp.subsec_nanos() as i32,
        }),
        trace_type: match event.kind {
            promptkit_executor::trace::TraceEventKind::Log { content } => {
                Some(trace::TraceType::Log(trace::Log { content }))
            }
            promptkit_executor::trace::TraceEventKind::Event {
                parent_id,
                kind,
                data,
            } => Some(trace::TraceType::Event(trace::Event {
                parent_id: i32::from(parent_id.unwrap_or_default()),
                kind: kind.into(),
                data: {
                    let mut map = HashMap::new();
                    if let Some(data) = data {
                        map.insert("attr".into(), data.to_string());
                    }
                    map
                },
            })),
            promptkit_executor::trace::TraceEventKind::SpanBegin {
                parent_id,
                kind,
                data,
            } => Some(trace::TraceType::SpanBegin(trace::SpanBegin {
                parent_id: i32::from(parent_id.unwrap_or_default()),
                kind: kind.into(),
                data: {
                    let mut map = HashMap::new();
                    if let Some(data) = data {
                        map.insert("attr".into(), data.to_string());
                    }
                    map
                },
            })),
            promptkit_executor::trace::TraceEventKind::SpanEnd { parent_id, data } => {
                Some(trace::TraceType::SpanEnd(trace::SpanEnd {
                    parent_id: i32::from(parent_id),
                    data: {
                        let mut map = HashMap::new();
                        if let Some(data) = data {
                            map.insert("attr".into(), data.to_string());
                        }
                        map
                    },
                }))
            }
        },
    }
}
