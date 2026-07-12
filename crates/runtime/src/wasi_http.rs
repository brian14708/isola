use futures::future::join;
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

pub struct HttpRequest {
    method: String,
    url: url::Url,
    headers: Vec<(String, Vec<u8>)>,
    body: Option<Vec<u8>>,
    timeout_ms: Option<u64>,
}

impl HttpRequest {
    #[must_use]
    pub const fn new(
        method: String,
        url: url::Url,
        headers: Vec<(String, Vec<u8>)>,
        body: Option<Vec<u8>>,
        timeout_ms: Option<u64>,
    ) -> Self {
        Self {
            method,
            url,
            headers,
            body,
            timeout_ms,
        }
    }

    #[must_use]
    pub const fn url(&self) -> &url::Url {
        &self.url
    }
}

pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, Vec<u8>)>,
    pub body: Vec<u8>,
}

/// Send and fully buffer a request through `wasi:http/client`.
///
/// # Errors
///
/// Returns an error when the request is invalid, the WASI HTTP exchange fails,
/// or the response body exceeds the configured limit.
pub async fn send(request: HttpRequest) -> Result<HttpResponse, String> {
    let HttpRequest {
        method,
        url,
        headers,
        body,
        timeout_ms,
    } = request;
    let fields = Fields::from_list(&headers).map_err(|e| format!("invalid HTTP headers: {e:?}"))?;
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
        if let Some(bytes) = body {
            let mut writer = body_writer;
            if !writer.write_all(bytes).await.is_empty() {
                return Err("HTTP request body stream closed early".to_string());
            }
        }
        Ok(())
    };
    let (body_result, response) = join(write_body, client::send(request)).await;
    body_result?;
    let response = response.map_err(|e| format_http_error("HTTP request", &e))?;
    let (response, transmission) =
        join(decode_response(response), async move { transmission.await }).await;
    let response = response?;
    transmission.map_err(|e| format_http_error("HTTP request transmission", &e))?;
    Ok(response)
}

async fn decode_response(response: Response) -> Result<HttpResponse, String> {
    let status = response.get_status_code();
    let headers = response.get_headers().copy_all();
    let (result_writer, result_reader) = wit_future::new(|| Ok(()));
    drop(result_writer);
    let (mut stream, trailers) = Response::consume_body(response, result_reader);
    let mut body = Vec::new();
    let mut chunk = Vec::with_capacity(HTTP_RESPONSE_BODY_CHUNK_BYTES);
    loop {
        let (result, read_chunk) = stream.read(chunk).await;
        chunk = read_chunk;
        if body.len().saturating_add(chunk.len()) > MAX_HTTP_RESPONSE_BODY_BYTES {
            return Err(format!(
                "HTTP response body exceeds maximum size of {MAX_HTTP_RESPONSE_BODY_BYTES} bytes"
            ));
        }
        body.extend_from_slice(&chunk);
        chunk.clear();

        match result {
            StreamResult::Complete(_) => {}
            StreamResult::Dropped => break,
            StreamResult::Cancelled => {
                unreachable!("awaited HTTP response body read was cancelled")
            }
        }
    }
    trailers
        .await
        .map_err(|e| format!("HTTP response body failed: {e:?}"))?;
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
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
