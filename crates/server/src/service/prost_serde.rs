use bytes::Bytes;
use promptkit_executor::Source as ExecSource;
use tonic::Status;

use crate::proto::script::v1::{
    self as script, ContentType, Source,
    argument::{ArgumentType, Marker},
    result,
    source::SourceType,
};

pub fn argument(s: Option<ArgumentType>) -> Result<Result<Bytes, Marker>, Status> {
    match s {
        None => Err(Status::invalid_argument("argument type is not specified")),
        Some(arg) => match arg {
            script::argument::ArgumentType::Value(s) => Ok(Ok(promptkit_cbor::prost_to_cbor(&s)
                .map_err(|_| {
                Status::internal("failed to serialize argument to cbor")
            })?)),
            script::argument::ArgumentType::Json(j) => Ok(Ok(promptkit_cbor::json_to_cbor(&j)
                .map_err(|_| Status::internal("failed to serialize argument to cbor"))?)),
            script::argument::ArgumentType::Cbor(c) => Ok(Ok(c.into())),
            script::argument::ArgumentType::Marker(m) => Ok(Err(Marker::try_from(m)
                .map_err(|e| Status::invalid_argument(format!("invalid marker: {e}")))?)),
        },
    }
}

pub fn parse_source(source: Option<Source>) -> Result<ExecSource, Status> {
    match source {
        Some(Source {
            source_type: Some(SourceType::ScriptInline(i)),
        }) => Ok(ExecSource::Script {
            prelude: i.prelude,
            code: i.script,
        }),
        Some(Source {
            source_type: Some(SourceType::BundleInline(i)),
        }) => Ok(ExecSource::Bundle(i.into())),
        Some(Source { source_type: None }) | None => {
            Err(Status::invalid_argument("source type is not specified"))
        }
    }
}

pub fn result_type(
    s: Bytes,
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
                let v = promptkit_cbor::cbor_to_json(&s)
                    .map_err(|_| Status::internal("failed to serialize result to json"))?;
                return Ok(script::Result {
                    result_type: Some(result::ResultType::Json(v)),
                });
            }
            ContentType::ProtobufValue => {
                let v = promptkit_cbor::cbor_to_prost(&s)
                    .map_err(|_| Status::internal("failed to serialize result to struct"))?;
                return Ok(script::Result {
                    result_type: Some(result::ResultType::Value(v)),
                });
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
