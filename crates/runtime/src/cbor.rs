use std::convert::Infallible;

use crate::isola::script::host::EmitType;

/// Default chunk size for streamed CBOR output.
///
/// Each full chunk crosses the host boundary as a separate `blocking-emit`
/// call, so small chunks dominate large-value latency while large chunks
/// delay when the first bytes reach the host.
pub const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;

/// A bounded CBOR writer that streams full chunks and marks the final chunk.
pub struct CallbackWriter<'a, F>
where
    F: FnMut(EmitType, &[u8]),
{
    buffer: Vec<u8>,
    chunk_size: usize,
    emit: &'a mut F,
    end_type: EmitType,
    finished: bool,
}

impl<'a, F> CallbackWriter<'a, F>
where
    F: FnMut(EmitType, &[u8]),
{
    #[must_use]
    pub fn new(emit: &'a mut F, end_type: EmitType) -> Self {
        Self::with_chunk_size(emit, end_type, DEFAULT_CHUNK_SIZE)
    }

    #[must_use]
    pub fn with_chunk_size(emit: &'a mut F, end_type: EmitType, chunk_size: usize) -> Self {
        let chunk_size = chunk_size.max(1);
        Self {
            buffer: Vec::with_capacity(chunk_size),
            chunk_size,
            emit,
            end_type,
            finished: false,
        }
    }

    fn flush(&mut self) {
        if !self.buffer.is_empty() {
            (self.emit)(EmitType::Continuation, &self.buffer);
            self.buffer.clear();
        }
    }

    /// Emit the buffered final chunk after serialization succeeds.
    pub fn finish(mut self) {
        (self.emit)(self.end_type, &self.buffer);
        self.finished = true;
    }
}

impl<F> Drop for CallbackWriter<'_, F>
where
    F: FnMut(EmitType, &[u8]),
{
    fn drop(&mut self) {
        if !self.finished {
            (self.emit)(EmitType::Abort, &[]);
        }
    }
}

impl<F> minicbor::encode::Write for CallbackWriter<'_, F>
where
    F: FnMut(EmitType, &[u8]),
{
    type Error = Infallible;

    fn write_all(&mut self, mut bytes: &[u8]) -> Result<(), Self::Error> {
        while !bytes.is_empty() {
            let available = self.chunk_size - self.buffer.len();
            if available == 0 {
                self.flush();
                continue;
            }

            let written = bytes.len().min(available);
            let (chunk, remaining) = bytes.split_at(written);
            self.buffer.extend_from_slice(chunk);
            bytes = remaining;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use minicbor::encode::Write as _;

    use super::{CallbackWriter, EmitType};

    #[test]
    fn finish_emits_continuations_and_final_chunk() {
        let mut emissions = Vec::new();
        let mut emit = |emit_type, bytes: &[u8]| emissions.push((emit_type, bytes.to_vec()));
        let mut writer = CallbackWriter::with_chunk_size(&mut emit, EmitType::End, 4);
        writer.write_all(b"abcdef").unwrap();
        writer.finish();

        assert_eq!(
            emissions,
            vec![
                (EmitType::Continuation, b"abcd".to_vec()),
                (EmitType::End, b"ef".to_vec()),
            ]
        );
    }

    #[test]
    fn drop_aborts_partial_output() {
        let mut emissions = Vec::new();
        let mut emit = |emit_type, bytes: &[u8]| emissions.push((emit_type, bytes.to_vec()));
        {
            let mut writer = CallbackWriter::with_chunk_size(&mut emit, EmitType::End, 4);
            writer.write_all(b"abcdef").unwrap();
        }

        assert_eq!(
            emissions,
            vec![
                (EmitType::Continuation, b"abcd".to_vec()),
                (EmitType::Abort, Vec::new()),
            ]
        );
    }
}
