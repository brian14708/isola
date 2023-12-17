use ::promptkit::script::{InputValue, Script};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};

#[pyfunction]
fn run_script(script: &str, json: &str) -> PyResult<String> {
    // Ok(json.to_string())
    let v = serde_json::from_str(json).unwrap();

    let ret = Script::new(script)
        .map_err(|e| PyValueError::new_err(e.to_string()))?
        .run("handle", [InputValue::Json(v)], [])
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

    serde_json::to_string(&ret).map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

#[pymodule]
fn promptkit(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(run_script, m)?)?;
    Ok(())
}
