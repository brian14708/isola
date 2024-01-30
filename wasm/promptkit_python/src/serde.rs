use pyo3::{
    types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple},
    IntoPy, PyAny, PyObject, Python,
};
use serde::{
    de::{DeserializeSeed, Visitor},
    ser::{SerializeMap, SerializeSeq},
};

#[derive(Clone)]
pub struct PyObjectDeserializer<'c> {
    py: Python<'c>,
}

impl<'c> PyObjectDeserializer<'c> {
    pub fn new(py: Python<'c>) -> Self {
        PyObjectDeserializer { py }
    }
}

impl<'de> DeserializeSeed<'de> for PyObjectDeserializer<'de> {
    type Value = PyObject;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for PyObjectDeserializer<'de> {
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
        while let Some(elem) = seq.next_element_seed(self.clone())? {
            elems.push(elem);
        }
        Ok(elems.into_py(self.py))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let dict = PyDict::new(self.py);
        while let Some((key, value)) = map.next_entry_seed(self.clone(), self.clone())? {
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
    pyobject: &'s PyAny,
    py: Python<'s>,
}

impl<'s> PyObjectSerializer<'s> {
    pub fn new(py: Python<'s>, pyobject: &'s PyAny) -> Self {
        PyObjectSerializer { pyobject, py }
    }

    fn clone_with_object(&self, pyobject: &'s PyAny) -> PyObjectSerializer {
        PyObjectSerializer {
            pyobject,
            py: self.py,
        }
    }
}

impl<'s> serde::Serialize for PyObjectSerializer<'s> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if self.pyobject.is_instance_of::<PyDict>() {
            let dict = self.pyobject.downcast::<PyDict>().unwrap();
            let mut map = serializer.serialize_map(Some(dict.len()))?;
            for (key, value) in dict {
                map.serialize_entry(&self.clone_with_object(key), &self.clone_with_object(value))?;
            }
            return map.end();
        }
        if self.pyobject.is_instance_of::<PyList>() {
            let list = self.pyobject.downcast::<PyList>().unwrap();
            let mut seq = serializer.serialize_seq(Some(list.len()))?;
            for elem in list {
                seq.serialize_element(&self.clone_with_object(elem))?;
            }
            return seq.end();
        }
        if self.pyobject.is_instance_of::<PyTuple>() {
            let tuple = self.pyobject.downcast::<PyTuple>().unwrap();
            let mut seq = serializer.serialize_seq(Some(tuple.len()))?;
            for elem in tuple {
                seq.serialize_element(&self.clone_with_object(elem))?;
            }
            return seq.end();
        }
        if self.pyobject.is_instance_of::<PyString>() {
            return serializer.serialize_str(self.pyobject.extract().unwrap());
        }
        if self.pyobject.is_instance_of::<PyBool>() {
            return serializer.serialize_bool(self.pyobject.extract().unwrap());
        }
        if self.pyobject.is_instance_of::<PyInt>() {
            return serializer.serialize_i64(self.pyobject.extract().unwrap());
        }
        if self.pyobject.is_instance_of::<PyFloat>() {
            return serializer.serialize_f64(self.pyobject.extract().unwrap());
        }
        if self.pyobject.is_none() {
            return serializer.serialize_none();
        }
        Err(serde::ser::Error::custom(format!(
            "Object of type '{}' is not serializable",
            self.pyobject.get_type()
        )))
    }
}
