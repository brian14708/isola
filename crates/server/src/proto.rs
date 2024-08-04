#[allow(clippy::pedantic)]
pub(crate) mod common {
    pub(crate) mod v1 {
        tonic::include_proto!("promptkit.common.v1");
    }
}

#[allow(clippy::pedantic)]
pub(crate) mod script {
    pub(crate) mod v1 {
        tonic::include_proto!("promptkit.script.v1");
    }
}

pub(crate) const FILE_DESCRIPTOR_SET: &[u8] =
    tonic::include_file_descriptor_set!("promptkit_descriptor");
