use std::rc::Rc;

use crate::error::Result;
use rustpython_vm::{
    function::{FuncArgs, KwArgs, PosArgs},
    protocol::{PyIter, PyIterReturn},
    py_serde::{deserialize, PyObjectDeserializer, PyObjectSerializer},
    scope::Scope,
    AsObject, Interpreter,
};

use serde::de::DeserializeSeed;
use serde_json::Value;

pub struct VM {
    interpreter: Rc<Interpreter>,
}

impl Default for VM {
    fn default() -> Self {
        Self::new()
    }
}

impl VM {
    pub fn new() -> Self {
        let interpreter = Interpreter::with_init(Default::default(), |vm| {
            vm.add_native_modules(rustpython_stdlib::get_module_inits());
            vm.add_frozen(rustpython_pylib::FROZEN_STDLIB);
        });

        Self {
            interpreter: Rc::new(interpreter),
        }
    }

    pub fn script(&self, content: impl AsRef<str>) -> Result<Script> {
        let scope = self.interpreter.enter(|vm| {
            let scope = vm.new_scope_with_builtins();
            vm.run_code_string(scope.clone(), content.as_ref(), "<init>".to_owned())
                .map_err(|e| {
                    vm.print_exception(e.clone());
                    e
                })?;

            Result::<Scope>::Ok(scope)
        })?;

        Ok(Script {
            interpreter: self.interpreter.clone(),
            scope,
        })
    }
}

pub struct Script {
    interpreter: Rc<Interpreter>,
    scope: Scope,
}

pub enum InputValue<'a> {
    Json(Value),
    JsonStr(&'a str),
}

impl Script {
    pub fn load_script(&self, content: impl AsRef<str>) -> Result<()> {
        self.interpreter.enter(|vm| {
            vm.run_code_string(
                self.scope.clone(),
                content.as_ref(),
                "<embedded>".to_owned(),
            )
            .map_err(|e| {
                vm.print_exception(e.clone());
                e
            })
        })?;

        Ok(())
    }

    pub fn run<'a>(
        &self,
        name: &str,
        positional: impl IntoIterator<Item = InputValue<'a>>,
        named: impl IntoIterator<Item = (&'a str, InputValue<'a>)>,
        mut callback: impl FnMut(&str),
    ) -> Result<Option<String>> {
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
                                InputValue::JsonStr(s) => PyObjectDeserializer::new(vm)
                                    .deserialize(&mut serde_json::Deserializer::from_str(s))
                                    .unwrap(),
                            })
                            .collect::<Vec<_>>(),
                    ),
                    KwArgs::from_iter(named.into_iter().map(|(k, v)| {
                        match v {
                            InputValue::Json(v) => (k.to_owned(), deserialize(vm, v).unwrap()),
                            InputValue::JsonStr(s) => (
                                k.to_owned(),
                                PyObjectDeserializer::new(vm)
                                    .deserialize(&mut serde_json::Deserializer::from_str(s))
                                    .unwrap(),
                            ),
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

            if PyIter::check(&m) {
                let it = PyIter::new(m);
                let mut buffer = Vec::with_capacity(128);
                loop {
                    match it.next(vm)? {
                        PyIterReturn::Return(r) => {
                            let cursor = std::io::Cursor::new(&mut buffer);
                            serde_json::to_writer(cursor, &PyObjectSerializer::new(vm, &r))
                                .unwrap();
                            // SAFETY: buffer is always valid utf8
                            callback(unsafe { std::str::from_utf8_unchecked(&buffer) });
                            buffer.clear();
                        }
                        PyIterReturn::StopIteration(r) => {
                            return Ok(r.map(|r| {
                                serde_json::to_string(&PyObjectSerializer::new(vm, &r)).unwrap()
                            }))
                        }
                    }
                }
            } else {
                Ok(Some(
                    serde_json::to_string(&PyObjectSerializer::new(vm, &m)).unwrap(),
                ))
            }
        })
    }
}

#[cfg(test)]
mod tests {

    use serde_json::json;

    use super::*;

    #[test]
    fn test() {
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

def gen():
    for i in range(10):
        yield i
"#;
        let vm = VM::new();
        let s = vm.script(content).unwrap();
        let x = s
            .run("hello", [InputValue::Json(json!(32))], [], |_| {})
            .unwrap();
        assert_eq!(x.unwrap(), "\"hello 54\"");

        let x = s.run("i", [], [], |_| {}).unwrap();
        assert_eq!(x.unwrap(), "22");

        let mut v = vec![];
        let x = s.run("gen", [], [], |s| v.push(s.to_owned())).unwrap();
        assert_eq!(x, None);
        assert_eq!(v, vec!["0", "1", "2", "3", "4", "5", "6", "7", "8", "9",]);
    }
}
