/// Reset process state that must not be retained in a preinitialized runtime.
pub fn reset_preinitialized_state() {
    // The preview1 adapter keeps its descriptor table in guest memory, so it
    // survives a wizer snapshot and must be reset per instance. On wasip2 there
    // is no adapter — all WASI state is host-side and constructed fresh per
    // instantiation.
    #[cfg(target_env = "p1")]
    {
        #[link(wasm_import_module = "wasi_snapshot_preview1")]
        unsafe extern "C" {
            #[cfg_attr(target_arch = "wasm32", link_name = "reset_adapter_state")]
            fn reset_adapter_state();
        }
        unsafe {
            reset_adapter_state();
        }
    }

    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        #[cfg_attr(target_arch = "wasm32", link_name = "__wasilibc_reset_preopens")]
        fn wasilibc_reset_preopens();
    }

    unsafe {
        wasilibc_reset_preopens();
    }
    crate::pending::clear();
    crate::pending::clear_http_streams();
    crate::time::reset_monotonic();
}

/// Re-read the process environment from the host.
///
/// wasi-libc fills `environ` once, eagerly, when it is first referenced. In a
/// preinitialized runtime that copy is baked into the snapshot, so every
/// instance would see the template's environment instead of its own. Call
/// this once per instance, before guest code observes the environment.
pub fn refresh_environ() {
    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        #[cfg_attr(target_arch = "wasm32", link_name = "__wasilibc_deinitialize_environ")]
        fn wasilibc_deinitialize_environ();
        #[cfg_attr(target_arch = "wasm32", link_name = "__wasilibc_initialize_environ")]
        fn wasilibc_initialize_environ();
    }

    unsafe {
        wasilibc_deinitialize_environ();
        wasilibc_initialize_environ();
    }
}
