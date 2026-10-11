//! A deliberately bounded Draft 2020-12 vocabulary. Unknown keywords fail closed.
//! No references, regexes, format assertions, network retrieval, or implicit coercion.
use crate::util::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub const MAX_BYTES: usize = 64 * 1024;
pub(crate) const MAX_DEPTH: usize = 32;
pub(crate) const MAX_NODES: usize = 4096;
const MAX_STEPS: usize = 100_000;
const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
}

impl Contract {
    pub fn is_empty(&self) -> bool {
        self.input.is_none() && self.output.is_none()
    }

    pub fn validate_input(&self, raw: Option<&str>) -> Result<()> {
        match (&self.input, raw) {
            (None, None) => Ok(()),
            (None, Some(_)) => Err(Error::new(
                "structured input supplied but agent has no input_schema",
            )),
            (Some(_), None) => Err(Error::new(
                "input_schema requires --input-file with a JSON artifact",
            )),
            (Some(schema), Some(raw)) => parse(raw.as_bytes())
                .and_then(|value| validate(schema, &value))
                .map_err(|e| Error::new(format!("structured input: {e}"))),
        }
    }

    pub fn prompt(&self, prompt: &str, raw: Option<&str>) -> String {
        if self.is_empty() {
            return prompt.into();
        }
        let mut text = prompt.to_string();
        if let Some(raw) = raw {
            text.push_str("\n\nStructured input (JSON task data):\n");
            text.push_str(raw);
        }
        if let Some(schema) = &self.output {
            text.push_str("\n\nWrite one JSON result to the file named by AHU_STRUCTURED_RESULT before finishing. The file must be a regular file of at most 65536 bytes, with no links. Its Draft 2020-12 schema is:\n");
            text.push_str(&schema.to_string());
        }
        text
    }

    /// Absence is compatible with old prompt-only attempts; no file is read.
    pub fn validate_result(&self, path: &Path) -> Result<Option<Value>> {
        if let Some(schema) = &self.output {
            let value =
                read_artifact(path).map_err(|e| Error::new(format!("structured result: {e}")))?;
            validate(schema, &value).map_err(|e| Error::new(format!("structured result: {e}")))?;
            return Ok(Some(value));
        }
        Ok(None)
    }
}

/// Descriptor checks precede bounded reading; errors never echo artifact bytes or paths.
pub fn read_artifact(path: &Path) -> Result<Value> {
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let read = || -> Result<Vec<u8>> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(parent)
            .map_err(|_| Error::new("artifact directory is missing, linked, or unreadable"))?;
        let name = std::ffi::CString::new(path.file_name().unwrap_or_default().as_encoded_bytes())
            .map_err(|_| Error::new("invalid artifact filename"))?;
        // SAFETY: the directory descriptor is pinned and name is NUL terminated.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(Error::new("artifact is missing or unreadable"));
        }
        // SAFETY: openat returned an owned descriptor.
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        let meta = file
            .metadata()
            .map_err(|_| Error::new("cannot inspect artifact"))?;
        if !meta.is_file() || meta.nlink() != 1 {
            return Err(Error::new("artifact must be a regular file with one link"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::new("cannot read artifact"))?;
        Ok(bytes)
    };
    parse(&read()?)
}

pub fn parse(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > MAX_BYTES {
        return Err(Error::new("JSON artifact exceeds 65536 bytes"));
    }
    // Bound recursion with serde_json's shared parser before recursively checking raw
    // subvalues. Each raw-value parser otherwise starts a fresh recursion budget.
    let preliminary: Value = serde_json::from_slice(bytes)
        .map_err(|_| Error::new("artifact must contain one valid JSON value"))?;
    bounded(&preliminary, 0, &mut 0)?;
    // Reject duplicate object keys rather than silently selecting the final constraint/value.
    let value = serde_json::from_slice::<UniqueValue>(bytes)
        .map_err(|_| {
            Error::new("artifact must contain one valid JSON value without duplicate keys")
        })?
        .0;
    bounded(&value, 0, &mut 0)?;
    reject_private_marker_keys(&value)?;
    Ok(value)
}

struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON value")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(v) = a.next_element::<UniqueValue>()? {
                    values.push(v.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, value)) = a.next_entry::<String, UniqueValue>()? {
                    if values.insert(key, value.0).is_some() {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        // Raw scalar parsing preserves decimal precision and avoids serde_json's
        // private arbitrary-precision number map being confused with user objects.
        let raw = Box::<serde_json::value::RawValue>::deserialize(d)?;
        let mut parser = serde_json::Deserializer::from_str(raw.get());
        match raw.get().as_bytes().first() {
            Some(b'{') => serde::Deserializer::deserialize_map(&mut parser, Visitor),
            Some(b'[') => serde::Deserializer::deserialize_seq(&mut parser, Visitor),
            // Scalars cannot contain marker keys, so deserialize them directly
            // as Value. Recursing through UniqueValue here turns serde_json's
            // arbitrary-precision scalar representation into its private map
            // marker instead of preserving it as a Number.
            _ => serde_json::from_str::<Value>(raw.get())
                .map(UniqueValue)
                .map_err(serde::de::Error::custom),
        }
        .map_err(serde::de::Error::custom)
    }
}

fn bounded(v: &Value, depth: usize, nodes: &mut usize) -> Result<()> {
    *nodes += 1;
    if depth > MAX_DEPTH || *nodes > MAX_NODES {
        return Err(Error::new("JSON exceeds depth 32 or node limit 4096"));
    }
    match v {
        Value::Object(m) => {
            for v in m.values() {
                bounded(v, depth + 1, nodes)?;
            }
        }
        Value::Array(a) => {
            for v in a {
                bounded(v, depth + 1, nodes)?;
            }
        }
        Value::Number(n) if number(n).is_none() => {
            return Err(Error::new("JSON number exponent exceeds supported range"));
        }
        _ => {}
    }
    Ok(())
}

fn schema_error(keyword: &str) -> Error {
    // keyword is always an implementation constant, never attacker-supplied text.
    Error::new(format!("invalid contract schema: check {keyword}"))
}

