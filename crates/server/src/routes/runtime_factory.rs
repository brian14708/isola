use std::{path::Path, sync::Arc};

use anyhow::anyhow;
use isola::host::Host;
use serde::{Deserialize, Serialize};
use tracing::info;
use utoipa::ToSchema;

use super::SandboxManager;

const PYTHON_WASM_PATH: &str = "target/python3.wasm";
const JS_WASM_PATH: &str = "target/js.wasm";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Python,
    Js,
}

impl Runtime {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Js => "js",
        }
    }

    pub const fn default_wasm_path(self) -> &'static str {
        match self {
            Self::Python => PYTHON_WASM_PATH,
            Self::Js => JS_WASM_PATH,
        }
    }
}

pub struct RuntimeFactory<E: Host + Clone> {
    python: Option<Arc<SandboxManager<E>>>,
    js: Option<Arc<SandboxManager<E>>>,
}

impl<E: Host + Clone> RuntimeFactory<E> {
    async fn load_manager_if_available(
        runtime: Runtime,
    ) -> anyhow::Result<Option<Arc<SandboxManager<E>>>> {
        let path = Path::new(runtime.default_wasm_path());
        if !path.is_file() {
            info!(
                runtime = runtime.as_str(),
                wasm_path = %path.display(),
                "Runtime bundle not found; runtime disabled"
            );
            return Ok(None);
        }

        info!(
            runtime = runtime.as_str(),
            wasm_path = %path.display(),
            "Loading runtime bundle"
        );
        Ok(Some(Arc::new(SandboxManager::new(path).await?)))
    }

    pub async fn new() -> anyhow::Result<Self> {
        let python = Self::load_manager_if_available(Runtime::Python).await?;
        let js = Self::load_manager_if_available(Runtime::Js).await?;

        if python.is_none() && js.is_none() {
            return Err(anyhow!(
                "No runtime bundles found. Build at least one runtime bundle: \
                 `cargo xtask build-python` (target/python3.wasm) or \
                 `cargo xtask build-js` (target/js.wasm)."
            ));
        }

        let available = {
            let mut values = Vec::new();
            if python.is_some() {
                values.push(Runtime::Python.as_str());
            }
            if js.is_some() {
                values.push(Runtime::Js.as_str());
            }
            values.join(",")
        };

        info!(available_runtimes = %available, "RuntimeFactory initialized");

        Ok(Self { python, js })
    }

    pub fn manager_for(&self, runtime: Runtime) -> anyhow::Result<Arc<SandboxManager<E>>> {
        let manager = match runtime {
            Runtime::Python => self.python.clone(),
            Runtime::Js => self.js.clone(),
        };

        manager.ok_or_else(|| {
            anyhow!(
                "Requested runtime '{}' is not available on this server. \
                 Build the '{}' bundle at '{}'.",
                runtime.as_str(),
                runtime.as_str(),
                runtime.default_wasm_path()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Runtime;

    #[test]
    fn runtime_deserializes_from_lowercase_name() {
        let runtime: Runtime = serde_json::from_str("\"python\"").expect("must deserialize");
        assert!(matches!(runtime, Runtime::Python));

        let runtime: Runtime = serde_json::from_str("\"js\"").expect("must deserialize");
        assert!(matches!(runtime, Runtime::Js));
    }

    #[test]
    fn runtime_uses_expected_default_bundle_paths() {
        assert_eq!(Runtime::Python.default_wasm_path(), "target/python3.wasm");
        assert_eq!(Runtime::Js.default_wasm_path(), "target/js.wasm");
    }
}
