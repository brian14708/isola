#[pyo3::pymodule]
#[pyo3(name = "_promptkit_protobuf")]
pub mod protobuf_module {
    use prost_reflect::{
        prost::Message, prost_types::FileDescriptorSet, DescriptorPool, DeserializeOptions,
        DynamicMessage, SerializeOptions,
    };
    use pyo3::{pyclass, pyfunction, pymethods, Bound, PyAny, PyErr, Python};
    use serde::de::IntoDeserializer;

    use crate::serde::PyValue;

    #[pyfunction]
    fn new() -> ProtoDescriptor {
        ProtoDescriptor(DescriptorPool::new())
    }

    #[pyclass]
    struct ProtoDescriptor(DescriptorPool);

    #[pymethods]
    impl ProtoDescriptor {
        pub fn add_descriptor(&mut self, buf: &[u8]) -> Result<(), PyErr> {
            let file_descriptor_set = FileDescriptorSet::decode(buf)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
            self.0
                .add_file_descriptor_set(file_descriptor_set)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
            Ok(())
        }

        pub fn encode(&self, name: &str, pyobject: Bound<'_, PyAny>) -> Result<Vec<u8>, PyErr> {
            let msg = self.0.get_message_by_name(name).ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>("message not found")
            })?;
            let msg = DynamicMessage::deserialize_with_options(
                msg,
                PyValue::new(pyobject).into_deserializer(),
                &DeserializeOptions::new(),
            )
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
            Ok(msg.encode_to_vec())
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
            let v = msg
                .serialize_with_options(PyValue::serializer(py), &SerializeOptions::new())
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e.to_string()))?;
            Ok(v)
        }
    }
}
