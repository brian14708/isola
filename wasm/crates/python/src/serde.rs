use pyo3::{
    Bound, IntoPyObject, PyAny, PyTypeInfo, Python,
    types::{PyAnyMethods, PyDict, PyFloat, PyInt, PyList, PyNone, PyTuple},
};
use serde::{
    Deserializer, Serialize, Serializer,
    de::{
        DeserializeSeed, Expected, IntoDeserializer, Unexpected, Visitor,
        value::{MapAccessDeserializer, MapDeserializer, SeqDeserializer},
    },
    ser::{
        SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
        SerializeTupleStruct, SerializeTupleVariant,
    },
};

const MAX_DEPTH: usize = 128;

pub enum PyValue<'py> {
    Empty(Python<'py>),
    Object(Bound<'py, PyAny>),
}

impl<'py> PyValue<'py> {
    pub fn new(obj: Bound<'py, PyAny>) -> Self {
        PyValue::Object(obj)
    }

    fn py(&self) -> Python<'py> {
        match self {
            PyValue::Empty(py) => *py,
            PyValue::Object(obj) => obj.py(),
        }
    }

    pub fn deserialize<'de, D>(
        py: Python<'py>,
        deserializer: D,
    ) -> Result<Bound<'py, PyAny>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        PyValue::Empty(py).deserialize(deserializer)
    }

    pub fn serializer(py: Python<'py>) -> impl Serializer<Ok = Bound<'py, PyAny>> {
        PySerializer(py)
    }
}

impl<'py> From<PyValue<'py>> for Bound<'py, PyAny> {
    fn from(val: PyValue<'py>) -> Self {
        match val {
            PyValue::Empty(py) => PyNone::get(py).to_owned().into_any(),
            PyValue::Object(obj) => obj,
        }
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
        let o = &self.get();
        let unexp = if PyDict::is_exact_type_of(o) {
            Unexpected::Map
        } else if PyList::is_exact_type_of(o) {
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
            if let Ok(i) = o.extract::<f64>() {
                Unexpected::Float(i)
            } else {
                Unexpected::Other("object does not fit into a float")
            }
        } else if PyInt::is_exact_type_of(o) {
            if let Ok(i) = o.extract::<i64>() {
                Unexpected::Signed(i)
            } else if let Ok(i) = o.extract::<u64>() {
                Unexpected::Unsigned(i)
            } else {
                Unexpected::Other("object does not fit into an integer")
            }
        } else {
            Unexpected::Other("object is not serializable")
        };
        serde::de::Error::invalid_type(unexp, exp)
    }

    #[inline]
    fn get(&self) -> Bound<'_, PyAny> {
        match self {
            PyValue::Empty(py) => PyNone::get(*py).to_owned().into_any(),
            PyValue::Object(o) => o.to_owned(),
        }
    }
}

impl<'de, 'py> DeserializeSeed<'de> for PyValue<'py> {
    type Value = Bound<'py, PyAny>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
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
            impl_py_visit!(visit_bytes, &[u8]);
            impl_py_visit!(visit_borrowed_bytes, &'de [u8]);
            impl_py_visit!(visit_byte_buf, Vec<u8>);

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
                while let Some(val) = seq.next_element_seed(PyValue::Empty(self.0))? {
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
                    map.next_entry_seed(PyValue::Empty(self.0), PyValue::Empty(self.0))?
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
            fn child(&self, obj: Bound<'s, PyAny>) -> Self {
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
                if let Ok(dict) = o.downcast_exact::<PyDict>() {
                    let len = dict.len().ok();
                    let mut map = serializer.serialize_map(len)?;
                    for (key, value) in dict {
                        map.serialize_entry(&self.child(key), &self.child(value))?;
                    }
                    map.end()
                } else if let Ok(list) = o.downcast_exact::<PyList>() {
                    let len = list.len().ok();
                    let mut seq = serializer.serialize_seq(len)?;
                    for elem in list {
                        seq.serialize_element(&self.child(elem))?;
                    }
                    seq.end()
                } else if let Ok(tuple) = o.downcast_exact::<PyTuple>() {
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
                    if let Ok(i) = o.extract::<f64>() {
                        serializer.serialize_f64(i)
                    } else {
                        Err(serde::ser::Error::custom(format!(
                            "object of type '{}' does not fit into a float",
                            o.get_type()
                        )))
                    }
                } else if PyInt::is_exact_type_of(o) {
                    if let Ok(i) = o.extract::<i32>() {
                        serializer.serialize_i32(i)
                    } else if let Ok(i) = o.extract::<i64>() {
                        serializer.serialize_i64(i)
                    } else if let Ok(i) = o.extract::<u64>() {
                        serializer.serialize_u64(i)
                    } else {
                        Err(serde::ser::Error::custom(format!(
                            "object of type '{}' does not fit into an integer",
                            o.get_type()
                        )))
                    }
                } else {
                    Err(serde::ser::Error::custom(format!(
                        "object of type '{}' is not serializable",
                        o.get_type()
                    )))
                }
            }
        }
        match self {
            PyValue::Empty(_) => serializer.serialize_none(),
            PyValue::Object(obj) => PySerialize(obj.to_owned(), MAX_DEPTH).serialize(serializer),
        }
    }
}

