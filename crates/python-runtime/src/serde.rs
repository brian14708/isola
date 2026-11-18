use std::{
    cell::RefCell,
    io::{self, Write},
};

use isola_runtime::CallbackWriter;
use pyo3::{
    Bound, IntoPyObject, PyAny, PyResult, PyTypeInfo, Python,
    types::{
        PyAnyMethods, PyBool, PyBoolMethods, PyBytes, PyBytesMethods, PyDict, PyFloat, PyInt,
        PyList, PyNone, PySet, PyString, PyStringMethods, PyTuple, PyTypeMethods,
    },
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

/// Strings at least this long take the SIMD ASCII fast path in
/// [`ascii_to_python`]; shorter ones are cheaper through CPython's decoder.
#[cfg(target_arch = "wasm32")]
const ASCII_FAST_PATH_MIN_LEN: usize = 256;

/// Whether every byte is ASCII, 64 bytes per step with WebAssembly SIMD.
///
/// CPython's UTF-8 decoder scans a `size_t` (4 bytes on wasm32) at a time,
/// which dominates the cost of creating large strings in the guest.
#[cfg(target_arch = "wasm32")]
#[target_feature(enable = "simd128")]
fn is_ascii(bytes: &[u8]) -> bool {
    use core::arch::wasm32::{u8x16_bitmask, v128, v128_load, v128_or};

    let mut chunks = bytes.chunks_exact(64);
    for chunk in &mut chunks {
        let ptr = chunk.as_ptr().cast::<v128>();
        // SAFETY: `chunk` holds exactly 64 bytes, i.e. four `v128` lanes, and
        // WebAssembly loads have no alignment requirement.
        let any_high = unsafe {
            v128_or(
                v128_or(v128_load(ptr), v128_load(ptr.add(1))),
                v128_or(v128_load(ptr.add(2)), v128_load(ptr.add(3))),
            )
        };
        if u8x16_bitmask(any_high) != 0 {
            return false;
        }
    }
    chunks.remainder().is_ascii()
}

/// Build a compact ASCII `str` directly when `bytes` is long and pure ASCII.
///
/// Returns `None` when the fast path does not apply, so callers fall back to
/// CPython's general decoder (which also validates UTF-8).
#[cfg(target_arch = "wasm32")]
fn ascii_to_python<'py>(py: Python<'py>, bytes: &[u8]) -> Option<PyResult<Bound<'py, PyString>>> {
    if bytes.len() < ASCII_FAST_PATH_MIN_LEN || !is_ascii(bytes) {
        return None;
    }
    let len = pyo3::ffi::Py_ssize_t::try_from(bytes.len()).ok()?;
    // SAFETY: `PyUnicode_New(len, 127)` returns a new compact ASCII string
    // with `len` writable one-byte code units (NUL-terminated by CPython).
    // Every byte copied in is ASCII, matching the declared maximum character.
    Some(unsafe {
        let obj = pyo3::ffi::PyUnicode_New(len, 127);
        if obj.is_null() {
            return Some(Err(pyo3::PyErr::fetch(py)));
        }
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            pyo3::ffi::PyUnicode_DATA(obj).cast::<u8>(),
            bytes.len(),
        );
        Ok(Bound::from_owned_ptr(py, obj).cast_into_unchecked())
    })
}

/// Host builds (tests, lints) may use the `PyO3` limited API, which hides the
/// `CPython` string internals, so they always take the `CPython` decoder.
#[cfg(not(target_arch = "wasm32"))]
const fn ascii_to_python<'py>(
    _py: Python<'py>,
    _bytes: &[u8],
) -> Option<PyResult<Bound<'py, PyString>>> {
    None
}

/// Create a Python `str` from validated UTF-8.
fn str_to_python<'py>(py: Python<'py>, s: &str) -> Bound<'py, PyAny> {
    match ascii_to_python(py, s.as_bytes()) {
        Some(Ok(value)) => value.into_any(),
        _ => PyString::new(py, s).into_any(),
    }
}

struct PyValue<'py>(Bound<'py, PyAny>);

impl<'py> PyValue<'py> {
    const fn new(obj: Bound<'py, PyAny>) -> Self {
        PyValue(obj)
    }

