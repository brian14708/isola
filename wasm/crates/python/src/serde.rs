use std::io::{BufWriter, Write};

use pyo3::{
    types::{PyAnyMethods, PyDict, PyFloat, PyInt, PyList, PyTuple},
    Bound, IntoPyObject, PyAny, PyObject, PyTypeInfo, Python,
};
use serde::{
    de::{DeserializeSeed, Visitor},
    ser::{SerializeMap, SerializeSeq},
};

pub struct PyObjectDeserializer<'c> {
    py: Python<'c>,
}

impl<'c> PyObjectDeserializer<'c> {
    pub fn new(py: Python<'c>) -> Self {
        Self { py }
    }
}

impl<'de> DeserializeSeed<'de> for &PyObjectDeserializer<'de> {
    type Value = PyObject;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for &PyObjectDeserializer<'de> {
    type Value = PyObject;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a type that can deserialize in Python")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_i8<E>(self, v: i8) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_i16<E>(self, v: i16) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_i32<E>(self, v: i32) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_i128<E>(self, v: i128) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_u8<E>(self, v: u8) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_u16<E>(self, v: u16) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_u32<E>(self, v: u32) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_u128<E>(self, v: u128) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_f32<E>(self, v: f32) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_char<E>(self, v: char) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_borrowed_bytes<E>(self, v: &'de [u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(().into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut elems = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(elem) = seq.next_element_seed(self)? {
            elems.push(elem);
        }
        Ok(elems
            .into_pyobject(self.py)
            .unwrap()
            .as_any()
            .clone()
            .unbind())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let dict = PyDict::new(self.py);
        while let Some((key, value)) = map.next_entry_seed(self, self)? {
            dict.set_item(key, value).unwrap();
        }
        Ok(dict
            .into_pyobject(self.py)
            .unwrap()
            .as_any()
            .clone()
            .unbind())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        self.deserialize(deserializer)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(().into_pyobject(self.py).unwrap().as_any().clone().unbind())
    }

    fn visit_newtype_struct<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        self.deserialize(deserializer)
    }
}

pub struct PyObjectSerializer<'s> {
    pyobject: Bound<'s, PyAny>,
    depth: usize,
}

impl<'s> PyObjectSerializer<'s> {
    fn new(pyobject: Bound<'s, PyAny>, depth: usize) -> Self {
        Self { pyobject, depth }
    }

    pub fn to_json_writer(
        writer: impl Write,
        object: Bound<'s, PyAny>,
    ) -> Result<(), serde_json::Error> {
        let w = BufWriter::new(writer);
        serde_json::to_writer(w, &Self::new(object, 0))
    }

    pub fn to_cbor(
        v: Vec<u8>,
        object: Bound<'s, PyAny>,
    ) -> Result<Vec<u8>, cbor4ii::serde::EncodeError<std::collections::TryReserveError>> {
        cbor4ii::serde::to_vec(v, &Self::new(object, 0))
    }
}

