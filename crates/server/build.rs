use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    tonic_prost_build::configure()
        .build_client(false)
        .file_descriptor_set_path(out_dir.join("promptkit_descriptor.bin"))
        .compile_protos(
            &[
                "../../specs/grpc/promptkit/script/v1/service.proto",
                "../../specs/grpc/promptkit/script/v1/error_code.proto",
            ],
            &["../../specs/grpc"],
        )?;
    Ok(())
}
