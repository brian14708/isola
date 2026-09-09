use std::{future::Future, pin::Pin};

use futures::{StreamExt, future::join, stream};
use wit_bindgen::rt::async_support::StreamResult;

use crate::{
    wasi::http::{
        client,
        types::{
            ErrorCode, Fields, Method, Request as WasiRequest, RequestOptions, Response, Scheme,
        },
    },
    wit_future, wit_stream,
};

const MAX_HTTP_RESPONSE_BODY_BYTES: usize = 16 * 1024 * 1024;
const HTTP_RESPONSE_BODY_CHUNK_BYTES: usize = 64 * 1024;

/// Headers that WASI HTTP forbids setting in `fields`. `host` is derived from
/// the request authority and the rest are hop-by-hop; caller-supplied values
/// are dropped so `fields.from-list` does not reject the request.
const FORBIDDEN_HEADERS: &[&str] = &[
    "connection",
    "host",
    "keep-alive",
    "http2-settings",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fn is_forbidden_header(name: &str) -> bool {
    FORBIDDEN_HEADERS
        .iter()
        .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
}

pub struct HttpRequest {
    method: String,
    url: url::Url,
    headers: Vec<(String, Vec<u8>)>,
    body: Option<HttpBodyStream>,
    timeout_ms: Option<u64>,
}

impl HttpRequest {
    #[must_use]
    pub fn new(
        method: String,
        url: url::Url,
        headers: Vec<(String, Vec<u8>)>,
        body: Option<Vec<u8>>,
        timeout_ms: Option<u64>,
    ) -> Self {
        let body =
            body.map(|bytes| Box::pin(stream::once(async move { Ok(bytes) })) as HttpBodyStream);
        Self {
            method,
            url,
            headers,
            body,
            timeout_ms,
        }
    }

    #[must_use]
    pub fn new_stream(
        method: String,
        url: url::Url,
        headers: Vec<(String, Vec<u8>)>,
        body: HttpBodyStream,
        timeout_ms: Option<u64>,
    ) -> Self {
        Self {
            method,
            url,
            headers,
            body: Some(body),
            timeout_ms,
        }
    }

    #[must_use]
    pub(crate) const fn url(&self) -> &url::Url {
        &self.url
    }
}

pub type HttpBodyStream = Pin<Box<dyn futures::Stream<Item = Result<Vec<u8>, String>> + 'static>>;

pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, Vec<u8>)>,
    pub body: HttpBodyStream,
}

/// Collect a response body for runtimes that expose buffered response APIs.
///
/// # Errors
///
/// Returns an error when a body chunk fails or the response exceeds the
/// configured size limit.
#[expect(
    clippy::future_not_send,
    reason = "WASI stream readers are local to the component runtime"
)]
pub async fn collect_body(mut body: HttpBodyStream) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        if output.len().saturating_add(chunk.len()) > MAX_HTTP_RESPONSE_BODY_BYTES {
            return Err(format!(
                "HTTP response body exceeds maximum size of {MAX_HTTP_RESPONSE_BODY_BYTES} bytes"
            ));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

/// Send a request through `wasi:http/client` and return its response headers
/// and a lazy body stream.
///
/// # Errors
///
/// Returns an error when the request is invalid, the WASI HTTP exchange fails,
/// or the response headers cannot be received. Body transport and size errors
/// are reported by the returned stream when it is consumed.
#[expect(
    clippy::future_not_send,
    reason = "WASI stream readers are local to the component runtime"
)]
pub(crate) async fn send(request: HttpRequest) -> Result<HttpResponse, String> {
    let HttpRequest {
        method,
        url,
        mut headers,
        body,
        timeout_ms,
    } = request;
    headers.retain(|(name, _)| !is_forbidden_header(name));
    let fields = Fields::from_list(&headers).map_err(|error| {
        let rejected = headers.iter().find_map(|(name, value)| {
            Fields::from_list(&[(name.clone(), value.clone())])
                .is_err()
                .then_some(name)
        });
        rejected.map_or_else(
            || format!("invalid HTTP headers: {error:?}"),
            |name| format!("invalid HTTP header {name:?}: {error:?}"),
        )
    })?;
    let (body_writer, body_reader) = wit_stream::new::<u8>();
    let (trailers_writer, trailers_reader) = wit_future::new(|| Ok(None));
    drop(trailers_writer);

    let options = timeout_ms
        .map(|millis| {
            let options = RequestOptions::new();
            let duration = millis.saturating_mul(1_000_000);
            options
                .set_first_byte_timeout(Some(duration))
                .map_err(|e| format!("invalid HTTP timeout: {e:?}"))?;
            options
                .set_between_bytes_timeout(Some(duration))
                .map_err(|e| format!("invalid HTTP timeout: {e:?}"))?;
            Ok::<_, String>(options)
        })
        .transpose()?;
    let (request, transmission) = WasiRequest::new(
        fields,
        body.as_ref().map(|_| body_reader),
        trailers_reader,
        options,
    );
    request
        .set_method(&parse_method(&method))
        .map_err(|()| "invalid HTTP method".to_string())?;
    let scheme = parse_scheme(url.scheme());
    request
        .set_scheme(Some(&scheme))
        .map_err(|()| "invalid HTTP scheme".to_string())?;
    request
        .set_authority(Some(url.authority()))
        .map_err(|()| "invalid HTTP authority".to_string())?;
    request
        .set_path_with_query(Some(&url[url::Position::BeforePath..]))
        .map_err(|()| "invalid HTTP path".to_string())?;

    let write_body = async move {
        if let Some(mut body) = body {
            let mut writer = body_writer;
            while let Some(chunk) = body.next().await {
                let chunk = chunk?;
                if chunk.is_empty() {
                    continue;
                }
                if !writer.write_all(chunk).await.is_empty() {
                    return Err("HTTP request body stream closed early".to_string());
                }
            }
        }
        Ok(())
    };
    let (body_result, response) = join(write_body, client::send(request)).await;
    body_result?;
    let response = response.map_err(|e| format_http_error("HTTP request", &e))?;
    Ok(decode_response(response, async move { transmission.await }))
}

