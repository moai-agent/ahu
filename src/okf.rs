//! Shared bounded OKF 0.2 documents. Only callers' strict typed `ahu` schemas
//! have operational meaning; metadata and Markdown are retained, never executed.
use crate::util::{Error, Result};
use serde::de::{DeserializeOwned, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::path::Path;

pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 100_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub fields: Map<String, Value>,
    pub body: String,
}
impl Document {
    pub fn parse(text: &str, kind: &str) -> Result<Self> {
        if text.len() as u64 > MAX_BYTES {
            return Err(Error::new("OKF document exceeds byte limit"));
        }
        let mut lines = text.split_inclusive('\n');
        if lines.next().map(str::trim_end) != Some("---") {
            return Err(Error::new("OKF document requires YAML frontmatter"));
        }
        let start = text
            .find('\n')
            .ok_or_else(|| Error::new("unclosed OKF frontmatter"))?
            + 1;
        let mut end = start;
        let mut body_start = None;
        for line in lines {
            if line.trim_end() == "---" {
                body_start = Some(end + line.len());
                break;
            }
            end += line.len();
        }
        let body_start = body_start.ok_or_else(|| Error::new("unclosed OKF frontmatter"))?;
        let mut nodes = 0;
        let mut bytes = 0;
        let value = BoundedValue { depth: 0, nodes: &mut nodes, bytes: &mut bytes }
            .deserialize(yaml_serde::Deserializer::from_str(&text[start..end]))
            .map_err(|_| Error::new("invalid bounded YAML: require unique string keys, finite values, no tags, and bounded depth/alias expansion"))?;
        let Value::Object(fields) = value else {
            return Err(Error::new("OKF frontmatter must be a mapping"));
        };
        let version = fields.get("okf_version");
        if version != Some(&Value::String("0.2".into())) && version != Some(&serde_json::json!(0.2))
        {
            return Err(Error::new("unsupported okf_version; expected 0.2"));
        }
        if fields.get("type").and_then(Value::as_str) != Some(kind) {
            return Err(Error::new("unsupported OKF document type"));
        }
        if !fields.get("ahu").is_some_and(Value::is_object) {
            return Err(Error::new("OKF document requires an ahu mapping"));
        }
        Ok(Self {
            fields,
            body: text[body_start..].into(),
        })
    }
    pub fn settings<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_value(self.fields["ahu"].clone()).map_err(|_| {
            Error::new("invalid ahu settings: unknown field, missing field, or incorrect type")
        })
    }
    pub fn new<T: Serialize>(kind: &str, settings: &T, body: &str) -> Result<Self> {
        let mut fields = Map::new();
        fields.insert("okf_version".into(), "0.2".into());
        fields.insert("type".into(), kind.into());
        fields.insert(
            "ahu".into(),
            serde_json::to_value(settings).map_err(|_| Error::new("cannot encode ahu settings"))?,
        );
        Ok(Self {
            fields,
            body: body.into(),
        })
    }
    pub fn set_settings<T: Serialize>(&mut self, settings: &T) -> Result<()> {
        self.fields.insert(
            "ahu".into(),
            serde_json::to_value(settings).map_err(|_| Error::new("cannot encode ahu settings"))?,
        );
        Ok(())
    }
    pub fn render(&self) -> Result<String> {
        // serde_json's arbitrary_precision Number uses a private serde struct
        // when sent directly to non-JSON serializers. Convert through JSON text
        // (a YAML subset) so numbers remain scalars, not metadata-shaped maps.
        let json = serde_json::to_string(&self.fields)
            .map_err(|_| Error::new("cannot encode OKF metadata"))?;
        let values: yaml_serde::Value =
            yaml_serde::from_str(&json).map_err(|_| Error::new("cannot encode OKF metadata"))?;
        let yaml = yaml_serde::to_string(&values)
            .map_err(|_| Error::new("cannot encode OKF frontmatter"))?;
        let text = format!("---\n{yaml}---\n{}", self.body);
        if text.len() as u64 > MAX_BYTES {
            return Err(Error::new("OKF document exceeds byte limit"));
        }
        Ok(text)
    }
}

