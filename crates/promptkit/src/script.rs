use crate::{
    error::Result,
    stream::{make_module, BlockingRecv, Stream},
};
use rustpython_vm::{
    class::PyClassImpl,
    compiler::{compile, CompileOpts, Mode},
    convert::ToPyObject,
    function::{FuncArgs, KwArgs, PosArgs},
    py_serde::{deserialize, PyObjectSerializer},
    scope::Scope,
    AsObject, Interpreter, PyPayload,
};

use serde_json::Value;

pub struct Script {
    interpreter: Interpreter,
    scope: Scope,
}

pub enum InputValue {
    Json(Value),
    Stream(Box<dyn BlockingRecv>),
}

impl Script {
    pub fn new(content: impl AsRef<str>) -> Result<Self> {
        let code = compile(
            content.as_ref(),
            Mode::Exec,
            "<embedded>".to_owned(),
            CompileOpts {
                ..Default::default()
            },
        )
        .unwrap();

        let interpreter = Interpreter::with_init(Default::default(), |vm| {
            vm.add_native_modules(rustpython_vm::stdlib::get_module_inits());
            vm.add_native_modules(rustpython_stdlib::get_module_inits());
            vm.add_frozen(rustpython_pylib::FROZEN_STDLIB);

            vm.add_native_module("_stream".to_owned(), Box::new(make_module));

            Stream::make_class(&vm.ctx);
        });

        let scope = interpreter.enter(|vm| {
            let scope = vm.new_scope_with_builtins();
            let code = vm.ctx.new_code(code.clone());
            vm.run_code_obj(code, scope.clone())
                .map_err(|e| {
                    vm.print_exception(e.clone());
                    e
                })
                .unwrap();

            scope
        });

        Ok(Self { interpreter, scope })
    }

    pub fn run<'a>(
        &self,
        name: &str,
        positional: impl IntoIterator<Item = InputValue>,
        named: impl IntoIterator<Item = (&'a str, InputValue)>,
    ) -> Result<Value> {
        self.interpreter.enter(|vm| {
            let m = self
                .scope
                .locals
                .as_object()
                .get_item(name, vm)
                .map_err(|e| {
                    vm.print_exception(e.clone());
                    e
                })?;
            let m = if let Some(func) = m.to_callable() {
                let args = FuncArgs::new(
                    PosArgs::from(
                        positional
                            .into_iter()
                            .map(|arg| match arg {
                                InputValue::Json(v) => deserialize(vm, v).unwrap(),
                                InputValue::Stream(s) => vm
                                    .ctx
                                    .new_pyref::<_, Stream>(Stream { stream: s })
                                    .to_pyobject(vm),
                            })
                            .collect::<Vec<_>>(),
                    ),
                    KwArgs::from_iter(named.into_iter().map(|(k, v)| match v {
                        InputValue::Json(v) => (k.to_owned(), deserialize(vm, v).unwrap()),
                        InputValue::Stream(s) => {
                            (k.to_owned(), Stream { stream: s }.into_pyobject(vm))
                        }
                    })),
                );

                func.invoke(args, vm).map_err(|e| {
                    vm.print_exception(e.clone());
                    e
                })?
            } else {
                m
            };

            Ok(serde_json::to_value(PyObjectSerializer::new(vm, &m)).unwrap())
        })
    }
}

#[cfg(test)]
mod tests {

    use serde_json::json;
    use tokio::sync::mpsc::channel;

    use super::*;

    #[tokio::test]
    async fn test() {
        let content = r#"
i = 1
def hello(n):
    n += i
    return f"hello {n}"

def sum(i):
    total = 0
    for x in i:
        total += x
    return total
i += 21
"#;
        let s = Script::new(content).unwrap();
        let x = s.run("hello", [InputValue::Json(json!(32))], []).unwrap();
        assert_eq!(x, json!("hello 54"));

        let x = s.run("i", [], []).unwrap();
        assert_eq!(x, json!(22));

        let (tx, rx) = channel::<Value>(12);
        tx.send(json!(1)).await.unwrap();
        tx.send(json!(2)).await.unwrap();
        drop(tx);

        let tot =
            tokio::task::spawn_blocking(move || s.run("sum", [InputValue::Stream(rx.into())], []))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(tot, json!(3));
    }
}