fn decode_response(
    response: Response,
    transmission: impl Future<Output = Result<(), ErrorCode>> + 'static,
) -> HttpResponse {
    let status = response.get_status_code();
    let headers = response.get_headers().copy_all();
    let (result_writer, result_reader) = wit_future::new(|| Ok(()));
    let (stream, trailers) = Response::consume_body(response, result_reader);
    drop(result_writer);

    let body = stream::unfold(
        (
            stream,
            Some(trailers),
            Some(transmission),
            Vec::with_capacity(HTTP_RESPONSE_BODY_CHUNK_BYTES),
            0usize,
        ),
        |(mut stream, mut trailers, mut transmission, mut chunk, total)| async move {
            loop {
                let (result, read_chunk) = stream.read(chunk).await;
                chunk = read_chunk;

                match result {
                    StreamResult::Complete(_) if !chunk.is_empty() => {
                        let chunk_len = chunk.len();
                        let next_total = total.saturating_add(chunk_len);
                        if next_total > MAX_HTTP_RESPONSE_BODY_BYTES {
                            let error = format!(
                                "HTTP response body exceeds maximum size of {MAX_HTTP_RESPONSE_BODY_BYTES} bytes"
                            );
                            return Some((
                                Err(error),
                                (stream, None, None, Vec::new(), next_total),
                            ));
                        }
                        let output = std::mem::take(&mut chunk);
                        return Some((
                            Ok(output),
                            (
                                stream,
                                trailers,
                                transmission,
                                Vec::with_capacity(HTTP_RESPONSE_BODY_CHUNK_BYTES),
                                next_total,
                            ),
                        ));
                    }
                    StreamResult::Complete(_) => {}
                    StreamResult::Dropped => {
                        let result = match (trailers.take(), transmission.take()) {
                            (Some(trailers), Some(transmission)) => match trailers.await {
                                Ok(_) => transmission.await.map_err(|e| {
                                    format_http_error("HTTP request transmission", &e)
                                }),
                                Err(error) => Err(format!("HTTP response body failed: {error:?}")),
                            },
                            _ => Ok(()),
                        };
                        return result
                            .err()
                            .map(|error| (Err(error), (stream, None, None, Vec::new(), total)));
                    }
                    StreamResult::Cancelled => {
                        unreachable!("awaited HTTP response body read was cancelled")
                    }
                }
            }
        },
    );
    HttpResponse {
        status,
        headers,
        body: Box::pin(body),
    }
}

fn format_http_error(context: &str, error: &ErrorCode) -> String {
    if matches!(
        error,
        ErrorCode::DnsTimeout
            | ErrorCode::ConnectionTimeout
            | ErrorCode::ConnectionReadTimeout
            | ErrorCode::ConnectionWriteTimeout
            | ErrorCode::HttpResponseTimeout
    ) {
        format!("{context} timed out: {error:?}")
    } else {
        format!("{context} failed: {error:?}")
    }
}

fn parse_method(method: &str) -> Method {
    match method.to_ascii_uppercase().as_str() {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "CONNECT" => Method::Connect,
        "OPTIONS" => Method::Options,
        "TRACE" => Method::Trace,
        "PATCH" => Method::Patch,
        _ => Method::Other(method.to_string()),
    }
}

fn parse_scheme(scheme: &str) -> Scheme {
    match scheme {
        "http" => Scheme::Http,
        "https" => Scheme::Https,
        _ => Scheme::Other(scheme.to_string()),
    }
}
