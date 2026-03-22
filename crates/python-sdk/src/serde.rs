use isola::value::Value;
use pyo3::{
    Bound, IntoPyObject, PyAny, PyResult, PyTypeInfo, Python,
    types::{PyAnyMethods, PyDict, PyFloat, PyInt, PyList, PyNone, PySet, PyTuple},
};
use serde::{
    Serialize,
    de::{
        DeserializeSeed, IntoDeserializer, Visitor,
        value::{MapDeserializer, SeqDeserializer},
    },
    ser::{SerializeMap, SerializeSeq, SerializeTuple},
};

use crate::{Error, Result};

const MAX_DEPTH: usize = 128;

// ─── PyValue: serde Serialize + Deserialize for Python objects ─────────────

struct PyValue<'py>(Bound<'py, PyAny>);

impl<'py> PyValue<'py> {
    const fn new(obj: Bound<'py, PyAny>) -> Self {
        Self(obj)
    }

    fn deserialize<'de, D>(
        py: Python<'py>,
        deserializer: D,
    ) -> std::result::Result<Bound<'py, PyAny>, D::Error>
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
    fn invalid_type<E>(&self, exp: &dyn serde::de::Expected) -> E
    where
        E: serde::de::Error,
    {
        use serde::de::Unexpected;
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

// ─── Serialize: Python → serde ─────────────────────────────────────────────

impl Serialize for PyValue<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
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
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
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
                        |f| serializer.serialize_f64(f),
                    )
                } else if PyInt::is_exact_type_of(o) {
                    if let Ok(i) = o.extract::<i32>() {
                        serializer.serialize_i32(i)
                    } else if let Ok(i) = o.extract::<i64>() {
                        serializer.serialize_i64(i)
                    } else {
                        o.extract::<u64>().map_or_else(
                            |_| {
                                Err(serde::ser::Error::custom(format!(
                                    "object of type '{}' does not fit into an integer",
                                    o.get_type()
                                )))
                            },
                            |u| serializer.serialize_u64(u),
                        )
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

// ─── Deserialize: serde → Python ───────────────────────────────────────────

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
        fn $name<E>(self, v: $t) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v.into_pyobject(self.0).unwrap().to_owned().into_any())
        }
    };
}

impl<'de, 'py> Visitor<'de> for PyVisitor<'py> {
    type Value = Bound<'py, PyAny>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a type that can be represented in Python")
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
    fn visit_bytes<E>(self, v: &[u8]) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        use pyo3::types::PyBytes;
        Ok(PyBytes::new(self.0, v).clone().into_any())
    }

    #[inline]
    fn visit_borrowed_bytes<E>(self, v: &'de [u8]) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        use pyo3::types::PyBytes;
        Ok(PyBytes::new(self.0, v).clone().into_any())
    }

    #[inline]
    fn visit_byte_buf<E>(self, v: Vec<u8>) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        use pyo3::types::PyBytes;
        Ok(PyBytes::new(self.0, &v).clone().into_any())
    }

    #[inline]
    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(PyNone::get(self.0).to_owned().into_any())
    }

    #[inline]
    fn visit_none<E>(self) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(PyNone::get(self.0).to_owned().into_any())
    }

    fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut elems = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(val) = seq.next_element_seed(PyDeserializer(self.0))? {
            elems.push(val);
        }
        Ok(elems.into_pyobject(self.0).unwrap().into_any())
    }

    fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
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

    fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        PyValue::deserialize(self.0, deserializer)
    }

    fn visit_newtype_struct<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        PyValue::deserialize(self.0, deserializer)
    }
}

impl<'de, 'py> DeserializeSeed<'de> for PyDeserializer<'py> {
    type Value = Bound<'py, PyAny>;

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(PyVisitor(self.py()))
    }
}

