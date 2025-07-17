#[pyo3::pymodule]
#[pyo3(name = "_promptkit_serde")]
pub mod serde_module {
    use pyo3::{Bound, PyAny, PyErr, PyResult, Python, pyfunction, types::PyAnyMethods};

    use crate::serde::{
        cbor_to_python, json_to_python, python_to_cbor, python_to_json, python_to_yaml,
        yaml_to_python,
    };

    #[pyfunction]
    fn dumps<'py>(
        py: Python<'py>,
        value: Bound<'_, PyAny>,
        format: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        match format {
            "json" => {
                let json_str = python_to_json(value).map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to serialize to JSON: {e}"
                    ))
                })?;
                Ok(pyo3::types::PyString::new(py, &json_str).into_any())
            }
            "yaml" => {
                let yaml_str = python_to_yaml(value).map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to serialize to YAML: {e}"
                    ))
                })?;
                Ok(pyo3::types::PyString::new(py, &yaml_str).into_any())
            }
            "cbor" => {
                let buffer = python_to_cbor(value).map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to serialize to CBOR: {e}"
                    ))
                })?;
                Ok(pyo3::types::PyBytes::new(py, &buffer).into_any())
            }
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Unsupported format. Use 'json', 'yaml', or 'cbor'.",
            )),
        }
    }

    #[pyfunction]
    fn loads<'py>(
        py: Python<'py>,
        s: &Bound<'_, PyAny>,
        format: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        match format {
            "json" => {
                let s_str = s.extract::<&str>()?;
                json_to_python(py, s_str).map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to deserialize JSON: {e}"
                    ))
                })
            }
            "yaml" => {
                let s_str = s.extract::<&str>()?;
                yaml_to_python(py, s_str).map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to deserialize YAML: {e}"
                    ))
                })
            }
            "cbor" => {
                let bytes = s.extract::<&[u8]>().map_err(|_e| {
                    PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                        "CBOR format requires bytes input",
                    )
                })?;
                cbor_to_python(py, bytes).map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to deserialize CBOR: {e}"
                    ))
                })
            }
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Unsupported format. Use 'json', 'yaml', or 'cbor'.",
            )),
        }
    }
}
