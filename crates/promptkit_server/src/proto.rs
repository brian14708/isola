#[allow(clippy::pedantic)]
pub mod script {
    tonic::include_proto!("promptkit.script.v1");

    pub(crate) const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("promptkit_script_v1_descriptor");
}