    fn deserialize<'de, D>(py: Python<'py>, deserializer: D) -> Result<Bound<'py, PyAny>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let keys = KeyCache::new();
        PyDeserializer {
            py,
            keys: &keys,
            is_key: false,
        }
        .deserialize(deserializer)
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

/// Per-decode cache of short dict-key strings.
///
/// Records repeat the same few keys; reusing one `str` per distinct key skips
/// an allocation per key and lets `dict` reuse the string's cached hash.
/// Direct-mapped so a map with many distinct keys costs one compare per key.
struct KeyCache<'py>(RefCell<[Option<CachedKey<'py>>; KEY_CACHE_SLOTS]>);

type CachedKey<'py> = (Box<str>, Bound<'py, PyString>);

const KEY_CACHE_SLOTS: usize = 64;
const KEY_CACHE_MAX_LEN: usize = 32;

impl<'py> KeyCache<'py> {
    const fn new() -> Self {
        Self(RefCell::new([const { None }; KEY_CACHE_SLOTS]))
    }

    fn get_or_insert(&self, py: Python<'py>, key: &str) -> Bound<'py, PyAny> {
        let bytes = key.as_bytes();
        let (Some(&first), Some(&last)) = (bytes.first(), bytes.last()) else {
            return PyString::new(py, key).into_any();
        };
        if bytes.len() > KEY_CACHE_MAX_LEN {
            return str_to_python(py, key);
        }
        let slot =
            (bytes.len() * 31 + usize::from(first) * 7 + usize::from(last)) % KEY_CACHE_SLOTS;
        let mut slots = self.0.borrow_mut();
        if let Some((cached, value)) = &slots[slot]
            && **cached == *key
        {
            return value.clone().into_any();
        }
        let value = PyString::new(py, key);
        slots[slot] = Some((key.into(), value.clone()));
        value.into_any()
    }
}

#[derive(Clone, Copy)]
struct PyDeserializer<'a, 'py> {
    py: Python<'py>,
    keys: &'a KeyCache<'py>,
    is_key: bool,
}

impl<'py> PyDeserializer<'_, 'py> {
    const fn value(self) -> Self {
        Self {
            is_key: false,
            ..self
        }
    }

    const fn key(self) -> Self {
        Self {
            is_key: true,
            ..self
        }
    }

    fn str(self, v: &str) -> Bound<'py, PyAny> {
        if self.is_key {
            self.keys.get_or_insert(self.py, v)
        } else {
            str_to_python(self.py, v)
        }
    }
}

struct PyVisitor<'a, 'py>(PyDeserializer<'a, 'py>);

macro_rules! impl_py_visit {
    ($name:ident, $t:ty) => {
        #[inline]
        fn $name<E>(self, v: $t) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v.into_pyobject(self.0.py).unwrap().to_owned().into_any())
        }
    };
}
impl<'de, 'py> Visitor<'de> for PyVisitor<'_, 'py> {
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

    #[inline]
    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(self.0.str(v))
    }

    #[inline]
    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(self.0.str(v))
    }

    #[inline]
    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(self.0.str(&v))
    }

    #[inline]
    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(PyBytes::new(self.0.py, v).into_any())
    }

    #[inline]
    fn visit_borrowed_bytes<E>(self, v: &'de [u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(PyBytes::new(self.0.py, v).into_any())
    }

    #[inline]
    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(PyBytes::new(self.0.py, &v).into_any())
    }

    #[inline]
    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(().into_pyobject(self.0.py).unwrap().into_any())
    }

    #[inline]
    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(PyNone::get(self.0.py).to_owned().into_any())
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut elems = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(val) = seq.next_element_seed(self.0.value())? {
            elems.push(val);
        }
        Ok(elems.into_pyobject(self.0.py).unwrap().into_any())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let dict = PyDict::new(self.0.py);
        while let Some((key, val)) = map.next_entry_seed(self.0.key(), self.0.value())? {
            dict.set_item(key, val).unwrap();
        }
        Ok(dict.into_pyobject(self.0.py).unwrap().into_any())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        self.0.value().deserialize(deserializer)
    }

    fn visit_newtype_struct<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        self.0.value().deserialize(deserializer)
    }
}

impl<'de, 'py> DeserializeSeed<'de> for PyDeserializer<'_, 'py> {
    type Value = Bound<'py, PyAny>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(PyVisitor(self))
    }
}

