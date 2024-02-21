use pyo3::{
    prelude::*,
    prepare_freethreaded_python,
    types::{PyDict, PyTuple},
};
use serde::de::DeserializeSeed;

use crate::error::{Error, Result};
use crate::serde::{PyObjectDeserializer, PyObjectSerializer};

pub struct Scope {
    locals: PyObject,
}
pub enum InputValue<'a> {
    #[allow(dead_code)]
    Json(serde_json::Value),
    JsonStr(&'a str),
}

impl Scope {
    pub fn new() -> Self {
        prepare_freethreaded_python();
        Python::with_gil(|py| {
            let locals = PyDict::new(py);
            Scope {
                locals: locals.to_object(py),
            }
        })
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
                    positional.into_iter().map(|v| match v {
                        InputValue::Json(v) => {
                            PyObjectDeserializer::new(py).deserialize(v).unwrap()
                        }
                        InputValue::JsonStr(v) => PyObjectDeserializer::new(py)
                            .deserialize(&mut serde_json::Deserializer::from_str(v))
                            .unwrap(),
                    }),
                );
                let kwargs = PyDict::new(py);
                for (k, v) in named {
                    match v {
                        InputValue::Json(v) => {
                            kwargs
                                .set_item(k, PyObjectDeserializer::new(py).deserialize(v).unwrap())
                                .map_err(|e| Error::from_pyerr(py, e))?;
                        }
                        InputValue::JsonStr(v) => {
                            kwargs
                                .set_item(
                                    k,
                                    PyObjectDeserializer::new(py)
                                        .deserialize(&mut serde_json::Deserializer::from_str(v))
                                        .unwrap(),
                                )
                                .map_err(|e| Error::from_pyerr(py, e))?;
                        }
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
                        &serde_json::to_string(&PyObjectSerializer::new(py, el.unwrap())).unwrap(),
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
