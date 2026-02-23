use std::path::PathBuf;

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("wasm32") {
        let outdir = PathBuf::from(std::env::var("WASI_PYTHON_DEV").unwrap());
        println!("cargo:rustc-link-arg=-shared");
        let lib_paths = vec!["lib"];
        for lib_path in &lib_paths {
            let mut dst = outdir.clone();
            dst.push(lib_path);
            println!("cargo:rustc-link-search=native={}", dst.display());
        }
        return;
    }

    // Workspace-wide feature unification can enable `pyo3/extension-module` through
    // `isola-py-binding`, which suppresses PyO3's normal libpython link flags.
    // Emit explicit link flags for `isola-python` so `cargo test --all-features`
    // still links test binaries correctly.
    let py_cfg = pyo3_build_config::get();
    if let Some(lib_name) = py_cfg.lib_name.as_deref() {
        let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        let alias = if target_os == "windows" {
            "pythonXY:"
        } else {
            ""
        };
        let link_model = if py_cfg.shared { "" } else { "static=" };
        println!("cargo:rustc-link-lib={link_model}{alias}{lib_name}");
    }
    if let Some(lib_dir) = py_cfg.lib_dir.as_deref() {
        println!("cargo:rustc-link-search=native={lib_dir}");
    }
}
