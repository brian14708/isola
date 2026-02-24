use std::{borrow::Cow, io::Write};

use rquickjs::{Array, Ctx, Function, Object, Value, object::Filter};
use url::Url;

use super::{
    future::{self, PendingOp},
    wasi::{
        http::{
            outgoing_handler::{RequestOptions, handle},
            types::{Fields, IncomingResponse, Method, OutgoingBody, Scheme},
        },
        io::streams::{InputStream, StreamError},
    },
};
use crate::serde as js_serde;

const MAX_INCOMING_HTTP_BODY_BYTES: usize = 16 * 1024 * 1024;

pub fn register(ctx: &Ctx<'_>) {
    let globals = ctx.globals();

    let http = Object::new(ctx.clone()).unwrap();

    // _isola_http._send(method, url, params, headers, body, timeout) -> handle: u32
    // Sends the HTTP request (non-blocking) and returns a pollable handle.
    // NOTE: params/timeout are legacy internal fields and may be undefined.
    http.set("_send", Function::new(ctx.clone(), js_send).unwrap())
        .unwrap();

    // _isola_http._recv(handle) -> response payload object
    // Reads the completed HTTP response transport payload for the given handle.
    http.set("_recv", Function::new(ctx.clone(), js_recv).unwrap())
        .unwrap();

    // _isola_http.new_buffer(kind) -> buffer object
    http.set(
        "new_buffer",
        Function::new(ctx.clone(), js_new_buffer).unwrap(),
    )
    .unwrap();

    globals.set("_isola_http", http).unwrap();
}

#[allow(clippy::needless_pass_by_value)]
fn js_send<'js>(
    _ctx: Ctx<'js>,
    method: String,
    url: String,
    params: Value<'js>,
    headers: Value<'js>,
    body: Value<'js>,
    timeout: Value<'js>,
) -> rquickjs::Result<u32> {
    send_impl(&method, &url, params, headers, body, timeout)
        .map_err(|e| rquickjs::Error::new_from_js_message("fetch", "error", &e))
}

