use std::env;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let bindings = cbindgen::generate(crate_dir).unwrap();
    bindings.write_to_file("include/isola.h");
}
