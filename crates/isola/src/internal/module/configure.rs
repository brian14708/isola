use wasmtime::Config;

pub fn configure_engine(cfg: &mut Config) {
    cfg.wasm_component_model(true);
    cfg.epoch_interruption(true);
    cfg.table_lazy_init(false);
    cfg.generate_address_map(false);
    cfg.wasm_backtrace(false);
    cfg.native_unwind_info(false);
    cfg.cranelift_opt_level(wasmtime::OptLevel::Speed);
}
