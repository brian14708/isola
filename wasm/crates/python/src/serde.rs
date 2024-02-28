use pyo3::{
    types::{PyDict, PyList, PyNone, PyTuple},
    FromPyObject, IntoPy, PyAny, PyObject, Python,
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
        let dict = PyDict::new(self.py);
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

#[derive(FromPyObject)]
enum PyType<'a> {
    Bool(bool),
    Int(i64),
    Float(f64),
    None(&'a PyNone),
    String(&'a str),
    Dict(&'a PyDict),
    List(&'a PyList),
    Tuple(&'a PyTuple),
}

impl<'s> serde::Serialize for PyObjectSerializer<'s> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if let Ok(t) = self.pyobject.extract::<PyType>() {
            match t {
                PyType::Dict(dict) => {
                    let mut map = serializer.serialize_map(Some(dict.len()))?;
                    for (key, value) in dict {
                        map.serialize_entry(
                            &self.clone_with_object(key),
                            &self.clone_with_object(value),
                        )?;
                    }
                    map.end()
                }
                PyType::List(list) => {
                    let mut seq = serializer.serialize_seq(Some(list.len()))?;
                    for elem in list {
                        seq.serialize_element(&self.clone_with_object(elem))?;
                    }
                    seq.end()
                }
                PyType::Tuple(tuple) => {
                    let mut seq = serializer.serialize_seq(Some(tuple.len()))?;
                    for elem in tuple {
                        seq.serialize_element(&self.clone_with_object(elem))?;
                    }
                    seq.end()
                }
                PyType::None(_) => serializer.serialize_none(),
                PyType::String(s) => serializer.serialize_str(s),
                PyType::Bool(b) => serializer.serialize_bool(b),
                PyType::Int(i) => serializer.serialize_i64(i),
                PyType::Float(f) => serializer.serialize_f64(f),
            }
        } else {
            Err(serde::ser::Error::custom(format!(
                "Object of type '{}' is not serializable",
                self.pyobject.get_type()
            )))
        }
    }
}
