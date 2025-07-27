use std::{borrow::Cow, collections::HashMap, pin::Pin, time::Duration};

use futures_util::{Stream, StreamExt};
use promptkit_executor::{ExecArgument, ExecArgumentValue, ExecStreamItem};
use promptkit_trace::{collect::CollectorSpanExt, consts::TRACE_TARGET_SCRIPT};
use tokio::{sync::mpsc, try_join};
use tokio_stream::{once, wrappers::UnboundedReceiverStream};
use tonic::{Response, Status};
use tracing::{Instrument, Span, info_span, level_filters::LevelFilter};

use crate::{
    proto::script::v1::{
        self as script, ContentType, ErrorCode, Trace, analyze_response, argument::Marker,
        execute_client_stream_request, execute_stream_request, result,
        script_service_server::ScriptService,
    },
    routes::{AppState, VmEnv},
    service::prost_serde::{argument, parse_source},
    utils::{
        stream::{join_with, stream_until},
        trace::TraceCollector,
    },
};

mod ipc;
mod prost_serde;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

pub struct ScriptServer {
    state: AppState,
    base_env: VmEnv,
}

impl ScriptServer {
    pub fn new(state: AppState) -> Self {
        let base_env = VmEnv {
            client: promptkit_request::Client::new(),
        };
        Self { state, base_env }
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

    async fn analyze(
        &self,
        mut request: tonic::Request<script::AnalyzeRequest>,
    ) -> Result<tonic::Response<script::AnalyzeResponse>, Status> {
        let ParsedSpec {
            method,
            args,
            timeout,
            stream_tx: None,
            span,
            log_level,
            trace_events: None,
            env,
        } = parse_spec(request.get_mut().spec.as_mut(), &self.base_env)?
        else {
            return Err(Status::invalid_argument("unexpected stream marker"));
        };
        if !(method.is_empty() && args.is_empty()) {
            return Err(Status::invalid_argument("method & args not allowed"));
        }
        let script = parse_source(request.get_mut().source.take())?;

        let req = promptkit_cbor::to_cbor(&Into::<ipc::AnalyzeRequest>::into(request.get_ref()))
            .map_err(|e| Status::internal(e.to_string()))?;

        let result = async {
            let run = async {
                let stream = self
                    .state
                    .vm
                    .exec(
                        "",
                        script,
                        "$analyze".to_string(),
                        vec![ExecArgument {
                            name: None,
                            value: ExecArgumentValue::Cbor(req),
                        }],
                        env.as_ref(),
                        log_level,
                    )
                    .await
                    .map_err(|e| {
                        Status::invalid_argument(format!("failed to start script: {e}"))
                    })?;
                let m = non_stream_result(stream, [ContentType::Cbor as i32]).await?;
                match m.result_type {
                    Some(result::ResultType::Cbor(c)) => {
                        let r: ipc::AnalyzeResult = promptkit_cbor::from_cbor(&c).map_err(|e| {
                            Status::internal(format!("failed to decode result: {e}"))
                        })?;
                        Ok(analyze_response::ResultType::AnalyzeResult(r.into()))
                    }
                    Some(result::ResultType::Error(e)) => {
                        Ok(analyze_response::ResultType::Error(e))
                    }
                    _ => Err(Status::internal("unexpected result type")),
                }
            };
            match tokio::time::timeout(timeout, run).await {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(s)) => Err(s),
                Err(_) => Ok(analyze_response::ResultType::Error(script::Error {
                    code: i32::from(script::ErrorCode::DeadlineExceeded),
                    message: "deadline exceeded".to_string(),
                })),
            }
        }
        .instrument(span)
        .await?;

        Ok(Response::new(script::AnalyzeResponse {
            result_type: Some(result),
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
            span,
            log_level,
            mut trace_events,
            env,
        } = parse_spec(request.get_mut().spec.as_mut(), &self.base_env)?
        else {
            return Err(Status::invalid_argument("unexpected stream marker"));
        };
        let script = parse_source(request.get_mut().source.take())?;
        let ns = request
            .metadata()
            .get("x-app-id")
            .and_then(|s| s.to_str().ok())
            .unwrap_or("");

        let result = async {
            let run = async {
                let stream = self
                    .state
                    .vm
                    .exec(ns, script, method, args, env.as_ref(), log_level)
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
        }
        .instrument(span);

        let trace_async = async move {
            let mut metadata = script::ExecutionMetadata::default();
            if let Some(trace_events) = trace_events.as_mut() {
                while let Some(event) = trace_events.recv().await {
                    metadata.traces.push(event);
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
            timeout: _,
        }) = request.get_mut().message().await?
        else {
            return Err(Status::invalid_argument("initial request not found"));
        };

        let script = parse_source(initial.source.take())?;
        let ParsedSpec {
            method,
            args,
            timeout,
            stream_tx: mut tx,
            span,
            log_level,
            mut trace_events,
            env,
        } = parse_spec(initial.spec.as_mut(), &self.base_env)?;

        let (md, _, mut body) = request.into_parts();
        let ns = md
            .get("x-app-id")
            .and_then(|s| s.to_str().ok())
            .unwrap_or("");
        let result = async {
            let run = async {
                let stream = self
                    .state
                    .vm
                    .exec(ns, script, method, args, env.as_ref(), log_level)
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
        }
        .instrument(span);

        let trace_async = async move {
            let mut metadata = script::ExecutionMetadata::default();
            if let Some(trace_events) = trace_events.as_mut() {
                while let Some(event) = trace_events.recv().await {
                    metadata.traces.push(event);
                }
            }
            Ok::<_, Status>(metadata)
        };
        let mover = async move {
            while let Some(msg) = body.message().await? {
                if let Some(tx) = tx.as_mut()
                    && let Some(execute_client_stream_request::RequestType::StreamValue(v)) =
                        msg.request_type
                {
                    let name = v.name.clone();
                    let arg =
                        argument(v).map_err(|_e| Status::invalid_argument("invalid arguments"))?;
                    match arg {
                        Err(Marker::StreamControlClose) => {
                            tx.remove(&name);
                        }
                        Err(_) => Err(Status::invalid_argument("invalid marker"))?,
                        Ok(arg) => {
                            let tx = tx.get_mut(&name).ok_or_else(|| {
                                Status::invalid_argument("invalid marker arguments")
                            })?;
                            match msg.timeout.and_then(|t| t.try_into().ok()) {
                                Some(timeout) => {
                                    tx.send_timeout(arg, timeout).await.map_err(|_| {
                                        Status::deadline_exceeded("stream argument timeout")
                                    })?;
                                }
                                None => tx.send(arg).await.map_err(|_| {
                                    Status::internal("failed to send stream argument")
                                })?,
                            }
                        }
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
            span,
            log_level,
            trace_events,
            env,
        } = parse_spec(request.get_mut().spec.as_mut(), &self.base_env)?
        else {
            return Err(Status::invalid_argument("unexpected stream marker"));
        };
        let script = parse_source(request.get_mut().source.take())?;
        let deadline = std::time::Instant::now() + timeout;

        let ns = request
            .metadata()
            .get("x-app-id")
            .and_then(|s| s.to_str().ok())
            .unwrap_or("");

        let stream = match tokio::time::timeout(
            timeout,
            self.state
                .vm
                .exec(ns, script, method, args, env.as_ref(), log_level),
        )
        .instrument(span.clone())
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
                )))));
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
            span,
            Ok(script::ExecuteServerStreamResponse {
                result: Some(timeout_error()),
                metadata: None,
            }),
        );
        match trace_events {
            Some(tracer_events) => {
                let trace_async = UnboundedReceiverStream::new(tracer_events)
                    .ready_chunks(4)
                    .map(|traces| {
                        Ok(script::ExecuteServerStreamResponse {
                            result: None,
                            metadata: Some(script::ExecutionStreamMetadata { traces }),
                        })
                    });
                Ok(Response::new(Box::pin(tokio_stream::StreamExt::merge(
                    stream,
                    trace_async,
                ))))
            }
            _ => Ok(Response::new(Box::pin(stream))),
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn execute_stream(
        &self,
        mut request: tonic::Request<tonic::Streaming<script::ExecuteStreamRequest>>,
    ) -> Result<tonic::Response<Self::ExecuteStreamStream>, Status> {
        let Some(script::ExecuteStreamRequest {
            request_type: Some(execute_stream_request::RequestType::InitialRequest(mut initial)),
            timeout: _,
        }) = request.get_mut().message().await?
        else {
            return Err(Status::invalid_argument("initial request not found"));
        };

        let script = parse_source(initial.source.take())?;
        let ParsedSpec {
            method,
            args,
            timeout,
            stream_tx: mut tx,
            span,
            trace_events,
            log_level,
            env,
        } = parse_spec(initial.spec.as_mut(), &self.base_env)?;

        let deadline = std::time::Instant::now() + timeout;
        let ns = request
            .metadata()
            .get("x-app-id")
            .and_then(|s| s.to_str().ok())
            .unwrap_or("");
        let stream = match tokio::time::timeout(
            timeout,
            self.state
                .vm
                .exec(ns, script, method, args, env.as_ref(), log_level),
        )
        .instrument(span.clone())
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
                )))));
            }
        };

        let mover = async move {
            while let Some(msg) = request.get_mut().message().await? {
                if let Some(tx) = tx.as_mut()
                    && let Some(execute_stream_request::RequestType::StreamValue(v)) =
                        msg.request_type
                {
                    let name = v.name.clone();
                    let arg =
                        argument(v).map_err(|_e| Status::invalid_argument("invalid arguments"))?;
                    match arg {
                        Err(Marker::StreamControlClose) => {
                            tx.remove(&name);
                        }
                        Err(_) => Err(Status::invalid_argument("invalid marker"))?,
                        Ok(arg) => {
                            let tx = tx.get_mut(&name).ok_or_else(|| {
                                Status::invalid_argument("invalid marker arguments")
                            })?;
                            match msg.timeout.and_then(|t| t.try_into().ok()) {
                                Some(timeout) => {
                                    tx.send_timeout(arg, timeout).await.map_err(|_| {
                                        Status::deadline_exceeded("stream argument timeout")
                                    })?;
                                }
                                None => tx.send(arg).await.map_err(|_| {
                                    Status::internal("failed to send stream argument")
                                })?,
                            }
                        }
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
            span,
            Ok(script::ExecuteStreamResponse {
                result: Some(timeout_error()),
                metadata: None,
            }),
        );
        match trace_events {
            Some(tracer_events) => {
                let trace_async = UnboundedReceiverStream::new(tracer_events)
                    .ready_chunks(4)
                    .map(|traces| {
                        Ok(script::ExecuteStreamResponse {
                            result: None,
                            metadata: Some(script::ExecutionStreamMetadata { traces }),
                        })
                    });
                Ok(Response::new(Box::pin(tokio_stream::StreamExt::merge(
                    stream,
                    trace_async,
                ))))
            }
            _ => Ok(Response::new(Box::pin(stream))),
        }
    }
}

async fn non_stream_result(
    mut stream: impl Stream<Item = ExecStreamItem> + Unpin,
    content_type: impl IntoIterator<Item = i32>,
) -> Result<script::Result, Status> {
    let mut o = match stream.next().await {
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
            let mut o = Vec::with_capacity(d.len() + 2);
            o.push(0x9F); // start of array
            o.extend_from_slice(&d);
            o
        }
        Some(ExecStreamItem::Error(err)) => return Ok(error_result(err)),
        None => return Err(Status::internal("empty stream")),
    };

    while let Some(item) = stream.next().await {
        match item {
            ExecStreamItem::Data(data) => {
                o.extend_from_slice(&data);
            }
            ExecStreamItem::End(None) => break,
            ExecStreamItem::End(Some(_)) => {
                return Err(Status::internal("unexpected end with data"));
            }
            ExecStreamItem::Error(err) => {
                return Err(Status::internal(err.to_string()));
            }
        }
    }
    o.push(0xFF); // end of array
    prost_serde::result_type(o.into(), content_type)
}

