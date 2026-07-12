use std::{
    env,
    path::{Path, PathBuf},
    str::FromStr,
    vec,
};

use anyhow::Result;
use xshell::{Shell, cmd};

use crate::async_shim::link_library;

mod async_shim;

fn main() -> Result<()> {
    let workspace_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    std::env::set_current_dir(workspace_dir)?;

    let task = std::env::args().nth(1);

    let sh = Shell::new()?;
    if let Some(cmd) = task.as_deref() {
        let f = TASKS
            .iter()
            .find_map(|(k, f)| (*k == cmd).then_some(*f))
            .unwrap_or(print_help);
        f(&sh)?;
    } else {
        print_help(&sh)?;
    }
    Ok(())
}

type Task = fn(&Shell) -> Result<()>;
const TASKS: &[(&str, Task)] = &[
    ("build-all", build_all),
    ("build-python", build_python),
    ("build-js", build_js),
];

#[expect(clippy::unnecessary_wraps, reason = "matches Task fn signature")]
fn print_help(_sh: &Shell) -> Result<()> {
    println!("Tasks:");
    for (name, _) in TASKS {
        println!("  - {name}");
    }
    Ok(())
}

fn build_all(sh: &Shell) -> Result<()> {
    build_python(sh)?;
    build_js(sh)?;
    Ok(())
}

fn wasm_rustflags(wasi_deps_dir: &str) -> String {
    format!(
        "-C relocation-model=pic -C link-arg=-shared -C link-arg=--allow-undefined \
         -C link-arg=-L{wasi_deps_dir}/lib"
    )
}

fn component_inputs(runtime: String) -> Vec<String> {
    vec![
        runtime,
        "crates/xtask/src/main.rs".to_string(),
        "crates/xtask/src/async_shim.rs".to_string(),
        "crates/xtask/Cargo.toml".to_string(),
        "Cargo.toml".to_string(),
        "Cargo.lock".to_string(),
    ]
}

fn build_python(sh: &Shell) -> Result<()> {
    const TARGET: &str = "wasm32-wasip1";

    let wasi_deps_dir = env::var("WASI_PYTHON_DEV").unwrap();
    let rustflags = wasm_rustflags(&wasi_deps_dir);

    cmd!(
        sh,
        "cargo b -Z build-std=std,panic_abort --release --target {TARGET} -p isola-python-runtime"
    )
    .env("PYO3_CROSS_PYTHON_VERSION", "3.14")
    .env("RUSTFLAGS", &rustflags)
    .run()?;

    run_if_changed(
        component_inputs(format!("target/{TARGET}/release/isola_python_runtime.wasm")),
        "target/python.wasm",
        |inp, out| -> Result<()> {
            fn lib(
                name: impl Into<String>,
                path: impl AsRef<Path>,
                dlopen: bool,
            ) -> (String, PathBuf, bool) {
                (name.into(), path.as_ref().to_path_buf(), dlopen)
            }

            let mut libs = vec![
                lib("libisola_python.so", &inp[0], false),
                lib("libc.so", format!("{wasi_deps_dir}/lib/libc.so"), false),
                lib(
                    "libwasi-emulated-signal.so",
                    format!("{wasi_deps_dir}/lib/libwasi-emulated-signal.so"),
                    false,
                ),
                lib(
                    "libwasi-emulated-getpid.so",
                    format!("{wasi_deps_dir}/lib/libwasi-emulated-getpid.so"),
                    false,
                ),
                lib(
                    "libwasi-emulated-process-clocks.so",
                    format!("{wasi_deps_dir}/lib/libwasi-emulated-process-clocks.so"),
                    false,
                ),
                lib("libc++.so", format!("{wasi_deps_dir}/lib/libc++.so"), false),
                lib(
                    "libc++abi.so",
                    format!("{wasi_deps_dir}/lib/libc++abi.so"),
                    false,
                ),
                lib(
                    "libpython3.14.so",
                    format!("{wasi_deps_dir}/lib/libpython3.14.so"),
                    false,
                ),
            ];
            let base = format!("{wasi_deps_dir}/lib/python3.14/site-packages/");
            for entry in glob::glob(&format!("{base}/**/*.so"))? {
                let entry = entry?;
                let filename = entry.to_str().unwrap().replace(&wasi_deps_dir, "");
                libs.push(lib(filename, entry, true));
            }

            let mut wasm = wit_component::Linker::default()
                .validate(true)
                .stack_size(8_388_608)
                .use_built_in_libdl(true);
            for lib in libs {
                let lib_path = lib.1.to_str().unwrap().to_string();
                let data = &std::fs::read(lib_path)?;
                let shim = (lib.0 == "libisola_python.so").then_some("libisola_python_async.so");
                wasm = link_library(wasm, &lib.0, data, lib.2, shim)?;
            }
            let wasm = wasm
                .adapter(
                    wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME,
                    wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
                )?
                .encode()?;

            std::fs::write(out, wasm)?;

            Ok(())
        },
    )?;

    Ok(())
}

