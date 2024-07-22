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

    let dbg = env::var("DEBUG").is_ok();
    if dbg {
        cmd!(
        sh,
        "cargo rustc --crate-type cdylib --profile release --target {TARGET} -p promptkit_python"
    )
        .env("PYO3_CROSS_PYTHON_VERSION", "3.12")
        .env("CARGO_PROFILE_RELEASE_OPT_LEVEL", "1")
        .run()?;
    } else {
        cmd!(
        sh,
        "cargo rustc --crate-type cdylib --profile release --target {TARGET} -p promptkit_python"
    )
        .env("PYO3_CROSS_PYTHON_VERSION", "3.12")
        .env("CARGO_PROFILE_RELEASE_LTO", "thin")
        .env("CARGO_PROFILE_RELEASE_OPT_LEVEL", "3")
        .env("CARGO_PROFILE_RELEASE_PANIC", "abort")
        .env("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "1")
        .run()?;
    }

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
        format!("target/{TARGET}/release/promptkit_python.init.wasm"),
        |inp, out| -> Result<()> {
            let workdir = sh.create_temp_dir()?;
            let workdir = workdir.path();
            let inp = &inp[0];
            cmd!(
                sh, "wizer {inp} --allow-wasi --wasm-bulk-memory true --mapdir /usr::target/{TARGET}/wasi-deps/usr --mapdir /workdir::{workdir} -o {out}"
            ).run()?;
            Ok(())
        },
    )?;

    run_if_changed(
        vec![format!(
            "target/{TARGET}/release/promptkit_python.init.wasm"
        )],
        format!("target/{TARGET}/release/promptkit_python.opt.wasm"),
        |inp, out| -> Result<()> {
            let inp = &inp[0];
            if dbg {
                cmd!(sh, "wasm-opt -O1 --strip-debug {inp} -o {out}").run()?;
            } else {
                cmd!(
                    sh,
                    "wasm-opt --precompute-propagate -O4 --gufa -O4 --strip-debug {inp} -o {out}"
                )
                .run()?;
            }
            Ok(())
        },
    )?;

    run_if_changed(
        vec![format!("target/{TARGET}/release/promptkit_python.opt.wasm")],
        "target/promptkit_python.wasm".to_string(),
        |inp, out| -> Result<()> {
            let wasm = std::fs::read(&inp[0])?;
            let wasm = wit_component::ComponentEncoder::default()
                .module(&wasm)?
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
