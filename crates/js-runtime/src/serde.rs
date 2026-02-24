use rquickjs::{Ctx, IntoJs, Value};
use serde::{
    Deserializer, Serialize,
    de::{
        DeserializeSeed, Expected, IntoDeserializer, Unexpected, Visitor,
        value::{MapAccessDeserializer, MapDeserializer, SeqDeserializer},
    },
    ser::{SerializeMap, SerializeSeq},
};

const MAX_DEPTH: usize = 128;

struct JsValue<'js>(Value<'js>);

impl<'js> JsValue<'js> {
    const fn new(val: Value<'js>) -> Self {
        Self(val)
    }

    fn deserialize_into<'de, D>(ctx: &Ctx<'js>, deserializer: D) -> Result<Value<'js>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        JsDeserializer(ctx.clone()).deserialize(deserializer)
    }
}

impl IntoDeserializer<'_> for JsValue<'_> {
    type Deserializer = Self;
    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}

impl JsValue<'_> {
    #[cold]
    fn invalid_type<E>(&self, exp: &dyn Expected) -> E
    where
        E: serde::de::Error,
    {
        let v = &self.0;
        let unexp = if v.is_object() {
            if v.is_array() {
                Unexpected::Seq
            } else {
                Unexpected::Map
            }
        } else if let Some(s) = v.as_string() {
            if let Ok(s) = s.to_string() {
                return serde::de::Error::invalid_type(Unexpected::Str(&s), exp);
            }
            Unexpected::Other("string")
        } else if let Some(b) = v.as_bool() {
            Unexpected::Bool(b)
        } else if v.is_null() || v.is_undefined() {
            Unexpected::Option
        } else if let Some(f) = v.as_float() {
            Unexpected::Float(f)
        } else if let Some(i) = v.as_int() {
            Unexpected::Signed(i64::from(i))
        } else {
            Unexpected::Other("non-serializable value")
        };
        serde::de::Error::invalid_type(unexp, exp)
    }
}

struct JsDeserializer<'js>(Ctx<'js>);

struct JsVisitor<'js>(Ctx<'js>);

impl<'de, 'js> Visitor<'de> for JsVisitor<'js> {
    type Value = Value<'js>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a type that can deserialize to a JS value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        v.into_js(&self.0)
            .map_err(|e| serde::de::Error::custom(e.to_string()))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        // If it fits in i32, use int; otherwise use float
        if let Ok(i) = i32::try_from(v) {
            i.into_js(&self.0)
                .map_err(|e| serde::de::Error::custom(e.to_string()))
        } else {
            #[allow(clippy::cast_precision_loss)]
            (v as f64)
                .into_js(&self.0)
                .map_err(|e| serde::de::Error::custom(e.to_string()))
        }
    }

    fn visit_i128<E>(self, v: i128) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        #[allow(clippy::cast_precision_loss)]
        self.visit_f64(v as f64)
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if let Ok(i) = i32::try_from(v) {
            i.into_js(&self.0)
                .map_err(|e| serde::de::Error::custom(e.to_string()))
        } else {
            #[allow(clippy::cast_precision_loss)]
            (v as f64)
                .into_js(&self.0)
                .map_err(|e| serde::de::Error::custom(e.to_string()))
        }
    }

    fn visit_u128<E>(self, v: u128) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        #[allow(clippy::cast_precision_loss)]
        self.visit_f64(v as f64)
    }

    fn visit_f32<E>(self, v: f32) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        f64::from(v)
            .into_js(&self.0)
            .map_err(|e| serde::de::Error::custom(e.to_string()))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        v.into_js(&self.0)
            .map_err(|e| serde::de::Error::custom(e.to_string()))
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        rquickjs::String::from_str(self.0.clone(), v)
            .map(rquickjs::String::into_value)
            .map_err(|e| serde::de::Error::custom(e.to_string()))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&v)
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        rquickjs::ArrayBuffer::new(self.0.clone(), v.to_vec())
            .map(rquickjs::ArrayBuffer::into_value)
            .map_err(|e| serde::de::Error::custom(e.to_string()))
    }

    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        rquickjs::ArrayBuffer::new(self.0.clone(), v)
            .map(rquickjs::ArrayBuffer::into_value)
            .map_err(|e| serde::de::Error::custom(e.to_string()))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Value::new_null(self.0))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Value::new_null(self.0))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let arr = rquickjs::Array::new(self.0.clone())
            .map_err(|e| serde::de::Error::custom(e.to_string()))?;
        let mut idx = 0usize;
        while let Some(val) = seq.next_element_seed(JsDeserializer(self.0.clone()))? {
            arr.set(idx, val)
                .map_err(|e| serde::de::Error::custom(e.to_string()))?;
            idx += 1;
        }
        Ok(arr.into_value())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let obj = rquickjs::Object::new(self.0.clone())
            .map_err(|e| serde::de::Error::custom(e.to_string()))?;
        while let Some((key, val)) = map.next_entry_seed(
            JsDeserializer(self.0.clone()),
            JsDeserializer(self.0.clone()),
        )? {
            obj.set::<Value<'_>, Value<'_>>(key, val)
                .map_err(|e| serde::de::Error::custom(e.to_string()))?;
        }
        Ok(obj.into_value())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        JsValue::deserialize_into(&self.0, deserializer)
    }

    fn visit_newtype_struct<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        JsValue::deserialize_into(&self.0, deserializer)
    }
}

