use pyo3::prelude::*;

/// Formats the sum of two numbers as string.
#[pyfunction]
fn run_script(script: &str, json: &str) -> PyResult<String> {
    Ok(json.to_string())
}

/// A Python module implemented in Rust.
#[pymodule]
fn python(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(run_script, m)?)?;
    Ok(())
}
