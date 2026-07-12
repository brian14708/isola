/// Reset process state that must not be retained in a preinitialized runtime.
pub fn reset_preinitialized_state() {
    #[link(wasm_import_module = "wasi_snapshot_preview1")]
    unsafe extern "C" {
        #[cfg_attr(target_arch = "wasm32", link_name = "reset_adapter_state")]
        fn reset_adapter_state();
    }

    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        #[cfg_attr(target_arch = "wasm32", link_name = "__wasilibc_reset_preopens")]
        fn wasilibc_reset_preopens();
    }

    unsafe {
        reset_adapter_state();
        wasilibc_reset_preopens();
    }
    crate::time::reset_monotonic();
}
