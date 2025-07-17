#[pyo3::pymodule]
#[pyo3(name = "_promptkit_grpc")]
pub mod grpc_module {
    use std::collections::BTreeSet;

    use prost_reflect::{
        DescriptorPool, DynamicMessage,
        prost::Message,
        prost_types::{FileDescriptorProto, FileDescriptorSet},
    };
    use pyo3::{
        Bound, PyAny, PyErr, Python, pyclass, pyfunction, pymethods,
        types::{PyAnyMethods, PyDict, PyDictMethods},
    };
    use tonic_reflection::pb::v1::{
        ServerReflectionRequest, ServerReflectionResponse,
        server_reflection_request::MessageRequest, server_reflection_response::MessageResponse,
    };

    use crate::{
        serde::{protobuf_to_python, python_to_protobuf},
        wasm::promptkit::script::outgoing_rpc::{
            self, ConnectRequest, Payload, RequestStream, ResponseStream,
        },
    };

    #[pyclass]
    pub struct ProtoDescriptor(DescriptorPool);

    #[pyfunction]
    #[pyo3(signature = (url, service, metadata=None, timeout=None))]
    pub fn grpc_reflection<'py>(
        py: Python<'py>,
        url: &str,
        service: &str,
        metadata: Option<&Bound<'_, PyDict>>,
        timeout: Option<f64>,
    ) -> Result<(ProtoDescriptor, Bound<'py, PyDict>), PyErr> {
        let mut md = vec![];
        if let Some(metadata) = metadata {
            for (k, v) in metadata {
                let k: String = k.extract()?;
                let v: &str = v.extract()?;
                let v = v.as_bytes().to_vec();
                md.push((k, v));
            }
        }
        if let Ok(v) = grpc_reflection_impl(py, url, "v1", service, Some(&md), timeout) {
            return Ok(v);
        }
        grpc_reflection_impl(py, url, "v1alpha", service, Some(&md), timeout)
    }

    #[allow(clippy::too_many_lines)]
    pub fn grpc_reflection_impl<'py>(
        py: Python<'py>,
        url: &str,
        reflect: &str,
        service: &str,
        metadata: Option<&[(String, Vec<u8>)]>,
        timeout: Option<f64>,
    ) -> Result<(ProtoDescriptor, Bound<'py, PyDict>), PyErr> {
        let req = ConnectRequest::new(
            &format!("{url}/grpc.reflection.{reflect}.ServerReflection/ServerReflectionInfo"),
            metadata,
        );
        if let Some(timeout) = timeout {
            req.set_connect_timeout(Some(
                u64::try_from(std::time::Duration::from_secs_f64(timeout).as_nanos())
                    .expect("duration is too large"),
            ))
            .map_err(|()| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    "Timeout value is too large or invalid",
                )
            })?;
        }

        let c = outgoing_rpc::connect(req);
        c.subscribe().block();
        let c = c.get().unwrap().unwrap().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyConnectionError, _>(format!(
                "gRPC connection failed: {e}"
            ))
        })?;
        let (tx, rx) = c.streams().unwrap();
        let resp = send_recv::<_, ServerReflectionResponse>(
            &tx,
            &rx,
            &ServerReflectionRequest {
                host: String::new(),
                message_request: Some(MessageRequest::FileContainingSymbol(service.to_string())),
            },
        )?;

        let mut files = vec![];
        let mut deps = BTreeSet::new();
        let methods = PyDict::new(py);
        if let Some(MessageResponse::FileDescriptorResponse(l)) = resp.message_response {
            for proto in l.file_descriptor_proto.iter().map(|s| {
                FileDescriptorProto::decode(s.as_slice()).map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to decode file descriptor: {e}"
                    ))
                })
            }) {
                let file = proto?;
                for d in file.dependency.iter().cloned() {
                    deps.insert(d);
                }
                let pkg = file.package.as_deref().unwrap_or("");
                if service.starts_with(pkg) && service.len() > pkg.len() {
                    let name = &service[pkg.len() + 1..];
                    for m in file
                        .service
                        .iter()
                        .filter(|s| s.name.as_deref() == Some(name))
                        .flat_map(|s| &s.method)
                    {
                        methods.set_item(
                            m.name.as_deref().unwrap_or(""),
                            (
                                m.input_type.as_deref().unwrap_or(""),
                                m.output_type.as_deref().unwrap_or(""),
                            ),
                        )?;
                    }
                }
                files.push(file);
            }
        }

        let mut done = BTreeSet::new();
        while let Some(d) = deps.pop_first() {
            if done.contains(&d) {
                continue;
            }
            done.insert(d.clone());

            let resp = send_recv::<_, ServerReflectionResponse>(
                &tx,
                &rx,
                &ServerReflectionRequest {
                    host: String::new(),
                    message_request: Some(MessageRequest::FileByFilename(d)),
                },
            )?;
            if let Some(MessageResponse::FileDescriptorResponse(l)) = resp.message_response {
                for proto in l.file_descriptor_proto.iter().map(|s| {
                    FileDescriptorProto::decode(s.as_slice())
                        .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))
                }) {
                    let file = proto?;
                    for d in file.dependency.iter().cloned() {
                        deps.insert(d);
                    }
                    files.push(file);
                }
            }
        }
        RequestStream::finish(tx)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;

        // recv eof
        while rx.read().is_none() {
            rx.subscribe().block();
        }

        let mut d = DescriptorPool::new();
        d.add_file_descriptor_set(FileDescriptorSet { file: files })
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
        Ok((ProtoDescriptor(d), methods))
    }

    fn send_recv<T, U>(tx: &RequestStream, rx: &ResponseStream, val: &T) -> Result<U, PyErr>
    where
        T: Message,
        U: Message + Default,
    {
        let m = Payload::new(&val.encode_to_vec());
        while !tx
            .check_write(&m)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?
        {
            tx.subscribe().block();
        }
        tx.write(m)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;

        loop {
            if let Some(t) = rx.read() {
                let t =
                    t.map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
                return U::decode(t.data().as_slice())
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()));
            }
            rx.subscribe().block();
        }
    }

    #[pymethods]
    impl ProtoDescriptor {
        pub fn encode(&self, name: &str, pyobject: Bound<'_, PyAny>) -> Result<Vec<u8>, PyErr> {
            let msg = self.0.get_message_by_name(name).ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>("message not found")
            })?;
            python_to_protobuf(msg, pyobject)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))
        }

        pub fn decode<'py>(
            &self,
            py: Python<'py>,
            name: &str,
            data: &[u8],
        ) -> Result<Bound<'py, PyAny>, PyErr> {
            let msg = self.0.get_message_by_name(name).ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>("message not found")
            })?;
            let msg = DynamicMessage::decode(msg, data)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
            let v = protobuf_to_python(py, &msg)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
            Ok(v)
        }
    }
}