impl Serialize for PyValue<'_> {
    #[expect(
        clippy::too_many_lines,
        reason = "the recursive serializer keeps Python type dispatch in one place"
    )]
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
                // Fast path for the most common leaves. These exact types can
                // never match the container checks below, so dispatching on
                // them first does not change which branch a value takes.
                if PyString::is_exact_type_of(o)
                    && let Some(s) = o.cast::<PyString>().ok().and_then(|s| s.to_str().ok())
                {
                    return serializer.serialize_str(s);
                }
                if PyInt::is_exact_type_of(o)
                    && let Ok(i) = o.extract::<i64>()
                {
                    return serializer.serialize_i64(i);
                }
                if let Ok(b) = o.cast_exact::<PyBool>() {
                    return serializer.serialize_bool(b.is_true());
                }
                if let Ok(dict) = o.cast_exact::<PyDict>() {
                    let len = dict.len().ok();
                    let mut map = serializer.serialize_map(len)?;
                    for (key, value) in dict {
                        map.serialize_entry(&self.child(key), &self.child(value))?;
                    }
                    map.end()
                } else if let Ok(s) = o.cast::<PyBytes>() {
                    serializer.serialize_bytes(s.as_bytes())
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
                } else if let Some(s) = o.cast::<PyString>().ok().and_then(|s| s.to_str().ok()) {
                    serializer.serialize_str(s)
                } else if let Ok(b) = o.cast::<PyBool>() {
                    serializer.serialize_bool(b.is_true())
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
    if let Some(encoded) = numpy_to_cbor(&py_obj)? {
        return Ok(encoded);
    }
    let mut serializer = minicbor_serde::Serializer::new(vec![]);
    PyValue::new(py_obj)
        .serialize(serializer.serialize_unit_as_null(true))
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    Ok(serializer.into_encoder().into_writer())
}

fn numpy_to_cbor(obj: &Bound<'_, PyAny>) -> PyResult<Option<Vec<u8>>> {
    if obj.get_type().module()?.to_str()? != "numpy" || !obj.hasattr("dtype")? {
        return Ok(None);
    }
    if obj.getattr("ndim")?.extract::<usize>()? != 1 {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "only flat NumPy arrays are supported",
        ));
    }
    let dtype = obj.getattr("dtype")?.getattr("str")?.extract::<String>()?;
    let tag = match dtype.as_str() {
        "|u1" => 64,
        "|i1" => 73,
        "<u2" => 69,
        ">u2" => 65,
        "<i2" => 77,
        ">i2" => 74,
        "<u4" => 70,
        ">u4" => 66,
        "<i4" => 78,
        ">i4" => 75,
        "<u8" => 71,
        ">u8" => 67,
        "<i8" => 79,
        ">i8" => 76,
        "<f4" => 84,
        ">f4" => 81,
        "<f8" => 85,
        ">f8" => 82,
        _ => {
            return Err(pyo3::exceptions::PyTypeError::new_err(format!(
                "unsupported NumPy dtype {dtype}"
            )));
        }
    };
    let raw = obj.call_method0("tobytes")?.extract::<Vec<u8>>()?;
    let mut out = Vec::with_capacity(raw.len() + 12);
    minicbor::Encoder::new(&mut out)
        .tag(minicbor::data::Tag::new(tag))
        .and_then(|e| e.bytes(&raw))
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    Ok(Some(out))
}

pub fn python_to_cbor_emit<F>(
    py_obj: Bound<'_, PyAny>,
    emit_type: crate::wasm::isola::script::host::EmitType,
    mut emit_fn: F,
) -> PyResult<()>
where
    F: FnMut(crate::wasm::isola::script::host::EmitType, &[u8]),
{
    if let Some(encoded) = numpy_to_cbor(&py_obj)? {
        emit_fn(emit_type, &encoded);
        return Ok(());
    }
    let mut writer = CallbackWriter::new(&mut emit_fn, emit_type);
    {
        let mut serializer = minicbor_serde::Serializer::new(&mut writer);
        PyValue::new(py_obj)
            .serialize(serializer.serialize_unit_as_null(true))
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    }
    writer.finish();
    Ok(())
}

