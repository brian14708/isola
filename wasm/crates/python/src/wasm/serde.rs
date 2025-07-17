#[pyo3::pymodule]
#[pyo3(name = "_promptkit_serde")]
pub mod serde_module {
    use pyo3::{Bound, PyAny, PyErr, PyResult, Python, pyfunction, types::PyAnyMethods};

    use crate::serde::PyValue;

    #[pyfunction]
    fn dumps<'py>(
        py: Python<'py>,
        value: Bound<'_, PyAny>,
        format: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        match format {
            "json" => {
                let json_str = serde_json::to_string(&PyValue::new(value)).map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to serialize to JSON: {e}"
                    ))
                })?;
                Ok(pyo3::types::PyString::new(py, &json_str).into_any())
            }
            "yaml" => {
                let yaml_str = serde_yaml::to_string(&PyValue::new(value)).map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to serialize to YAML: {e}"
                    ))
                })?;
                Ok(pyo3::types::PyString::new(py, &yaml_str).into_any())
            }
            "cbor" => {
                let buffer = minicbor_serde::to_vec(PyValue::new(value)).map_err(|e| {
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
                PyValue::deserialize(py, &mut serde_json::Deserializer::from_str(s_str)).map_err(
                    |e| {
                        PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                            "Failed to deserialize JSON: {e}"
                        ))
                    },
                )
            }
            "yaml" => {
                let s_str = s.extract::<&str>()?;
                PyValue::deserialize(py, serde_yaml::Deserializer::from_str(s_str)).map_err(|e| {
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
                PyValue::deserialize(py, &mut minicbor_serde::Deserializer::new(bytes)).map_err(
                    |e| {
                        PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                            "Failed to deserialize CBOR: {e}"
                        ))
                    },
                )
            }
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Unsupported format. Use 'json', 'yaml', or 'cbor'.",
            )),
        }
    }
}
