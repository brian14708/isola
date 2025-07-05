#[pyo3::pymodule]
#[pyo3(name = "_promptkit_serde")]
pub mod serde_module {
    use pyo3::{Bound, PyAny, PyErr, PyResult, Python, pyfunction};

    use crate::serde::PyValue;

    #[pyfunction]
    fn dumps(value: Bound<'_, PyAny>, format: &str) -> PyResult<String> {
        match format {
            "json" => serde_json::to_string(&PyValue::new(value))
                .map_err(|_e| PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error")),
            "yaml" => serde_yaml::to_string(&PyValue::new(value))
                .map_err(|_e| PyErr::new::<pyo3::exceptions::PyTypeError, _>("serde error")),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Unsupported format. Use 'json' or 'yaml'.",
            )),
        }
    }

    #[pyfunction]
    fn loads<'py>(py: Python<'py>, s: &str, format: &str) -> PyResult<Bound<'py, PyAny>> {
        match format {
            "json" => PyValue::deserialize(py, &mut serde_json::Deserializer::from_str(s))
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())),
            "yaml" => PyValue::deserialize(py, serde_yaml::Deserializer::from_str(s))
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string())),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Unsupported format. Use 'json' or 'yaml'.",
            )),
        }
    }
}
