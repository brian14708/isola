use serde_json::Value;
use std::sync::Mutex;
use tokio::sync::mpsc::Receiver;

use rustpython_vm::pymodule;

pub(crate) use _stream::make_module;
pub(crate) use _stream::Stream;

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
#[pymodule]
mod _stream {
    use std::fmt::Debug;

    use rustpython_vm::{
        protocol::PyIterReturn,
        py_serde::deserialize,
        pyclass,
        types::{IterNext, SelfIter},
        Py, PyPayload, PyResult, VirtualMachine,
    };

    use super::BlockingRecv;

    #[pyattr]
    #[pyclass(name = "stream")]
    #[derive(PyPayload)]
    pub struct Stream {
        pub stream: Box<dyn BlockingRecv>,
    }

    impl Debug for Stream {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Stream").finish()
        }
    }

    #[pyclass(with(IterNext))]
    impl Stream {
        #[pymethod]
        fn run(&self) -> PyResult<()> {
            Ok(())
        }
    }

    impl SelfIter for Stream {}

    impl IterNext for Stream {
        fn next(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyIterReturn> {
            match zelf.stream.blocking_recv() {
                Some(v) => Ok(PyIterReturn::Return(deserialize(vm, v).unwrap())),
                None => Ok(PyIterReturn::StopIteration(None)),
            }
        }
    }
}
