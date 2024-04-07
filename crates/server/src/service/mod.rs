use std::{
    pin::Pin,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use cbor4ii::core::{enc::Write, types::Array, utils::BufWriter};
use futures_util::{Stream, StreamExt};
use promptkit_executor::{
    trace::{BoxedTracer, MemoryTracer, TraceEvent},
    ExecArgument, ExecStreamItem,
};
use tokio::{sync::mpsc, try_join};
use tokio_stream::{once, wrappers::ReceiverStream};
use tonic::{Response, Status};

use crate::{
    proto::script::{
        self, argument::Marker, execute_client_stream_request, execute_stream_request, result,
        script_service_server::ScriptService, ErrorCode,
    },
    routes::AppState,
    utils::stream::{join_with, stream_until},
};

use self::prost_serde::{argument, parse_source, trace_convert};

mod prost_serde;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

pub struct ScriptServer {
    state: AppState,
}

impl ScriptServer {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl ScriptService for ScriptServer {
    type ExecuteServerStreamStream =
        Pin<Box<dyn Stream<Item = Result<script::ExecuteServerStreamResponse, Status>> + Send>>;
    type ExecuteStreamStream =
        Pin<Box<dyn Stream<Item = Result<script::ExecuteStreamResponse, Status>> + Send>>;

    async fn list_runtime(
        &self,
        _request: tonic::Request<script::ListRuntimeRequest>,
    ) -> Result<tonic::Response<script::ListRuntimeResponse>, Status> {
        Ok(Response::new(script::ListRuntimeResponse {
            runtimes: vec![script::Runtime {
                name: "python3".into(),
            }],
        }))
    }

    async fn execute(
        &self,
        mut request: tonic::Request<script::ExecuteRequest>,
    ) -> Result<tonic::Response<script::ExecuteResponse>, Status> {
        let ParsedSpec {
            method,
            args,
            timeout,
            stream_tx: None,
            tracer,
            mut trace_events,
        } = parse_spec(request.get_mut().spec.as_mut())?
        else {
            return Err(Status::invalid_argument("unexpected stream marker"));
        };
        let (script, old_method) = parse_source(&request.get_ref().source)?;

        let result = async {
            let run = async {
                let stream = self
                    .state
                    .vm
                    .exec(script, old_method.unwrap_or(&method), args, tracer)
                    .await
                    .map_err(|e| {
                        Status::invalid_argument(format!("failed to start script: {e}"))
                    })?;
                non_stream_result(
                    stream,
                    request.get_ref().result_content_type.iter().copied(),
                )
                .await
            };
            match tokio::time::timeout(timeout, run).await {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(s)) => Err(s),
                Err(_) => Ok(timeout_error()),
            }
        };

        let trace_async = async move {
            let mut metadata = script::ExecutionMetadata::default();
            if let Some((start, trace_events)) = trace_events.as_mut() {
                while let Some(event) = trace_events.recv().await {
                    metadata.traces.push(trace_convert(event, start));
                }
            }
            Ok::<_, Status>(metadata)
        };

        let (result, metadata) = try_join!(result, trace_async)?;
        Ok(Response::new(script::ExecuteResponse {
            metadata: Some(metadata),
            result: Some(result),
        }))
    }

    async fn execute_client_stream(
        &self,
        mut request: tonic::Request<tonic::Streaming<script::ExecuteClientStreamRequest>>,
    ) -> Result<tonic::Response<script::ExecuteClientStreamResponse>, Status> {
        let Some(script::ExecuteClientStreamRequest {
            request_type:
                Some(execute_client_stream_request::RequestType::InitialRequest(mut initial)),
        }) = request.get_mut().message().await?
        else {
            return Err(Status::invalid_argument("initial request not found"));
        };

        let (script, old_method) = parse_source(&initial.source)?;
        let ParsedSpec {
            method,
            args,
            timeout,
            stream_tx: mut tx,
            tracer,
            mut trace_events,
        } = parse_spec(initial.spec.as_mut())?;

        let result = async {
            let run = async {
                let stream = self
                    .state
                    .vm
                    .exec(script, old_method.unwrap_or(&method), args, tracer)
                    .await
                    .map_err(|e| {
                        Status::invalid_argument(format!("failed to start script: {e}"))
                    })?;
                non_stream_result(stream, initial.result_content_type.iter().copied()).await
            };
            match tokio::time::timeout(timeout, run).await {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(s)) => Err(s),
                Err(_) => Ok(timeout_error()),
            }
        };

        let trace_async = async move {
            let mut metadata = script::ExecutionMetadata::default();
            if let Some((start, trace_events)) = trace_events.as_mut() {
                while let Some(event) = trace_events.recv().await {
                    metadata.traces.push(trace_convert(event, start));
                }
            }
            Ok::<_, Status>(metadata)
        };
        let mover = async move {
            while let Some(msg) = request.get_mut().message().await? {
                if let Some(tx) = tx.as_mut() {
                    if let Some(execute_client_stream_request::RequestType::StreamValue(v)) =
                        msg.request_type
                    {
                        let _ = tx
                            .send(
                                argument(v)
                                    .map_err(|_e| Status::invalid_argument("invalid arguments"))?
                                    .map_err(|_| Status::invalid_argument("invalid marker"))?,
                            )
                            .await;
                    }
                }
            }
            Ok(())
        };

        let (result, metadata, ()) = try_join!(result, trace_async, mover)?;
        Ok(Response::new(script::ExecuteClientStreamResponse {
            metadata: Some(metadata),
            result: Some(result),
        }))
    }

    async fn execute_server_stream(
        &self,
        mut request: tonic::Request<script::ExecuteServerStreamRequest>,
    ) -> Result<tonic::Response<Self::ExecuteServerStreamStream>, Status> {
        let ParsedSpec {
            method,
            args,
            timeout,
            stream_tx: None,
            tracer,
            trace_events,
        } = parse_spec(request.get_mut().spec.as_mut())?
        else {
            return Err(Status::invalid_argument("unexpected stream marker"));
        };
        let (script, old_method) = parse_source(&request.get_ref().source)?;
        let deadline = std::time::Instant::now() + timeout;
        let stream = match tokio::time::timeout(
            timeout,
            self.state
                .vm
                .exec(script, old_method.unwrap_or(&method), args, tracer),
        )
        .await
        {
            Ok(s) => {
                s.map_err(|e| Status::invalid_argument(format!("failed to start script: {e}")))?
            }
            Err(_) => {
                return Ok(Response::new(Box::pin(once(Ok(
                    script::ExecuteServerStreamResponse {
                        result: Some(timeout_error()),
                        metadata: None,
                    },
                )))))
            }
        };

        let content_type = request.get_ref().result_content_type.clone();
        let m = stream.map(move |s| match s {
            ExecStreamItem::Data(d) | ExecStreamItem::End(Some(d)) => {
                Ok(script::ExecuteServerStreamResponse {
                    result: Some(prost_serde::result_type(
                        d.into(),
                        content_type.iter().copied(),
                    )?),
                    metadata: None,
                })
            }
            ExecStreamItem::End(None) => Ok(script::ExecuteServerStreamResponse::default()),
            ExecStreamItem::Error(err) => Ok(script::ExecuteServerStreamResponse {
                result: Some(error_result(err)),
                metadata: None,
            }),
        });

        let stream = stream_until(
            m,
            deadline,
            Ok(script::ExecuteServerStreamResponse {
                result: Some(timeout_error()),
                metadata: None,
            }),
        );
        if let Some((start, tracer_events)) = trace_events {
            let trace_async = ReceiverStream::new(tracer_events).chunks(4).map(move |e| {
                Ok(script::ExecuteServerStreamResponse {
                    result: None,
                    metadata: Some(script::ExecutionStreamMetadata {
                        traces: e.into_iter().map(|e| trace_convert(e, &start)).collect(),
                    }),
                })
            });
            Ok(Response::new(Box::pin(tokio_stream::StreamExt::merge(
                stream,
                trace_async,
            ))))
        } else {
            Ok(Response::new(Box::pin(stream)))
        }
    }

    async fn execute_stream(
        &self,
        mut request: tonic::Request<tonic::Streaming<script::ExecuteStreamRequest>>,
    ) -> Result<tonic::Response<Self::ExecuteStreamStream>, Status> {
        let Some(script::ExecuteStreamRequest {
            request_type: Some(execute_stream_request::RequestType::InitialRequest(mut initial)),
        }) = request.get_mut().message().await?
        else {
            return Err(Status::invalid_argument("initial request not found"));
        };

        let (script, old_method) = parse_source(&initial.source)?;
        let ParsedSpec {
            method,
            args,
            timeout,
            stream_tx: mut tx,
            tracer,
            trace_events,
        } = parse_spec(initial.spec.as_mut())?;
        let deadline = std::time::Instant::now() + timeout;
        let stream = match tokio::time::timeout(
            timeout,
            self.state
                .vm
                .exec(script, old_method.unwrap_or(&method), args, tracer),
        )
        .await
        {
            Ok(s) => {
                s.map_err(|e| Status::invalid_argument(format!("failed to start script: {e}")))?
            }
            Err(_) => {
                return Ok(Response::new(Box::pin(once(Ok(
                    script::ExecuteStreamResponse {
                        result: Some(timeout_error()),
                        metadata: None,
                    },
                )))))
            }
        };

        let mover = async move {
            while let Some(msg) = request.get_mut().message().await? {
                if let Some(tx) = tx.as_mut() {
                    if let Some(execute_stream_request::RequestType::StreamValue(v)) =
                        msg.request_type
                    {
                        let _ = tx
                            .send(
                                argument(v)
                                    .map_err(|_e| Status::invalid_argument("invalid arguments"))?
                                    .map_err(|_| Status::invalid_argument("invalid marker"))?,
                            )
                            .await;
                    }
                }
            }
            Ok::<_, Status>(())
        };

        let content_type = initial.result_content_type.clone();
        let m = stream.map(move |s| match s {
            ExecStreamItem::Data(d) | ExecStreamItem::End(Some(d)) => {
                Ok(script::ExecuteStreamResponse {
                    result: Some(prost_serde::result_type(
                        d.into(),
                        content_type.iter().copied(),
                    )?),
                    metadata: None,
                })
            }
            ExecStreamItem::End(None) => Ok(script::ExecuteStreamResponse::default()),
            ExecStreamItem::Error(err) => Ok(script::ExecuteStreamResponse {
                result: Some(error_result(err)),
                metadata: None,
            }),
        });
        let stream = stream_until(
            join_with(m, mover),
            deadline,
            Ok(script::ExecuteStreamResponse {
                result: Some(timeout_error()),
                metadata: None,
            }),
        );
        if let Some((start, tracer_events)) = trace_events {
            let trace_async = ReceiverStream::new(tracer_events).chunks(4).map(move |e| {
                Ok(script::ExecuteStreamResponse {
                    result: None,
                    metadata: Some(script::ExecutionStreamMetadata {
                        traces: e.into_iter().map(|e| trace_convert(e, &start)).collect(),
                    }),
                })
            });
            Ok(Response::new(Box::pin(tokio_stream::StreamExt::merge(
                stream,
                trace_async,
            ))))
        } else {
            Ok(Response::new(Box::pin(stream)))
        }
    }
}

