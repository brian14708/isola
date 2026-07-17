use std::{
    collections::hash_map::DefaultHasher,
    env,
    hash::Hasher,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result};
use xshell::{Shell, cmd};

use crate::async_shim::link_library;

mod async_shim;

const TARGET: &str = "wasm32-wasip1";
const COMPONENT_FINGERPRINT_VERSION: &[u8] = b"isola-component-v1";
const COMPONENT_BUILD_INPUTS: &[(&str, &[u8])] = &[
    ("crates/xtask/src/main.rs", include_bytes!("main.rs")),
    (
        "crates/xtask/src/async_shim.rs",
        include_bytes!("async_shim.rs"),
    ),
    ("crates/xtask/Cargo.toml", include_bytes!("../Cargo.toml")),
    ("Cargo.toml", include_bytes!("../../../Cargo.toml")),
    ("Cargo.lock", include_bytes!("../../../Cargo.lock")),
];

struct ComponentLibrary {
    name: String,
    path: PathBuf,
    dlopen: bool,
    async_shim_name: Option<&'static str>,
}

impl ComponentLibrary {
    fn new(
        name: impl Into<String>,
        path: impl AsRef<Path>,
        dlopen: bool,
        async_shim_name: Option<&'static str>,
    ) -> Self {
        Self {
            name: name.into(),
            path: path.as_ref().to_path_buf(),
            dlopen,
            async_shim_name,
        }
    }

    fn load(self) -> Result<LoadedComponentLibrary> {
        let module = std::fs::read(&self.path)
            .with_context(|| format!("failed to read component library {}", self.path.display()))?;
        Ok(LoadedComponentLibrary {
            name: self.name,
            module,
            dlopen: self.dlopen,
            async_shim_name: self.async_shim_name,
        })
    }
}

struct LoadedComponentLibrary {
    name: String,
    module: Vec<u8>,
    dlopen: bool,
    async_shim_name: Option<&'static str>,
}

struct FingerprintHasher {
    first: DefaultHasher,
    second: DefaultHasher,
}

impl FingerprintHasher {
    fn new() -> Self {
        let first = DefaultHasher::new();
        let mut second = DefaultHasher::new();
        second.write_u8(1);
        Self { first, second }
    }

    fn write(&mut self, value: &[u8]) {
        self.first.write(value);
        self.second.write(value);
    }

    fn finish(self) -> String {
        format!("{:016x}{:016x}", self.first.finish(), self.second.finish())
    }
}

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
         -Lnative={wasi_deps_dir}/lib"
    )
}

fn build_python(sh: &Shell) -> Result<()> {
    let wasi_deps_dir = env::var("WASI_PYTHON_DEV").unwrap();
    let rustflags = wasm_rustflags(&wasi_deps_dir);

    cmd!(
        sh,
        "cargo build --locked -Z build-std=std,panic_abort --release --target {TARGET} -p isola-python-runtime"
    )
    .env("PYO3_CROSS_PYTHON_VERSION", "3.14")
    .env("RUSTFLAGS", &rustflags)
    .run()?;

    let runtime = PathBuf::from(format!("target/{TARGET}/release/isola_python_runtime.wasm"));
    let libraries = python_libraries(Path::new(&wasi_deps_dir), &runtime)?;
    write_component_if_changed(libraries, Path::new("target/python.wasm"), 8_388_608)?;

    Ok(())
}

fn build_js(sh: &Shell) -> Result<()> {
    let wasi_deps_dir = env::var("WASI_PYTHON_DEV").unwrap();
    let rustflags = wasm_rustflags(&wasi_deps_dir);

    cmd!(
        sh,
        "cargo build --locked -Z build-std=std,panic_abort --release --target {TARGET} -p isola-js-runtime"
    )
    .env("RUSTFLAGS", &rustflags)
    .run()?;

    let runtime = PathBuf::from(format!("target/{TARGET}/release/isola_js_runtime.wasm"));
    let libraries = js_libraries(Path::new(&wasi_deps_dir), &runtime);
    write_component_if_changed(libraries, Path::new("target/js.wasm"), 2_097_152)?;

    Ok(())
}

