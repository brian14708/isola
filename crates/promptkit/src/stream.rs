use std::{
    fmt::{self, Debug, Display},
    sync::Mutex,
};

use allocative::Allocative;
use serde_json::Value;

use starlark::values::{
    starlark_value, AllocValue, Heap, NoSerialize, ProvidesStaticType, StarlarkValue,
    Value as StarlarkAllocValue,
};
use tokio::sync::mpsc::Receiver;

#[derive(Allocative, NoSerialize, ProvidesStaticType)]
pub struct Stream {
    #[allocative(skip)]
    pub stream: Box<dyn BlockingRecv>,
}

impl Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StarlarkStream").finish()
    }
}

impl Display for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StarlarkStream")
    }
}

impl<'v> AllocValue<'v> for Stream {
    fn alloc_value(self, heap: &'v Heap) -> StarlarkAllocValue<'v> {
        heap.alloc_simple(self)
    }
}

#[starlark_value(type = "stream")]
impl<'v> StarlarkValue<'v> for Stream {
    unsafe fn iterate(
        &self,
        me: StarlarkAllocValue<'v>,
        _heap: &'v Heap,
    ) -> anyhow::Result<StarlarkAllocValue<'v>> {
        Ok(me)
    }

    unsafe fn iter_next(&self, _index: usize, heap: &'v Heap) -> Option<StarlarkAllocValue<'v>> {
        self.stream.blocking_recv().map(|v| heap.alloc(v))
    }

    unsafe fn iter_stop(&self) {}
}

pub trait BlockingRecv: Send + Sync {
    fn blocking_recv(&self) -> Option<Value>;
}

impl BlockingRecv for Mutex<Receiver<Value>> {
    fn blocking_recv(&self) -> Option<Value> {
        self.lock().unwrap().blocking_recv()
    }
}

impl From<Receiver<Value>> for Box<dyn BlockingRecv> {
    fn from(val: Receiver<Value>) -> Self {
        Box::new(Mutex::new(val))
    }
}
