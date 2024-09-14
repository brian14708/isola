use pyo3::{pyclass, pymethods, Bound};

use super::wasi;

#[pyclass]
pub struct PyPollable {
    inner: Option<wasi::io::poll::Pollable>,
}

impl From<wasi::io::poll::Pollable> for PyPollable {
    fn from(p: wasi::io::poll::Pollable) -> Self {
        Self { inner: Some(p) }
    }
}

impl PyPollable {
    pub(crate) fn get_pollable(&self) -> &wasi::io::poll::Pollable {
        self.inner.as_ref().expect("pollable already released")
    }
}

#[pymethods]
impl PyPollable {
    fn subscribe(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    #[allow(clippy::unused_self)]
    fn get(&self) {}

    fn release(&mut self) {
        self.inner.take();
    }

    fn wait(&mut self) {
        self.inner
            .take()
            .expect("pollable already released")
            .block();
    }
}

macro_rules! create_future {
    ($name:ident, $future_type:ty, $type:ty) => {
        #[pyclass]
        struct $name {
            inner: Option<$future_type>,
        }

        impl $name {
            fn new(f: $future_type) -> Self {
                Self { inner: Some(f) }
            }
        }

        #[pymethods]
        impl $name {
            fn wait(mut slf: ::pyo3::PyRefMut<'_, Self>) -> PyResult<$type> {
                match slf.inner.take() {
                    Some(f) => {
                        f.subscribe().block();
                        f.get().expect("not ready").expect("wasm error").try_into()
                    }
                    _ => panic!("invalid state"),
                }
            }

            fn subscribe(slf: ::pyo3::PyRef<'_, Self>) -> crate::wasm::future::PyPollable {
                match slf.inner.as_ref() {
                    Some(f) => f.subscribe().into(),
                    _ => panic!("invalid state"),
                }
            }

            fn get(mut slf: ::pyo3::PyRefMut<'_, Self>) -> PyResult<$type> {
                match slf.inner.take() {
                    Some(f) => f.get().expect("not ready").expect("wasm error").try_into(),
                    _ => panic!("invalid state"),
                }
            }

            fn release(mut slf: ::pyo3::PyRefMut<'_, Self>) {
                slf.inner.take();
            }
        }
    };
    ($name:ident, $future_type:ty, $convert:ident -> $type:ty) => {
        #[pyclass]
        struct $name {
            inner: Option<$future_type>,
        }

        impl $name {
            fn new(f: $future_type) -> Self {
                Self { inner: Some(f) }
            }
        }

        #[pymethods]
        impl $name {
            fn wait(mut slf: ::pyo3::PyRefMut<'_, Self>) -> $type {
                match slf.inner.take() {
                    Some(f) => {
                        f.subscribe().block();
                        $convert(slf.py(), f.get().expect("not ready").expect("wasm error"))
                    }
                    _ => panic!("invalid state"),
                }
            }

            fn subscribe(slf: ::pyo3::PyRef<'_, Self>) -> crate::wasm::future::PyPollable {
                match slf.inner.as_ref() {
                    Some(f) => f.subscribe().into(),
                    _ => panic!("invalid state"),
                }
            }

            fn get(mut slf: ::pyo3::PyRefMut<'_, Self>) -> $type {
                match slf.inner.take() {
                    Some(f) => $convert(slf.py(), f.get().expect("not ready").expect("wasm error")),
                    _ => panic!("invalid state"),
                }
            }

            fn release(mut slf: ::pyo3::PyRefMut<'_, Self>) {
                slf.inner.take();
            }
        }
    };
}

pub(crate) use create_future;