macro_rules! impl_py_deserialize {
    ($name:ident, $visit:ident) => {
        fn $name<V>(self, visitor: V) -> Result<V::Value, Self::Error>
        where
            V: Visitor<'de>,
        {
            if let Ok(v) = self.get().extract() {
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
        let o = self.get();
        if PyDict::is_exact_type_of(&o) {
            drop(o);
            self.deserialize_map(visitor)
        } else if PyList::is_exact_type_of(&o) || PyTuple::is_exact_type_of(&o) {
            drop(o);
            self.deserialize_seq(visitor)
        } else if let Ok(s) = o.extract::<&str>() {
            visitor.visit_str(s)
        } else if let Ok(b) = o.extract::<bool>() {
            visitor.visit_bool(b)
        } else if o.is_none() {
            visitor.visit_unit()
        } else if let Ok(f) = o.extract::<f64>() {
            visitor.visit_f64(f)
        } else if PyInt::is_exact_type_of(&o) {
            if let Ok(i) = o.extract::<i64>() {
                visitor.visit_i64(i)
            } else if let Ok(i) = o.extract::<u64>() {
                visitor.visit_u64(i)
            } else {
                Err(self.invalid_type(&visitor))
            }
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
        if self.get().is_none() {
            visitor.visit_unit()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if self.get().is_none() {
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
        if let Ok(list) = self.get().downcast_exact::<PyList>() {
            let mut deserializer = SeqDeserializer::new(list.into_iter().map(PyValue::new));
            let seq = visitor.visit_seq(&mut deserializer)?;
            deserializer.end()?;
            Ok(seq)
        } else if let Ok(tuple) = self.get().downcast_exact::<PyTuple>() {
            let mut deserializer = SeqDeserializer::new(tuple.into_iter().map(PyValue::new));
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
        if let Ok(dict) = self.get().downcast_exact::<PyDict>() {
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
        let o = &self.get();
        if let Ok(s) = o.extract::<&str>() {
            visitor.visit_enum(s.into_deserializer())
        } else if let Ok(dict) = o.downcast_exact::<PyDict>() {
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

macro_rules! impl_py_serialize {
    ($name:ident, $type:ty) => {
        fn $name(self, v: $type) -> Result<Self::Ok, Self::Error> {
            Ok(v.into_pyobject(self.0).unwrap().to_owned().into_any())
        }
    };
}

struct PySerializer<'py>(Python<'py>);

impl<'py> Serializer for PySerializer<'py> {
    type Ok = Bound<'py, PyAny>;

    type Error = serde::de::value::Error;

    type SerializeSeq = SeqSerializer<'py>;
    type SerializeTuple = SeqSerializer<'py>;
    type SerializeTupleStruct = SeqSerializer<'py>;
    type SerializeTupleVariant = SeqSerializer<'py>;
    type SerializeMap = MapSerializer<'py>;
    type SerializeStruct = MapSerializer<'py>;
    type SerializeStructVariant = MapSerializer<'py>;

    impl_py_serialize!(serialize_bool, bool);
    impl_py_serialize!(serialize_i8, i8);
    impl_py_serialize!(serialize_i16, i16);
    impl_py_serialize!(serialize_i32, i32);
    impl_py_serialize!(serialize_i64, i64);
    impl_py_serialize!(serialize_u8, u8);
    impl_py_serialize!(serialize_u16, u16);
    impl_py_serialize!(serialize_u32, u32);
    impl_py_serialize!(serialize_u64, u64);
    impl_py_serialize!(serialize_f32, f32);
    impl_py_serialize!(serialize_f64, f64);
    impl_py_serialize!(serialize_char, char);
    impl_py_serialize!(serialize_str, &str);
    impl_py_serialize!(serialize_bytes, &[u8]);

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(PyNone::get(self.0).to_owned().into_any())
    }

    fn serialize_some<T>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(().into_pyobject(self.0).unwrap().into_any())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(().into_pyobject(self.0).unwrap().into_any())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        variant.serialize(self)
    }

    fn serialize_newtype_struct<T>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(SeqSerializer {
            py: self.0,
            elems: Vec::with_capacity(len.unwrap_or(0)),
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Ok(SeqSerializer {
            py: self.0,
            elems: Vec::with_capacity(len),
        })
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Ok(SeqSerializer {
            py: self.0,
            elems: Vec::with_capacity(len),
        })
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Ok(SeqSerializer {
            py: self.0,
            elems: Vec::with_capacity(len),
        })
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(MapSerializer {
            py: self.0,
            dict: Vec::with_capacity(len.unwrap_or(0)),
        })
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(MapSerializer {
            py: self.0,
            dict: Vec::with_capacity(len),
        })
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Ok(MapSerializer {
            py: self.0,
            dict: Vec::with_capacity(len),
        })
    }
}

pub struct SeqSerializer<'py> {
    py: Python<'py>,
    elems: Vec<Bound<'py, PyAny>>,
}

impl<'py> SerializeSeq for SeqSerializer<'py> {
    type Ok = Bound<'py, PyAny>;
    type Error = serde::de::value::Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.elems.push(value.serialize(PySerializer(self.py))?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.elems.into_pyobject(self.py).unwrap().into_any())
    }
}

impl<'py> SerializeTuple for SeqSerializer<'py> {
    type Ok = Bound<'py, PyAny>;
    type Error = serde::de::value::Error;
    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(PyTuple::new(self.py, self.elems).unwrap().into_any())
    }
}

impl<'py> SerializeTupleStruct for SeqSerializer<'py> {
    type Ok = Bound<'py, PyAny>;
    type Error = serde::de::value::Error;
    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(PyTuple::new(self.py, self.elems).unwrap().into_any())
    }
}

impl<'py> SerializeTupleVariant for SeqSerializer<'py> {
    type Ok = Bound<'py, PyAny>;
    type Error = serde::de::value::Error;
    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(PyTuple::new(self.py, self.elems).unwrap().into_any())
    }
}

pub struct MapSerializer<'py> {
    py: Python<'py>,
    dict: Vec<(Bound<'py, PyAny>, Option<Bound<'py, PyAny>>)>,
}

impl<'py> SerializeMap for MapSerializer<'py> {
    type Ok = Bound<'py, PyAny>;
    type Error = serde::de::value::Error;

    fn serialize_key<T>(&mut self, key: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        let key = key.serialize(PySerializer(self.py))?;
        self.dict.push((key, None));
        Ok(())
    }

    fn serialize_value<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        let value = value.serialize(PySerializer(self.py))?;
        self.dict.last_mut().unwrap().1 = Some(value);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        let dict = PyDict::new(self.py);
        for (key, value) in self.dict {
            dict.set_item(key, value).unwrap();
        }
        Ok(dict.into_any())
    }
}

impl SerializeStruct for MapSerializer<'_> {
    type Ok = <Self as SerializeMap>::Ok;
    type Error = <Self as SerializeMap>::Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        let key = key.serialize(PySerializer(self.py))?;
        let value = value.serialize(PySerializer(self.py))?;
        self.dict.push((key, Some(value)));
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        SerializeMap::end(self)
    }
}

