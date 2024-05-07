use super::{DecodeOption, EncodeOption, Encoding, Tokenizer, TokenizerError};

struct SentencePieceModel {
    inner: sentencepiece::SentencePieceProcessor,
}

pub fn load_spm(data: &[u8]) -> Result<impl Tokenizer, sentencepiece::SentencePieceError> {
    let inner = sentencepiece::SentencePieceProcessor::from_serialized_proto(data)?;
    Ok(SentencePieceModel { inner })
}

impl Tokenizer for SentencePieceModel {
    fn encode(&self, text: &str, _opts: &EncodeOption) -> Result<Encoding, TokenizerError> {
        let tokens = self.inner.encode(text)?;
        Ok(Encoding {
            ids: tokens.into_iter().map(|t| t.id).collect(),
        })
    }

    fn decode(&self, ids: &[u32], _opts: &DecodeOption) -> Result<String, TokenizerError> {
        Ok(self.inner.decode_piece_ids(ids)?)
    }

    fn special_token(&self, name: &str) -> Option<u32> {
        match name {
            "bos" => self.inner.bos_id(),
            "eos" => self.inner.eos_id(),
            "pad" => self.inner.pad_id(),
            "unk" => Some(self.inner.unk_id()),
            _ => None,
        }
    }
}
