use crate::sandbox::DirectoryMapping;

pub mod cache;
pub mod call;
pub mod compile;
pub mod configure;
pub mod embedded;
pub mod epoch;

#[derive(Clone, Debug)]
pub struct ModuleConfig {
    pub max_memory: usize,
    pub directory_mappings: Vec<DirectoryMapping>,
    pub env: Vec<(String, String)>,
    pub prelude: Option<String>,
}
