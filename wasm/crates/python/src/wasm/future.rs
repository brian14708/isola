use std::ops::Deref;

use pyo3::{Borrowed, PyAny, PyRefMut, PyResult, intern, pyclass, pymethods, types::PyAnyMethods};

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
    fn get_pollable(&self) -> &wasi::io::poll::Pollable {
        self.inner.as_ref().expect("pollable already released")
    }
}

#[pymethods]
impl PyPollable {
    fn subscribe(mut slf: PyRefMut<'_, PyPollable>) -> Option<PyRefMut<'_, PyPollable>> {
        if let Some(inner) = &slf.inner {
            if inner.ready() {
                slf.refcnt = 0;
                slf.inner.take();
                None
            } else {
                slf.refcnt += 1;
                Some(slf)
            }
        } else {
            None
        }
    }

    fn get(&self) {
        let _ = self;
    }

    #[inline]
    fn release(&mut self) {
        if self.refcnt > 1 {
            self.refcnt -= 1;
            return;
        }
        self.inner.take();
    }

    fn wait(&mut self) {
        if let Some(inner) = self.inner.take() {
            inner.block();
        }
    }
}

pub struct Pollable<'py>(PyRefMut<'py, PyPollable>);

impl<'py> Pollable<'py> {
    pub fn subscribe(p: Borrowed<'_, 'py, PyAny>) -> PyResult<Option<Self>> {
        let p = p.call_method0(intern!(p.py(), "subscribe"))?;
        if p.is_none() {
            return Ok(None);
        }
        Ok(Some(Self(p.downcast_exact::<PyPollable>()?.borrow_mut())))
    }
}

impl Deref for Pollable<'_> {
    type Target = wasi::io::poll::Pollable;
    fn deref(&self) -> &Self::Target {
        self.0.get_pollable()
    }
}

impl Drop for Pollable<'_> {
    fn drop(&mut self) {
        self.0.release();
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
