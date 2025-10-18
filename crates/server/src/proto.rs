#[allow(clippy::pedantic, clippy::nursery)]
pub mod script {
    pub mod v1 {
        tonic::include_proto!("promptkit.script.v1");
    }
}

pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("promptkit_descriptor");
