use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("starlark error: {0}")]
    Starlark(anyhow::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