pub fn cbor_to_python<'py>(py: Python<'py>, cbor: &[u8]) -> PyResult<Bound<'py, PyAny>> {
    // Let CPython validate UTF-8 as it creates strings with a four-byte CBOR
    // length (typically 64 KiB and up).
    const CBOR_TEXT_WITH_U32_LENGTH: u8 = 0x7a;
    if let [CBOR_TEXT_WITH_U32_LENGTH, a, b, c, d, data @ ..] = cbor
        && let Ok(len) = usize::try_from(u32::from_be_bytes([*a, *b, *c, *d]))
        && let Some(bytes) = data.get(..len)
    {
        if let Some(value) = ascii_to_python(py, bytes) {
            return value.map(Bound::into_any);
        }
        match PyString::from_bytes(py, bytes) {
            Ok(value) => return Ok(value.into_any()),
            Err(error) if error.is_instance_of::<pyo3::exceptions::PyUnicodeDecodeError>(py) => {}
            Err(error) => return Err(error),
        }
    }
    let mut decoder = minicbor::Decoder::new(cbor);
    if decoder
        .datatype()
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?
        == minicbor::data::Type::Tag
    {
        let tag = decoder
            .tag()
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?
            .as_u64();
        let dtype = match tag {
            64 | 72 => "u1",
            73 => "i1",
            65 => ">u2",
            69 => "<u2",
            74 => ">i2",
            77 => "<i2",
            66 => ">u4",
            70 => "<u4",
            75 => ">i4",
            78 => "<i4",
            67 => ">u8",
            71 => "<u8",
            76 => ">i8",
            79 => "<i8",
            81 => ">f4",
            84 => "<f4",
            82 => ">f8",
            85 => "<f8",
            _ => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "unsupported CBOR typed array tag {tag}"
                )));
            }
        };
        let bytes = decoder
            .bytes()
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
        let numpy = py.import("numpy")?;
        return numpy.call_method1("frombuffer", (PyBytes::new(py, bytes), dtype));
    }
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
    use pyo3::types::PyBytes;
    use serde_json::json;

    use super::*;

    #[test]
    fn large_cbor_text_roundtrips_and_rejects_invalid_data() {
        Python::initialize();
        Python::attach(|py| {
            let text = "a\0é🦦".repeat(16_384);
            let mut cbor = Vec::new();
            minicbor::Encoder::new(&mut cbor).str(&text).unwrap();
            let value = cbor_to_python(py, &cbor).unwrap();
            assert_eq!(value.extract::<&str>().unwrap(), text);

            assert!(cbor_to_python(py, &cbor[..cbor.len() - 1]).is_err());
            cbor[5] = 0xff;
            let error = cbor_to_python(py, &cbor).unwrap_err();
            assert!(error.is_instance_of::<pyo3::exceptions::PyValueError>(py));
        });
    }

    #[test]
    fn test_numpy_typed_array_roundtrip() {
        Python::initialize();
        Python::attach(|py| {
            let Ok(np) = py.import("numpy") else { return };
            let array = np.call_method1("array", (vec![1.5, -2.25], "<f4")).unwrap();
            let encoded = python_to_cbor(array).unwrap();
            let mut decoder = minicbor::Decoder::new(&encoded);
            assert_eq!(decoder.tag().unwrap().as_u64(), 84);
            assert_eq!(decoder.bytes().unwrap(), &[0, 0, 192, 63, 0, 0, 16, 192]);
            let roundtrip = cbor_to_python(py, &encoded).unwrap();
            assert_eq!(
                roundtrip
                    .getattr("dtype")
                    .unwrap()
                    .getattr("str")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "<f4"
            );
            assert!(
                roundtrip
                    .call_method1("__eq__", (vec![1.5, -2.25],))
                    .unwrap()
                    .call_method0("all")
                    .unwrap()
                    .is_truthy()
                    .unwrap()
            );
        });
    }

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
            // Test various Python types for round-trip serialization through
            // minicbor
            let mut test_cases = vec![
                // Basic types
                42i32.into_pyobject(py).unwrap().into_any(),
                "hello world".into_pyobject(py).unwrap().into_any(),
                std::f64::consts::PI.into_pyobject(py).unwrap().into_any(),
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
            let set = PySet::new(py, [1, 2, 3, 2, 1]).unwrap();
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
