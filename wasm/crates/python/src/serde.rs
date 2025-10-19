use std::io::{self, Write};

use pyo3::{
    Bound, IntoPyObject, PyAny, PyResult, PyTypeInfo, Python,
    types::{PyAnyMethods, PyBytes, PyDict, PyFloat, PyInt, PyList, PyNone, PySet, PyTuple},
};
use serde::{
    Deserializer, Serialize,
    de::{
        DeserializeSeed, Expected, IntoDeserializer, Unexpected, Visitor,
        value::{MapAccessDeserializer, MapDeserializer, SeqDeserializer},
    },
    ser::{SerializeMap, SerializeSeq, SerializeTuple},
};

const MAX_DEPTH: usize = 128;

struct PyValue<'py>(Bound<'py, PyAny>);

impl<'py> PyValue<'py> {
    const fn new(obj: Bound<'py, PyAny>) -> Self {
        PyValue(obj)
    }

    fn deserialize<'de, D>(py: Python<'py>, deserializer: D) -> Result<Bound<'py, PyAny>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        PyDeserializer(py).deserialize(deserializer)
    }
}

impl IntoDeserializer<'_> for PyValue<'_> {
    type Deserializer = Self;
    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}

impl PyValue<'_> {
    #[cold]
    fn invalid_type<E>(&self, exp: &dyn Expected) -> E
    where
        E: serde::de::Error,
    {
        let o = &self.0;
        let unexp = if PyDict::is_exact_type_of(o) {
            Unexpected::Map
        } else if let Ok(s) = o.extract::<&[u8]>() {
            Unexpected::Bytes(s)
        } else if PyList::is_exact_type_of(o) || PySet::is_exact_type_of(o) {
            Unexpected::Seq
        } else if PyTuple::is_exact_type_of(o) {
            Unexpected::TupleVariant
        } else if let Ok(s) = o.extract::<&str>() {
            Unexpected::Str(s)
        } else if let Ok(b) = o.extract::<bool>() {
            Unexpected::Bool(b)
        } else if o.is_none() {
            Unexpected::Option
        } else if PyFloat::is_exact_type_of(o) {
            o.extract::<f64>().map_or_else(
                |_| Unexpected::Other("object does not fit into a float"),
                Unexpected::Float,
            )
        } else if PyInt::is_exact_type_of(o) {
            o.extract::<i64>().map_or_else(
                |_| {
                    o.extract::<u64>().map_or_else(
                        |_| Unexpected::Other("object does not fit into an integer"),
                        Unexpected::Unsigned,
                    )
                },
                Unexpected::Signed,
            )
        } else {
            Unexpected::Other("object is not serializable")
        };
        serde::de::Error::invalid_type(unexp, exp)
    }
}

struct PyDeserializer<'py>(Python<'py>);

impl<'py> PyDeserializer<'py> {
    const fn py(&self) -> Python<'py> {
        self.0
    }
}

struct PyVisitor<'py>(Python<'py>);

macro_rules! impl_py_visit {
    ($name:ident, $t:ty) => {
        #[inline]
        fn $name<E>(self, v: $t) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v.into_pyobject(self.0).unwrap().to_owned().into_any())
        }
    };
}
impl<'de, 'py> Visitor<'de> for PyVisitor<'py> {
    type Value = Bound<'py, PyAny>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a type that can deserialize in Python")
    }

    impl_py_visit!(visit_bool, bool);
    impl_py_visit!(visit_i8, i8);
    impl_py_visit!(visit_i16, i16);
    impl_py_visit!(visit_i32, i32);
    impl_py_visit!(visit_i64, i64);
    impl_py_visit!(visit_i128, i128);
    impl_py_visit!(visit_u8, u8);
    impl_py_visit!(visit_u16, u16);
    impl_py_visit!(visit_u32, u32);
    impl_py_visit!(visit_u64, u64);
    impl_py_visit!(visit_u128, u128);
    impl_py_visit!(visit_f32, f32);
    impl_py_visit!(visit_f64, f64);
    impl_py_visit!(visit_char, char);
    impl_py_visit!(visit_str, &str);
    impl_py_visit!(visit_borrowed_str, &'de str);
    impl_py_visit!(visit_string, String);

    #[inline]
    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(PyBytes::new(self.0, v).clone().into_any())
    }

    #[inline]
    fn visit_borrowed_bytes<E>(self, v: &'de [u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(PyBytes::new(self.0, v).clone().into_any())
    }

    #[inline]
    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(PyBytes::new(self.0, &v).clone().into_any())
    }

    #[inline]
    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(().into_pyobject(self.0).unwrap().into_any())
    }

    #[inline]
    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(PyNone::get(self.0).to_owned().into_any())
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut elems = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(val) = seq.next_element_seed(PyDeserializer(self.0))? {
            elems.push(val);
        }
        Ok(elems.into_pyobject(self.0).unwrap().into_any())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let dict = PyDict::new(self.0);
        while let Some((key, val)) =
            map.next_entry_seed(PyDeserializer(self.0), PyDeserializer(self.0))?
        {
            dict.set_item(key, val).unwrap();
        }
        Ok(dict.into_pyobject(self.0).unwrap().into_any())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        PyValue::deserialize(self.0, deserializer)
    }

    fn visit_newtype_struct<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        PyValue::deserialize(self.0, deserializer)
    }
}

