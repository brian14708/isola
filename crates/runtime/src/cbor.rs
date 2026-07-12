use std::convert::Infallible;

use crate::isola::script::host::EmitType;

/// A bounded CBOR writer that streams full chunks and marks the final chunk.
pub struct CallbackWriter<'a, F, const N: usize = 1024>
where
    F: FnMut(EmitType, &[u8]),
{
    buffer: heapless::Vec<u8, N>,
    emit: &'a mut F,
    end_type: EmitType,
    finished: bool,
}

impl<'a, F, const N: usize> CallbackWriter<'a, F, N>
where
    F: FnMut(EmitType, &[u8]),
{
    #[must_use]
    pub const fn new(emit: &'a mut F, end_type: EmitType) -> Self {
        Self {
            buffer: heapless::Vec::new(),
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

impl<F, const N: usize> Drop for CallbackWriter<'_, F, N>
where
    F: FnMut(EmitType, &[u8]),
{
    fn drop(&mut self) {
        if !self.finished {
            (self.emit)(EmitType::Abort, &[]);
        }
    }
}

impl<F, const N: usize> minicbor::encode::Write for CallbackWriter<'_, F, N>
where
    F: FnMut(EmitType, &[u8]),
{
    type Error = Infallible;

    fn write_all(&mut self, mut bytes: &[u8]) -> Result<(), Self::Error> {
        while !bytes.is_empty() {
            let available = N - self.buffer.len();
            if available == 0 {
                self.flush();
                continue;
            }

            let written = bytes.len().min(available);
            let (chunk, remaining) = bytes.split_at(written);
            self.buffer.extend_from_slice(chunk).ok();
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
        let mut writer: CallbackWriter<_, 4> = CallbackWriter::new(&mut emit, EmitType::End);
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
            let mut writer: CallbackWriter<_, 4> = CallbackWriter::new(&mut emit, EmitType::End);
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
