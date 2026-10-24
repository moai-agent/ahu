//! YAML schema frontmatter, bounded while deserializing (including aliases).
//! Legacy prompt-only frontmatter retains its original scalar parser.
use crate::structured;
use crate::util::{Error, Result};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Frontmatter {
    pub fields: BTreeMap<String, String>,
    pub input: Option<Value>,
    pub output: Option<Value>,
}

pub(super) fn declares_schema(front: &str) -> bool {
    // Decode root keys before selecting the structured path: quoted/escaped keys
    // must not silently disable contracts. The textual fallback also catches
    // declarations in malformed YAML so they cannot fall back to legacy parsing.
    let declared = yaml_serde::from_str::<BTreeMap<String, serde::de::IgnoredAny>>(front)
        .is_ok_and(|fields| {
            fields.contains_key("input_schema") || fields.contains_key("output_schema")
        });
    declared
        || front.lines().any(|line| {
            line.trim().split_once(':').is_some_and(|(key, _)| {
                let decoded = yaml_serde::from_str::<String>(key.trim()).ok();
                matches!(decoded.as_deref(), Some("input_schema" | "output_schema"))
            })
        })
}

pub(super) fn parse(front: &str) -> Result<Frontmatter> {
    if front.len() as u64 > crate::util::MAX_TASK_RECORD_BYTES {
        return Err(Error::new(
            "schema frontmatter exceeds the manifest byte limit",
        ));
    }
    let parsed: Frontmatter = yaml_serde::from_str(front).map_err(|_| {
        Error::new("invalid schema frontmatter: use a YAML mapping with unique string keys and bounded JSON-compatible schema objects; floating-point YAML numbers and custom YAML tags are unsupported")
    })?;
    for (name, schema) in [
        ("input_schema", &parsed.input),
        ("output_schema", &parsed.output),
    ] {
        if let Some(schema) = schema {
            structured::check_schema(schema)
                .map_err(|error| Error::new(format!("{name}: {error}")))?;
        }
    }
    Ok(parsed)
}

impl<'de> Deserialize<'de> for Frontmatter {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Header;
        impl<'de> Visitor<'de> for Header {
            type Value = Frontmatter;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("frontmatter mapping")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut parsed = Frontmatter::default();
                let mut keys = BTreeSet::new();
                while let Some(key) = map.next_key::<StringKey>()? {
                    if !keys.insert(key.0.clone()) {
                        return Err(serde::de::Error::custom("duplicate frontmatter key"));
                    }
                    match key.0.as_str() {
                        "input_schema" | "output_schema" => {
                            let mut nodes = 0;
                            let mut bytes = 0;
                            let value = map.next_value_seed(SchemaValue {
                                depth: 0,
                                nodes: &mut nodes,
                                bytes: &mut bytes,
                            })?;
                            let slot = if key.0 == "input_schema" {
                                &mut parsed.input
                            } else {
                                &mut parsed.output
                            };
                            *slot = Some(value);
                        }
                        "okf_version" | "type" | "title" | "version" | "description"
                        | "harness" | "model" | "permissions" | "status" | "source_format"
                        | "source_path" | "native_helpers" => {
                            // YAML's string deserializer preserves scalar text (including
                            // the unquoted numeric OKF version) without number coercion.
                            parsed.fields.insert(key.0, map.next_value::<String>()?);
                        }
                        _ => {
                            let _ = map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(parsed)
            }
        }
        d.deserialize_map(Header)
    }
}

struct StringKey(String);
impl<'de> Deserialize<'de> for StringKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Key;
        impl Visitor<'_> for Key {
            type Value = StringKey;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("string key")
            }
            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StringKey(value.into()))
            }
        }
        // deserialize_string would silently coerce non-string YAML keys.
        d.deserialize_any(Key)
    }
}

struct SchemaValue<'a> {
    depth: usize,
    nodes: &'a mut usize,
    bytes: &'a mut usize,
}
impl SchemaValue<'_> {
    fn charge<E: serde::de::Error>(&mut self, bytes: usize) -> std::result::Result<(), E> {
        *self.bytes = self.bytes.saturating_add(bytes);
        if *self.bytes > structured::MAX_BYTES {
            return Err(E::custom("schema byte limit"));
        }
        Ok(())
    }
    fn child(&mut self) -> SchemaValue<'_> {
        SchemaValue {
            depth: self.depth + 1,
            nodes: self.nodes,
            bytes: self.bytes,
        }
    }
}
impl<'de> DeserializeSeed<'de> for SchemaValue<'_> {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(mut self, d: D) -> std::result::Result<Value, D::Error> {
        *self.nodes += 1;
        if self.depth > structured::MAX_DEPTH || *self.nodes > structured::MAX_NODES {
            return Err(serde::de::Error::custom("schema depth or node limit"));
        }
        self.charge::<D::Error>(1)?;
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for SchemaValue<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("bounded JSON-compatible YAML schema")
    }
    fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_i128<E: serde::de::Error>(self, v: i128) -> std::result::Result<Value, E> {
        v.to_string().parse().map(Value::Number).map_err(E::custom)
    }
    fn visit_u128<E: serde::de::Error>(self, v: u128) -> std::result::Result<Value, E> {
        v.to_string().parse().map(Value::Number).map_err(E::custom)
    }
    // No f64 visitor: yaml_serde has already rounded decimals by that point.
    // Refuse them instead of weakening enum/const or integer assertions silently.
    fn visit_str<E: serde::de::Error>(mut self, v: &str) -> std::result::Result<Value, E> {
        self.charge::<E>(v.len())?;
        Ok(v.into())
    }
    fn visit_seq<A: SeqAccess<'de>>(mut self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(self.child())? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<StringKey>()? {
            self.charge::<A::Error>(key.0.len())?;
            if values.contains_key(&key.0) {
                return Err(serde::de::Error::custom("duplicate schema key"));
            }
            let value = map.next_value_seed(self.child())?;
            values.insert(key.0, value);
        }
        Ok(Value::Object(values))
    }
}
