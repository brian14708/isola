use pyo3::{PyRefMut, pyclass, pymethods};

use super::wasi;

#[pyclass]
pub struct PyPollable {
    inner: Option<wasi::io::poll::Pollable>,
    refcnt: usize,
}

impl From<wasi::io::poll::Pollable> for PyPollable {
    fn from(p: wasi::io::poll::Pollable) -> Self {
        Self {
            inner: Some(p),
            refcnt: 1,
        }
    }
}

impl PyPollable {
    pub(crate) fn get_pollable(&self) -> &wasi::io::poll::Pollable {
        self.inner.as_ref().expect("pollable already released")
    }
}

#[pymethods]
impl PyPollable {
    fn subscribe(mut slf: PyRefMut<'_, PyPollable>) -> Option<PyRefMut<'_, PyPollable>> {
        if slf.inner.is_some() {
            slf.refcnt += 1;
            Some(slf)
        } else {
            None
        }
    }

    #[allow(clippy::unused_self)]
    fn get(&self) {}

    pub(crate) fn release(&mut self) {
        if self.refcnt > 1 {
            self.refcnt -= 1;
            return;
        }
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

            fn subscribe(slf: ::pyo3::PyRef<'_, Self>) -> Option<crate::wasm::future::PyPollable> {
                match slf.inner.as_ref() {
                    Some(f) => Some(f.subscribe().into()),
                    _ => None,
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

            fn subscribe(slf: ::pyo3::PyRef<'_, Self>) -> Option<crate::wasm::future::PyPollable> {
                match slf.inner.as_ref() {
                    Some(f) => Some(f.subscribe().into()),
                    _ => None,
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
