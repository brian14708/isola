use std::{
    env,
    fs::remove_dir_all,
    path::{Path, PathBuf},
    str::FromStr,
    vec,
};

use anyhow::Result;
use xshell::{Shell, cmd};

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
const TASKS: &[(&str, Task)] = &[("build-all", build_all), ("build-python", build_python)];

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
        "cargo b -Z build-std=std,panic_abort --profile release --target {TARGET} -p promptkit-python"
    )
    .env("PYO3_CROSS_PYTHON_VERSION", "3.13")
    .env("RUSTFLAGS", "-C relocation-model=pic")
    .run()?;

    run_if_changed(
        vec!["crates/python/requirements.txt".to_string()],
        format!("target/{TARGET}/python-deps/setuptools/__init__.py"),
        |_, _| {
            let _ = remove_dir_all("target/{TARGET}/python-deps");

            let r = cmd!(
                sh,
                "uv pip install -U -r crates/python/requirements.txt --no-deps --target target/{TARGET}/python-deps"
            ).run();

            if r.is_err() {
                cmd!(
                sh,
                "pip install -U -r crates/python/requirements.txt --no-deps --target target/{TARGET}/python-deps"
            ).run()?;
            }

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
            fn lib(
                name: impl Into<String>,
                path: impl AsRef<Path>,
                dlopen: bool,
            ) -> (String, PathBuf, bool) {
                (name.into(), path.as_ref().to_path_buf(), dlopen)
            }

            let mut libs = vec![
                lib("", &inp[0], false),
                lib(
                    "libc.so",
                    format!("target/{TARGET}/wasi-deps/lib/libc.so"),
                    false,
                ),
                lib(
                    "libwasi-emulated-signal.so",
                    format!("target/{TARGET}/wasi-deps/lib/libwasi-emulated-signal.so"),
                    false,
                ),
                lib(
                    "libwasi-emulated-getpid.so",
                    format!("target/{TARGET}/wasi-deps/lib/libwasi-emulated-getpid.so"),
                    false,
                ),
                lib(
                    "libwasi-emulated-process-clocks.so",
                    format!("target/{TARGET}/wasi-deps/lib/libwasi-emulated-process-clocks.so"),
                    false,
                ),
                lib(
                    "libc++.so",
                    format!("target/{TARGET}/wasi-deps/lib/libc++.so"),
                    false,
                ),
                lib(
                    "libc++abi.so",
                    format!("target/{TARGET}/wasi-deps/lib/libc++abi.so"),
                    false,
                ),
                lib(
                    "libpython3.13.so",
                    format!("target/{TARGET}/wasi-deps/lib/libpython3.13.so"),
                    false,
                ),
            ];
            let base = format!("target/{TARGET}/wasi-deps/usr/local/lib/python3.13/site-packages/");
            for entry in glob::glob(&format!("{base}/**/*.so"))? {
                let entry = entry?;
                let filename = entry
                    .to_str()
                    .unwrap()
                    .replace(&base, "/usr/local/lib/python3.13/site-packages/");
                libs.push(lib(filename, entry, true));
            }

            let mut wasm = wit_component::Linker::default()
                .validate(true)
                .stack_size(8388608)
                .use_built_in_libdl(true);
            for lib in libs {
                let filename = lib.1.file_name().unwrap().to_str().unwrap();
                let outname = format!("target/{TARGET}/wasi-deps/lib/opt.{filename}");
                run_if_changed(
                    vec![lib.1.to_str().unwrap().to_string()],
                    outname.clone(),
                    |inp, out| -> Result<()> {
                        let inp = inp[0].clone();
                        cmd!(sh, "wasm-opt {inp} -all -g -O4 --strip-debug -o {out}").run()?;
                        Ok(())
                    },
                )?;
                let data = &std::fs::read(outname)?;
                wasm = wasm.library(&lib.0, data, lib.2)?;
            }
            let wasm = wasm
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
