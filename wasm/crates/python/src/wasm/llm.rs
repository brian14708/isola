use core::slice;

use pyo3::{
    intern, pyclass, pymethods, pymodule,
    types::{PyAnyMethods, PyByteArray, PyBytes, PyBytesMethods, PyMemoryView, PyModule},
    Bound, PyAny, PyObject, PyResult,
};

use super::promptkit::llm::tokenizer;

#[pymodule]
#[pyo3(name = "llm")]
pub fn llm_module(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<Tokenizer>()?;
    Ok(())
}

#[pyclass]
struct Tokenizer {
    inner: tokenizer::Tokenizer,
}

#[pymethods]
impl Tokenizer {
    #[new]
    #[pyo3(signature = (name, /))]
    fn py_new(name: &str) -> Self {
        Self {
            inner: tokenizer::Tokenizer::new(name),
        }
    }

    #[pyo3(signature = (text, /))]
    fn encode(slf: &Bound<'_, Self>, text: &str) -> PyResult<PyObject> {
        let ids = slf.borrow().inner.encode(text);
        let raw = unsafe {
            slice::from_raw_parts(
                ids.as_ptr().cast::<u8>(),
                ids.len() * std::mem::size_of::<u32>(),
            )
        };
        let py = slf.py();
        let bytes = PyByteArray::new_bound(py, raw);
        let mem = PyMemoryView::from_bound(&bytes)?;
        let obj = mem.call_method1("cast", (intern!(py, "I"),))?;
        Ok(obj.into())
    }

    #[pyo3(signature = (ids, /))]
    fn decode(&self, ids: &Bound<'_, PyAny>) -> PyResult<String> {
        if let Ok(v) = ids.downcast::<PyMemoryView>() {
            let format = v
                .getattr("format")
                .is_ok_and(|f| matches!(f.extract::<&str>(), Ok("I" | "@I")));
            if format {
                if let Ok(b) = v.call_method0("tobytes") {
                    let raw = b.downcast_exact::<PyBytes>().unwrap().as_bytes();
                    let ptr = raw.as_ptr();
                    assert!(
                        ptr.align_offset(std::mem::align_of::<u32>()) == 0
                            && raw.len() % std::mem::size_of::<u32>() == 0
                    );
                    let ids = unsafe {
                        slice::from_raw_parts(
                            #[allow(clippy::cast_ptr_alignment)]
                            ptr.cast::<u32>(),
                            raw.len() / std::mem::size_of::<u32>(),
                        )
                    };
                    return Ok(self.inner.decode(ids));
                }
            }
        }

        let ids: Vec<u32> = ids.extract()?;
        Ok(self.inner.decode(&ids))
    }
}
