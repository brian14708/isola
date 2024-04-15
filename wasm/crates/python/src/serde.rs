use pyo3::{
    types::{PyAnyMethods, PyDict, PyList, PyTuple},
    Bound, IntoPy, PyAny, PyObject, Python,
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
        PyObjectDeserializer { py }
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
        Ok(v.into_py(self.py))
    }

    fn visit_i8<E>(self, v: i8) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_i16<E>(self, v: i16) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_i32<E>(self, v: i32) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_i128<E>(self, v: i128) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_u8<E>(self, v: u8) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_u16<E>(self, v: u16) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_u32<E>(self, v: u32) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_u128<E>(self, v: u128) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_f32<E>(self, v: f32) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_char<E>(self, v: char) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_borrowed_bytes<E>(self, v: &'de [u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.into_py(self.py))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(().into_py(self.py))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut elems = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(elem) = seq.next_element_seed(self)? {
            elems.push(elem);
        }
        Ok(elems.into_py(self.py))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let dict = PyDict::new_bound(self.py);
        while let Some((key, value)) = map.next_entry_seed(self, self)? {
            dict.set_item(key, value).unwrap();
        }
        Ok(dict.into_py(self.py))
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
        Ok(().into_py(self.py))
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
}

impl<'s> PyObjectSerializer<'s> {
    pub fn new(pyobject: Bound<'s, PyAny>) -> Self {
        PyObjectSerializer { pyobject }
    }
}

impl<'s> serde::Serialize for PyObjectSerializer<'s> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if let Ok(dict) = self.pyobject.downcast::<PyDict>() {
            let len = dict.len().ok();
            let mut map = serializer.serialize_map(len)?;
            for (key, value) in dict {
                map.serialize_entry(&Self::new(key), &Self::new(value))?;
            }
            map.end()
        } else if let Ok(list) = self.pyobject.downcast::<PyList>() {
            let len = list.len().ok();
            let mut seq = serializer.serialize_seq(len)?;
            for elem in list {
                seq.serialize_element(&Self::new(elem))?;
            }
            seq.end()
        } else if let Ok(tuple) = self.pyobject.downcast::<PyTuple>() {
            let len = tuple.len().ok();
            let mut seq = serializer.serialize_seq(len)?;
            for elem in tuple {
                seq.serialize_element(&Self::new(elem))?;
            }
            seq.end()
        } else if let Ok(s) = self.pyobject.extract::<&str>() {
            serializer.serialize_str(s)
        } else if let Ok(b) = self.pyobject.extract::<bool>() {
            serializer.serialize_bool(b)
        } else if self.pyobject.is_none() {
            serializer.serialize_none()
        } else if let Ok(i) = self.pyobject.extract::<i32>() {
            serializer.serialize_i32(i)
        } else if let Ok(i) = self.pyobject.extract::<i64>() {
            serializer.serialize_i64(i)
        } else if let Ok(i) = self.pyobject.extract::<u64>() {
            serializer.serialize_u64(i)
        } else if let Ok(i) = self.pyobject.extract::<f64>() {
            serializer.serialize_f64(i)
        } else {
            Err(serde::ser::Error::custom(format!(
                "Object of type '{}' is not serializable",
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
    pub fn new<'s>(dict: Option<&'s Bound<'s, PyDict>>, msg: Bound<'s, PyAny>) -> PyLogDict<'s> {
        PyLogDict { dict, msg }
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
                        &PyObjectSerializer::new(key),
                        &PyObjectSerializer::new(value.call0().map_err(serde::ser::Error::custom)?),
                    )?;
                } else {
                    map.serialize_entry(
                        &PyObjectSerializer::new(key),
                        &PyObjectSerializer::new(value),
                    )?;
                }
            }
            map
        } else {
            serializer.serialize_map(Some(1))?
        };
        map.serialize_entry("message", &PyObjectSerializer::new(self.msg.clone()))?;
        map.end()
    }
}
