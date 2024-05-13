#![allow(clippy::missing_safety_doc, clippy::module_name_repetitions)]

mod http;
mod llm;
mod logging;

use std::cell::RefCell;

use cbor4ii::core::utils::SliceReader;
use pyo3::{append_to_inittab, prelude::*, wrap_pymodule};
use serde::de::DeserializeSeed;

use crate::{
    error::Error,
    script::{InputValue, Scope},
    serde::PyObjectDeserializer,
};

use self::{exports::promptkit::vm::guest, promptkit::vm::host};

wit_bindgen::generate!({
    world: "sandbox",
    with: {
        "wasi:io/poll@0.2.0": wasi::io::poll,
        "wasi:io/error@0.2.0": wasi::io::error,
        "wasi:io/streams@0.2.0": wasi::io::streams,
    },
});

export!(Global);

pub struct Global;

impl guest::Guest for Global {
    fn set_log_level(level: Option<host::LogLevel>) {
        logging::set_log_level(level);
    }

    fn eval_bundle(bundle_path: String, entrypoint: String) -> Result<(), guest::Error> {
        GLOBAL_SCOPE.with_borrow_mut(|vm| {
            if let Some(vm) = vm.as_mut() {
                vm.load_zip(&bundle_path, &entrypoint)
                    .map_err(Into::<guest::Error>::into)
            } else {
                Err(Error::UnexpectedError("VM not initialized").into())
            }
        })
    }

    fn eval_script(script: String) -> Result<(), guest::Error> {
        GLOBAL_SCOPE.with_borrow(|vm| {
            if let Some(vm) = vm.as_ref() {
                vm.load_script(&script).map_err(Into::<guest::Error>::into)
            } else {
                Err(Error::UnexpectedError("VM not initialized").into())
            }
        })
    }

    fn call_func(func: String, args: Vec<host::Argument>) -> Result<Option<Vec<u8>>, guest::Error> {
        GLOBAL_SCOPE.with_borrow(|vm| {
            if let Some(vm) = vm.as_ref() {
                let ret = vm
                    .run(
                        &func,
                        args.into_iter().map(|f| match f {
                            host::Argument::Cbor(s) => InputValue::Cbor(s.into()),
                            host::Argument::Iterator(e) => InputValue::Iter(ArgIter { iter: e }),
                        }),
                        [],
                        host::emit,
                    )
                    .map_err(Into::<guest::Error>::into);
                vm.flush();
                ret
            } else {
                Err(Error::UnexpectedError("VM not initialized").into())
            }
        })
    }
}

#[pymodule]
#[pyo3(name = "models")]
fn models_module(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_wrapped(wrap_pymodule!(llm::llm_module))?;
    Ok(())
}

#[pymodule]
#[pyo3(name = "promptkit")]
fn promptkit_module(module: Bound<'_, PyModule>) -> PyResult<()> {
    module.add_wrapped(wrap_pymodule!(http::http_module))?;
    module.add_wrapped(wrap_pymodule!(logging::logging_module))?;
    module.add_wrapped(wrap_pymodule!(models_module))?;
    Ok(())
}

#[pyclass]
pub struct ArgIter {
    iter: host::ArgumentIterator,
}

#[pymethods]
impl ArgIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    #[allow(clippy::needless_pass_by_value)]
    fn __next__(slf: PyRefMut<'_, Self>) -> PyResult<Option<PyObject>> {
        match slf.iter.read() {
            Some(a) => match a {
                host::Argument::Cbor(c) => {
                    let c = SliceReader::new(&c);
                    Ok(Some(
                        PyObjectDeserializer::new(slf.py())
                            .deserialize(&mut cbor4ii::serde::Deserializer::new(c))
                            .map_err(|_| {
                                PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error")
                            })?,
                    ))
                }
                host::Argument::Iterator(_) => todo!(),
            },
            None => Ok(None),
        }
    }
}

thread_local! {
    static GLOBAL_SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

#[export_name = "wizer.initialize"]
pub extern "C" fn _initialize() {
    extern "C" {
        fn __wasm_call_ctors();
    }
    unsafe { __wasm_call_ctors() };

    GLOBAL_SCOPE.with(|scope| {
        append_to_inittab!(promptkit_module);
        let v = Scope::new();
        let code = include_str!("prelude.py");
        v.load_script(code).unwrap();
        v.flush();
        scope.borrow_mut().replace(v);
    });
}
