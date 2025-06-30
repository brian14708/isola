#[pyo3::pymodule]
#[pyo3(name = "_promptkit_serde")]
pub mod serde_module {
    use pyo3::{Bound, PyAny, PyErr, PyResult, Python, pyfunction};

    use crate::serde::PyValue;

    #[pyfunction]
    fn json_loads<'py>(py: Python<'py>, s: &str) -> PyResult<Bound<'py, PyAny>> {
        PyValue::deserialize(py, &mut serde_json::Deserializer::from_str(s))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))
    }

    #[pyfunction]
    fn json_dumps(value: Bound<'_, PyAny>) -> PyResult<String> {
        serde_json::to_string(&PyValue::new(value))
            .map_err(|_e| PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error"))
    }
}
