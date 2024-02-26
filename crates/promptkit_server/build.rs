use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    tonic_build::configure()
        .file_descriptor_set_path(out_dir.join("promptkit_script_v1_descriptor.bin"))
        .compile(&["proto/promptkit/script/v1/service.proto"], &(["proto"]))?;
    Ok(())
}