impl<'de, 'py> DeserializeSeed<'de> for PyDeserializer<'py> {
    type Value = Bound<'py, PyAny>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(PyVisitor(self.py()))
    }
}

impl Serialize for PyValue<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        struct PySerialize<'s>(Bound<'s, PyAny>, usize);
        impl<'s> PySerialize<'s> {
            const fn child(&self, obj: Bound<'s, PyAny>) -> Self {
                Self(obj, self.1 - 1)
            }
        }
        impl serde::Serialize for PySerialize<'_> {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                if self.1 == 0 {
                    return Err(serde::ser::Error::custom(
                        "maximum serialization depth exceeded, possible circular reference",
                    ));
                }

                let o = &self.0;
                if let Ok(dict) = o.cast_exact::<PyDict>() {
                    let len = dict.len().ok();
                    let mut map = serializer.serialize_map(len)?;
                    for (key, value) in dict {
                        map.serialize_entry(&self.child(key), &self.child(value))?;
                    }
                    map.end()
                } else if let Ok(s) = o.extract::<&[u8]>() {
                    serializer.serialize_bytes(s)
                } else if let Ok(list) = o.cast_exact::<PyList>() {
                    let len = list.len().ok();
                    let mut seq = serializer.serialize_seq(len)?;
                    for elem in list {
                        seq.serialize_element(&self.child(elem))?;
                    }
                    seq.end()
                } else if let Ok(set) = o.cast_exact::<PySet>() {
                    let len = set.len().ok();
                    let mut seq = serializer.serialize_seq(len)?;
                    for elem in set {
                        seq.serialize_element(&self.child(elem))?;
                    }
                    seq.end()
                } else if let Ok(tuple) = o.cast_exact::<PyTuple>() {
                    let len = tuple.len().map_err(serde::ser::Error::custom)?;
                    let mut seq = serializer.serialize_tuple(len)?;
                    for elem in tuple {
                        seq.serialize_element(&self.child(elem))?;
                    }
                    seq.end()
                } else if let Ok(s) = o.extract::<&str>() {
                    serializer.serialize_str(s)
                } else if let Ok(b) = o.extract::<bool>() {
                    serializer.serialize_bool(b)
                } else if o.is_none() {
                    serializer.serialize_none()
                } else if PyFloat::is_exact_type_of(o) {
                    o.extract::<f64>().map_or_else(
                        |_| {
                            Err(serde::ser::Error::custom(format!(
                                "object of type '{}' does not fit into a float",
                                o.get_type()
                            )))
                        },
                        |i| serializer.serialize_f64(i),
                    )
                } else if PyInt::is_exact_type_of(o) {
                    if let Ok(i) = o.extract::<i32>() {
                        serializer.serialize_i32(i)
                    } else {
                        match o.extract::<i64>() {
                            Ok(i) => serializer.serialize_i64(i),
                            Err(_) => o.extract::<u64>().map_or_else(
                                |_| {
                                    Err(serde::ser::Error::custom(format!(
                                        "object of type '{}' does not fit into an integer",
                                        o.get_type()
                                    )))
                                },
                                |i| serializer.serialize_u64(i),
                            ),
                        }
                    }
                } else {
                    Err(serde::ser::Error::custom(format!(
                        "object of type '{}' is not serializable",
                        o.get_type()
                    )))
                }
            }
        }
        PySerialize(self.0.clone(), MAX_DEPTH).serialize(serializer)
    }
}

