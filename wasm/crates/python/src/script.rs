use std::borrow::Cow;

use cbor4ii::core::utils::SliceReader;
use pyo3::{
    exceptions::PyNameError,
    intern,
    prelude::*,
    prepare_freethreaded_python,
    types::{
        PyBool, PyByteArray, PyBytes, PyDict, PyFloat, PyInt, PyList, PyMemoryView, PyString,
        PyTuple,
    },
    PyTypeInfo,
};
use serde::de::DeserializeSeed;

use crate::{
    error::{Error, Result},
    serde::{PyObjectDeserializer, PyObjectSerializer},
    wasm::ArgIter,
};

pub struct Scope {
    locals: PyObject,
    stdio: Option<(PyObject, PyObject)>,
}

#[allow(dead_code)]
pub enum InputValue<'a> {
    Json(serde_json::Value),
    JsonStr(Cow<'a, str>),
    Cbor(Cow<'a, [u8]>),
    Iter(ArgIter),
}

impl Scope {
    pub fn new() -> Self {
        prepare_freethreaded_python();
        Python::with_gil(|py| {
            let locals = PyDict::new_bound(py);
            let stdio = if let Ok(sys) = PyModule::import_bound(py, intern!(py, "sys")) {
                if let Ok(path) = sys.getattr(intern!(py, "path")) {
                    let path = path.downcast_exact::<PyList>().ok();
                    if let Some(path) = path {
                        let _ = path.insert(1, "/usr/local/lib/bundle.zip");
                    }
                }
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

    pub fn load_zip(&mut self, p: &str, entrypoint: &str) -> crate::error::Result<()> {
        Python::with_gil(|py| {
            let sys = PyModule::import_bound(py, intern!(py, "sys"))
                .map_err(|e| Error::from_pyerr(py, e))?;
            let binding = sys
                .getattr(intern!(py, "path"))
                .map_err(|e| Error::from_pyerr(py, e))?;
            let path = binding
                .downcast_exact::<PyList>()
                .map_err(|e| Error::from_pyerr(py, e))?;
            path.insert(0, p).map_err(|e| Error::from_pyerr(py, e))?;
            let module = py
                .import_bound(PyString::new_bound(py, entrypoint))
                .map_err(|e| Error::from_pyerr(py, e))?;
            self.locals = module.dict().to_object(py);
            Ok(())
        })
    }

    pub fn load_script(&self, code: &str) -> crate::error::Result<()> {
        Python::with_gil(|py| {
            py.run_bound(
                code,
                Some(
                    self.locals
                        .downcast_bound(py)
                        .map_err(|e| Error::from_pyerr(py, e))?,
                ),
                None,
            )
            .map_err(|e| Error::from_pyerr(py, e))?;
            Ok(())
        })
    }

    fn is_serializable(pyobject: &Bound<'_, PyAny>) -> bool {
        pyobject.is_none()
            || PyDict::is_exact_type_of_bound(pyobject)
            || PyList::is_exact_type_of_bound(pyobject)
            || PyTuple::is_exact_type_of_bound(pyobject)
            || PyString::is_exact_type_of_bound(pyobject)
            || PyBool::is_exact_type_of_bound(pyobject)
            || PyInt::is_exact_type_of_bound(pyobject)
            || PyFloat::is_exact_type_of_bound(pyobject)
            || PyBytes::is_exact_type_of_bound(pyobject)
            || PyByteArray::is_exact_type_of_bound(pyobject)
            || PyMemoryView::is_exact_type_of_bound(pyobject)
    }

    pub fn run<'a, U>(
        &self,
        name: &str,
        positional: impl IntoIterator<Item = InputValue<'a>, IntoIter = U>,
        named: impl IntoIterator<Item = (&'a str, InputValue<'a>)>,
        mut callback: impl FnMut(&[u8]),
    ) -> Result<Option<Vec<u8>>>
    where
        U: ExactSizeIterator<Item = InputValue<'a>>,
    {
        Python::with_gil(|py| {
            let dict: &Bound<'_, PyDict> = self
                .locals
                .downcast_bound(py)
                .map_err(|e| Error::from_pyerr(py, e))?;
            let Some(f) = dict.get_item(name).map_err(|e| Error::from_pyerr(py, e))? else {
                return Err(Error::from_pyerr(
                    py,
                    PyNameError::new_err(format!("name '{name}' is not defined")),
                ));
            };

            let obj = if f.is_callable() {
                let args = PyTuple::new_bound(
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
                            InputValue::Cbor(v) => {
                                let c = SliceReader::new(v.as_ref());
                                Ok(PyObjectDeserializer::new(py)
                                    .deserialize(&mut cbor4ii::serde::Deserializer::new(c))
                                    .map_err(|_| Error::UnexpectedError("serde error"))?)
                            }
                        })
                        .collect::<Result<Vec<_>>>()?,
                );
                let kwargs = PyDict::new_bound(py);
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
                        InputValue::Cbor(v) => {
                            let c = SliceReader::new(v.as_ref());
                            kwargs
                                .set_item(
                                    k,
                                    PyObjectDeserializer::new(py)
                                        .deserialize(&mut cbor4ii::serde::Deserializer::new(c))
                                        .map_err(|_| Error::UnexpectedError("serde error"))?,
                                )
                                .map_err(|e| Error::from_pyerr(py, e))?;
                        }
                        InputValue::Iter(it) => kwargs
                            .set_item(k, it.into_py(py))
                            .map_err(|e| Error::from_pyerr(py, e))?,
                    }
                }
                f.as_borrowed()
                    .call(args, Some(&kwargs))
                    .map_err(|e| Error::from_pyerr(py, e))?
            } else {
                f
            };

            let obj = if obj.hasattr("__await__").unwrap_or_default()
                || obj.hasattr("__aiter__").unwrap_or_default()
            {
                let module = py
                    .import_bound(intern!(py, "promptkit.asyncio"))
                    .expect("failed to import promptkit.asyncio");
                module
                    .getattr(intern!(py, "run"))
                    .expect("failed to get asyncio.run")
                    .call1((obj,))
                    .map_err(|e| Error::from_pyerr(py, e))?
            } else {
                obj
            };

            if Self::is_serializable(&obj) {
                match PyObjectSerializer::to_cbor(vec![], obj.clone()) {
                    Ok(s) => return Ok(Some(s)),
                    Err(cbor4ii::serde::EncodeError::Core(_)) => {
                        return Err(Error::UnexpectedError("serde error"))
                    }
                    Err(cbor4ii::serde::EncodeError::Custom(s)) => {
                        return Err(Error::PythonError {
                            cause: s.to_string(),
                            traceback: None,
                        });
                    }
                };
            }

            let mut ret: Option<Vec<u8>> = None;
            if let Ok(iter) = obj.iter() {
                for el in iter {
                    let mut tmp = PyObjectSerializer::to_cbor(
                        std::mem::take(&mut ret).unwrap_or_default(),
                        el.map_err(|e| Error::from_pyerr(py, e))?,
                    )
                    .map_err(|_| Error::UnexpectedError("serde error"))?;
                    callback(&tmp);
                    tmp.clear();
                    ret = Some(tmp);
                }
                return Ok(None);
            }

            Err(Error::UnexpectedError("unsupported return type"))
        })
    }

    pub fn analyze(&self, request: InputValue<'_>) -> Result<Option<Vec<u8>>> {
        Python::with_gil(|py| {
            let module = py
                .import_bound(intern!(py, "promptkit._analyze"))
                .expect("failed to import promptkit._analyze");
            let dict: &Bound<'_, PyDict> = self
                .locals
                .downcast_bound(py)
                .map_err(|e| Error::from_pyerr(py, e))?;
            let obj = module
                .getattr(intern!(py, "analyze"))
                .expect("failed to get analyze")
                .call(
                    (
                        dict,
                        match request {
                            InputValue::Json(v) => PyObjectDeserializer::new(py)
                                .deserialize(v)
                                .map_err(|_| Error::UnexpectedError("serde error"))?,
                            InputValue::JsonStr(v) => PyObjectDeserializer::new(py)
                                .deserialize(&mut serde_json::Deserializer::from_str(&v))
                                .map_err(|_| Error::UnexpectedError("serde error"))?,
                            InputValue::Iter(_) => {
                                return Err(Error::UnexpectedError("unsupported"))
                            }
                            InputValue::Cbor(v) => {
                                let c = SliceReader::new(v.as_ref());
                                PyObjectDeserializer::new(py)
                                    .deserialize(&mut cbor4ii::serde::Deserializer::new(c))
                                    .map_err(|_| Error::UnexpectedError("serde error"))?
                            }
                        },
                    ),
                    None,
                )
                .map_err(|e| Error::from_pyerr(py, e))?;
            match PyObjectSerializer::to_cbor(vec![], obj.clone()) {
                Ok(s) => Ok(Some(s)),
                Err(cbor4ii::serde::EncodeError::Core(_)) => {
                    Err(Error::UnexpectedError("serde error"))
                }
                Err(cbor4ii::serde::EncodeError::Custom(s)) => Err(Error::PythonError {
                    cause: s.to_string(),
                    traceback: None,
                }),
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
        let s = Scope::new();
        s.load_script(content).unwrap();
        let x = s
            .run("hello", [InputValue::Json(json!(32))], [], |_| {})
            .unwrap();
        assert_eq!(
            x.unwrap(),
            cbor4ii::serde::to_vec(vec![], &"hello 54").unwrap()
        );

        let x = s.run("i", [], [], |_| {}).unwrap();
        assert_eq!(x.unwrap(), cbor4ii::serde::to_vec(vec![], &22).unwrap());

        let mut v = vec![];
        let x = s.run("gen", [], [], |s| v.push(s.to_owned())).unwrap();
        assert_eq!(x, None);
        for i in 0..10 {
            assert_eq!(v[i], cbor4ii::serde::to_vec(vec![], &i).unwrap(),);
        }
    }
}
