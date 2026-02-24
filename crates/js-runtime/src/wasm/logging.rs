use rquickjs::{Ctx, Function, Object};

use super::wasi::logging::logging::{Level, log};

pub fn register(ctx: &Ctx<'_>) {
    let globals = ctx.globals();

    // _isola_logging global
    let logging = Object::new(ctx.clone()).unwrap();

    logging
        .set(
            "debug",
            Function::new(ctx.clone(), |msg: String| {
                let json = format!(r#"{{"message":"{}"}}"#, msg.replace('"', "\\\""));
                log(Level::Debug, "log", &json);
            })
            .unwrap(),
        )
        .unwrap();

    logging
        .set(
            "info",
            Function::new(ctx.clone(), |msg: String| {
                let json = format!(r#"{{"message":"{}"}}"#, msg.replace('"', "\\\""));
                log(Level::Info, "log", &json);
            })
            .unwrap(),
        )
        .unwrap();

    logging
        .set(
            "warn",
            Function::new(ctx.clone(), |msg: String| {
                let json = format!(r#"{{"message":"{}"}}"#, msg.replace('"', "\\\""));
                log(Level::Warn, "log", &json);
            })
            .unwrap(),
        )
        .unwrap();

    logging
        .set(
            "error",
            Function::new(ctx.clone(), |msg: String| {
                let json = format!(r#"{{"message":"{}"}}"#, msg.replace('"', "\\\""));
                log(Level::Error, "log", &json);
            })
            .unwrap(),
        )
        .unwrap();

    globals.set("_isola_logging", logging).unwrap();

    // Also set up console.log/warn/error/debug
    let console = Object::new(ctx.clone()).unwrap();

    console
        .set(
            "log",
            Function::new(ctx.clone(), |args: rquickjs::function::Rest<String>| {
                let msg = args.0.join(" ");
                let json = format!(r#"{{"message":"{}"}}"#, msg.replace('"', "\\\""));
                log(Level::Info, "log", &json);
            })
            .unwrap(),
        )
        .unwrap();

    console
        .set(
            "debug",
            Function::new(ctx.clone(), |args: rquickjs::function::Rest<String>| {
                let msg = args.0.join(" ");
                let json = format!(r#"{{"message":"{}"}}"#, msg.replace('"', "\\\""));
                log(Level::Debug, "log", &json);
            })
            .unwrap(),
        )
        .unwrap();

    console
        .set(
            "warn",
            Function::new(ctx.clone(), |args: rquickjs::function::Rest<String>| {
                let msg = args.0.join(" ");
                let json = format!(r#"{{"message":"{}"}}"#, msg.replace('"', "\\\""));
                log(Level::Warn, "log", &json);
            })
            .unwrap(),
        )
        .unwrap();

    console
        .set(
            "error",
            Function::new(ctx.clone(), |args: rquickjs::function::Rest<String>| {
                let msg = args.0.join(" ");
                let json = format!(r#"{{"message":"{}"}}"#, msg.replace('"', "\\\""));
                log(Level::Error, "log", &json);
            })
            .unwrap(),
        )
        .unwrap();

    globals.set("console", console).unwrap();
}
