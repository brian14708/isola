use std::time::Duration;

use isola_runtime::wasi_http::{HttpRequest, HttpResponse};
use rquickjs::{Array, Ctx, Function, Object, Value, object::Filter};
use url::Url;

use super::future;
use crate::serde as js_serde;

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

    globals.set("_isola_http", http).unwrap();
}

#[expect(clippy::needless_pass_by_value)]
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

#[expect(clippy::needless_pass_by_value)]
fn js_recv(ctx: Ctx<'_>, handle: u32) -> rquickjs::Result<Object<'_>> {
    future::recv_http(&ctx, handle)
}

fn value_to_string(value: &Value<'_>) -> Option<String> {
    value
        .as_string()
        .and_then(|s| s.to_string().ok())
        .or_else(|| value.as_int().map(|v| v.to_string()))
        .or_else(|| value.as_float().map(|v| v.to_string()))
        .or_else(|| value.as_bool().map(|v| v.to_string()))
}

fn append_headers(
    header_fields: &mut Vec<(String, Vec<u8>)>,
    headers: &Value<'_>,
) -> Result<(), String> {
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

            header_fields.push((name.to_ascii_lowercase(), value.into_bytes()));
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
            header_fields.push((k.to_ascii_lowercase(), v.as_bytes().to_vec()));
        }
    }

    Ok(())
}

/// Send an HTTP request (non-blocking). Returns a pollable handle.
#[expect(clippy::needless_pass_by_value)]
fn send_impl(
    method: &str,
    url: &str,
    params: Value<'_>,
    headers: Value<'_>,
    body: Value<'_>,
    timeout: Value<'_>,
) -> Result<u32, String> {
    let mut header_fields = Vec::new();
    append_headers(&mut header_fields, &headers)?;

    // If body is an object (not null/string/arraybuffer), set content-type to JSON
    let is_json_body = body.is_object()
        && !body.is_null()
        && body.as_string().is_none()
        && rquickjs::ArrayBuffer::from_value(body.clone()).is_none();

    if is_json_body && !header_fields.iter().any(|(name, _)| name == "content-type") {
        header_fields.push(("content-type".to_string(), b"application/json".to_vec()));
    }

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

    let timeout_ms = timeout_ms_from_value(&timeout);

    let has_body = !body.is_null() && !body.is_undefined();
    let body = if has_body {
        Some(body_to_bytes(body, is_json_body)?)
    } else {
        None
    };

    Ok(future::register_http(HttpRequest::new(
        method.to_string(),
        u,
        header_fields,
        body,
        timeout_ms,
    )))
}

fn timeout_ms_from_value(timeout: &Value<'_>) -> Option<u64> {
    if timeout.is_null() || timeout.is_undefined() {
        return None;
    }
    let timeout_secs = timeout
        .as_float()
        .or_else(|| timeout.as_int().map(f64::from))?;
    if !timeout_secs.is_finite() || timeout_secs <= 0.0 {
        return None;
    }
    Some(u64::try_from(Duration::from_secs_f64(timeout_secs).as_millis()).unwrap_or(u64::MAX))
}

pub fn build_response_object<'js>(
    ctx: &Ctx<'js>,
    response: HttpResponse,
    url: &str,
) -> Result<Object<'js>, String> {
    let resp_obj = Object::new(ctx.clone()).map_err(|e| e.to_string())?;

    // status metadata
    let status = response.status;
    resp_obj.set("status", status).map_err(|e| e.to_string())?;
    resp_obj.set("statusText", "").map_err(|e| e.to_string())?;
    resp_obj.set("url", url).map_err(|e| e.to_string())?;

    // headersList: Array<[name, value]>, preserving duplicates/order
    let hdr_list = Array::new(ctx.clone()).map_err(|e| e.to_string())?;
    let mut header_index = 0;
    {
        for (k, v) in response.headers {
            if let Ok(v_str) = std::str::from_utf8(&v) {
                let pair = Array::new(ctx.clone()).map_err(|e| e.to_string())?;
                pair.set(0, k).map_err(|e| e.to_string())?;
                pair.set(1, v_str).map_err(|e| e.to_string())?;
                hdr_list
                    .set(header_index, pair)
                    .map_err(|e| e.to_string())?;
                header_index += 1;
            }
        }
    }
    resp_obj
        .set("headersList", hdr_list)
        .map_err(|e| e.to_string())?;

    let buf = response.body;

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

fn body_to_bytes(body: Value<'_>, is_json_body: bool) -> Result<Vec<u8>, String> {
    if let Some(s) = body.as_string() {
        Ok(s.to_string().map_err(|e| e.to_string())?.into_bytes())
    } else if let Some(buf) = rquickjs::ArrayBuffer::from_value(body.clone()) {
        Ok(buf.as_bytes().unwrap_or_default().to_vec())
    } else if let Ok(ta) = rquickjs::TypedArray::<u8>::from_value(body.clone()) {
        Ok(ta.as_bytes().unwrap_or_default().to_vec())
    } else if is_json_body {
        Ok(js_serde::js_to_json(body)?.into_bytes())
    } else {
        Ok(Vec::new())
    }
}