macro_rules! impl_py_deserialize {
    ($name:ident, $visit:ident) => {
        fn $name<V>(self, visitor: V) -> Result<V::Value, Self::Error>
        where
            V: Visitor<'de>,
        {
            if let Ok(v) = self.0.extract() {
                visitor.$visit(v)
            } else {
                Err(self.invalid_type(&visitor))
            }
        }
    };
}

impl<'de> Deserializer<'de> for PyValue<'_> {
    type Error = serde::de::value::Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let o = &self.0;
        if PyDict::is_exact_type_of(o) {
            self.deserialize_map(visitor)
        } else if let Ok(s) = o.extract::<&[u8]>() {
            visitor.visit_bytes(s)
        } else if PyList::is_exact_type_of(o)
            || PyTuple::is_exact_type_of(o)
            || PySet::is_exact_type_of(o)
        {
            self.deserialize_seq(visitor)
        } else if let Ok(s) = o.extract::<&str>() {
            visitor.visit_str(s)
        } else if let Ok(b) = o.extract::<bool>() {
            visitor.visit_bool(b)
        } else if o.is_none() {
            visitor.visit_unit()
        } else if PyInt::is_exact_type_of(o) {
            if let Ok(i) = o.extract::<i64>() {
                visitor.visit_i64(i)
            } else if let Ok(i) = o.extract::<u64>() {
                visitor.visit_u64(i)
            } else {
                Err(self.invalid_type(&visitor))
            }
        } else if let Ok(f) = o.extract::<f64>() {
            visitor.visit_f64(f)
        } else {
            Err(self.invalid_type(&visitor))
        }
    }

    impl_py_deserialize!(deserialize_bool, visit_bool);
    impl_py_deserialize!(deserialize_i8, visit_i8);
    impl_py_deserialize!(deserialize_i16, visit_i16);
    impl_py_deserialize!(deserialize_i32, visit_i32);
    impl_py_deserialize!(deserialize_i64, visit_i64);
    impl_py_deserialize!(deserialize_u8, visit_u8);
    impl_py_deserialize!(deserialize_u16, visit_u16);
    impl_py_deserialize!(deserialize_u32, visit_u32);
    impl_py_deserialize!(deserialize_u64, visit_u64);
    impl_py_deserialize!(deserialize_f32, visit_f32);
    impl_py_deserialize!(deserialize_f64, visit_f64);
    impl_py_deserialize!(deserialize_char, visit_char);
    impl_py_deserialize!(deserialize_str, visit_str);
    impl_py_deserialize!(deserialize_string, visit_string);
    impl_py_deserialize!(deserialize_bytes, visit_bytes);
    impl_py_deserialize!(deserialize_byte_buf, visit_byte_buf);

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if self.0.is_none() {
            visitor.visit_unit()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if self.0.is_none() {
            visitor.visit_unit()
        } else {
            Err(self.invalid_type(&visitor))
        }
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if let Ok(list) = self.0.cast_exact::<PyList>() {
            let mut deserializer = SeqDeserializer::new(list.into_iter().map(PyValue::new));
            let seq = visitor.visit_seq(&mut deserializer)?;
            deserializer.end()?;
            Ok(seq)
        } else if let Ok(tuple) = self.0.cast_exact::<PyTuple>() {
            let mut deserializer = SeqDeserializer::new(tuple.into_iter().map(PyValue::new));
            let seq = visitor.visit_seq(&mut deserializer)?;
            deserializer.end()?;
            Ok(seq)
        } else if let Ok(set) = self.0.cast_exact::<PySet>() {
            let mut deserializer = SeqDeserializer::new(set.into_iter().map(PyValue::new));
            let seq = visitor.visit_seq(&mut deserializer)?;
            deserializer.end()?;
            Ok(seq)
        } else {
            Err(self.invalid_type(&visitor))
        }
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if let Ok(dict) = self.0.cast_exact::<PyDict>() {
            let mut map = MapDeserializer::new(
                dict.into_iter()
                    .map(|(k, v)| (PyValue::new(k), PyValue::new(v))),
            );
            let o = visitor.visit_map(&mut map)?;
            map.end()?;
            Ok(o)
        } else {
            Err(self.invalid_type(&visitor))
        }
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let o = &self.0;
        if let Ok(s) = o.extract::<&str>() {
            visitor.visit_enum(s.into_deserializer())
        } else if let Ok(dict) = o.cast_exact::<PyDict>() {
            let map = MapDeserializer::new(
                dict.into_iter()
                    .map(|(k, v)| (PyValue::new(k), PyValue::new(v))),
            );
            let o = visitor.visit_enum(MapAccessDeserializer::new(map))?;
            Ok(o)
        } else {
            Err(self.invalid_type(&visitor))
        }
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_string(visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        drop(self);
        visitor.visit_unit()
    }
}

