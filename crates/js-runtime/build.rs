use std::path::PathBuf;

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("wasm32") {
        // Use PIC-compiled WASI libs from WASI_PYTHON_DEV (shared with the Python
        // runtime build environment). The Rust sysroot's libc.a is not compiled
        // with -fPIC, so we must link against the WASI SDK's libc.so instead.
        let wasi_dev = std::env::var("WASI_PYTHON_DEV")
            .expect("WASI_PYTHON_DEV must be set for wasm32 builds (run inside `nix develop`)");
        let lib_dir = PathBuf::from(&wasi_dev).join("lib");
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
        println!("cargo:rustc-link-arg=-shared");
    }
}