impl<'de, 'js> DeserializeSeed<'de> for JsDeserializer<'js> {
    type Value = Value<'js>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(JsVisitor(self.0))
    }
}

impl Serialize for JsValue<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        struct JsSerialize<'s>(Value<'s>, usize);
        impl<'s> JsSerialize<'s> {
            const fn child(&self, val: Value<'s>) -> Self {
                Self(val, self.1 - 1)
            }
        }
        impl serde::Serialize for JsSerialize<'_> {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                if self.1 == 0 {
                    return Err(serde::ser::Error::custom(
                        "maximum serialization depth exceeded, possible circular reference",
                    ));
                }

                let v = &self.0;

                if v.is_null() || v.is_undefined() {
                    serializer.serialize_none()
                } else if let Some(b) = v.as_bool() {
                    serializer.serialize_bool(b)
                } else if let Some(i) = v.as_int() {
                    serializer.serialize_i32(i)
                } else if v.is_number() {
                    if let Some(f) = v.as_float() {
                        serializer.serialize_f64(f)
                    } else {
                        // as_number returns f64 for all numbers
                        serializer.serialize_f64(
                            v.as_number().ok_or_else(|| {
                                serde::ser::Error::custom("value is not a number")
                            })?,
                        )
                    }
                } else if let Some(s) = v.as_string() {
                    let s = s.to_string().map_err(serde::ser::Error::custom)?;
                    serializer.serialize_str(&s)
                } else if v.is_object() && !v.is_array() {
                    // Check if it's an ArrayBuffer
                    if let Some(obj) = v.as_object() {
                        if let Some(buf) = obj.as_array_buffer()
                            && let Some(bytes) = buf.as_bytes()
                        {
                            return serializer.serialize_bytes(bytes);
                        }
                        // Check if it's a TypedArray (Uint8Array, etc.)
                        if let Some(ta) = obj.as_typed_array::<u8>()
                            && let Some(bytes) = ta.as_bytes()
                        {
                            return serializer.serialize_bytes(bytes);
                        }
                    }
                    // Fall through to object serialization below
                    let obj = v
                        .as_object()
                        .ok_or_else(|| serde::ser::Error::custom("expected object"))?;
                    let props: Vec<(rquickjs::atom::Atom<'_>, Value<'_>)> = obj
                        .own_props(rquickjs::object::Filter::new().string().enum_only())
                        .flatten()
                        .collect();
                    let mut map = serializer.serialize_map(Some(props.len()))?;
                    for (key, val) in props {
                        let key_str: String = key.to_string().map_err(serde::ser::Error::custom)?;
                        map.serialize_entry(&key_str, &self.child(val))?;
                    }
                    map.end()
                } else if v.is_array() {
                    let arr = v
                        .as_array()
                        .ok_or_else(|| serde::ser::Error::custom("expected array"))?;
                    let len = arr.len();
                    let mut seq = serializer.serialize_seq(Some(len))?;
                    for i in 0..len {
                        let elem: Value<'_> = arr.get(i).map_err(serde::ser::Error::custom)?;
                        seq.serialize_element(&self.child(elem))?;
                    }
                    seq.end()
                } else {
                    Err(serde::ser::Error::custom(format!(
                        "non-serializable JS value type: {:?}",
                        v.type_of()
                    )))
                }
            }
        }
        JsSerialize(self.0.clone(), MAX_DEPTH).serialize(serializer)
    }
}

