use std::path::PathBuf;

fn main() {
    let is_wasm = std::env::var("TARGET")
        .unwrap_or_default()
        .starts_with("wasm32");
    if is_wasm {
        let outdir = PathBuf::from(std::env::var("WASI_PYTHON_DEV").unwrap());
        println!("cargo:rustc-link-arg=-shared");
        let lib_paths = vec!["lib"];
        for lib_path in &lib_paths {
            let mut dst = outdir.clone();
            dst.push(lib_path);
            println!("cargo:rustc-link-search=native={}", dst.display());
        }
    }
}
