use wasmtime::Config;

pub fn configure_engine(cfg: &mut Config) {
    cfg.epoch_interruption(true);
    cfg.table_lazy_init(false);
    cfg.generate_address_map(false);
    cfg.wasm_backtrace_max_frames(None);
    cfg.wasm_branch_hinting(true);
    // Wasmtime rejects `native_unwind_info(false)` on Windows (ABI requires unwind
    // info).
    #[cfg(not(target_os = "windows"))]
    cfg.native_unwind_info(false);
    cfg.cranelift_opt_level(wasmtime::OptLevel::Speed);
}