pub fn python_to_cbor(py_obj: Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    let mut serializer = minicbor_serde::Serializer::new(vec![]);
    PyValue::new(py_obj)
        .serialize(serializer.serialize_unit_as_null(true))
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    Ok(serializer.into_encoder().into_writer())
}

pub fn python_to_cbor_emit<F>(
    py_obj: Bound<'_, PyAny>,
    emit_type: crate::wasm::promptkit::script::host::EmitType,
    mut emit_fn: F,
) -> PyResult<()>
where
    F: FnMut(crate::wasm::promptkit::script::host::EmitType, &[u8]),
{
    let mut writer: CallbackWriter<_, 1024> = CallbackWriter::new(&mut emit_fn, emit_type);
    let mut serializer = minicbor_serde::Serializer::new(&mut writer);
    PyValue::new(py_obj)
        .serialize(serializer.serialize_unit_as_null(true))
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    Ok(())
}

/// A buffered writer that emits CBOR data through a callback with streaming control
pub struct CallbackWriter<'a, F, const N: usize = 1024>
where
    F: FnMut(crate::wasm::promptkit::script::host::EmitType, &[u8]),
{
    buffer: heapless::Vec<u8, N>,
    emit_fn: &'a mut F,
    end_type: crate::wasm::promptkit::script::host::EmitType,
}

impl<'a, F, const N: usize> CallbackWriter<'a, F, N>
where
    F: FnMut(crate::wasm::promptkit::script::host::EmitType, &[u8]),
{
    /// Creates a new `CallbackWriter` with the given callback and end type
    pub const fn new(
        emit_fn: &'a mut F,
        end_type: crate::wasm::promptkit::script::host::EmitType,
    ) -> Self {
        Self {
            emit_fn,
            buffer: heapless::Vec::new(),
            end_type,
        }
    }

    /// Flushes any buffered data as a continuation
    fn flush(&mut self) {
        if !self.buffer.is_empty() {
            (self.emit_fn)(
                crate::wasm::promptkit::script::host::EmitType::Continuation,
                &self.buffer,
            );
            self.buffer.clear();
        }
    }
}

impl<F, const N: usize> minicbor::encode::Write for CallbackWriter<'_, F, N>
where
    F: FnMut(crate::wasm::promptkit::script::host::EmitType, &[u8]),
{
    type Error = std::convert::Infallible;

    fn write_all(&mut self, buf: &[u8]) -> std::result::Result<(), Self::Error> {
        let mut remaining = buf;

        while !remaining.is_empty() {
            let available_space = N - self.buffer.len();
            if available_space == 0 {
                // Buffer is full, flush it
                self.flush();
                continue;
            }

            let to_write = remaining.len().min(available_space);
            let (chunk, rest) = remaining.split_at(to_write);

            // This should never fail because we checked available space
            self.buffer.extend_from_slice(chunk).ok();
            remaining = rest;
        }

        Ok(())
    }
}

impl<F, const N: usize> Drop for CallbackWriter<'_, F, N>
where
    F: FnMut(crate::wasm::promptkit::script::host::EmitType, &[u8]),
{
    fn drop(&mut self) {
        (self.emit_fn)(self.end_type, &self.buffer);
    }
}

pub fn cbor_to_python<'py>(py: Python<'py>, cbor: &[u8]) -> PyResult<Bound<'py, PyAny>> {
    let mut deserializer = minicbor_serde::Deserializer::new(cbor);
    PyValue::deserialize(py, &mut deserializer)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}

pub fn python_to_json(py_obj: Bound<'_, PyAny>) -> PyResult<String> {
    let mut o = vec![];
    python_to_json_writer(py_obj, &mut o)?;
    String::from_utf8(o).map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}

pub fn python_to_json_writer(py_obj: Bound<'_, PyAny>, mut w: impl Write) -> PyResult<()> {
    PyValue::new(py_obj)
        .serialize(&mut serde_json::Serializer::with_formatter(
            &mut w,
            Base64Formatter,
        ))
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    Ok(())
}