macro_rules! impl_py_deserialize {
    ($name:ident, $visit:ident) => {
        fn $name<V>(self, visitor: V) -> std::result::Result<V::Value, Self::Error>
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

impl<'de> serde::Deserializer<'de> for PyValue<'_> {
    type Error = serde::de::value::Error;

    fn deserialize_any<V>(self, visitor: V) -> std::result::Result<V::Value, Self::Error>
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
            } else if let Ok(u) = o.extract::<u64>() {
                visitor.visit_u64(u)
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

    fn deserialize_option<V>(self, visitor: V) -> std::result::Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if self.0.is_none() {
            visitor.visit_unit()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> std::result::Result<V::Value, Self::Error>
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
    ) -> std::result::Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> std::result::Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> std::result::Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if let Ok(list) = self.0.cast_exact::<PyList>() {
            let mut de = SeqDeserializer::new(list.into_iter().map(PyValue::new));
            let seq = visitor.visit_seq(&mut de)?;
            de.end()?;
            Ok(seq)
        } else if let Ok(tuple) = self.0.cast_exact::<PyTuple>() {
            let mut de = SeqDeserializer::new(tuple.into_iter().map(PyValue::new));
            let seq = visitor.visit_seq(&mut de)?;
            de.end()?;
            Ok(seq)
        } else if let Ok(set) = self.0.cast_exact::<PySet>() {
            let mut de = SeqDeserializer::new(set.into_iter().map(PyValue::new));
            let seq = visitor.visit_seq(&mut de)?;
            de.end()?;
            Ok(seq)
        } else {
            Err(self.invalid_type(&visitor))
        }
    }

    fn deserialize_tuple<V>(
        self,
        _len: usize,
        visitor: V,
    ) -> std::result::Result<V::Value, Self::Error>
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
    ) -> std::result::Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V>(self, visitor: V) -> std::result::Result<V::Value, Self::Error>
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
    ) -> std::result::Result<V::Value, Self::Error>
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
    ) -> std::result::Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        use serde::de::value::MapAccessDeserializer;
        let o = &self.0;
        if let Ok(s) = o.extract::<&str>() {
            visitor.visit_enum(s.into_deserializer())
        } else if let Ok(dict) = o.cast_exact::<PyDict>() {
            let map = MapDeserializer::new(
                dict.into_iter()
                    .map(|(k, v)| (PyValue::new(k), PyValue::new(v))),
            );
            visitor.visit_enum(MapAccessDeserializer::new(map))
        } else {
            Err(self.invalid_type(&visitor))
        }
    }

    fn deserialize_identifier<V>(self, visitor: V) -> std::result::Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_string(visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> std::result::Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        drop(self);
        visitor.visit_unit()
    }
}

// ─── Public API ────────────────────────────────────────────────────────────

/// Deserialize a Python object directly into a serde type without going
/// through any intermediate representation.
pub fn py_to_serde<T: serde::de::DeserializeOwned>(obj: &Bound<'_, PyAny>) -> Result<T> {
    T::deserialize(PyValue::new(obj.clone()))
        .map_err(|e| Error::InvalidArgument(format!("failed to deserialize config: {e}")))
}

/// Convert a Python object to an isola `Value` (stored as CBOR internally).
pub fn py_to_value(obj: &Bound<'_, PyAny>) -> Result<Value> {
    let mut serializer = minicbor_serde::Serializer::new(vec![]);
    PyValue::new(obj.clone())
        .serialize(serializer.serialize_unit_as_null(true))
        .map_err(|e| Error::InvalidArgument(format!("value is not serializable: {e}")))?;
    Ok(Value::from_cbor(serializer.into_encoder().into_writer()))
}

/// Convert an isola `Value` (CBOR) to a Python object.
pub fn value_to_py(py: Python<'_>, value: &Value) -> PyResult<pyo3::Py<PyAny>> {
    let mut deserializer = minicbor_serde::Deserializer::new(value.as_cbor());
    PyValue::deserialize(py, &mut deserializer)
        .map(Bound::unbind)
        .map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("failed to decode value: {e}"))
        })
}
