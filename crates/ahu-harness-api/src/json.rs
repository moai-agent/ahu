//! Strict JSON decoding shared by independent runtime authors and core.
use serde::de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::HashSet;
use std::fmt;

/// Decode a protocol frame only after checking that no JSON object repeats a
/// key. serde_json::Value intentionally keeps the last duplicate, which would
/// otherwise erase ambiguous host assertions before typed validation.
pub fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> std::result::Result<T, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    UniqueJsonValue::deserialize(&mut deserializer)?;
    deserializer.end()?;
    serde_json::from_slice(bytes)
}

struct UniqueJsonValue;

impl<'de> Deserialize<'de> for UniqueJsonValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueJsonVisitor)
    }
}

struct UniqueJsonVisitor;

impl<'de> Visitor<'de> for UniqueJsonVisitor {
    type Value = UniqueJsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(UniqueJsonValue)
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(UniqueJsonValue)
    }

    fn visit_bool<E>(self, _: bool) -> std::result::Result<Self::Value, E> {
        Ok(UniqueJsonValue)
    }

    fn visit_i64<E>(self, _: i64) -> std::result::Result<Self::Value, E> {
        Ok(UniqueJsonValue)
    }

    fn visit_u64<E>(self, _: u64) -> std::result::Result<Self::Value, E> {
        Ok(UniqueJsonValue)
    }

    fn visit_f64<E>(self, _: f64) -> std::result::Result<Self::Value, E> {
        Ok(UniqueJsonValue)
    }

    fn visit_str<E>(self, _: &str) -> std::result::Result<Self::Value, E> {
        Ok(UniqueJsonValue)
    }

    fn visit_borrowed_str<E>(self, _: &'de str) -> std::result::Result<Self::Value, E> {
        Ok(UniqueJsonValue)
    }

    fn visit_string<E>(self, _: String) -> std::result::Result<Self::Value, E> {
        Ok(UniqueJsonValue)
    }

    fn visit_seq<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while access.next_element::<UniqueJsonValue>()?.is_some() {}
        Ok(UniqueJsonValue)
    }

    fn visit_map<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        while let Some(key) = access.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            access.next_value::<UniqueJsonValue>()?;
        }
        Ok(UniqueJsonValue)
    }
}
