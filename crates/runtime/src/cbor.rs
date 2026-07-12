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
        }
    }

    fn flush(&mut self) {
        if !self.buffer.is_empty() {
            (self.emit)(EmitType::Continuation, &self.buffer);
            self.buffer.clear();
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

impl<F, const N: usize> Drop for CallbackWriter<'_, F, N>
where
    F: FnMut(EmitType, &[u8]),
{
    fn drop(&mut self) {
        (self.emit)(self.end_type, &self.buffer);
    }
}
