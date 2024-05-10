use std::{
    path::{Path, PathBuf},
    str::FromStr,
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
    .env("PYO3_CROSS_PYTHON_VERSION", "3.12")
    .env("CARGO_PROFILE_RELEASE_LTO", "thin")
    .env("CARGO_PROFILE_RELEASE_OPT_LEVEL", "3")
    .env("CARGO_PROFILE_RELEASE_PANIC", "abort")
    .env("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "1")
    .run()?;

    run_if_changed(
        vec![format!("target/{TARGET}/release/promptkit_python.wasm")],
        format!("target/{TARGET}/release/promptkit_python.init.wasm"),
        |inp, out| -> Result<()> {
            let workdir = sh.create_temp_dir()?;
            #[cfg(feature = "static")]
            {
                let wasm = std::fs::read(&inp[0])?;
                let wasm = wizer::Wizer::new()
                    .allow_wasi(true)?
                    .wasm_bulk_memory(true)
                    .map_dir("/usr", "target/wasm32-wasip1/wasi-deps/usr")
                    .map_dir("/workdir", workdir.path())
                    .run(&wasm)?;

                std::fs::write(out, wasm)?;
            }

            #[cfg(not(feature = "static"))]
            {
                let workdir = workdir.path();
                let inp = &inp[0];
                cmd!(
                    sh,
                    "wizer --allow-wasi --wasm-bulk-memory true {inp} --mapdir /usr::target/wasm32-wasip1/wasi-deps/usr --mapdir /workdir::{workdir} -o {out}"
                ).run()?;
            }
            Ok(())
        },
    )?;

    run_if_changed(
        vec![format!(
            "target/{TARGET}/release/promptkit_python.init.wasm"
        )],
        format!("target/{TARGET}/release/promptkit_python.opt.wasm"),
        |inp, out| -> Result<()> {
            #[cfg(feature = "static")]
            {
                let tmp = sh.create_temp_dir()?;
                let mut tmp = tmp.path().to_path_buf();
                tmp.push("promptkit_python.tmp.wasm");

                wasm_opt::OptimizationOptions::new_opt_level_4()
                    .all_features()
                    .add_pass(wasm_opt::Pass::Gufa)
                    .run(&inp[0], &tmp)?;
                wasm_opt::OptimizationOptions::new_opt_level_4()
                    .all_features()
                    .add_pass(wasm_opt::Pass::StripDebug)
                    .run(tmp, out)?;
            }

            #[cfg(not(feature = "static"))]
            {
                let inp = &inp[0];
                cmd!(sh, "wasm-opt -O4 --gufa -O4 --strip-debug {inp} -o {out}").run()?;
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
                    include_bytes!("../wasi_snapshot_preview1.reactor.wasm"),
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
