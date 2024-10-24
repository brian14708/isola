use std::{path::PathBuf, process::Command};

fn main() {
    let is_wasm = std::env::var("TARGET").unwrap_or_default() == "wasm32-wasip1";
    if is_wasm {
        let outdir = PathBuf::from(format!(
            "{}/{}",
            env!("CARGO_MANIFEST_DIR"),
            "../../target/wasm32-wasip1/wasi-deps"
        ));

        let odir = std::env::var("OUT_DIR").unwrap();
        let cmd = std::env::var("CMAKE").unwrap_or(String::from("cmake"));
        let e = Command::new(&cmd)
            .arg(String::from("-DCMAKE_INSTALL_PREFIX=") + outdir.to_str().unwrap())
            .arg("-DCMAKE_BUILD_TYPE=Release")
            .arg(format!("{}/lib", env!("CARGO_MANIFEST_DIR")))
            .current_dir(odir.clone())
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
        if !e.success() {
            panic!("Failed to run cmake {:?}", e);
        }
        let e = Command::new(&cmd)
            .arg("--build")
            .arg(".")
            .arg("--parallel")
            .arg("4")
            .arg("--target")
            .arg("install")
            .current_dir(odir)
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
        if !e.success() {
            panic!("Failed to run cmake {:?}", e);
        }

        println!("cargo:rustc-link-arg=-shared");
        let libpython_binary = "python3.13";
        let lib_paths = vec!["lib"];
        let libs = vec![libpython_binary];
        for lib_path in &lib_paths {
            let mut dst = outdir.clone();
            dst.push(lib_path);
            println!("cargo:rustc-link-search=native={}", dst.display());
        }

        for lib in &libs {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}
