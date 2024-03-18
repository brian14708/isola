use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("[{0}] {1}")]
    ExecutionError(u16, String),
}