fn python_libraries(wasi_deps_dir: &Path, runtime: &Path) -> Result<Vec<ComponentLibrary>> {
    let lib_dir = wasi_deps_dir.join("lib");
    let mut libraries = vec![
        ComponentLibrary::new(
            "libisola_python.so",
            runtime,
            false,
            Some("libisola_python_async.so"),
        ),
        ComponentLibrary::new("libc.so", lib_dir.join("libc.so"), false, None),
        ComponentLibrary::new(
            "libwasi-emulated-signal.so",
            lib_dir.join("libwasi-emulated-signal.so"),
            false,
            None,
        ),
        ComponentLibrary::new(
            "libwasi-emulated-getpid.so",
            lib_dir.join("libwasi-emulated-getpid.so"),
            false,
            None,
        ),
        ComponentLibrary::new(
            "libwasi-emulated-process-clocks.so",
            lib_dir.join("libwasi-emulated-process-clocks.so"),
            false,
            None,
        ),
        ComponentLibrary::new("libc++.so", lib_dir.join("libc++.so"), false, None),
        ComponentLibrary::new("libc++abi.so", lib_dir.join("libc++abi.so"), false, None),
        ComponentLibrary::new(
            "libpython3.14.so",
            lib_dir.join("libpython3.14.so"),
            false,
            None,
        ),
    ];

    let site_packages = lib_dir.join("python3.14/site-packages");
    let pattern = site_packages.join("**/*.so");
    let pattern = pattern
        .to_str()
        .context("WASI Python dependency path is not valid UTF-8")?;
    let mut extension_paths = Vec::new();
    for entry in glob::glob(pattern)? {
        extension_paths.push(entry?);
    }
    extension_paths.sort();

    for path in extension_paths {
        let relative = path
            .strip_prefix(wasi_deps_dir)
            .with_context(|| format!("{} is outside the linker input root", path.display()))?;
        let name = format!("/{}", relative.to_string_lossy().replace('\\', "/"));
        libraries.push(ComponentLibrary::new(name, path, true, None));
    }

    Ok(libraries)
}

fn js_libraries(wasi_deps_dir: &Path, runtime: &Path) -> Vec<ComponentLibrary> {
    let lib_dir = wasi_deps_dir.join("lib");
    vec![
        ComponentLibrary::new(
            "libisola_js.so",
            runtime,
            false,
            Some("libisola_js_async.so"),
        ),
        ComponentLibrary::new("libc.so", lib_dir.join("libc.so"), false, None),
        ComponentLibrary::new(
            "libwasi-emulated-signal.so",
            lib_dir.join("libwasi-emulated-signal.so"),
            false,
            None,
        ),
        ComponentLibrary::new(
            "libwasi-emulated-getpid.so",
            lib_dir.join("libwasi-emulated-getpid.so"),
            false,
            None,
        ),
        ComponentLibrary::new(
            "libwasi-emulated-process-clocks.so",
            lib_dir.join("libwasi-emulated-process-clocks.so"),
            false,
            None,
        ),
    ]
}

fn hash_field(hasher: &mut FingerprintHasher, name: &str, value: &[u8]) {
    hasher.write(&u64::try_from(name.len()).unwrap().to_le_bytes());
    hasher.write(name.as_bytes());
    hasher.write(&u64::try_from(value.len()).unwrap().to_le_bytes());
    hasher.write(value);
}

fn component_hasher(stack_size: u32) -> FingerprintHasher {
    let mut hasher = FingerprintHasher::new();
    hash_field(
        &mut hasher,
        "fingerprint-version",
        COMPONENT_FINGERPRINT_VERSION,
    );
    hash_field(&mut hasher, "stack-size", &stack_size.to_le_bytes());
    hash_field(
        &mut hasher,
        "adapter-name",
        wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME.as_bytes(),
    );
    hash_field(
        &mut hasher,
        "adapter-module",
        wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
    );
    for (name, contents) in COMPONENT_BUILD_INPUTS {
        hash_field(&mut hasher, name, contents);
    }
    hasher
}

fn component_metadata_fingerprint(
    libraries: &[ComponentLibrary],
    stack_size: u32,
) -> Result<String> {
    let mut hasher = component_hasher(stack_size);
    for library in libraries {
        hash_field(&mut hasher, "library-name", library.name.as_bytes());
        hash_field(
            &mut hasher,
            "library-path",
            library.path.to_string_lossy().as_bytes(),
        );
        hash_field(&mut hasher, "library-dlopen", &[u8::from(library.dlopen)]);
        hash_field(
            &mut hasher,
            "library-async-shim",
            library.async_shim_name.unwrap_or_default().as_bytes(),
        );
        let metadata = library.path.metadata().with_context(|| {
            format!(
                "failed to read component library metadata {}",
                library.path.display()
            )
        })?;
        hash_field(&mut hasher, "library-size", &metadata.len().to_le_bytes());
        let (after_epoch, modified) = match metadata.modified()?.duration_since(UNIX_EPOCH) {
            Ok(modified) => (true, modified),
            Err(error) => (false, error.duration()),
        };
        hash_field(
            &mut hasher,
            "library-modified-after-epoch",
            &[u8::from(after_epoch)],
        );
        hash_field(
            &mut hasher,
            "library-modified-seconds",
            &modified.as_secs().to_le_bytes(),
        );
        hash_field(
            &mut hasher,
            "library-modified-nanoseconds",
            &modified.subsec_nanos().to_le_bytes(),
        );
    }
    Ok(hasher.finish())
}

