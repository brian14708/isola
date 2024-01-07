use std::{collections::HashMap, rc::Rc};

use crate::error::{Error, Result};
use rustpython_vm::{
    builtins::{PyBaseExceptionRef, PyDict, PyStr, PyType},
    function::{FuncArgs, KwArgs, PosArgs},
    protocol::{PyIter, PyIterReturn},
    py_serde::{deserialize, PyObjectDeserializer, PyObjectSerializer},
    scope::Scope,
    AsObject, Interpreter, PyObjectRef, VirtualMachine,
};

use serde::de::DeserializeSeed;
use serde_json::{json, Map, Value};

pub struct VM {
    interpreter: Rc<Interpreter>,
}

impl VM {
    pub fn new(f: impl FnOnce(&mut VirtualMachine)) -> Self {
        let interpreter = Interpreter::with_init(Default::default(), |vm| {
            vm.add_native_modules(rustpython_stdlib::get_module_inits());
            vm.add_frozen(rustpython_pylib::FROZEN_STDLIB);
            f(vm);
        });

        Self {
            interpreter: Rc::new(interpreter),
        }
    }

    pub fn script(&self, content: impl AsRef<str>) -> Result<Script> {
        let scope = self.interpreter.enter(|vm| {
            let scope = vm.new_scope_with_builtins();
            vm.run_code_string(scope.clone(), content.as_ref(), "<init>".to_owned())
                .map_err(|e| exception_to_string(vm, &e))?;

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
            .map_err(|e| exception_to_string(vm, &e))
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
                .map_err(|e| exception_to_string(vm, &e))?;
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

                func.invoke(args, vm)
                    .map_err(|e| exception_to_string(vm, &e))?
            } else {
                m
            };

            if PyIter::check(&m) {
                let it = PyIter::new(m);
                let mut buffer = Vec::with_capacity(128);
                loop {
                    match it.next(vm).map_err(|e| exception_to_string(vm, &e))? {
                        PyIterReturn::Return(r) => {
                            serde_json::to_writer(&mut buffer, &PyObjectSerializer::new(vm, &r))
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

    pub fn get_jsonschema(&self, name: &str) -> Result<Value> {
        self.interpreter.enter(|vm| {
            let m = self
                .scope
                .locals
                .as_object()
                .get_item(name, vm)
                .map_err(|e| exception_to_string(vm, &e))?;

            let typing = HashMap::from_iter([
                (vm.ctx.types.int_type.get_id(), "number"),
                (vm.ctx.types.float_type.get_id(), "number"),
                (vm.ctx.types.str_type.get_id(), "string"),
                (vm.ctx.types.none_type.get_id(), "null"),
                (vm.ctx.types.bool_type.get_id(), "boolean"),
                (vm.ctx.types.dict_type.get_id(), "object"),
            ]);

            fn to_value(
                obj: PyObjectRef,
                mapping: &HashMap<usize, &str>,
                vm: &VirtualMachine,
            ) -> Option<(Map<String, Value>, bool)> {
                if let Some(t) = obj.downcast_ref_if_exact::<PyType>(vm) {
                    // basic types
                    if let Some(&v) = mapping.get(&t.get_id()) {
                        return Some((
                            Map::from_iter([("type".into(), Value::String(v.into()))]),
                            true,
                        ));
                    }
                }

                if let Ok(annotations) = obj.get_attr("__annotations__", vm) {
                    // struct
                    let dict = match annotations.downcast_ref_if_exact::<PyDict>(vm) {
                        Some(it) => it,
                        _ => return None,
                    };
                    let mut properties = Map::new();
                    let mut required = Vec::new();
                    let mut field = if let Ok(desc) = obj.get_attr("__field__", vm) {
                        if let Ok(Value::Object(o)) =
                            serde_json::to_value(PyObjectSerializer::new(vm, &desc))
                        {
                            o
                        } else {
                            Map::new()
                        }
                    } else {
                        Map::new()
                    };

                    for (k, v) in dict.into_iter() {
                        let key = k.downcast_ref::<PyStr>().unwrap();
                        let value = v;

                        if let Some((mut v, req)) = to_value(value, mapping, vm) {
                            match field.remove(key.as_str()) {
                                Some(Value::String(desc)) => {
                                    v.insert("description".into(), Value::String(desc));
                                }
                                Some(Value::Object(desc)) => {
                                    for (k, vv) in desc {
                                        v.insert(k, vv);
                                    }
                                }
                                _ => {}
                            }
                            properties.insert(key.as_str().into(), Value::Object(v));
                            if req {
                                required.push(Value::String(key.as_str().into()));
                            }
                        }
                    }
                    let mut schema = Map::from_iter([
                        ("type".into(), Value::String("object".into())),
                        ("properties".into(), Value::Object(properties)),
                        ("required".into(), Value::Array(required)),
                    ]);

                    if let Some(name) = obj
                        .get_attr("__name__", vm)
                        .ok()
                        .and_then(|e| e.downcast_exact::<PyStr>(vm).ok())
                    {
                        schema.insert("title".into(), Value::String(name.as_str().into()));
                    };

                    if let Some(s) = obj
                        .get_attr("__doc__", vm)
                        .ok()
                        .and_then(|e| e.downcast_exact::<PyStr>(vm).ok())
                    {
                        schema.insert("description".into(), Value::String(s.as_str().into()));
                    }
                    Some((schema, true))
                } else if let Ok(args) = obj.get_attr("__args__", vm) {
                    let it = if let Ok(it) = args.get_iter(vm) {
                        it
                    } else {
                        return None;
                    };
                    #[derive(PartialEq, Eq)]
                    enum Type {
                        Array,
                        Object,
                        Union,
                        Generator,
                    }
                    let origin = {
                        let origin = obj.get_attr("__origin__", vm);
                        if let Ok(origin) = origin {
                            if origin.is(vm.ctx.types.list_type) {
                                Type::Array
                            } else if origin.is(vm.ctx.types.dict_type) {
                                Type::Object
                            } else if let Some(name) = origin
                                .get_attr("__name__", vm)
                                .ok()
                                .and_then(|e| e.downcast_exact::<PyStr>(vm).ok())
                            {
                                match name.as_str() {
                                    "Union" | "Literal" => Type::Union,
                                    "Generator" => Type::Generator,
                                    _ => return None,
                                }
                            } else {
                                return None;
                            }
                        } else {
                            Type::Union
                        }
                    };

                    match origin {
                        Type::Array => {
                            // array
                            let mut schema = Map::new();
                            if let Ok(PyIterReturn::Return(t)) = it.next(vm) {
                                let value = t;
                                if let Some((v, _)) = to_value(value, mapping, vm) {
                                    schema.insert("items".into(), Value::Object(v));
                                }
                            }
                            return Some((schema, true));
                        }
                        Type::Object => {
                            // dict
                            let mut schema =
                                Map::from_iter([("type".into(), Value::String("object".into()))]);
                            if let Ok(PyIterReturn::Return(t)) = it.next(vm) {
                                if t.get_id() != vm.ctx.types.str_type.get_id() {
                                    return None;
                                }
                            }
                            if let Ok(PyIterReturn::Return(t)) = it.next(vm) {
                                // get last
                                let value = t;
                                if let Some((v, _)) = to_value(value, mapping, vm) {
                                    schema.insert("additionalProperties".into(), Value::Object(v));
                                }
                            }
                            return Some((schema, true));
                        }
                        Type::Union => {
                            // union
                            let mut oneof = Vec::new();
                            let mut required = true;
                            while let Ok(PyIterReturn::Return(t)) = it.next(vm) {
                                let value = t;
                                if value.get_id() == vm.ctx.types.none_type.get_id() {
                                    required = false;
                                    continue;
                                }
                                if let Some((v, _)) = to_value(value, mapping, vm) {
                                    oneof.push(Value::Object(v));
                                }
                            }
                            if oneof.len() == 1 {
                                match oneof.remove(0) {
                                    Value::Object(mut o) => {
                                        if !required {
                                            o["type"] = Value::Array(vec![
                                                Value::String("null".into()),
                                                o["type"].clone(),
                                            ]);
                                        }
                                        return Some((o, required));
                                    }
                                    _ => unreachable!(),
                                }
                            }
                            Some((
                                Map::from_iter([("oneOf".into(), Value::Array(oneof))]),
                                required,
                            ))
                        }
                        Type::Generator => {
                            let mut schema = Map::new();
                            if let Ok(PyIterReturn::Return(t)) = it.next(vm) {
                                let value = t;
                                if let Some((v, _)) = to_value(value, mapping, vm) {
                                    schema.insert("items".into(), Value::Object(v));
                                    schema.insert("generator".into(), Value::Bool(true));
                                }
                            }
                            Some((schema, true))
                        }
                    }
                } else {
                    serde_json::to_value(PyObjectSerializer::new(vm, &obj))
                        .ok()
                        .map(|v| (Map::from_iter([("const".into(), v)]), true))
                }
            }

            Ok(if let Some((schema, _)) = to_value(m, &typing, vm) {
                Value::Object(schema)
            } else {
                json!({
                    "type": "object",
                })
            })
        })
    }
}

fn exception_to_string(vm: &VirtualMachine, e: &PyBaseExceptionRef) -> Error {
    let mut buffer = String::new();
    if vm.write_exception(&mut buffer, e).is_ok() {
        Error::PythonError(buffer)
    } else {
        Error::UnexpectedError("fail to write exception")
    }
}

#[cfg(test)]
mod tests {

    use jsonschema::JSONSchema;
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
        let vm = VM::new(|_| {});
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

    #[test]
    fn typing() {
        let content = r#"
import typing, json

class Nested:
    data: str

class Data(typing.TypedDict):
    __field__ = {
        's': 'THIS IS A TEST',
        'i': {
            'title': 'int',
        }
    }

    s: str
    i: int
    f: float
    b: bool
    d: dict[str, int]
    o: typing.Optional[str]
    nested: Nested | list[str] | str
    literal: typing.Literal[123, 'abc']

Generator = typing.Generator[int, None, None]
"#;
        let vm = VM::new(|_| {});
        let s = vm.script(content).unwrap();
        let x = s.get_jsonschema("Data").unwrap();
        JSONSchema::compile(&x).unwrap();
        assert_eq!(
            x,
            json!({
              "properties": {
                "b": {
                  "type": "boolean"
                },
                "d": {
                  "additionalProperties": {
                    "type": "number"
                  },
                  "type": "object"
                },
                "f": {
                  "type": "number"
                },
                "i": {
                  "title": "int",
                  "type": "number"
                },
                "literal": {
                  "oneOf": [
                    {
                      "const": 123
                    },
                    {
                      "const": "abc"
                    }
                  ]
                },
                "nested": {
                  "oneOf": [
                    {
                      "properties": {
                        "data": {
                          "type": "string"
                        }
                      },
                      "required": [
                        "data"
                      ],
                      "title": "Nested",
                      "type": "object"
                    },
                    {
                      "items": {
                        "type": "string"
                      }
                    },
                    {
                      "type": "string"
                    }
                  ]
                },
                "o": {
                  "type": ["null", "string"]
                },
                "s": {
                  "description": "THIS IS A TEST",
                  "type": "string"
                }
              },
              "required": [
                "s",
                "i",
                "f",
                "b",
                "d",
                "nested",
                "literal"
              ],
              "title": "Data",
              "type": "object"
            })
        );

        let x = s.get_jsonschema("Generator").unwrap();
        JSONSchema::compile(&x).unwrap();
    }
}