impl SerializeStructVariant for MapSerializer<'_> {
    type Ok = <Self as SerializeMap>::Ok;
    type Error = <Self as SerializeMap>::Error;
    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        SerializeStruct::serialize_field(self, key, value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        SerializeStruct::end(self)
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

impl serde::Serialize for PyLogDict<'_> {
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
                        &PyValue::new(key),
                        &PyValue::new(value.call0().map_err(serde::ser::Error::custom)?),
                    )?;
                } else {
                    map.serialize_entry(&PyValue::new(key), &PyValue::new(value))?;
                }
            }
            map
        } else {
            serializer.serialize_map(Some(1))?
        };
        map.serialize_entry("message", &PyValue::new(self.msg.clone()))?;
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn to_json(object: Bound<'_, PyAny>) -> Result<String, serde_json::Error> {
        serde_json::to_string(&PyValue::new(object))
    }

    #[test]
    fn test_pyobject_serializer() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            assert_eq!(
                to_json(PyList::new(py, [1]).unwrap().into_any()).unwrap(),
                json!([1]).to_string()
            );

            let p = u64::MAX.into_pyobject(py).unwrap();
            let p = p
                .call_method("__add__", (1_u32.into_pyobject(py).unwrap(),), None)
                .unwrap();
            assert!(to_json(p).is_err());
        });
    }
}
