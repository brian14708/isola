use isola::value::Value;
use napi_derive::napi;
use parking_lot::Mutex;

use crate::error::{Error, invalid_argument};

const DEFAULT_STREAM_CAPACITY: usize = 1024;

#[napi]
pub struct StreamHandle {
    sender: Mutex<Option<tokio::sync::mpsc::Sender<Value>>>,
    receiver: Mutex<Option<tokio::sync::mpsc::Receiver<Value>>>,
}

impl StreamHandle {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity);
        Self {
            sender: Mutex::new(Some(sender)),
            receiver: Mutex::new(Some(receiver)),
        }
    }

    pub(crate) fn take_receiver(&self) -> crate::error::Result<tokio::sync::mpsc::Receiver<Value>> {
        self.receiver
            .lock()
            .take()
            .ok_or_else(|| invalid_argument("stream receiver already taken"))
    }

    pub(crate) fn restore_receiver(&self, receiver: tokio::sync::mpsc::Receiver<Value>) {
        let mut slot = self.receiver.lock();
        debug_assert!(slot.is_none());
        *slot = Some(receiver);
    }

    fn sender(&self) -> crate::error::Result<tokio::sync::mpsc::Sender<Value>> {
        self.sender
            .lock()
            .as_ref()
            .cloned()
            .ok_or(Error::StreamClosed)
    }

    fn try_send(&self, value: Value) -> crate::error::Result<()> {
        let sender_guard = self.sender.lock();
        let sender = sender_guard.as_ref().ok_or(Error::StreamClosed)?;
        let result = sender.try_send(value);
        drop(sender_guard);
        match result {
            Ok(()) => Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Err(Error::StreamFull),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => Err(Error::StreamClosed),
        }
    }
}

#[napi]
impl StreamHandle {
    #[napi(constructor)]
    pub fn new(capacity: Option<u32>) -> napi::Result<Self> {
        let capacity = capacity.map_or(DEFAULT_STREAM_CAPACITY, |c| c as usize);
        if capacity == 0 {
            return Err(napi::Error::from(invalid_argument(
                "stream capacity must be greater than 0",
            )));
        }

        Ok(Self::with_capacity(capacity))
    }

    #[napi]
    pub fn push(&self, value: serde_json::Value) -> napi::Result<()> {
        let value = {
            let json = value;
            Value::from_serde(&json)
                .map_err(|e| napi::Error::from(invalid_argument(format!("invalid value: {e}"))))?
        };
        self.try_send(value).map_err(napi::Error::from)
    }

    #[napi]
    pub async fn push_async(&self, value: serde_json::Value) -> napi::Result<()> {
        let value = Value::from_serde(&value)
            .map_err(|e| napi::Error::from(invalid_argument(format!("invalid value: {e}"))))?;
        let sender = self.sender().map_err(napi::Error::from)?;

        sender
            .send(value)
            .await
            .map_err(|_| napi::Error::from(Error::StreamClosed))
    }

    #[napi]
    pub fn end(&self) {
        self.sender.lock().take();
    }
}