/// serde_json reserves these object keys for its own number and raw-value
/// encodings. With those features enabled, an ordinary `Value` deserialize can
/// reinterpret a user object carrying one of the keys as a different JSON value.
/// Reject them before a contract or artifact crosses a serialize/deserialize
/// boundary so validation and later result reads see the same data.
fn reject_private_marker_keys(value: &Value) -> Result<()> {
    match value {
        Value::Object(object) => {
            if object.keys().any(|key| {
                matches!(
                    key.as_str(),
                    "$serde_json::private::Number" | "$serde_json::private::RawValue"
                )
            }) {
                return Err(Error::new(
                    "JSON contains a reserved serde_json marker property",
                ));
            }
            for nested in object.values() {
                reject_private_marker_keys(nested)?;
            }
        }
        Value::Array(values) => {
            for nested in values {
                reject_private_marker_keys(nested)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn check_schema(schema: &Value) -> Result<()> {
    bounded(schema, 0, &mut 0)?;
    reject_private_marker_keys(schema)?;
    if serde_json::to_vec(schema)?.len() > MAX_BYTES {
        return Err(schema_error("65536 byte limit"));
    }
    fn walk(s: &Value, steps: &mut usize) -> Result<()> {
        tick(steps)?;
        if s.is_boolean() {
            return Ok(());
        }
        let m = s
            .as_object()
            .ok_or_else(|| schema_error("schema object or boolean"))?;
        for (key, v) in m {
            match key.as_str() {
                "$schema" if v.as_str() == Some(DIALECT) => {}
                "title" | "description" if v.is_string() => {}
                "default" | "const" => {}
                "examples" if v.is_array() => {}
                "type" => {
                    let valid = |v: &Value| {
                        matches!(
                            v.as_str(),
                            Some(
                                "null"
                                    | "boolean"
                                    | "object"
                                    | "array"
                                    | "number"
                                    | "integer"
                                    | "string"
                            )
                        )
                    };
                    if !valid(v)
                        && !v.as_array().is_some_and(|a| {
                            !a.is_empty()
                                && a.iter().all(valid)
                                && (0..a.len()).all(|i| !a[..i].contains(&a[i]))
                        })
                    {
                        return Err(schema_error("type"));
                    }
                }
                "properties" => {
                    for s in v
                        .as_object()
                        .ok_or_else(|| schema_error("properties"))?
                        .values()
                    {
                        walk(s, steps)?;
                    }
                }
                "required" => {
                    let a = v.as_array().ok_or_else(|| schema_error("required"))?;
                    if !a.iter().all(Value::is_string)
                        || a.iter()
                            .filter_map(Value::as_str)
                            .collect::<std::collections::BTreeSet<_>>()
                            .len()
                            != a.len()
                    {
                        return Err(schema_error("required"));
                    }
                }
                "additionalProperties" | "items" | "not" => walk(v, steps)?,
                "prefixItems" | "allOf" | "anyOf" | "oneOf" => {
                    let a = v
                        .as_array()
                        .filter(|a| !a.is_empty())
                        .ok_or_else(|| schema_error("nonempty schema array"))?;
                    for s in a {
                        walk(s, steps)?;
                    }
                }
                "enum" => {
                    let a = v
                        .as_array()
                        .filter(|a| !a.is_empty())
                        .ok_or_else(|| schema_error("enum"))?;
                    for (i, v) in a.iter().enumerate() {
                        for other in &a[..i] {
                            if equal(v, other, steps)? {
                                return Err(schema_error("unique enum values"));
                            }
                        }
                    }
                }
                "minLength" | "maxLength" | "minItems" | "maxItems" | "minProperties"
                | "maxProperties"
                    if v.as_u64().is_some() => {}
                "uniqueItems" if v.is_boolean() => {}
                // Numeric constraints require exact arithmetic; reject rather than round.
                _ => {
                    return Err(schema_error(
                        "unsupported keyword, dialect, or keyword value; see structured contract vocabulary",
                    ));
                }
            }
        }
        Ok(())
    }
    let mut steps = MAX_STEPS;
    walk(schema, &mut steps)
}

// Exact decimal normalization: JSON Schema compares mathematical numbers,
// including integers written with a decimal point or exponent. Never round to f64.
fn number(n: &serde_json::Number) -> Option<(bool, String, i64)> {
    let text = n.to_string();
    let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((&text, "0"));
    let mut exponent = exponent.parse::<i64>().ok()?;
    if exponent.unsigned_abs() > 1_000_000 {
        return None;
    }
    let negative = mantissa.starts_with('-');
    let mantissa = mantissa.trim_start_matches('-');
    let decimals = mantissa.split_once('.').map_or(0, |(_, tail)| tail.len());
    exponent -= decimals as i64;
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some((false, "0".into(), 0));
    }
    let trimmed = digits.trim_end_matches('0');
    exponent += (digits.len() - trimmed.len()) as i64;
    Some((negative, trimmed.into(), exponent))
}

fn equal(a: &Value, b: &Value, steps: &mut usize) -> Result<bool> {
    tick(steps)?;
    Ok(match (a, b) {
        (Value::Number(a), Value::Number(b)) => number(a) == number(b),
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            for (a, b) in a.iter().zip(b) {
                if !equal(a, b, steps)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Object(a), Value::Object(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            for (k, a) in a {
                let Some(b) = b.get(k) else {
                    return Ok(false);
                };
                if !equal(a, b, steps)? {
                    return Ok(false);
                }
            }
            true
        }
        _ => a == b,
    })
}

pub fn validate(schema: &Value, value: &Value) -> Result<()> {
    check_schema(schema)?;
    bounded(value, 0, &mut 0)?;
    let mut steps = MAX_STEPS;
    let mut reason = "schema";
    if !matches_schema(schema, value, &mut steps, &mut reason)? {
        return Err(Error::new(format!(
            "JSON does not satisfy schema keyword {reason}; check the declared contract and artifact"
        )));
    }
    Ok(())
}

fn tick(steps: &mut usize) -> Result<()> {
    *steps = steps
        .checked_sub(1)
        .ok_or_else(|| Error::new("structured validation exceeds 100000 evaluation steps"))?;
    Ok(())
}
fn reject(reason: &mut &'static str, keyword: &'static str) -> Result<bool> {
    *reason = keyword;
    Ok(false)
}
fn matches_schema(
    s: &Value,
    v: &Value,
    steps: &mut usize,
    reason: &mut &'static str,
) -> Result<bool> {
    tick(steps)?;
    if let Some(b) = s.as_bool() {
        return if b {
            Ok(true)
        } else {
            reject(reason, "false schema")
        };
    }
    let m = s.as_object().expect("checked schema");
    if let Some(t) = m.get("type") {
        let is = |t: &Value| match t.as_str().unwrap_or_default() {
            "null" => v.is_null(),
            "boolean" => v.is_boolean(),
            "string" => v.is_string(),
            "object" => v.is_object(),
            "array" => v.is_array(),
            "number" => v.is_number(),
            "integer" => v.as_number().is_some_and(|n| {
                number(n).is_some_and(|(_, digits, exponent)| digits == "0" || exponent >= 0)
            }),
            _ => false,
        };
        if !t.as_array().map_or_else(|| is(t), |a| a.iter().any(is)) {
            return reject(reason, "type");
        }
    }
    if let Some(c) = m.get("const")
        && !equal(c, v, steps)?
    {
        return reject(reason, "const");
    }
    if let Some(a) = m.get("enum").and_then(Value::as_array) {
        let mut found = false;
        for c in a {
            if equal(c, v, steps)? {
                found = true;
                break;
            }
        }
        if !found {
            return reject(reason, "enum");
        }
    }
    for (keyword, mode) in [("allOf", 0), ("anyOf", 1), ("oneOf", 2)] {
        if let Some(a) = m.get(keyword).and_then(Value::as_array) {
            let mut count = 0;
            for s in a {
                count += usize::from(matches_schema(s, v, steps, reason)?);
            }
            if match mode {
                0 => count != a.len(),
                1 => count == 0,
                _ => count != 1,
            } {
                return reject(reason, keyword);
            }
        }
    }
    if let Some(s) = m.get("not")
        && matches_schema(s, v, steps, reason)?
    {
        return reject(reason, "not");
    }
    let range = |len: usize, min: &str, max: &str| {
        m.get(min)
            .is_none_or(|n| (len as u64) >= n.as_u64().unwrap())
            && m.get(max)
                .is_none_or(|n| (len as u64) <= n.as_u64().unwrap())
    };
    if let Some(text) = v.as_str()
        && !range(text.chars().count(), "minLength", "maxLength")
    {
        return reject(reason, "minLength/maxLength");
    }
    if let Some(obj) = v.as_object() {
        if !range(obj.len(), "minProperties", "maxProperties") {
            return reject(reason, "minProperties/maxProperties");
        }
        if let Some(a) = m.get("required").and_then(Value::as_array)
            && a.iter().any(|k| !obj.contains_key(k.as_str().unwrap()))
        {
            return reject(reason, "required");
        }
        for (k, v) in obj {
            tick(steps)?;
            if let Some(s) = m
                .get("properties")
                .and_then(|p| p.get(k))
                .or_else(|| m.get("additionalProperties"))
                && !matches_schema(s, v, steps, reason)?
            {
                return Ok(false);
            }
        }
    }
    if let Some(a) = v.as_array() {
        if !range(a.len(), "minItems", "maxItems") {
            return reject(reason, "minItems/maxItems");
        }
        if m.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
            for (i, v) in a.iter().enumerate() {
                for other in &a[..i] {
                    tick(steps)?;
                    if equal(v, other, steps)? {
                        return reject(reason, "uniqueItems");
                    }
                }
            }
        }
        for (i, v) in a.iter().enumerate() {
            if let Some(s) = m
                .get("prefixItems")
                .and_then(|p| p.get(i))
                .or_else(|| m.get("items"))
                && !matches_schema(s, v, steps, reason)?
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contract() -> Contract {
        Contract {
            input: Some(
                json!({"$schema":DIALECT,"type":"object","required":["files"],"properties":{"files":{"type":"array","items":{"type":"string","minLength":1},"minItems":1}},"additionalProperties":false}),
            ),
            output: Some(
                json!({"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"}},"additionalProperties":false}),
            ),
        }
    }

    #[test]
    fn structured_valid_contract_input_and_result() {
        let c = contract();
        c.validate_input(Some(r#"{"files":["src/lib.rs"]}"#))
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let result = temp.path().join("result.json");
        std::fs::write(&result, r#"{"ok":true}"#).unwrap();
        c.validate_result(&result).unwrap();
        assert!(
            c.prompt("review", Some("{}"))
                .contains("AHU_STRUCTURED_RESULT")
        );
    }

    #[test]
    fn structured_invalid_input_and_result_are_refused() {
        let c = contract();
        for raw in [
            None,
            Some("{}"),
            Some(r#"{"files":[]}"#),
            Some(r#"{"files":[""]}"#),
            Some(r#"{"files":["ok"],"extra":true}"#),
            Some("null"),
            Some("{"),
            Some("{} {}"),
            Some(r#"{"files":[],"files":["ok"]}"#),
        ] {
            assert!(c.validate_input(raw).is_err(), "{raw:?}");
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("result.json");
        assert!(c.validate_result(&path).is_err());
        for raw in [
            "{}",
            r#"{"ok":"yes"}"#,
            "null",
            "{",
            r#"{"ok":true,"ok":false}"#,
        ] {
            std::fs::write(&path, raw).unwrap();
            assert!(c.validate_result(&path).is_err(), "{raw}");
        }
    }

    #[test]
    fn structured_legacy_contract_neither_changes_prompt_nor_requires_artifacts() {
        let c: Contract = serde_json::from_str("{}").unwrap();
        assert!(c.is_empty());
        c.validate_input(None).unwrap();
        c.validate_result(Path::new("/absent/result.json")).unwrap();
        assert_eq!(c.prompt("exact\nbytes", None), "exact\nbytes");
        assert!(c.validate_input(Some("null")).is_err());
        let nullable = Contract {
            input: Some(json!({"type":"null"})),
            ..Default::default()
        };
        nullable.validate_input(Some("null")).unwrap();
        assert!(nullable.validate_input(None).is_err());
    }

    #[test]
    fn structured_schemas_fail_closed_without_fetching_references() {
        for s in [
            json!(null),
            json!([]),
            json!({"type":"secret /private/path"}),
            json!({"type":[]}),
            json!({"type":["string","string"]}),
            json!({"required":["a","a"]}),
            json!({"required":[1]}),
            json!({"properties":{"a":null}}),
            json!({"items":[]}),
            json!({"allOf":[]}),
            json!({"enum":[]}),
            json!({"enum":[1,1.0]}),
            json!({"minItems":-1}),
            json!({"uniqueItems":"yes"}),
            json!({"$schema":"http://json-schema.org/draft-07/schema#"}),
            json!({"$ref":"https://example.invalid/private"}),
            json!({"$ref":"#/$defs/a"}),
            json!({"$dynamicRef":"#a"}),
            json!({"unevaluatedProperties":false}),
            json!({"format":"email"}),
            json!({"pattern":".*"}),
            json!({"minimum":0}),
            json!({"properties":{"nested":{"unknown":true}}}),
        ] {
            let error = check_schema(&s).unwrap_err().to_string();
            assert!(!error.contains("secret"));
            assert!(!error.contains("/private"));
        }
        check_schema(&json!(true)).unwrap();
        check_schema(&json!(false)).unwrap();
    }

    #[test]
    fn structured_reserved_serde_marker_keys_are_rejected_before_round_trip() {
        let raw_value = br#"{"$serde_json::private::RawValue":"{\"approved\":true}"}"#;
        let raw_number = br#"{"nested":{"$serde_json::private::Number":"123"}}"#;
        for artifact in [raw_value.as_slice(), raw_number.as_slice()] {
            assert!(parse(artifact).is_err());
        }
        for schema in [
            json!({"const":{"$serde_json::private::Number":"123"}}),
            json!({"properties":{"nested":{"enum":[{"$serde_json::private::RawValue":"{}"}]}}}),
        ] {
            assert!(check_schema(&schema).is_err());
        }

        let schema = json!({"const":{"approved":"yes"}});
        let contract = Contract {
            output: Some(schema.clone()),
            ..Default::default()
        };
        let persisted = serde_json::to_vec(&contract).unwrap();
        let restored: Contract = serde_json::from_slice(&persisted).unwrap();
        assert_eq!(restored.output, Some(schema));
    }

    #[test]
    fn structured_draft202012_tuple_and_applicator_semantics() {
        let cases = [
            (
                json!({"prefixItems":[{"type":"integer"}],"items":false}),
                json!([1]),
                true,
            ),
            (
                json!({"prefixItems":[true],"items":false}),
                json!([1, 2]),
                false,
            ),
            (json!({"items":{"type":"string"}}), json!(["a", "b"]), true),
            (
                json!({"allOf":[{"type":"string"},{"minLength":2}]}),
                json!("a"),
                false,
            ),
            (
                json!({"anyOf":[{"type":"string"},{"type":"boolean"}]}),
                json!(false),
                true,
            ),
            (json!({"oneOf":[true,{"type":"integer"}]}), json!(1), false),
            (json!({"not":{"type":"string"}}), json!(false), true),
            (json!({"required":["a"]}), json!(1), true),
            (json!({"type":["null","string"]}), json!(null), true),
            (
                json!({"properties":{"a":true},"additionalProperties":{"type":"boolean"}}),
                json!({"a":1,"b":false}),
                true,
            ),
            (json!({"minLength":1,"maxLength":1}), json!("🦀"), true),
            (
                json!({"minProperties":1,"maxProperties":1}),
                json!({"a":1,"b":2}),
                false,
            ),
            (json!({"minItems":1,"maxItems":1}), json!([]), false),
            (
                json!({"uniqueItems":true}),
                json!([{"a":1},{"a":1.0}]),
                false,
            ),
            (json!({"const":{"a":1}}), json!({"a":1.0}), true),
            (json!({"enum":[false,"ok"]}), json!("ok"), true),
            (json!(false), json!(1), false),
        ];
        for (schema, value, valid) in cases {
            assert_eq!(
                validate(&schema, &value).is_ok(),
                valid,
                "{schema} / {value}"
            );
        }
    }

    #[test]
    fn structured_numbers_never_round_into_acceptance() {
        let schema = parse(br#"{"const":9007199254740993}"#).unwrap();
        assert!(validate(&schema, &parse(b"9007199254740992").unwrap()).is_err());
        assert!(validate(&schema, &parse(b"9007199254740993.0").unwrap()).is_ok());
        let integer = json!({"type":"integer"});
        for raw in ["1.0", "1e3", "-0.0", "184467440737095516160000"] {
            validate(&integer, &parse(raw.as_bytes()).unwrap()).unwrap();
        }
        for raw in ["1.0000000000000000001", "1e-100", "9007199254740993.1"] {
            assert!(validate(&integer, &parse(raw.as_bytes()).unwrap()).is_err());
        }
        assert!(parse(b"1e1000001").is_err());
        assert!(parse(br#"{"$serde_json::private::Number":"123"}"#).is_err());
    }

    #[test]
    fn structured_bounds_cover_bytes_depth_nodes_and_evaluation_work() {
        let exact = format!("\"{}\"", "a".repeat(MAX_BYTES - 2));
        assert!(parse(exact.as_bytes()).is_ok());
        assert!(parse(format!("{exact} ").as_bytes()).is_err());
        assert!(parse(format!("{}0{}", "[".repeat(33), "]".repeat(33)).as_bytes()).is_err());
        assert!(parse(format!("[{}]", vec!["0"; 4096].join(",")).as_bytes()).is_err());
        assert!(parse(format!("{}0{}", "[".repeat(10000), "]".repeat(10000)).as_bytes()).is_err());
        let many = Value::Array((0..1000).map(|n| json!(n)).collect());
        assert!(
            validate(&json!({"uniqueItems":true}), &many)
                .unwrap_err()
                .to_string()
                .contains("evaluation steps")
        );
    }

    #[test]
    fn structured_artifact_links_devices_and_oversize_are_refused_without_content_leaks() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("secret-capability.json");
        for raw in [
            vec![b'a'; MAX_BYTES + 1],
            b"private-capability".to_vec(),
            vec![0xff],
        ] {
            std::fs::write(&path, raw).unwrap();
            let error = read_artifact(&path).unwrap_err().to_string();
            assert!(!error.contains("capability"));
            assert!(!error.contains(&temp.path().display().to_string()));
        }
        std::fs::write(&path, b"null").unwrap();
        let alias = temp.path().join("alias");
        symlink(&path, &alias).unwrap();
        assert!(read_artifact(&alias).is_err());
        std::fs::remove_file(&alias).unwrap();
        std::fs::hard_link(&path, &alias).unwrap();
        assert!(read_artifact(&alias).is_err());
        assert!(read_artifact(temp.path()).is_err());
        assert!(read_artifact(Path::new("/dev/null")).is_err());
        let fifo = temp.path().join("fifo");
        let cpath = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        assert!(read_artifact(&fifo).is_err());
    }
}
