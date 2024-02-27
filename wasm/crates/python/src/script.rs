use std::borrow::Cow;

use pyo3::{
    intern,
    prelude::*,
    prepare_freethreaded_python,
    types::{PyDict, PyTuple},
};
use serde::de::DeserializeSeed;

use crate::serde::{PyObjectDeserializer, PyObjectSerializer};
use crate::{
    error::{Error, Result},
    wasm::ArgIter,
};

pub struct Scope {
    locals: PyObject,
    stdio: Option<(PyObject, PyObject)>,
}
pub enum InputValue<'a> {
    #[allow(dead_code)]
    Json(serde_json::Value),
    JsonStr(Cow<'a, str>),
    Iter(ArgIter),
}

impl Scope {
    pub fn new() -> Self {
        prepare_freethreaded_python();
        Python::with_gil(|py| {
            let locals = PyDict::new(py);
            let stdio = if let Ok(sys) = PyModule::import(py, intern!(py, "sys")) {
                if let (Ok(stdout), Ok(stderr)) = (
                    sys.getattr(intern!(py, "stdout")),
                    sys.getattr(intern!(py, "stderr")),
                ) {
                    Some((stdout.to_object(py), stderr.to_object(py)))
                } else {
                    None
                }
            } else {
                None
            };

            Scope {
                locals: locals.to_object(py),
                stdio,
            }
        })
    }

    pub fn flush(&self) {
        let _ = Python::with_gil(|py| {
            if let Some((stdout, stderr)) = &self.stdio {
                let flush = intern!(py, "flush");
                stdout.call_method0(py, flush)?;
                stderr.call_method0(py, flush)?;
            }
            Ok::<_, PyErr>(())
        });
    }

    pub fn load_script(&self, code: &str) -> crate::error::Result<()> {
        Python::with_gil(|py| {
            py.run(
                code,
                Some(
                    self.locals
                        .downcast(py)
                        .map_err(|e| Error::from_pyerr(py, e))?,
                ),
                None,
            )
            .map_err(|e| Error::from_pyerr(py, e))?;
            Ok(())
        })
    }

    pub fn run<'a, U>(
        &self,
        name: &str,
        positional: impl IntoIterator<Item = InputValue<'a>, IntoIter = U>,
        named: impl IntoIterator<Item = (&'a str, InputValue<'a>)>,
        mut callback: impl FnMut(&str),
    ) -> Result<Option<String>>
    where
        U: ExactSizeIterator<Item = InputValue<'a>>,
    {
        Python::with_gil(|py| {
            let dict: &PyDict = self
                .locals
                .downcast(py)
                .map_err(|e| Error::from_pyerr(py, e))?;
            let Some(f) = dict.get_item(name).map_err(|e| Error::from_pyerr(py, e))? else {
                return Ok(None);
            };

            let obj = if f.is_callable() {
                let args = PyTuple::new(
                    py,
                    positional
                        .into_iter()
                        .map(|v| match v {
                            InputValue::Json(v) => Ok(PyObjectDeserializer::new(py)
                                .deserialize(v)
                                .map_err(|_| Error::UnexpectedError("serde error"))?),
                            InputValue::JsonStr(v) => Ok(PyObjectDeserializer::new(py)
                                .deserialize(&mut serde_json::Deserializer::from_str(&v))
                                .map_err(|_| Error::UnexpectedError("serde error"))?),
                            InputValue::Iter(it) => Ok(it.into_py(py)),
                        })
                        .collect::<Result<Vec<_>>>()?,
                );
                let kwargs = PyDict::new(py);
                for (k, v) in named {
                    match v {
                        InputValue::Json(v) => {
                            kwargs
                                .set_item(
                                    k,
                                    PyObjectDeserializer::new(py)
                                        .deserialize(v)
                                        .map_err(|_| Error::UnexpectedError("serde error"))?,
                                )
                                .map_err(|e| Error::from_pyerr(py, e))?;
                        }
                        InputValue::JsonStr(v) => {
                            kwargs
                                .set_item(
                                    k,
                                    PyObjectDeserializer::new(py)
                                        .deserialize(&mut serde_json::Deserializer::from_str(&v))
                                        .map_err(|_| Error::UnexpectedError("serde error"))?,
                                )
                                .map_err(|e| Error::from_pyerr(py, e))?;
                        }
                        InputValue::Iter(it) => kwargs
                            .set_item(k, it.into_py(py))
                            .map_err(|e| Error::from_pyerr(py, e))?,
                    }
                }
                f.call(args, Some(kwargs))
                    .map_err(|e| Error::from_pyerr(py, e))?
            } else {
                f
            };

            if let Ok(s) = serde_json::to_string(&PyObjectSerializer::new(py, obj)) {
                return Ok(Some(s));
            }

            if let Ok(iter) = obj.iter() {
                for el in iter {
                    callback(
                        &serde_json::to_string(&PyObjectSerializer::new(
                            py,
                            el.map_err(|e| Error::from_pyerr(py, e))?,
                        ))
                        .map_err(|_| Error::UnexpectedError("serde error"))?,
                    );
                }
                return Ok(None);
            }

            Err(Error::UnexpectedError("unsupported return type"))
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
        let s = Scope::new();
        s.load_script(content).unwrap();
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
