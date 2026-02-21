use std::path::PathBuf;

use crate::sandbox::DirectoryMapping;

pub mod cache;
pub mod call;
pub mod compile;
pub mod configure;
pub mod epoch;

#[derive(Clone, Debug)]
pub struct ModuleConfig {
    pub cache: Option<PathBuf>,
    pub max_memory: usize,
    pub directory_mappings: Vec<DirectoryMapping>,
    pub env: Vec<(String, String)>,
    pub prelude: Option<String>,
}