async fn non_stream_result(
    mut stream: impl Stream<Item = ExecStreamItem> + Unpin,
    content_type: impl IntoIterator<Item = i32>,
) -> Result<script::Result, Status> {
    let mut b = match stream.next().await {
        Some(ExecStreamItem::End(Some(value))) => {
            return prost_serde::result_type(value.into(), content_type);
        }
        Some(ExecStreamItem::End(None)) => {
            return prost_serde::result_type(
                // empty array
                std::borrow::Cow::Borrowed(b"\x80"),
                content_type,
            );
        }
        Some(ExecStreamItem::Data(d)) => {
            let mut b = BufWriter::new(Vec::with_capacity(d.len() + 2));
            Array::unbounded(&mut b).map_err(|_| Status::internal("failed to encode array"))?;
            b.push(&d)
                .map_err(|_| Status::internal("failed to write data"))?;
            b
        }
        Some(ExecStreamItem::Error(err)) => return Ok(error_result(err)),
        None => return Err(Status::internal("empty stream")),
    };

    while let Some(item) = stream.next().await {
        match item {
            ExecStreamItem::Data(data) => {
                b.push(&data)
                    .map_err(|_| Status::internal("failed to write data"))?;
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
    Array::end(&mut b).map_err(|_| Status::internal("failed to end array"))?;
    prost_serde::result_type(b.into_inner().into(), content_type)
}

struct ParsedSpec {
    method: String,
    args: Vec<ExecArgument>,
    timeout: Duration,
    stream_tx: Option<mpsc::Sender<Vec<u8>>>,
    tracer: Option<BoxedTracer>,
    trace_events: Option<(Duration, mpsc::Receiver<TraceEvent>)>,
}

fn parse_spec(spec: Option<&mut script::ExecutionSpec>) -> Result<ParsedSpec, Status> {
    if let Some(spec) = spec {
        let (tx, rx) = mpsc::channel(4);
        let mut rx = Some(rx);
        let args = std::mem::take(&mut spec.arguments)
            .into_iter()
            .map(|a| match argument(a) {
                Ok(Ok(a)) => Ok(ExecArgument::Cbor(a)),
                Ok(Err(Marker::Stream)) => {
                    if let Some(rx) = rx.take() {
                        Ok(ExecArgument::CborStream(rx))
                    } else {
                        Err(Status::invalid_argument("invalid marker arguments"))
                    }
                }
                Ok(Err(_)) => Err(Status::invalid_argument("invalid marker arguments")),
                Err(e) => Err(e),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (tracer, trace_events) = (spec.trace_level == i32::from(script::TraceLevel::All))
            .then(MemoryTracer::new)
            .map(|(a, b)| -> (Option<BoxedTracer>, _) {
                (
                    Some(a),
                    Some((
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default(),
                        b,
                    )),
                )
            })
            .unwrap_or_default();

        Ok(ParsedSpec {
            method: std::mem::take(&mut spec.method),
            args,
            timeout: spec.timeout.as_ref().map_or(DEFAULT_TIMEOUT, |t| {
                #[allow(clippy::cast_precision_loss)]
                std::time::Duration::from_secs_f64(t.seconds as f64 + f64::from(t.nanos) * 1e-9)
            }),
            stream_tx: if rx.is_none() { Some(tx) } else { None },
            tracer,
            trace_events,
        })
    } else {
        Ok(ParsedSpec {
            method: String::new(),
            args: vec![],
            timeout: DEFAULT_TIMEOUT,
            stream_tx: None,
            tracer: None,
            trace_events: None,
        })
    }
}

fn timeout_error() -> script::Result {
    script::Result {
        result_type: Some(result::ResultType::Error(script::Error {
            code: i32::from(script::ErrorCode::DeadlineExceeded),
            message: "deadline execeeded".to_string(),
        })),
    }
}

fn error_result(err: anyhow::Error) -> script::Result {
    script::Result {
        result_type: Some(result::ResultType::Error(match err
            .downcast::<promptkit_executor::error::Error>()
        {
            Ok(err) => match err {
                promptkit_executor::error::Error::ExecutionError(c, cause) => script::Error {
                    code: match c {
                        promptkit_executor::error::ErrorCode::Unknown => ErrorCode::Unknown,
                        promptkit_executor::error::ErrorCode::Internal => ErrorCode::Internal,
                        promptkit_executor::error::ErrorCode::Aborted => ErrorCode::GuestAborted,
                    }
                    .into(),
                    message: cause,
                },
            },
            Err(err) => script::Error {
                code: i32::from(script::ErrorCode::Internal),
                message: err.to_string(),
            },
        })),
    }
}
