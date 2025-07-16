use serde::{
    Deserializer, Serialize, Serializer,
    de::{Expected, Unexpected, Visitor},
    ser::{Impossible, SerializeMap, SerializeSeq},
};
pub struct ProstValue<'a>(pub &'a prost_types::Value);

impl<'a> ProstValue<'a> {
    pub const fn new(value: &'a prost_types::Value) -> Self {
        Self(value)
    }

    pub const fn serializer() -> impl Serializer<Ok = prost_types::Value> {
        ProstValueSerializer
    }

    #[cold]
    fn invalid_type<E>(&self, exp: &dyn Expected) -> E
    where
        E: serde::de::Error,
    {
        match &self.0.kind {
            Some(prost_types::value::Kind::NullValue(_)) => {
                E::invalid_type(Unexpected::Option, exp)
            }
            Some(prost_types::value::Kind::NumberValue(f)) => {
                E::invalid_type(Unexpected::Float(*f), exp)
            }
            Some(prost_types::value::Kind::StringValue(s)) => {
                E::invalid_type(Unexpected::Str(s), exp)
            }
            Some(prost_types::value::Kind::BoolValue(b)) => {
                E::invalid_type(Unexpected::Bool(*b), exp)
            }
            Some(prost_types::value::Kind::StructValue(_)) => E::invalid_type(Unexpected::Map, exp),
            Some(prost_types::value::Kind::ListValue(_)) => E::invalid_type(Unexpected::Seq, exp),
            None => E::invalid_type(Unexpected::Other("empty value"), exp),
        }
    }
}

struct StructMapAccess<'a> {
    iter: std::collections::btree_map::Iter<'a, String, prost_types::Value>,
    value: Option<&'a prost_types::Value>,
}

impl<'a, 'de> serde::de::MapAccess<'de> for StructMapAccess<'a>
where
    'a: 'de,
{
    type Error = serde::de::value::Error;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
    where
        K: serde::de::DeserializeSeed<'de>,
    {
        if let Some((key, value)) = self.iter.next() {
            self.value = Some(value);
            let de = serde::de::value::StrDeserializer::new(key.as_str());
            seed.deserialize(de).map(Some)
        } else {
            self.value = None;
            Ok(None)
        }
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::DeserializeSeed<'de>,
    {
        match self.value.take() {
            Some(value) => {
                let de = ProstValue(value);
                seed.deserialize(de)
            }
            None => Err(serde::de::Error::custom(
                "value called before key or after end of map",
            )),
        }
    }
}

struct ListSeqAccess<'a> {
    iter: std::slice::Iter<'a, prost_types::Value>,
}

impl<'a, 'de> serde::de::SeqAccess<'de> for ListSeqAccess<'a>
where
    'a: 'de,
{
    type Error = serde::de::value::Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: serde::de::DeserializeSeed<'de>,
    {
        match self.iter.next() {
            Some(value) => {
                let de = ProstValue(value);
                seed.deserialize(de).map(Some)
            }
            None => Ok(None),
        }
    }
}

macro_rules! impl_deserialize_number {
    ($method:ident, $visit:ident) => {
        fn $method<V>(self, visitor: V) -> Result<V::Value, Self::Error>
        where
            V: Visitor<'de>,
        {
            match &self.0.kind {
                Some(prost_types::value::Kind::NumberValue(f)) => visitor.$visit(*f as _),
                _ => Err(self.invalid_type(&visitor)),
            }
        }
    };
}