impl<'s> serde::Serialize for PyObjectSerializer<'s> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        const MAX_DEPTH: usize = 128;

        let depth = self.depth + 1;
        if self.depth > MAX_DEPTH {
            return Err(serde::ser::Error::custom(
                "maximum serialization depth exceeded, possible circular reference",
            ));
        }

        if let Ok(dict) = self.pyobject.downcast_exact::<PyDict>() {
            let len = dict.len().ok();
            let mut map = serializer.serialize_map(len)?;
            for (key, value) in dict {
                map.serialize_entry(&Self::new(key, depth), &Self::new(value, depth))?;
            }
            map.end()
        } else if let Ok(list) = self.pyobject.downcast_exact::<PyList>() {
            let len = list.len().ok();
            let mut seq = serializer.serialize_seq(len)?;
            for elem in list {
                seq.serialize_element(&Self::new(elem, depth))?;
            }
            seq.end()
        } else if let Ok(tuple) = self.pyobject.downcast_exact::<PyTuple>() {
            let len = tuple.len().ok();
            let mut seq = serializer.serialize_seq(len)?;
            for elem in tuple {
                seq.serialize_element(&Self::new(elem, depth))?;
            }
            seq.end()
        } else if let Ok(s) = self.pyobject.extract::<&str>() {
            serializer.serialize_str(s)
        } else if let Ok(b) = self.pyobject.extract::<bool>() {
            serializer.serialize_bool(b)
        } else if self.pyobject.is_none() {
            serializer.serialize_none()
        } else if PyFloat::is_exact_type_of(&self.pyobject) {
            if let Ok(i) = self.pyobject.extract::<f64>() {
                serializer.serialize_f64(i)
            } else {
                Err(serde::ser::Error::custom(format!(
                    "object of type '{}' does not fit into a float",
                    self.pyobject.get_type()
                )))
            }
        } else if PyInt::is_exact_type_of(&self.pyobject) {
            if let Ok(i) = self.pyobject.extract::<i32>() {
                serializer.serialize_i32(i)
            } else if let Ok(i) = self.pyobject.extract::<i64>() {
                serializer.serialize_i64(i)
            } else if let Ok(i) = self.pyobject.extract::<u64>() {
                serializer.serialize_u64(i)
            } else {
                Err(serde::ser::Error::custom(format!(
                    "object of type '{}' does not fit into an integer",
                    self.pyobject.get_type()
                )))
            }
        } else {
            Err(serde::ser::Error::custom(format!(
                "object of type '{}' is not serializable",
                self.pyobject.get_type()
            )))
        }
    }
}

pub struct PyLogDict<'s> {
    dict: Option<&'s Bound<'s, PyDict>>,
    msg: Bound<'s, PyAny>,
}

impl PyLogDict<'_> {
    fn new<'s>(dict: Option<&'s Bound<'s, PyDict>>, msg: Bound<'s, PyAny>) -> PyLogDict<'s> {
        PyLogDict { dict, msg }
    }

    pub fn to_json<'s>(
        dict: Option<&'s Bound<'s, PyDict>>,
        msg: Bound<'s, PyAny>,
    ) -> Result<String, serde_json::Error> {
        serde_json::to_string(&Self::new(dict, msg))
    }
}

impl<'s> serde::Serialize for PyLogDict<'s> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = if let Some(dict) = self.dict {
            let len = dict.len().ok().map(|i| i + 1);
            let mut map = serializer.serialize_map(len)?;
            for (key, value) in dict {
                if value.is_callable() {
                    map.serialize_entry(
                        &PyObjectSerializer::new(key, 0),
                        &PyObjectSerializer::new(
                            value.call0().map_err(serde::ser::Error::custom)?,
                            0,
                        ),
                    )?;
                } else {
                    map.serialize_entry(
                        &PyObjectSerializer::new(key, 0),
                        &PyObjectSerializer::new(value, 0),
                    )?;
                }
            }
            map
        } else {
            serializer.serialize_map(Some(1))?
        };
        map.serialize_entry("message", &PyObjectSerializer::new(self.msg.clone(), 0))?;
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::ToPyObject;
    use serde_json::json;

    #[test]
    fn test_pyobject_serializer() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let mut v = vec![];
            PyObjectSerializer::to_json_writer(&mut v, PyList::new(py, [1]).into_any()).unwrap();
            assert_eq!(v, json!([1]).to_string().into_bytes());

            let p = u64::MAX.to_object(py);
            let p = p
                .call_method_bound(py, "__add__", (1.to_object(py),), None)
                .unwrap()
                .into_bound(py);
            v.clear();
            assert!(PyObjectSerializer::to_json_writer(&mut v, p).is_err());
            #[allow(clippy::cast_precision_loss)]
            let p = PyFloat::new(py, u64::MAX as f64 + 1.0);
            v.clear();
            assert!(PyObjectSerializer::to_json_writer(&mut v, p.into_any()).is_ok());
        });
    }
}