fn build_js(sh: &Shell) -> Result<()> {
    const TARGET: &str = "wasm32-wasip1";

    let wasi_deps_dir = env::var("WASI_PYTHON_DEV").unwrap();
    let rustflags = wasm_rustflags(&wasi_deps_dir);

    cmd!(
        sh,
        "cargo b -Z build-std=std,panic_abort --release --target {TARGET} -p isola-js-runtime"
    )
    .env("RUSTFLAGS", &rustflags)
    .run()?;

    run_if_changed(
        component_inputs(format!("target/{TARGET}/release/isola_js_runtime.wasm")),
        "target/js.wasm",
        |inp, out| -> Result<()> {
            fn lib(
                name: impl Into<String>,
                path: impl AsRef<Path>,
                dlopen: bool,
            ) -> (String, PathBuf, bool) {
                (name.into(), path.as_ref().to_path_buf(), dlopen)
            }

            // JS runtime needs libc and the WASI emulated libs (same as Python)
            let libs = vec![
                lib("libisola_js.so", &inp[0], false),
                lib("libc.so", format!("{wasi_deps_dir}/lib/libc.so"), false),
                lib(
                    "libwasi-emulated-signal.so",
                    format!("{wasi_deps_dir}/lib/libwasi-emulated-signal.so"),
                    false,
                ),
                lib(
                    "libwasi-emulated-getpid.so",
                    format!("{wasi_deps_dir}/lib/libwasi-emulated-getpid.so"),
                    false,
                ),
                lib(
                    "libwasi-emulated-process-clocks.so",
                    format!("{wasi_deps_dir}/lib/libwasi-emulated-process-clocks.so"),
                    false,
                ),
            ];

            let mut wasm = wit_component::Linker::default()
                .validate(true)
                .stack_size(2_097_152) // 2MB stack (lighter than Python's 8MB)
                .use_built_in_libdl(true);
            for lib in libs {
                let lib_path = lib.1.to_str().unwrap().to_string();
                let data = &std::fs::read(lib_path)?;
                let shim = (lib.0 == "libisola_js.so").then_some("libisola_js_async.so");
                wasm = link_library(wasm, &lib.0, data, lib.2, shim)?;
            }
            let wasm = wasm
                .adapter(
                    wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME,
                    wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
                )?
                .encode()?;

            std::fs::write(out, wasm)?;

            Ok(())
        },
    )?;

    Ok(())
}

fn run_if_changed(
    inputs: Vec<String>,
    output: &str,
    f: impl FnOnce(Vec<PathBuf>, &Path) -> Result<()>,
) -> Result<()> {
    let inputs = inputs
        .into_iter()
        .map(|s| PathBuf::from_str(&s).unwrap())
        .collect::<Vec<_>>();
    let output = Path::new(output);
    if output.exists() {
        let output_time = output.metadata()?.modified()?;
        for input in &inputs {
            if input.metadata()?.modified()? > output_time {
                return f(inputs, output);
            }
        }
    } else {
        return f(inputs, output);
    }
    Ok(())
}