impl<'de> Deserializer<'de> for ProstValue<'de> {
    type Error = serde::de::value::Error;
    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match &self.0.kind {
            Some(prost_types::value::Kind::NullValue(_)) => visitor.visit_none(),
            Some(prost_types::value::Kind::NumberValue(f)) => visitor.visit_f64(*f),
            Some(prost_types::value::Kind::StringValue(s)) => visitor.visit_str(s),
            Some(prost_types::value::Kind::BoolValue(b)) => visitor.visit_bool(*b),
            Some(prost_types::value::Kind::StructValue(s)) => {
                let mut map_access = StructMapAccess {
                    iter: s.fields.iter(),
                    value: None,
                };
                visitor.visit_map(&mut map_access)
            }
            Some(prost_types::value::Kind::ListValue(l)) => {
                let mut seq_access = ListSeqAccess {
                    iter: l.values.iter(),
                };
                visitor.visit_seq(&mut seq_access)
            }
            None => visitor.visit_unit(),
        }
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match &self.0.kind {
            Some(prost_types::value::Kind::BoolValue(b)) => visitor.visit_bool(*b),
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    impl_deserialize_number!(deserialize_i8, visit_i8);
    impl_deserialize_number!(deserialize_i16, visit_i16);
    impl_deserialize_number!(deserialize_i32, visit_i32);
    impl_deserialize_number!(deserialize_i64, visit_i64);
    impl_deserialize_number!(deserialize_u8, visit_u8);
    impl_deserialize_number!(deserialize_u16, visit_u16);
    impl_deserialize_number!(deserialize_u32, visit_u32);
    impl_deserialize_number!(deserialize_u64, visit_u64);
    impl_deserialize_number!(deserialize_f32, visit_f32);
    impl_deserialize_number!(deserialize_f64, visit_f64);

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match &self.0.kind {
            Some(prost_types::value::Kind::StringValue(s)) => {
                let mut chars = s.chars();
                if let (Some(c), None) = (chars.next(), chars.next()) {
                    visitor.visit_char(c)
                } else {
                    Err(self.invalid_type(&visitor))
                }
            }
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match &self.0.kind {
            Some(prost_types::value::Kind::StringValue(s)) => visitor.visit_str(s),
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match &self.0.kind {
            Some(prost_types::value::Kind::StringValue(s)) => visitor.visit_string(s.clone()),
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match &self.0.kind {
            Some(prost_types::value::Kind::StringValue(s)) => visitor.visit_bytes(s.as_bytes()),
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match &self.0.kind {
            Some(prost_types::value::Kind::StringValue(s)) => {
                visitor.visit_byte_buf(s.clone().into_bytes())
            }
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match &self.0.kind {
            Some(prost_types::value::Kind::NullValue(_)) | None => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match &self.0.kind {
            None | Some(prost_types::value::Kind::NullValue(_)) => visitor.visit_unit(),
            _ => Err(self.invalid_type(&visitor)),
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
        match &self.0.kind {
            Some(prost_types::value::Kind::ListValue(l)) => {
                let mut seq_access = ListSeqAccess {
                    iter: l.values.iter(),
                };
                visitor.visit_seq(&mut seq_access)
            }
            _ => Err(self.invalid_type(&visitor)),
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
        match &self.0.kind {
            Some(prost_types::value::Kind::StructValue(s)) => {
                let mut map_access = StructMapAccess {
                    iter: s.fields.iter(),
                    value: None,
                };
                visitor.visit_map(&mut map_access)
            }
            _ => Err(self.invalid_type(&visitor)),
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
        match &self.0.kind {
            Some(prost_types::value::Kind::StructValue(s)) => {
                let mut map_access = StructMapAccess {
                    iter: s.fields.iter(),
                    value: None,
                };
                visitor.visit_enum(serde::de::value::MapAccessDeserializer::new(
                    &mut map_access,
                ))
            }
            Some(prost_types::value::Kind::StringValue(s)) => {
                visitor.visit_enum(serde::de::value::StrDeserializer::new(s.as_str()))
            }
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }
}

impl Serialize for ProstValue<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self.0.kind {
            Some(prost_types::value::Kind::NullValue(_)) => serializer.serialize_none(),
            Some(prost_types::value::Kind::NumberValue(f)) => serializer.serialize_f64(*f),
            Some(prost_types::value::Kind::StringValue(s)) => serializer.serialize_str(s),
            Some(prost_types::value::Kind::BoolValue(b)) => serializer.serialize_bool(*b),
            Some(prost_types::value::Kind::StructValue(s)) => {
                let mut map = serializer.serialize_map(Some(s.fields.len()))?;
                for (key, value) in &s.fields {
                    map.serialize_entry(key, &ProstValue(value))?;
                }
                map.end()
            }
            Some(prost_types::value::Kind::ListValue(l)) => {
                let mut seq = serializer.serialize_seq(Some(l.values.len()))?;
                for value in &l.values {
                    seq.serialize_element(&ProstValue(value))?;
                }
                seq.end()
            }
            None => serializer.serialize_unit(),
        }
    }
}

struct ProstValueSerializer;

impl serde::ser::Serializer for ProstValueSerializer {
    type Ok = prost_types::Value;
    type Error = serde::de::value::Error;

    type SerializeSeq = SerializeList;
    type SerializeTuple = SerializeList;
    type SerializeTupleStruct = SerializeList;
    type SerializeTupleVariant = Impossible<prost_types::Value, serde::de::value::Error>;
    type SerializeMap = ProstSerializeMap;
    type SerializeStruct = ProstSerializeMap;
    type SerializeStructVariant = Impossible<prost_types::Value, serde::de::value::Error>;

    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error> {
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::BoolValue(v)),
        })
    }
    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(v))
    }
    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(v))
    }
    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(v))
    }
    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::NumberValue(v as f64)),
        })
    }
    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(u64::from(v))
    }
    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(u64::from(v))
    }
    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(u64::from(v))
    }

    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::NumberValue(v as f64)),
        })
    }
    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        self.serialize_f64(f64::from(v))
    }
    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::NumberValue(v)),
        })
    }
    fn serialize_char(self, v: char) -> Result<Self::Ok, Self::Error> {
        self.serialize_str(&v.to_string())
    }
    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::StringValue(v.to_owned())),
        })
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<Self::Ok, Self::Error> {
        Err(serde::de::Error::custom(
            "bytes serialization not supported",
        ))
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::NullValue(0)),
        })
    }
    fn serialize_some<T>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.serialize_none()
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::NullValue(0)),
        })
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.serialize_str(variant)
    }
    fn serialize_newtype_struct<T>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        let map = std::collections::BTreeMap::from([(
            variant.to_owned(),
            value.serialize(ProstValueSerializer)?,
        )]);
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::StructValue(prost_types::Struct {
                fields: map,
            })),
        })
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(SerializeList {
            values: if let Some(cap) = len {
                Vec::with_capacity(cap)
            } else {
                Vec::new()
            },
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Ok(SerializeList {
            values: Vec::with_capacity(len),
        })
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Ok(SerializeList {
            values: Vec::with_capacity(len),
        })
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(serde::de::Error::custom("tuple variants not supported"))
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(ProstSerializeMap {
            map: std::collections::BTreeMap::new(),
            key: None,
        })
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(ProstSerializeMap {
            map: std::collections::BTreeMap::new(),
            key: None,
        })
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(serde::de::Error::custom("struct variants not supported"))
    }
}