impl<'de> Deserializer<'de> for JsValue<'_> {
    type Error = serde::de::value::Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let v = &self.0;
        if v.is_null() || v.is_undefined() {
            visitor.visit_unit()
        } else if let Some(b) = v.as_bool() {
            visitor.visit_bool(b)
        } else if let Some(i) = v.as_int() {
            visitor.visit_i64(i64::from(i))
        } else if v.is_number() {
            let f = v
                .as_number()
                .ok_or_else(|| serde::de::Error::custom("expected number"))?;
            visitor.visit_f64(f)
        } else if let Some(s) = v.as_string() {
            let s = s.to_string().map_err(serde::de::Error::custom)?;
            visitor.visit_string(s)
        } else if v.is_array() {
            self.deserialize_seq(visitor)
        } else if v.is_object() {
            // Check ArrayBuffer first
            if let Some(buf) = rquickjs::ArrayBuffer::from_value(v.clone())
                && let Some(bytes) = buf.as_bytes()
            {
                return visitor.visit_bytes(bytes);
            }
            if let Ok(ta) = rquickjs::TypedArray::<u8>::from_value(v.clone())
                && let Some(bytes) = ta.as_bytes()
            {
                return visitor.visit_bytes(bytes);
            }
            self.deserialize_map(visitor)
        } else {
            Err(self.invalid_type(&visitor))
        }
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if let Some(b) = self.0.as_bool() {
            visitor.visit_bool(b)
        } else {
            Err(self.invalid_type(&visitor))
        }
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if self.0.is_null() || self.0.is_undefined() {
            visitor.visit_unit()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if self.0.is_null() || self.0.is_undefined() {
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
        if let Some(arr) = self.0.as_array() {
            let items = (0..arr.len()).map(|i| {
                JsValue::new(
                    arr.get::<Value<'_>>(i)
                        .unwrap_or_else(|_| Value::new_null(arr.ctx().clone())),
                )
            });
            let mut deserializer = SeqDeserializer::new(items);
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
        if let Some(obj) = self.0.as_object() {
            let entries: Vec<(JsValue<'_>, JsValue<'_>)> = obj
                .own_props::<Value<'_>, Value<'_>>(
                    rquickjs::object::Filter::new().string().enum_only(),
                )
                .flatten()
                .map(|(k, v)| (JsValue::new(k), JsValue::new(v)))
                .collect();
            let mut map = MapDeserializer::new(entries.into_iter());
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
        let v = &self.0;
        if let Some(s) = v.as_string() {
            let s = s.to_string().map_err(serde::de::Error::custom)?;
            visitor.visit_enum(s.into_deserializer())
        } else if v.is_object() {
            let obj = v
                .as_object()
                .ok_or_else(|| serde::de::Error::custom("expected object"))?;
            let entries: Vec<(JsValue<'_>, JsValue<'_>)> = obj
                .own_props::<Value<'_>, Value<'_>>(
                    rquickjs::object::Filter::new().string().enum_only(),
                )
                .flatten()
                .map(|(k, v)| (JsValue::new(k), JsValue::new(v)))
                .collect();
            let map = MapDeserializer::new(entries.into_iter());
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

pub fn js_to_cbor(val: Value<'_>) -> Result<Vec<u8>, String> {
    let mut serializer = minicbor_serde::Serializer::new(vec![]);
    JsValue::new(val)
        .serialize(serializer.serialize_unit_as_null(true))
        .map_err(|e| e.to_string())?;
    Ok(serializer.into_encoder().into_writer())
}

pub fn js_to_cbor_emit<F>(
    val: Value<'_>,
    emit_type: crate::wasm::isola::script::host::EmitType,
    mut emit_fn: F,
) -> Result<(), String>
where
    F: FnMut(crate::wasm::isola::script::host::EmitType, &[u8]),
{
    let mut writer: CallbackWriter<_, 1024> = CallbackWriter::new(&mut emit_fn, emit_type);
    let mut serializer = minicbor_serde::Serializer::new(&mut writer);
    JsValue::new(val)
        .serialize(serializer.serialize_unit_as_null(true))
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub struct CallbackWriter<'a, F, const N: usize = 1024>
where
    F: FnMut(crate::wasm::isola::script::host::EmitType, &[u8]),
{
    buffer: heapless::Vec<u8, N>,
    emit_fn: &'a mut F,
    end_type: crate::wasm::isola::script::host::EmitType,
}

impl<'a, F, const N: usize> CallbackWriter<'a, F, N>
where
    F: FnMut(crate::wasm::isola::script::host::EmitType, &[u8]),
{
    pub const fn new(
        emit_fn: &'a mut F,
        end_type: crate::wasm::isola::script::host::EmitType,
    ) -> Self {
        Self {
            emit_fn,
            buffer: heapless::Vec::new(),
            end_type,
        }
    }

    fn flush(&mut self) {
        if !self.buffer.is_empty() {
            (self.emit_fn)(
                crate::wasm::isola::script::host::EmitType::Continuation,
                &self.buffer,
            );
            self.buffer.clear();
        }
    }
}

impl<F, const N: usize> minicbor::encode::Write for CallbackWriter<'_, F, N>
where
    F: FnMut(crate::wasm::isola::script::host::EmitType, &[u8]),
{
    type Error = std::convert::Infallible;

    fn write_all(&mut self, buf: &[u8]) -> std::result::Result<(), Self::Error> {
        let mut remaining = buf;

        while !remaining.is_empty() {
            let available_space = N - self.buffer.len();
            if available_space == 0 {
                self.flush();
                continue;
            }

            let to_write = remaining.len().min(available_space);
            let (chunk, rest) = remaining.split_at(to_write);
            self.buffer.extend_from_slice(chunk).ok();
            remaining = rest;
        }

        Ok(())
    }
}

impl<F, const N: usize> Drop for CallbackWriter<'_, F, N>
where
    F: FnMut(crate::wasm::isola::script::host::EmitType, &[u8]),
{
    fn drop(&mut self) {
        (self.emit_fn)(self.end_type, &self.buffer);
    }
}

pub fn cbor_to_js<'js>(ctx: &Ctx<'js>, cbor: &[u8]) -> Result<Value<'js>, String> {
    let mut deserializer = minicbor_serde::Deserializer::new(cbor);
    JsValue::deserialize_into(ctx, &mut deserializer).map_err(|e| e.to_string())
}

pub fn js_to_json(val: Value<'_>) -> Result<String, String> {
    let mut o = vec![];
    JsValue::new(val)
        .serialize(&mut serde_json::Serializer::new(&mut o))
        .map_err(|e| e.to_string())?;
    String::from_utf8(o).map_err(|e| e.to_string())
}

pub fn json_to_js<'js>(ctx: &Ctx<'js>, json: &str) -> Result<Value<'js>, String> {
    let mut de = serde_json::Deserializer::from_str(json);
    JsValue::deserialize_into(ctx, &mut de).map_err(|e| e.to_string())
}

pub fn js_to_yaml(val: Value<'_>) -> Result<String, String> {
    let mut o = vec![];
    JsValue::new(val)
        .serialize(&mut serde_yaml::Serializer::new(&mut o))
        .map_err(|e| e.to_string())?;
    String::from_utf8(o).map_err(|e| e.to_string())
}

pub fn yaml_to_js<'js>(ctx: &Ctx<'js>, yaml: &str) -> Result<Value<'js>, String> {
    let deserializer = serde_yaml::Deserializer::from_str(yaml);
    JsValue::deserialize_into(ctx, deserializer).map_err(|e| e.to_string())
}