pub fn json_to_python<'py>(py: Python<'py>, json: &str) -> PyResult<Bound<'py, PyAny>> {
    let mut de = serde_json::Deserializer::from_str(json);
    PyValue::deserialize(py, &mut de)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}

pub fn python_to_yaml(py_obj: Bound<'_, PyAny>) -> PyResult<String> {
    let mut o = vec![];
    PyValue::new(py_obj)
        .serialize(&mut serde_yaml::Serializer::new(&mut o))
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    String::from_utf8(o).map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}

pub fn yaml_to_python<'py>(py: Python<'py>, yaml: &str) -> PyResult<Bound<'py, PyAny>> {
    let deserializer = serde_yaml::Deserializer::from_str(yaml);
    PyValue::deserialize(py, deserializer)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}

struct Base64Formatter;

impl serde_json::ser::Formatter for Base64Formatter {
    fn write_byte_array<W>(&mut self, mut writer: &mut W, value: &[u8]) -> io::Result<()>
    where
        W: io::Write + ?Sized,
    {
        writer.write_all(b"\"")?;
        base64::write::EncoderWriter::new(&mut writer, &base64::engine::general_purpose::STANDARD)
            .write_all(value)?;
        writer.write_all(b"\"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyBytes;
    use serde_json::json;

    #[test]
    fn test_pyobject_serializer() {
        Python::initialize();
        Python::attach(|py| {
            assert_eq!(
                python_to_json(PyList::new(py, [1]).unwrap().into_any()).unwrap(),
                json!([1]).to_string()
            );

            let p = u64::MAX.into_pyobject(py).unwrap();
            let p = p
                .call_method("__add__", (1_u32.into_pyobject(py).unwrap(),), None)
                .unwrap();
            assert!(python_to_json(p).is_err());
        });
    }

    #[test]
    fn test_pyvalue_minicbor_roundtrip() {
        Python::initialize();
        Python::attach(|py| {
            // Test various Python types for round-trip serialization through minicbor
            let mut test_cases = vec![
                // Basic types
                42i32.into_pyobject(py).unwrap().into_any(),
                "hello world".into_pyobject(py).unwrap().into_any(),
                3.14f64.into_pyobject(py).unwrap().into_any(),
                // Collections
                PyList::new(py, [1, 2, 3]).unwrap().into_any(),
                PyTuple::new(py, [1, 2, 3]).unwrap().into_any(),
                // Bytes
                PyBytes::new(py, b"test bytes").into_any(),
            ];

            // Add bool and None separately to handle cloning
            test_cases.push(true.into_pyobject(py).unwrap().to_owned().into_any());
            test_cases.push(false.into_pyobject(py).unwrap().to_owned().into_any());
            test_cases.push(PyNone::get(py).to_owned().into_any());

            // Test dictionary
            let dict = PyDict::new(py);
            dict.set_item("key1", "value1").unwrap();
            dict.set_item("key2", 42).unwrap();
            dict.set_item("key3", PyList::new(py, [1, 2, 3]).unwrap())
                .unwrap();
            test_cases.push(dict.into_any());

            // Test set
            let set = PySet::new(py, &[1, 2, 3, 2, 1]).unwrap();
            test_cases.push(set.into_any());

            // Test nested structures
            let nested_list = PyList::new(
                py,
                [
                    PyList::new(py, [1, 2]).unwrap().into_any(),
                    PyDict::new(py).into_any(),
                ],
            )
            .unwrap();
            test_cases.push(nested_list.into_any());

            for original in test_cases {
                // Serialize to CBOR
                let cbor_bytes =
                    python_to_cbor(original.clone()).expect("Failed to serialize to CBOR");

                // Deserialize from CBOR
                let deserialized =
                    cbor_to_python(py, &cbor_bytes).expect("Failed to deserialize from CBOR");

                // Compare by serializing both to JSON (as a proxy for equality)
                let original_json =
                    python_to_json(original.clone()).expect("Failed to serialize original to JSON");
                let deserialized_json =
                    python_to_json(deserialized).expect("Failed to serialize deserialized to JSON");

                assert_eq!(
                    original_json,
                    deserialized_json,
                    "Round-trip failed for type: {}",
                    original.get_type()
                );
            }
        });
    }
}