struct SerializeList {
    values: Vec<prost_types::Value>,
}

impl serde::ser::SerializeSeq for SerializeList {
    type Ok = prost_types::Value;
    type Error = serde::de::value::Error;
    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        self.values.push(value.serialize(ProstValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::ListValue(
                prost_types::ListValue {
                    values: self.values,
                },
            )),
        })
    }
}

impl serde::ser::SerializeTuple for SerializeList {
    type Ok = prost_types::Value;
    type Error = serde::de::value::Error;
    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        self.values.push(value.serialize(ProstValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::ListValue(
                prost_types::ListValue {
                    values: self.values,
                },
            )),
        })
    }
}

impl serde::ser::SerializeTupleStruct for SerializeList {
    type Ok = prost_types::Value;
    type Error = serde::de::value::Error;
    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        self.values.push(value.serialize(ProstValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::ListValue(
                prost_types::ListValue {
                    values: self.values,
                },
            )),
        })
    }
}

struct ProstSerializeMap {
    map: std::collections::BTreeMap<String, prost_types::Value>,
    key: Option<String>,
}

impl serde::ser::SerializeMap for ProstSerializeMap {
    type Ok = prost_types::Value;
    type Error = serde::de::value::Error;
    fn serialize_key<T>(&mut self, key: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        let k = key.serialize(StringKeySerializer)?;
        self.key = Some(k);
        Ok(())
    }
    fn serialize_value<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        let k = self.key.take().ok_or_else(|| {
            serde::de::Error::custom("serialize_value called before serialize_key")
        })?;
        let v = value.serialize(ProstValueSerializer)?;
        self.map.insert(k, v);
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::StructValue(prost_types::Struct {
                fields: self.map,
            })),
        })
    }
}

impl serde::ser::SerializeStruct for ProstSerializeMap {
    type Ok = prost_types::Value;
    type Error = serde::de::value::Error;
    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        let v = value.serialize(ProstValueSerializer)?;
        self.map.insert(key.to_owned(), v);
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(prost_types::Value {
            kind: Some(prost_types::value::Kind::StructValue(prost_types::Struct {
                fields: self.map,
            })),
        })
    }
}

struct StringKeySerializer;

impl serde::Serializer for StringKeySerializer {
    type Ok = String;
    type Error = serde::de::value::Error;
    type SerializeSeq = Impossible<String, serde::de::value::Error>;
    type SerializeTuple = Impossible<String, serde::de::value::Error>;
    type SerializeTupleStruct = Impossible<String, serde::de::value::Error>;
    type SerializeTupleVariant = Impossible<String, serde::de::value::Error>;
    type SerializeMap = Impossible<String, serde::de::value::Error>;
    type SerializeStruct = Impossible<String, serde::de::value::Error>;
    type SerializeStructVariant = Impossible<String, serde::de::value::Error>;
    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_owned())
    }
    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }
    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }
    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }
    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }
    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }
    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }
    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }
    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }
    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }
    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }
    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }
    fn serialize_char(self, v: char) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<Self::Ok, Self::Error> {
        Err(serde::de::Error::custom(
            "bytes serialization not supported",
        ))
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(String::new())
    }
    fn serialize_some<T>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(String::new())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(String::new())
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(variant.to_owned())
    }
    fn serialize_newtype_struct<T>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        let mut s = variant.to_owned();
        s.push_str(&value.serialize(self)?);
        Ok(s)
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Err(serde::de::Error::custom("unsupported"))
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Err(serde::de::Error::custom("unsupported"))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Err(serde::de::Error::custom("unsupported"))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(serde::de::Error::custom("unsupported"))
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Err(serde::de::Error::custom("unsupported"))
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Err(serde::de::Error::custom("unsupported"))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(serde::de::Error::custom("unsupported"))
    }
}
