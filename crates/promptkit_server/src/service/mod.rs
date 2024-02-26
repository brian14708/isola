use std::{time::UNIX_EPOCH, usize};

use futures_util::Stream;
use promptkit_executor::{
    trace::{BoxedTracer, MemoryTracer},
    ExecStreamItem,
};
use tokio::join;
use tonic::{Response, Status};

use crate::{
    proto::script::{
        self, script_service_server::ScriptService, ExecuteClientStreamRequest,
        ExecuteClientStreamResponse, ExecuteRequest, ExecuteResponse, ExecuteServerStreamRequest,
        ExecuteServerStreamResponse, ExecuteStreamRequest, ExecuteStreamResponse,
    },
    routes::AppState,
};

use self::prost_serde::{argument, parse_source, trace_convert};

mod prost_serde;

pub struct ScriptServer {
    state: AppState,
}

pub struct ExecuteServerStreamStream;
pub struct ExecuteStreamStream;

impl ScriptServer {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl ScriptService for ScriptServer {
    type ExecuteServerStreamStream = ExecuteServerStreamStream;
    type ExecuteStreamStream = ExecuteStreamStream;

    async fn execute(
        &self,
        request: tonic::Request<ExecuteRequest>,
    ) -> Result<tonic::Response<ExecuteResponse>, Status> {
        let (script, method) = parse_source(&request.get_ref().source)?;

        let (tracer, mut trace_events) = request
            .get_ref()
            .spec
            .as_ref()
            .map(|spec| spec.trace_level == script::TraceLevel::All as i32)
            .unwrap_or_default()
            .then(MemoryTracer::new)
            .map(|(a, b)| -> (Option<BoxedTracer>, _) { (Some(a), Some(b)) })
            .unwrap_or_default();
        let args = request
            .get_ref()
            .spec
            .as_ref()
            .map_or(Ok(Vec::new()), |spec| {
                spec.arguments
                    .iter()
                    .map(|a| match argument(a) {
                        Ok(Ok(a)) => Ok(a),
                        Ok(Err(_)) => Err(Status::invalid_argument("invalid marker arguments")),
                        Err(e) => Err(e),
                    })
                    .collect::<Result<Vec<_>, _>>()
            })?;

        let start = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let stream = self
            .state
            .vm
            .exec(script, method, args, tracer)
            .await
            .map_err(|e| Status::internal(format!("failed to execute script: {e}")))?;
        let result = non_stream_result(
            stream,
            request.get_ref().result_content_type.iter().copied(),
        );

        let trace_async = async move {
            let mut metadata = script::ExecutionMetadata::default();
            if let Some(trace_events) = trace_events.as_mut() {
                while let Some(event) = trace_events.recv().await {
                    metadata.traces.push(trace_convert(event, &start));
                }
            }
            metadata
        };

        let (result, metadata) = join!(result, trace_async);
        Ok(Response::new(script::ExecuteResponse {
            metadata: Some(metadata),
            result: Some(result?),
        }))
    }

    async fn execute_client_stream(
        &self,
        _request: tonic::Request<tonic::Streaming<ExecuteClientStreamRequest>>,
    ) -> Result<tonic::Response<ExecuteClientStreamResponse>, Status> {
        todo!()
    }

    async fn execute_server_stream(
        &self,
        _request: tonic::Request<ExecuteServerStreamRequest>,
    ) -> Result<tonic::Response<Self::ExecuteServerStreamStream>, Status> {
        todo!()
    }

    async fn execute_stream(
        &self,
        _request: tonic::Request<tonic::Streaming<ExecuteStreamRequest>>,
    ) -> Result<tonic::Response<Self::ExecuteStreamStream>, Status> {
        todo!()
    }
}

impl Stream for ExecuteServerStreamStream {
    type Item = Result<ExecuteServerStreamResponse, Status>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context,
    ) -> std::task::Poll<Option<Self::Item>> {
        todo!()
    }
}

impl Stream for ExecuteStreamStream {
    type Item = Result<ExecuteStreamResponse, Status>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context,
    ) -> std::task::Poll<Option<Self::Item>> {
        todo!()
    }
}

async fn non_stream_result(
    stream: impl Stream<Item = ExecStreamItem>,
    content_type: impl IntoIterator<Item = i32>,
) -> Result<script::Result, Status> {
    let stream = tokio_stream::StreamExt::collect::<Vec<_>>(stream).await;

    if stream.len() == 1 {
        return match stream.into_iter().next().unwrap() {
            ExecStreamItem::End(Some(value)) => {
                Ok(prost_serde::result_type(value.into(), content_type)?)
            }
            ExecStreamItem::End(None) => Ok(prost_serde::result_type("[]".into(), content_type)?),
            ExecStreamItem::Data(_) => return Err(Status::internal("unexpected data")),
            ExecStreamItem::Error(err) => Err(Status::internal(err.to_string())),
        };
    }

    let mut str = String::with_capacity(
        stream
            .iter()
            .map(|item| match item {
                ExecStreamItem::Data(data) => data.len() + 1,
                _ => 0,
            })
            .sum::<usize>()
            + 1,
    );
    for (i, item) in stream.into_iter().enumerate() {
        match item {
            ExecStreamItem::Data(data) => {
                str.push(if i > 0 { ',' } else { '[' });
                str.push_str(&data);
            }
            ExecStreamItem::End(None) => break,
            ExecStreamItem::End(Some(_)) => {
                return Err(Status::internal("unexpected end with data"))
            }
            ExecStreamItem::Error(err) => {
                return Err(Status::internal(err.to_string()));
            }
        }
    }
    str.push(']');

    prost_serde::result_type(str.into(), content_type)
}
