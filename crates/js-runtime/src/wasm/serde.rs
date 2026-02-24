use rquickjs::{Ctx, Function, Object, Value};

use crate::serde as js_serde;

pub fn register(ctx: &Ctx<'_>) {
    let globals = ctx.globals();

    let serde_mod = Object::new(ctx.clone()).unwrap();

    // _isola_serde.dumps(value, format) -> string|ArrayBuffer
    serde_mod
        .set("dumps", Function::new(ctx.clone(), js_serde_dumps).unwrap())
        .unwrap();

    // _isola_serde.loads(data, format) -> value
    serde_mod
        .set("loads", Function::new(ctx.clone(), js_serde_loads).unwrap())
        .unwrap();

    globals.set("_isola_serde", serde_mod).unwrap();
}

#[allow(clippy::needless_pass_by_value)]
fn js_serde_dumps<'js>(
    ctx: Ctx<'js>,
    value: Value<'js>,
    format: String,
) -> rquickjs::Result<Value<'js>> {
    match format.as_str() {
        "json" => {
            let json_str = js_serde::js_to_json(value)
                .map_err(|e| rquickjs::Error::new_from_js_message("value", "json", &e))?;
            rquickjs::String::from_str(ctx, &json_str).map(rquickjs::String::into_value)
        }
        "yaml" => {
            let yaml_str = js_serde::js_to_yaml(value)
                .map_err(|e| rquickjs::Error::new_from_js_message("value", "yaml", &e))?;
            rquickjs::String::from_str(ctx, &yaml_str).map(rquickjs::String::into_value)
        }
        "cbor" => {
            let cbor_bytes = js_serde::js_to_cbor(value)
                .map_err(|e| rquickjs::Error::new_from_js_message("value", "cbor", &e))?;
            rquickjs::ArrayBuffer::new(ctx, cbor_bytes).map(rquickjs::ArrayBuffer::into_value)
        }
        _ => Err(rquickjs::Error::new_from_js_message(
            "format",
            "string",
            "Unsupported format. Use 'json', 'yaml', or 'cbor'.",
        )),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn js_serde_loads<'js>(
    ctx: Ctx<'js>,
    data: Value<'js>,
    format: String,
) -> rquickjs::Result<Value<'js>> {
    match format.as_str() {
        "json" => {
            let s = data
                .as_string()
                .ok_or_else(|| {
                    rquickjs::Error::new_from_js_message(
                        "data",
                        "string",
                        "JSON format requires string input",
                    )
                })?
                .to_string()?;
            js_serde::json_to_js(&ctx, &s)
                .map_err(|e| rquickjs::Error::new_from_js_message("json", "value", &e))
        }
        "yaml" => {
            let s = data
                .as_string()
                .ok_or_else(|| {
                    rquickjs::Error::new_from_js_message(
                        "data",
                        "string",
                        "YAML format requires string input",
                    )
                })?
                .to_string()?;
            js_serde::yaml_to_js(&ctx, &s)
                .map_err(|e| rquickjs::Error::new_from_js_message("yaml", "value", &e))
        }
        "cbor" => {
            if let Some(buf) = rquickjs::ArrayBuffer::from_value(data.clone())
                && let Some(bytes) = buf.as_bytes()
            {
                return js_serde::cbor_to_js(&ctx, bytes)
                    .map_err(|e| rquickjs::Error::new_from_js_message("cbor", "value", &e));
            }
            if let Ok(ta) = rquickjs::TypedArray::<u8>::from_value(data)
                && let Some(bytes) = ta.as_bytes()
            {
                return js_serde::cbor_to_js(&ctx, bytes)
                    .map_err(|e| rquickjs::Error::new_from_js_message("cbor", "value", &e));
            }
            Err(rquickjs::Error::new_from_js_message(
                "data",
                "ArrayBuffer",
                "CBOR format requires ArrayBuffer or Uint8Array input",
            ))
        }
        _ => Err(rquickjs::Error::new_from_js_message(
            "format",
            "string",
            "Unsupported format. Use 'json', 'yaml', or 'cbor'.",
        )),
    }
}
