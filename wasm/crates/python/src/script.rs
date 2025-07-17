use std::borrow::Cow;

use pyo3::{
    PyTypeInfo,
    exceptions::PyNameError,
    intern,
    prelude::*,
    prepare_freethreaded_python,
    sync::GILOnceCell,
    types::{
        PyBool, PyByteArray, PyBytes, PyDict, PyFloat, PyInt, PyList, PyMemoryView, PyString,
        PyTuple,
    },
};

use crate::{
    error::{Error, Result},
    pymeta,
    serde::{cbor_to_python, python_to_cbor},
    wasm::ArgIter,
};

pub struct Scope {
    locals: PyObject,
    stdio: Option<(PyObject, PyObject)>,
}

pub enum InputValue<'a> {
    Cbor(Cow<'a, [u8]>),
    Iter(ArgIter),
}

impl Scope {
    pub fn new() -> Self {
        prepare_freethreaded_python();
        Python::with_gil(|py| {
            let locals = PyDict::new(py);
            locals
                .set_item(
                    "__builtins__",
                    PyModule::import(py, intern!(py, "builtins")).unwrap(),
                )
                .unwrap();

            let stdio = if let Ok(sys) = PyModule::import(py, intern!(py, "sys")) {
                if let Ok(path) = sys.getattr(intern!(py, "path")) {
                    let path = path.downcast_exact::<PyList>().ok();
                    if let Some(path) = path {
                        let _ = path.insert(1, "/usr/local/lib/bundle.zip");
                        let _ = path.insert(0, "/workdir");
                    }
                }
                if let (Ok(stdout), Ok(stderr)) = (
                    sys.getattr(intern!(py, "stdout")),
                    sys.getattr(intern!(py, "stderr")),
                ) {
                    Some((
                        stdout.into_pyobject(py).unwrap().into(),
                        stderr.into_pyobject(py).unwrap().into(),
                    ))
                } else {
                    None
                }
            } else {
                None
            };

            Scope {
                locals: locals.into_pyobject(py).unwrap().into(),
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
        static INIT: GILOnceCell<PyObject> = GILOnceCell::new();

        Python::with_gil(|py| {
            if let Some(meta) = pymeta::parse_pep723(code.as_bytes()) {
                INIT.import(py, "promptkit.importlib", "_initialize_pep723")
                    .expect("failed to import promptkit.importlib")
                    .call1((meta,))
                    .map_err(|e| Error::from_pyerr(py, e))?;
            }
            let code = std::ffi::CString::new(code).unwrap();
            py.run(
                &code,
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
            || PyDict::is_exact_type_of(pyobject)
            || PyList::is_exact_type_of(pyobject)
            || PyTuple::is_exact_type_of(pyobject)
            || PyString::is_exact_type_of(pyobject)
            || PyBool::is_exact_type_of(pyobject)
            || PyInt::is_exact_type_of(pyobject)
            || PyFloat::is_exact_type_of(pyobject)
            || PyBytes::is_exact_type_of(pyobject)
            || PyByteArray::is_exact_type_of(pyobject)
            || PyMemoryView::is_exact_type_of(pyobject)
    }

    pub fn run<'a, U>(
        &self,
        name: &str,
        positional: impl IntoIterator<Item = InputValue<'a>, IntoIter = U>,
        named: impl IntoIterator<Item = (Cow<'a, str>, InputValue<'a>)>,
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
                let args = PyTuple::new(
                    py,
                    positional
                        .into_iter()
                        .map(|v| match v {
                            InputValue::Iter(it) => Ok(it.into_pyobject(py).unwrap().into_any()),
                            InputValue::Cbor(v) => Ok(cbor_to_python(py, v.as_ref())
                                .map_err(|e| Error::from_pyerr(py, e))?),
                        })
                        .collect::<Result<Vec<_>>>()?,
                )
                .map_err(|_e| Error::UnexpectedError("Failed to create Python tuple"))?;
                let kwargs = PyDict::new(py);
                for (k, v) in named {
                    match v {
                        InputValue::Cbor(v) => {
                            kwargs
                                .set_item(
                                    k,
                                    cbor_to_python(py, v.as_ref())
                                        .map_err(|e| Error::from_pyerr(py, e))?,
                                )
                                .map_err(|e| Error::from_pyerr(py, e))?;
                        }
                        InputValue::Iter(it) => kwargs
                            .set_item(k, it.into_pyobject(py).unwrap())
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
                static ASYNC_RUN: GILOnceCell<PyObject> = GILOnceCell::new();
                ASYNC_RUN
                    .import(py, "promptkit.asyncio", "run")
                    .expect("failed to import promptkit.asyncio")
                    .call1((obj,))
                    .map_err(|e| Error::from_pyerr(py, e))?
            } else {
                obj
            };

            if Self::is_serializable(&obj) {
                match python_to_cbor(obj) {
                    Ok(s) => return Ok(Some(s)),
                    Err(e) => {
                        return Err(Error::from_pyerr(py, e));
                    }
                };
            }

            if let Ok(iter) = obj.try_iter() {
                for el in iter {
                    let mut tmp = {
                        let object = el.map_err(|e| Error::from_pyerr(py, e))?;
                        python_to_cbor(object)
                    }
                    .map_err(|e| Error::from_pyerr(py, e))?;
                    callback(&tmp);
                    tmp.clear();
                }
                return Ok(None);
            }

            Err(Error::UnexpectedError(
                "Return type is not serializable or iterable",
            ))
        })
    }

    pub fn analyze(&self, request: InputValue<'_>) -> Result<Option<Vec<u8>>> {
        Python::with_gil(|py| {
            let module = py
                .import(intern!(py, "promptkit._analyze"))
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
                            InputValue::Iter(_) => {
                                return Err(Error::UnexpectedError(
                                    "Iterator input not supported for analyze",
                                ));
                            }
                            InputValue::Cbor(v) => cbor_to_python(py, v.as_ref())
                                .map_err(|e| Error::from_pyerr(py, e))?,
                        },
                    ),
                    None,
                )
                .map_err(|e| Error::from_pyerr(py, e))?;
            match python_to_cbor(obj) {
                Ok(s) => Ok(Some(s)),
                Err(e) => Err(Error::from_pyerr(py, e)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
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
            .run(
                "hello",
                [InputValue::Cbor(minicbor_serde::to_vec(32).unwrap().into())],
                [],
                |_| {},
            )
            .unwrap();
        assert_eq!(x.unwrap(), minicbor_serde::to_vec("hello 54").unwrap());

        let x = s.run("i", [], [], |_| {}).unwrap();
        assert_eq!(x.unwrap(), minicbor_serde::to_vec(22).unwrap());

        let mut v = vec![];
        let x = s.run("gen", [], [], |s| v.push(s.to_owned())).unwrap();
        assert_eq!(x, None);
        assert_eq!(v.len(), 10);
        for (i, vv) in v.iter().enumerate() {
            assert_eq!(*vv, minicbor_serde::to_vec(i).unwrap());
        }
    }
}