struct ParsedSpec<'a> {
    method: String,
    args: Vec<ExecArgument>,
    timeout: Duration,
    stream_tx: Option<HashMap<String, mpsc::Sender<Vec<u8>>>>,
    log_level: LevelFilter,
    span: tracing::Span,
    trace_events: Option<mpsc::UnboundedReceiver<Trace>>,
    env: Cow<'a, VmEnv>,
}

#[allow(clippy::result_large_err)]
fn parse_spec<'a>(
    spec: Option<&mut script::ExecutionSpec>,
    base_env: &'a VmEnv,
) -> Result<ParsedSpec<'a>, Status> {
    if let Some(spec) = spec {
        let mut streams = HashMap::new();
        let args = std::mem::take(&mut spec.arguments)
            .into_iter()
            .map(|mut a| {
                let name = if a.name.is_empty() {
                    None
                } else {
                    Some(std::mem::take(&mut a.name))
                };
                match argument(a) {
                    Ok(Ok(a)) => Ok(ExecArgument {
                        name,
                        value: ExecArgumentValue::Cbor(a),
                    }),
                    Ok(Err(Marker::Stream)) => {
                        let (tx, rx) = mpsc::channel(64);
                        let key = name.as_ref().map_or("", |v| v);
                        if streams.contains_key(key) {
                            Err(Status::invalid_argument("invalid marker arguments"))
                        } else {
                            streams.insert(key.to_string(), tx);
                            Ok(ExecArgument {
                                name,
                                value: ExecArgumentValue::CborStream(rx),
                            })
                        }
                    }
                    Ok(Err(_)) => Err(Status::invalid_argument("invalid marker arguments")),
                    Err(e) => Err(e),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        let (span, trace, log_level) = match script::TraceLevel::try_from(spec.trace_level) {
            Ok(script::TraceLevel::All) => {
                let (collector, rx) = TraceCollector::new();
                let s = info_span!(
                    target: TRACE_TARGET_SCRIPT,
                    parent: Span::current(),
                    "script.exec"
                );
                if s.collect_into(TRACE_TARGET_SCRIPT, LevelFilter::DEBUG, collector)
                    .is_some()
                {
                    (s, Some(rx), LevelFilter::DEBUG)
                } else {
                    (Span::none(), None, LevelFilter::DEBUG)
                }
            }
            Ok(script::TraceLevel::None) => (Span::none(), None, LevelFilter::OFF),
            _ => (Span::none(), None, LevelFilter::INFO),
        };
        Ok(ParsedSpec {
            method: std::mem::take(&mut spec.method),
            args,
            timeout: spec
                .timeout
                .take()
                .and_then(|t| t.try_into().ok())
                .unwrap_or(DEFAULT_TIMEOUT),
            stream_tx: if streams.is_empty() {
                None
            } else {
                Some(streams)
            },
            span,
            log_level,
            trace_events: trace,
            env: base_env.update(),
        })
    } else {
        Ok(ParsedSpec {
            method: String::new(),
            args: vec![],
            timeout: DEFAULT_TIMEOUT,
            stream_tx: None,
            span: Span::none(),
            log_level: LevelFilter::OFF,
            trace_events: None,
            env: Cow::Borrowed(base_env),
        })
    }
}

fn timeout_error() -> script::Result {
    script::Result {
        result_type: Some(result::ResultType::Error(script::Error {
            code: i32::from(script::ErrorCode::DeadlineExceeded),
            message: "deadline exceeded".to_string(),
        })),
    }
}

fn error_result(err: promptkit_executor::error::Error) -> script::Result {
    script::Result {
        result_type: Some(result::ResultType::Error(match err {
            promptkit_executor::error::Error::ExecutionError(c, cause) => script::Error {
                code: match c {
                    promptkit_executor::error::ErrorCode::Unknown => ErrorCode::Unknown,
                    promptkit_executor::error::ErrorCode::Internal => ErrorCode::Internal,
                    promptkit_executor::error::ErrorCode::Aborted => ErrorCode::GuestAborted,
                }
                .into(),
                message: cause,
            },
            promptkit_executor::error::Error::Other(err) => script::Error {
                code: ErrorCode::Unknown.into(),
                message: err.to_string(),
            },
        })),
    }
}