/// Read regular files only, without following the leaf symlink. Callers resolve
/// repository parents with resolve_existing_within before using this helper.
pub fn read(path: &Path) -> Result<Vec<u8>> {
    read_with_limit(path, MAX_BYTES)
}

pub fn read_with_limit(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;
    if limit > MAX_BYTES {
        return Err(Error::new("document read limit exceeds OKF maximum"));
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::new("document must be a regular file"));
    }
    Ok(crate::util::read_limited(&mut file, limit)?)
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

struct BoundedValue<'a> {
    depth: usize,
    nodes: &'a mut usize,
    bytes: &'a mut usize,
}
impl BoundedValue<'_> {
    fn charge<E: serde::de::Error>(&mut self, bytes: usize) -> std::result::Result<(), E> {
        *self.bytes = self.bytes.saturating_add(bytes);
        if *self.bytes > MAX_BYTES as usize {
            return Err(E::custom("YAML byte limit"));
        }
        Ok(())
    }
    fn child(&mut self) -> BoundedValue<'_> {
        BoundedValue {
            depth: self.depth + 1,
            nodes: self.nodes,
            bytes: self.bytes,
        }
    }
}
impl<'de> DeserializeSeed<'de> for BoundedValue<'_> {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(mut self, d: D) -> std::result::Result<Value, D::Error> {
        *self.nodes += 1;
        if self.depth > MAX_DEPTH || *self.nodes > MAX_NODES {
            return Err(serde::de::Error::custom("YAML depth or node limit"));
        }
        self.charge::<D::Error>(1)?;
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for BoundedValue<'_> {
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
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Value, E> {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("nonfinite YAML number"))
    }
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
                return Err(serde::de::Error::custom("duplicate YAML key"));
            }
            let value = map.next_value_seed(self.child())?;
            values.insert(key.0, value);
        }
        Ok(Value::Object(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_metadata_round_trips_without_changing_body_or_granting_settings() {
        let text = "---\nokf_version: 0.2\ntype: ahu:project\nahu: {schema_version: 1}\nnotes:\n  nested: [one, {two: [true, null, 42, 0.5]}]\nexecution: {command: inert}\n---\n# Body\nRun nothing.\n";
        let doc = Document::parse(text, "ahu:project").unwrap();
        assert_eq!(doc.fields["notes"]["nested"][1]["two"][2], 42);
        let rendered = doc.render().unwrap();
        assert_eq!(Document::parse(&rendered, "ahu:project").unwrap(), doc);
        assert_eq!(doc.body, "# Body\nRun nothing.\n");
    }
    #[test]
    fn rejects_duplicates_at_every_depth_and_malformed_versions() {
        for input in [
            "notes: {x: 1, x: 2}",
            "notes: [{a: {b: 1, b: 2}}]",
            "ahu: {}",
            "notes: !custom x",
            "notes: {.nan: value}",
            "notes: .inf",
        ] {
            let text =
                format!("---\nokf_version: '0.2'\ntype: ahu:project\nahu: {{}}\n{input}\n---\n");
            assert!(Document::parse(&text, "ahu:project").is_err(), "{input}");
        }
        for text in [
            "",
            "---\n",
            "---\nokf_version: '9'\ntype: ahu:project\nahu: {}\n---\n",
            "---\nokf_version: '0.2'\ntype: wrong\nahu: {}\n---\n",
        ] {
            assert!(Document::parse(text, "ahu:project").is_err());
        }
    }
    #[test]
    fn bounds_bytes_depth_and_alias_expansion() {
        let prefix = "---\nokf_version: '0.2'\ntype: ahu:project\nahu: {}\n";
        let large = format!("{prefix}---\n{}", "x".repeat(MAX_BYTES as usize));
        assert!(Document::parse(&large, "ahu:project").is_err());
        let deep = format!("{prefix}x: {}0{}\n---\n", "[".repeat(64), "]".repeat(64));
        assert!(Document::parse(&deep, "ahu:project").is_err());
        let mut aliases = format!("{prefix}a: &a [{}]\n", vec!["text"; 50].join(","));
        aliases.push_str(&format!("b: &b [{}]\n", vec!["*a"; 50].join(",")));
        aliases.push_str(&format!("c: [{}]\n---\n", vec!["*b"; 50].join(",")));
        assert!(Document::parse(&aliases, "ahu:project").is_err());
    }
}
