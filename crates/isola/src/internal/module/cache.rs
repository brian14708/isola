use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use wasmtime::Engine;

use super::ModuleConfig;
use crate::sandbox::{Error, Result};

fn engine_fingerprint(engine: &Engine) -> u64 {
    let mut hasher = DefaultHasher::new();
    engine.precompile_compatibility_hash().hash(&mut hasher);
    hasher.finish()
}

pub fn cache_key(engine: &Engine, cfg: &ModuleConfig, wasm_bytes: &[u8]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"isola-cache-v2\0");
    h.update(blake3::hash(wasm_bytes).as_bytes());
    h.update(&engine_fingerprint(engine).to_le_bytes());

    h.update(&(cfg.directory_mappings.len() as u64).to_le_bytes());
    for mapping in &cfg.directory_mappings {
        h.update(mapping.guest.as_bytes());
        h.update(&[0]);
        let host = mapping.host.to_string_lossy();
        h.update(host.as_bytes());
        h.update(&[0]);
        h.update(&[match mapping.perms {
            wasmtime_wasi::FsPerms::ReadOnly => 0,
            wasmtime_wasi::FsPerms::ReadWrite => 1,
        }]);
    }

    h.update(&(cfg.env.len() as u64).to_le_bytes());
    for (k, v) in &cfg.env {
        h.update(k.as_bytes());
        h.update(&[0]);
        h.update(v.as_bytes());
        h.update(&[0]);
    }

    if let Some(prelude) = &cfg.prelude {
        h.update(&[1]);
        h.update(prelude.as_bytes());
    } else {
        h.update(&[0]);
    }

    h.update(&(cfg.max_memory as u64).to_le_bytes());
    // Optimization level is fixed in `configure_engine`.
    h.update(&[1]);

    h.finalize().to_hex().to_string()
}

pub async fn write_cache_file_atomic(cache_path: &Path, bytes: &[u8]) -> Result<()> {
    static CACHE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let sequence = CACHE_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let tmp_path =
        cache_path.with_extension(format!("cwasm.tmp-{}-{sequence}", std::process::id()));

    tokio::fs::write(&tmp_path, bytes)
        .await
        .map_err(Error::from)?;
    match tokio::fs::rename(&tmp_path, cache_path).await {
        Ok(()) => Ok(()),
        // Windows doesn't atomically replace by default; treat a concurrent winner as success.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            Ok(())
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            Err(e.into())
        }
    }
}
