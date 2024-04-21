use std::{fs, path::PathBuf};

use url::Url;

fn download(url: &str, dest: &str) -> PathBuf {
    let url = Url::parse(url).unwrap();

    let mut dest = PathBuf::from(dest);
    dest.push("downloads");
    fs::create_dir_all(&dest).unwrap();
    dest.push(url.path_segments().unwrap().last().unwrap());
    if dest.is_file() {
        return dest;
    }

    {
        let mut response = ureq::get(url.as_str()).call().unwrap().into_reader();
        let mut tmp = dest.clone();
        tmp.set_extension("tmp");
        let mut f = std::fs::File::create(&tmp).unwrap();
        std::io::copy(&mut response, &mut f).unwrap();
        drop(f);
        std::fs::rename(tmp, &dest).unwrap();
    }

    dest
}

fn download_and_unarchive(url: &str, dest: &str) {
    let t = download(url, dest);
    let t = std::fs::File::open(t).unwrap();
    let decoder = flate2::read::GzDecoder::new(t);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest).unwrap();
}

fn unarchive(file: &str, dest: &str) {
    println!("cargo:rerun-if-changed={}", file);
    let t = std::fs::File::open(file).unwrap();
    let decoder = flate2::read::GzDecoder::new(t);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest).unwrap();
}

fn main() {
    let is_wasm = std::env::var("TARGET").unwrap_or_default() == "wasm32-wasip1";
    if is_wasm {
        let wasi_deps_path = format!(
            "{}/{}",
            env!("CARGO_MANIFEST_DIR"),
            "../../target/wasm32-wasip1/wasi-deps"
        );
        let libpython_binary = "python3.12";

        let mut lib_paths = vec!["lib/wasi", "lib/wasm32-wasi"];

        if std::fs::File::open("../../lib/libpython.tar.gz").is_ok() {
            unarchive("../../lib/libpython.tar.gz", &wasi_deps_path);
            lib_paths.insert(0, "wasi-sysroot/lib/wasm32-wasip1");
        } else {
            // https://github.com/vmware-labs/webassembly-language-runtimes/blob/main/python/tools/wlr-libpy/src/bld_cfg.rs
            let wasi_sdk_sysroot_url= "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-20/wasi-sysroot-20.0.tar.gz";
            let wasi_sdk_clang_builtins_url = "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-20/libclang_rt.builtins-wasm32-wasi-20.0.tar.gz";
            let libpython_url= "https://github.com/vmware-labs/webassembly-language-runtimes/releases/download/python%2F3.12.0%2B20231211-040d5a6/libpython-3.12.0-wasi-sdk-20.0.tar.gz";

            download_and_unarchive(wasi_sdk_sysroot_url, &wasi_deps_path);
            download_and_unarchive(wasi_sdk_clang_builtins_url, &wasi_deps_path);
            download_and_unarchive(libpython_url, &wasi_deps_path);
            lib_paths.insert(0, "wasi-sysroot/lib/wasm32-wasi");
        }

        let libs = vec![
            "wasi-emulated-signal",
            "wasi-emulated-getpid",
            "wasi-emulated-process-clocks",
            "clang_rt.builtins-wasm32",
            libpython_binary,
        ];

        for lib_path in &lib_paths {
            println!("cargo:rustc-link-search=native={wasi_deps_path}/{lib_path}");
        }

        for lib in &libs {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}
