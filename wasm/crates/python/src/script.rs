use std::borrow::Cow;

use pyo3::{
    PyTypeInfo,
    exceptions::PyNameError,
    intern,
    prelude::*,
    sync::PyOnceLock,
    types::{
        PyBool, PyByteArray, PyBytes, PyDict, PyFloat, PyInt, PyList, PyMemoryView, PyString,
        PyTuple,
    },
};

use crate::{
    error::{Error, Result},
    pymeta,
    serde::{cbor_to_python, python_to_cbor_emit},
    wasm::{ArgIter, promptkit::script::host::EmitType},
};

pub struct Scope {
    locals: Py<PyAny>,
    stdio: Option<(Py<PyAny>, Py<PyAny>)>,
}

pub enum InputValue<'a> {
    Cbor(Cow<'a, [u8]>),
    Iter(ArgIter),
}

impl Scope {
    pub fn new() -> Self {
        Python::initialize();
        Python::attach(|py| {
            let locals = PyDict::new(py);
            locals
                .set_item(
                    "__builtins__",
                    PyModule::import(py, intern!(py, "builtins")).unwrap(),
                )
                .unwrap();

            let stdio = if let Ok(sys) = PyModule::import(py, intern!(py, "sys")) {
                if let Ok(path) = sys.getattr(intern!(py, "path")) {
                    let path = path.cast_exact::<PyList>().ok();
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
        let _ = Python::attach(|py| {
            if let Some((stdout, stderr)) = &self.stdio {
                let flush = intern!(py, "flush");
                stdout.call_method0(py, flush)?;
                stderr.call_method0(py, flush)?;
            }
            Ok::<_, PyErr>(())
        });
    }

    pub fn load_script(&self, code: &str) -> crate::error::Result<()> {
        static INIT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

        Python::attach(|py| {
            if let Some(meta) = pymeta::parse_pep723(code) {
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
                        .cast_bound(py)
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
        mut callback: impl FnMut(crate::wasm::promptkit::script::host::EmitType, &[u8]),
    ) -> Result<()>
    where
        U: ExactSizeIterator<Item = InputValue<'a>>,
    {
        Python::attach(|py| {
            let dict: &Bound<'_, PyDict> = self
                .locals
                .cast_bound(py)
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
                static ASYNC_RUN: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
                ASYNC_RUN
                    .import(py, "promptkit.asyncio", "run")
                    .expect("failed to import promptkit.asyncio")
                    .call1((obj,))
                    .map_err(|e| Error::from_pyerr(py, e))?
            } else {
                obj
            };

            if Self::is_serializable(&obj) {
                return python_to_cbor_emit(obj, EmitType::End, callback)
                    .map_err(|e| Error::from_pyerr(py, e));
            }

            if let Ok(iter) = obj.try_iter() {
                for el in iter {
                    python_to_cbor_emit(
                        el.map_err(|e| Error::from_pyerr(py, e))?,
                        EmitType::PartialResult,
                        &mut callback,
                    )
                    .map_err(|e| Error::from_pyerr(py, e))?;
                }

                callback(EmitType::End, &[]);
                return Ok(());
            }

            Err(Error::UnexpectedError(
                "Return type is not serializable or iterable",
            ))
        })
    }

    pub fn analyze(
        &self,
        request: InputValue<'_>,
        callback: impl FnMut(crate::wasm::promptkit::script::host::EmitType, &[u8]),
    ) -> Result<()> {
        Python::attach(|py| {
            let module = py
                .import(intern!(py, "promptkit._analyze"))
                .expect("failed to import promptkit._analyze");
            let dict: &Bound<'_, PyDict> = self
                .locals
                .cast_bound(py)
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
            python_to_cbor_emit(obj, EmitType::End, callback).map_err(|e| Error::from_pyerr(py, e))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_python_to_cbor_emit() {
        use std::cell::RefCell;
        let emissions = RefCell::new(Vec::new());

        Python::initialize();

        {
            let emit_fn = |emit_type: crate::wasm::promptkit::script::host::EmitType,
                           data: &[u8]| {
                emissions.borrow_mut().push((emit_type, data.to_vec()));
            };

            // Test the python_to_cbor_emit function with a simple value
            use pyo3::{Python, types::PyString};
            Python::attach(|py| {
                let test_string = PyString::new(py, "test");
                let test_value = test_string.as_any();
                python_to_cbor_emit(test_value.clone(), EmitType::End, emit_fn).unwrap();
            });
        }

        // Check that emission occurred
        let emissions = emissions.into_inner();
        assert_eq!(emissions.len(), 1);
        assert_eq!(emissions[0].0, EmitType::End);
        // The exact content will be CBOR-encoded "test"
        assert!(!emissions[0].1.is_empty());
    }

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
        let mut x = vec![];
        s.run(
            "hello",
            [InputValue::Cbor(minicbor_serde::to_vec(32).unwrap().into())],
            [],
            |_emit_type, data| {
                x.push(data.to_owned());
            },
        )
        .unwrap();
        assert_eq!(x[0], minicbor_serde::to_vec("hello 54").unwrap());

        let mut x = vec![];
        s.run("i", [], [], |_emit_type, data| {
            x.push(data.to_owned());
        })
        .unwrap();
        assert_eq!(x[0], minicbor_serde::to_vec(22).unwrap());

        let mut v = vec![];
        s.run("gen", [], [], |emit_type, data| {
            if emit_type == EmitType::PartialResult {
                v.push(data.to_owned());
            }
        })
        .unwrap();
        assert_eq!(v.len(), 10);
        for (i, vv) in v.iter().enumerate() {
            assert_eq!(*vv, minicbor_serde::to_vec(i).unwrap());
        }
    }
}
