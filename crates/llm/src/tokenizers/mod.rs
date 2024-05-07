mod spm;

pub use spm::load_spm;
use thiserror::Error;

#[derive(Debug)]
pub struct Encoding {
    pub ids: Vec<u32>,
}

#[derive(serde::Deserialize)]
pub struct EncodeOption {}

#[derive(serde::Deserialize)]
pub struct DecodeOption {}

pub trait Tokenizer {
    fn encode(&self, text: &str, opts: &EncodeOption) -> Result<Encoding, TokenizerError>;

    fn decode(&self, ids: &[u32], opts: &DecodeOption) -> Result<String, TokenizerError>;

    fn special_token(&self, name: &str) -> Option<u32>;
}

#[derive(Error, Debug)]
pub enum TokenizerError {
    #[error("SentencePiece error: {0}")]
    SentencePiece(#[from] sentencepiece::SentencePieceError),
}