fn component_content_fingerprint(libraries: &[LoadedComponentLibrary], stack_size: u32) -> String {
    let mut hasher = component_hasher(stack_size);
    for library in libraries {
        hash_field(&mut hasher, "library-name", library.name.as_bytes());
        hash_field(&mut hasher, "library-module", &library.module);
        hash_field(&mut hasher, "library-dlopen", &[u8::from(library.dlopen)]);
        hash_field(
            &mut hasher,
            "library-async-shim",
            library.async_shim_name.unwrap_or_default().as_bytes(),
        );
    }
    hasher.finish()
}

fn write_component_fingerprints(
    path: &Path,
    metadata_fingerprint: &str,
    content_fingerprint: &str,
) -> Result<()> {
    std::fs::write(
        path,
        format!("{metadata_fingerprint}\n{content_fingerprint}\n"),
    )
    .with_context(|| format!("failed to write component fingerprint {}", path.display()))
}

fn write_component_if_changed(
    libraries: Vec<ComponentLibrary>,
    output: &Path,
    stack_size: u32,
) -> Result<()> {
    let metadata_fingerprint = component_metadata_fingerprint(&libraries, stack_size)?;
    let fingerprint_path = output.with_extension("wasm.fingerprint");
    let cached = std::fs::read_to_string(&fingerprint_path).unwrap_or_default();
    let mut cached = cached.lines();
    let cached_metadata_fingerprint = cached.next().unwrap_or_default();
    let cached_content_fingerprint = cached.next().unwrap_or_default();
    if output.is_file() && cached_metadata_fingerprint == metadata_fingerprint {
        return Ok(());
    }

    let libraries = libraries
        .into_iter()
        .map(ComponentLibrary::load)
        .collect::<Result<Vec<_>>>()?;
    let content_fingerprint = component_content_fingerprint(&libraries, stack_size);
    if output.is_file() && cached_content_fingerprint == content_fingerprint {
        write_component_fingerprints(
            &fingerprint_path,
            &metadata_fingerprint,
            &content_fingerprint,
        )?;
        return Ok(());
    }

    println!("Linking {}", output.display());
    let mut linker = wit_component::Linker::default()
        .validate(true)
        .stack_size(stack_size)
        .use_built_in_libdl(true);
    for library in libraries {
        linker = link_library(
            linker,
            &library.name,
            &library.module,
            library.dlopen,
            library.async_shim_name,
        )?;
    }
    let component = linker
        .adapter(
            wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME,
            wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
        )?
        .encode()?;

    std::fs::write(output, component)
        .with_context(|| format!("failed to write component {}", output.display()))?;
    write_component_fingerprints(
        &fingerprint_path,
        &metadata_fingerprint,
        &content_fingerprint,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(
        name: &str,
        module: &[u8],
        dlopen: bool,
        async_shim_name: Option<&'static str>,
    ) -> LoadedComponentLibrary {
        LoadedComponentLibrary {
            name: name.to_string(),
            module: module.to_vec(),
            dlopen,
            async_shim_name,
        }
    }

    #[test]
    fn component_content_fingerprint_tracks_every_link_input() {
        let original = component_content_fingerprint(
            &[library("runtime.so", b"runtime", false, Some("shim.so"))],
            1024,
        );

        assert_ne!(
            original,
            component_content_fingerprint(
                &[library("renamed.so", b"runtime", false, Some("shim.so"))],
                1024
            )
        );
        assert_ne!(
            original,
            component_content_fingerprint(
                &[library("runtime.so", b"changed", false, Some("shim.so"))],
                1024
            )
        );
        assert_ne!(
            original,
            component_content_fingerprint(
                &[library("runtime.so", b"runtime", true, Some("shim.so"))],
                1024
            )
        );
        assert_ne!(
            original,
            component_content_fingerprint(&[library("runtime.so", b"runtime", false, None)], 1024)
        );
        assert_ne!(
            original,
            component_content_fingerprint(
                &[library("runtime.so", b"runtime", false, Some("shim.so"))],
                2048
            )
        );
    }
}