#[allow(clippy::needless_pass_by_value)]
fn js_recv(ctx: Ctx<'_>, handle: u32) -> rquickjs::Result<Object<'_>> {
    future::recv_http(&ctx, handle)
}

#[allow(clippy::needless_pass_by_value)]
fn js_new_buffer(ctx: Ctx<'_>, kind: String) -> rquickjs::Result<Object<'_>> {
    let buf = Object::new(ctx.clone())
        .map_err(|e| rquickjs::Error::new_from_js_message("buffer", "error", &e.to_string()))?;
    buf.set("_kind", kind)
        .map_err(|e| rquickjs::Error::new_from_js_message("buffer", "error", &e.to_string()))?;
    buf.set("_data", rquickjs::Array::new(ctx).unwrap())
        .map_err(|e| rquickjs::Error::new_from_js_message("buffer", "error", &e.to_string()))?;
    Ok(buf)
}

fn value_to_string(value: &Value<'_>) -> Option<String> {
    value
        .as_string()
        .and_then(|s| s.to_string().ok())
        .or_else(|| value.as_int().map(|v| v.to_string()))
        .or_else(|| value.as_float().map(|v| v.to_string()))
        .or_else(|| value.as_bool().map(|v| v.to_string()))
}

fn append_headers(header_fields: &Fields, headers: &Value<'_>) -> Result<(), String> {
    if headers.is_null() || headers.is_undefined() {
        return Ok(());
    }

    if let Some(hdr_arr) = headers.as_array() {
        for i in 0..hdr_arr.len() {
            let entry: Value<'_> = hdr_arr.get(i).map_err(|e| e.to_string())?;
            let pair = entry
                .as_array()
                .ok_or_else(|| "header entry must be a [name, value] pair".to_string())?;
            if pair.len() < 2 {
                return Err("header entry must include name and value".to_string());
            }

            let name: Value<'_> = pair.get(0).map_err(|e| e.to_string())?;
            let value: Value<'_> = pair.get(1).map_err(|e| e.to_string())?;
            let name = value_to_string(&name)
                .ok_or_else(|| "header name must be string-coercible".to_string())?;
            let value = value_to_string(&value)
                .ok_or_else(|| "header value must be string-coercible".to_string())?;

            header_fields
                .append(&name.to_ascii_lowercase(), value.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    if let Some(hdr_obj) = headers.as_object() {
        let props: Vec<(String, String)> = hdr_obj
            .own_props(Filter::new().string().enum_only())
            .flatten()
            .filter_map(|(k, v): (String, Value<'_>)| value_to_string(&v).map(|s| (k, s)))
            .collect();
        for (k, v) in &props {
            header_fields
                .append(&k.to_ascii_lowercase(), v.as_bytes())
                .map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

/// Send an HTTP request (non-blocking). Returns a pollable handle.
#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
fn send_impl(
    method: &str,
    url: &str,
    params: Value<'_>,
    headers: Value<'_>,
    body: Value<'_>,
    timeout: Value<'_>,
) -> Result<u32, String> {
    let header_fields = Fields::new();
    append_headers(&header_fields, &headers)?;

    // If body is an object (not null/string/arraybuffer), set content-type to JSON
    let is_json_body = body.is_object()
        && !body.is_null()
        && body.as_string().is_none()
        && rquickjs::ArrayBuffer::from_value(body.clone()).is_none();

    if is_json_body && !header_fields.has("content-type") {
        header_fields
            .append("content-type", b"application/json")
            .map_err(|e| e.to_string())?;
    }

    let request = super::wasi::http::outgoing_handler::OutgoingRequest::new(header_fields);
    request
        .set_method(&to_method(method))
        .map_err(|()| "invalid method".to_string())?;

    let mut u = Url::parse(url).map_err(|e| e.to_string())?;
    // Add query params
    if let Some(params_obj) = params.as_object() {
        let params_props: Vec<(String, String)> = params_obj
            .own_props(Filter::new().string().enum_only())
            .flatten()
            .filter_map(|(k, v): (String, Value<'_>)| {
                v.as_string()
                    .and_then(|s| s.to_string().ok())
                    .map(|s| (k, s))
            })
            .collect();
        for (k, v) in &params_props {
            u.query_pairs_mut().append_pair(k, v);
        }
    }

    request
        .set_authority(Some(u.authority()))
        .map_err(|()| "invalid authority".to_string())?;
    request
        .set_scheme(Some(&match u.scheme() {
            "http" => Scheme::Http,
            "https" => Scheme::Https,
            s => Scheme::Other(s.to_string()),
        }))
        .map_err(|()| "invalid scheme".to_string())?;

    let mut pq = Cow::Borrowed(u.path());
    if let Some(q) = u.query() {
        let mut copy = pq.to_string();
        copy.push('?');
        copy.push_str(q);
        pq = Cow::Owned(copy);
    }
    request
        .set_path_with_query(Some(&pq))
        .map_err(|()| "invalid path".to_string())?;

    let opt = RequestOptions::new();
    if let Some(t) = timeout.as_float() {
        opt.set_first_byte_timeout(Some(
            u64::try_from(std::time::Duration::from_secs_f64(t).as_nanos())
                .expect("duration is too large"),
        ))
        .map_err(|()| "invalid timeout".to_string())?;
    } else if let Some(t) = timeout.as_int()
        && t > 0
    {
        opt.set_first_byte_timeout(Some(
            u64::try_from(
                std::time::Duration::from_secs(u64::try_from(t).unwrap_or(30)).as_nanos(),
            )
            .expect("duration is too large"),
        ))
        .map_err(|()| "invalid timeout".to_string())?;
    }

    let has_body = !body.is_null() && !body.is_undefined();
    let ob = if has_body {
        Some(request.body().map_err(|()| "invalid body".to_string())?)
    } else {
        None
    };

    let resp = handle(request, Some(opt)).map_err(|e| e.to_string())?;

    // Write body
    if let Some(ob) = ob {
        let mut os = ob.write().map_err(|()| "invalid body".to_string())?;

        if let Some(s) = body.as_string() {
            let s = s.to_string().map_err(|e| e.to_string())?;
            os.write_all(s.as_bytes()).map_err(|e| e.to_string())?;
        } else if let Some(buf) = rquickjs::ArrayBuffer::from_value(body.clone()) {
            if let Some(bytes) = buf.as_bytes() {
                os.write_all(bytes).map_err(|e| e.to_string())?;
            }
        } else if let Ok(ta) = rquickjs::TypedArray::<u8>::from_value(body.clone()) {
            if let Some(bytes) = ta.as_bytes() {
                os.write_all(bytes).map_err(|e| e.to_string())?;
            }
        } else if is_json_body {
            // Serialize as JSON
            let json = js_serde::js_to_json(body)?;
            os.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
        }

        drop(os);
        OutgoingBody::finish(ob, None).map_err(|_| "failed to finish body".to_string())?;
    }

    // Register the pollable + future response (non-blocking)
    let pollable = resp.subscribe();
    let handle = future::register(
        pollable,
        PendingOp::Http {
            response: resp,
            url: u.to_string(),
        },
    );
    Ok(handle)
}

#[allow(clippy::needless_pass_by_value)]
pub fn build_response_object<'js>(
    ctx: &Ctx<'js>,
    response: IncomingResponse,
    url: &str,
) -> Result<Object<'js>, String> {
    let resp_obj = Object::new(ctx.clone()).map_err(|e| e.to_string())?;

    // status metadata
    let status = response.status();
    resp_obj.set("status", status).map_err(|e| e.to_string())?;
    resp_obj.set("statusText", "").map_err(|e| e.to_string())?;
    resp_obj.set("url", url).map_err(|e| e.to_string())?;

    // headersList: Array<[name, value]>, preserving duplicates/order
    let hdrs = response.headers();
    let hdr_list = Array::new(ctx.clone()).map_err(|e| e.to_string())?;
    let mut header_index = 0;
    for (k, v) in hdrs.entries() {
        if let Ok(v_str) = std::str::from_utf8(&v) {
            let pair = Array::new(ctx.clone()).map_err(|e| e.to_string())?;
            pair.set(0, k.as_str()).map_err(|e| e.to_string())?;
            pair.set(1, v_str).map_err(|e| e.to_string())?;
            hdr_list
                .set(header_index, pair)
                .map_err(|e| e.to_string())?;
            header_index += 1;
        }
    }
    resp_obj
        .set("headersList", hdr_list)
        .map_err(|e| e.to_string())?;

    // Read body eagerly
    let body = response
        .consume()
        .map_err(|()| "response already read".to_string())?;
    let stream = body
        .stream()
        .map_err(|()| "response already read".to_string())?;

    let mut buf = Vec::new();
    loop {
        match InputStream::read(&stream, 16384) {
            Ok(v) => {
                if !v.is_empty() {
                    if buf.len().saturating_add(v.len()) > MAX_INCOMING_HTTP_BODY_BYTES {
                        return Err(format!(
                            "response body exceeds maximum size of {MAX_INCOMING_HTTP_BODY_BYTES} bytes"
                        ));
                    }
                    buf.extend_from_slice(&v);
                    continue;
                }
                stream.subscribe().block();
            }
            Err(StreamError::Closed) => break,
            Err(StreamError::LastOperationFailed(e)) => {
                return Err(e.to_debug_string());
            }
        }
    }

    // bodyBytes as ArrayBuffer
    let ab = rquickjs::ArrayBuffer::new(ctx.clone(), buf.clone()).map_err(|e| e.to_string())?;
    resp_obj.set("bodyBytes", ab).map_err(|e| e.to_string())?;

    // UTF-8 decoded text hint used by JS Response.text/json.
    let body_text = String::from_utf8_lossy(&buf).into_owned();
    resp_obj
        .set("bodyText", body_text.as_str())
        .map_err(|e| e.to_string())?;

    Ok(resp_obj)
}

fn to_method(method: &str) -> Method {
    match method {
        "GET" => Method::Get,
        "POST" => Method::Post,
        "DELETE" => Method::Delete,
        "HEAD" => Method::Head,
        "PATCH" => Method::Patch,
        "PUT" => Method::Put,
        m => Method::Other(m.to_string()),
    }
}
