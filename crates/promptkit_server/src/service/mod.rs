use std::{
    pin::Pin,
    time::{Duration, SystemTime, UNIX_EPOCH},
    usize,
};

use futures_util::{Stream, StreamExt};
use promptkit_executor::{
    trace::{BoxedTracer, MemoryTracer, TraceEvent},
    ExecArgument, ExecStreamItem,
};
use tokio::{sync::mpsc, try_join};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Response, Status};

use crate::{
    proto::script::{
        self, argument::Marker, execute_client_stream_request, execute_stream_request,
        script_service_server::ScriptService, ExecuteClientStreamRequest,
        ExecuteClientStreamResponse, ExecuteRequest, ExecuteResponse, ExecuteServerStreamRequest,
        ExecuteServerStreamResponse, ExecuteStreamRequest, ExecuteStreamResponse,
        ExecutionStreamMetadata,
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
        Pin<Box<dyn Stream<Item = Result<ExecuteServerStreamResponse, Status>> + Send>>;
    type ExecuteStreamStream =
        Pin<Box<dyn Stream<Item = Result<ExecuteStreamResponse, Status>> + Send>>;

    async fn execute(
        &self,
        mut request: tonic::Request<ExecuteRequest>,
    ) -> Result<tonic::Response<ExecuteResponse>, Status> {
        let ParsedSpec {
            args,
            timeout,
            stream_tx: None,
            tracer,
            mut trace_events,
        } = parse_spec(request.get_mut().spec.as_mut())?
        else {
            return Err(Status::invalid_argument("unexpected stream marker"));
        };
        let (script, method) = parse_source(&request.get_ref().source)?;

        tokio::time::timeout(timeout, async {
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
        })
        .await
        .map_err(|_| Status::deadline_exceeded("execution timed out"))?
    }

    async fn execute_client_stream(
        &self,
        mut request: tonic::Request<tonic::Streaming<ExecuteClientStreamRequest>>,
    ) -> Result<tonic::Response<ExecuteClientStreamResponse>, Status> {
        let Some(ExecuteClientStreamRequest {
            request_type:
                Some(execute_client_stream_request::RequestType::InitialRequest(mut initial)),
        }) = request.get_mut().message().await?
        else {
            return Err(Status::invalid_argument("initial request not found"));
        };

        let (script, method) = parse_source(&initial.source)?;
        let ParsedSpec {
            args,
            timeout,
            stream_tx: mut tx,
            tracer,
            mut trace_events,
        } = parse_spec(initial.spec.as_mut())?;

        tokio::time::timeout(timeout, async {
            let stream = self
                .state
                .vm
                .exec(script, method, args, tracer)
                .await
                .map_err(|e| Status::internal(format!("failed to execute script: {e}")))?;
            let result = non_stream_result(stream, initial.result_content_type.iter().copied());
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
                                        .map_err(|_e| {
                                            Status::invalid_argument("invalid arguments")
                                        })?
                                        .map_err(|_| Status::invalid_argument("invalid marker"))?
                                        .to_string(),
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
        })
        .await
        .map_err(|_| Status::deadline_exceeded("execution timed out"))?
    }

    async fn execute_server_stream(
        &self,
        mut request: tonic::Request<ExecuteServerStreamRequest>,
    ) -> Result<tonic::Response<Self::ExecuteServerStreamStream>, Status> {
        let ParsedSpec {
            args,
            timeout,
            stream_tx: None,
            tracer,
            trace_events,
        } = parse_spec(request.get_mut().spec.as_mut())?
        else {
            return Err(Status::invalid_argument("unexpected stream marker"));
        };
        let (script, method) = parse_source(&request.get_ref().source)?;
        let deadline = std::time::Instant::now() + timeout;
        let stream =
            tokio::time::timeout(timeout, self.state.vm.exec(script, method, args, tracer))
                .await
                .map_err(|_| Status::deadline_exceeded("execution timed out"))?
                .map_err(|e| Status::internal(format!("failed to execute script: {e}")))?;

        let content_type = request.get_ref().result_content_type.clone();
        let m = stream.map::<Result<ExecuteServerStreamResponse, Status>, _>(move |s| match s {
            ExecStreamItem::Data(d) | ExecStreamItem::End(Some(d)) => {
                Ok(ExecuteServerStreamResponse {
                    result: Some(prost_serde::result_type(
                        d.into(),
                        content_type.iter().copied(),
                    )?),
                    metadata: None,
                })
            }
            ExecStreamItem::End(None) => Ok(ExecuteServerStreamResponse {
                result: None,
                metadata: None,
            }),
            ExecStreamItem::Error(err) => Err(Status::internal(err.to_string())),
        });
        if let Some((start, tracer_events)) = trace_events {
            let trace_async =
                ReceiverStream::new(tracer_events).chunks(4).map::<Result<
                    ExecuteServerStreamResponse,
                    Status,
                >, _>(move |e| {
                    Ok(ExecuteServerStreamResponse {
                        result: None,
                        metadata: Some(ExecutionStreamMetadata {
                            traces: e.into_iter().map(|e| trace_convert(e, &start)).collect(),
                        }),
                    })
                });
            Ok(Response::new(Box::pin(stream_until(
                tokio_stream::StreamExt::merge(m, trace_async),
                deadline,
                Status::deadline_exceeded("execution timed out"),
            ))))
        } else {
            Ok(Response::new(Box::pin(stream_until(
                m,
                deadline,
                Status::deadline_exceeded("execution timed out"),
            ))))
        }
    }

    async fn execute_stream(
        &self,
        mut request: tonic::Request<tonic::Streaming<ExecuteStreamRequest>>,
    ) -> Result<tonic::Response<Self::ExecuteStreamStream>, Status> {
        let Some(ExecuteStreamRequest {
            request_type: Some(execute_stream_request::RequestType::InitialRequest(mut initial)),
        }) = request.get_mut().message().await?
        else {
            return Err(Status::invalid_argument("initial request not found"));
        };

        let (script, method) = parse_source(&initial.source)?;
        let ParsedSpec {
            args,
            timeout,
            stream_tx: mut tx,
            tracer,
            trace_events,
        } = parse_spec(initial.spec.as_mut())?;
        let deadline = std::time::Instant::now() + timeout;
        let stream =
            tokio::time::timeout(timeout, self.state.vm.exec(script, method, args, tracer))
                .await
                .map_err(|_| Status::deadline_exceeded("execution timed out"))?
                .map_err(|e| Status::internal(format!("failed to execute script: {e}")))?;
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
                                    .map_err(|_| Status::invalid_argument("invalid marker"))?
                                    .to_string(),
                            )
                            .await;
                    }
                }
            }
            Ok::<_, Status>(())
        };

        let content_type = initial.result_content_type.clone();
        let m = stream.map::<Result<ExecuteStreamResponse, Status>, _>(move |s| match s {
            ExecStreamItem::Data(d) | ExecStreamItem::End(Some(d)) => Ok(ExecuteStreamResponse {
                result: Some(prost_serde::result_type(
                    d.into(),
                    content_type.iter().copied(),
                )?),
                metadata: None,
            }),
            ExecStreamItem::End(None) => Ok(ExecuteStreamResponse {
                result: None,
                metadata: None,
            }),
            ExecStreamItem::Error(err) => Err(Status::internal(err.to_string())),
        });
        if let Some((start, tracer_events)) = trace_events {
            let trace_async =
                ReceiverStream::new(tracer_events)
                    .chunks(4)
                    .map::<Result<ExecuteStreamResponse, Status>, _>(move |e| {
                        Ok(ExecuteStreamResponse {
                            result: None,
                            metadata: Some(ExecutionStreamMetadata {
                                traces: e.into_iter().map(|e| trace_convert(e, &start)).collect(),
                            }),
                        })
                    });
            Ok(Response::new(Box::pin(stream_until(
                join_with(tokio_stream::StreamExt::merge(m, trace_async), mover),
                deadline,
                Status::deadline_exceeded("execution timed out"),
            ))))
        } else {
            Ok(Response::new(Box::pin(stream_until(
                join_with(m, mover),
                deadline,
                Status::deadline_exceeded("execution timed out"),
            ))))
        }
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

struct ParsedSpec {
    args: Vec<ExecArgument>,
    timeout: Duration,
    stream_tx: Option<mpsc::Sender<String>>,
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
                Ok(Ok(a)) => Ok(ExecArgument::Json(a)),
                Ok(Err(Marker::Stream)) => {
                    if let Some(rx) = rx.take() {
                        Ok(ExecArgument::JsonStream(rx))
                    } else {
                        Err(Status::invalid_argument("invalid marker arguments"))
                    }
                }
                Ok(Err(_)) => Err(Status::invalid_argument("invalid marker arguments")),
                Err(e) => Err(e),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (tracer, trace_events) = (spec.trace_level == script::TraceLevel::All as i32)
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
            args: vec![],
            timeout: DEFAULT_TIMEOUT,
            stream_tx: None,
            tracer: None,
            trace_events: None,
        })
    }
}
