use std::{
    env,
    fs::remove_dir_all,
    path::{Path, PathBuf},
    str::FromStr,
    vec,
};

use anyhow::Result;
use xshell::{cmd, Shell};

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

#[allow(clippy::type_complexity)]
const TASKS: &[(&str, fn(&Shell) -> Result<()>)] =
    &[("build-all", build_all), ("build-python", build_python)];

fn print_help(_sh: &Shell) -> Result<()> {
    println!("Tasks:");
    for (name, _) in TASKS {
        println!("  - {name}");
    }
    Ok(())
}

fn build_all(sh: &Shell) -> Result<()> {
    build_python(sh)
}

fn build_python(sh: &Shell) -> Result<()> {
    const TARGET: &str = "wasm32-wasip1";

    cmd!(
        sh,
        "cargo rustc --crate-type cdylib --profile release --target {TARGET} -p promptkit_python"
    )
    .env("PYO3_CROSS_PYTHON_VERSION", "3.13")
    .env("RUSTFLAGS", "-C relocation-model=pic")
    .run()?;

    run_if_changed(
        vec!["crates/python/bundled/requirements.txt".to_string()],
        format!("target/{TARGET}/python-deps/setuptools/__init__.py"),
        |_, _| {
            let _ = remove_dir_all("target/{TARGET}/python-deps");

            cmd!(
                sh,
                "python3 -m pip install -U -r crates/python/bundled/requirements.txt --target target/{TARGET}/python-deps"
            ).run()?;

            Ok(())
        },
    )?;

    cmd!(
        sh,
        "python3 crates/python/bundle.py target/{TARGET}/wasi-deps/usr/local/lib/bundle.zip target/{TARGET}/python-deps crates/python/bundled"
    ).run()?;

    run_if_changed(
        vec![format!("target/{TARGET}/release/promptkit_python.wasm")],
        "target/promptkit_python.wasm".to_string(),
        |inp, out| -> Result<()> {
            let wasm = std::fs::read(&inp[0])?;
            let wasm = wit_component::Linker::default()
            .validate(true)
            .stack_size(8388608)
            .use_built_in_libdl(true)
            .library("", &wasm, false)?
            .library(
                "libc.so",
                &std::fs::read(format!("target/{TARGET}/wasi-deps/lib/libc.so"))?,
                false,
            )?
            .library(
                "libwasi-emulated-signal.so",
                &std::fs::read(format!(
                    "target/{TARGET}/wasi-deps/lib/libwasi-emulated-signal.so"
                ))?,
                false,
            )?
            .library(
                "libwasi-emulated-getpid.so",
                &std::fs::read(format!(
                    "target/{TARGET}/wasi-deps/lib/libwasi-emulated-getpid.so"
                ))?,
                false,
            )?
            .library(
                "libwasi-emulated-process-clocks.so",
                &std::fs::read(format!(
                    "target/{TARGET}/wasi-deps/lib/libwasi-emulated-process-clocks.so"
                ))?,
                false,
            )?
            .library(
                "libpython3.13.so",
                &std::fs::read(format!("target/{TARGET}/wasi-deps/lib/libpython3.13.so"))?,
                false,
            )?
                .adapter(
                    "wasi_snapshot_preview1",
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
    output: String,
    f: impl FnOnce(Vec<PathBuf>, &Path) -> Result<()>,
) -> Result<()> {
    let inputs = inputs
        .into_iter()
        .map(|s| PathBuf::from_str(&s).unwrap())
        .collect::<Vec<_>>();
    let output = Path::new(&output);
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
