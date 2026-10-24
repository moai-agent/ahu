//! Executable host protocol V2. Frames are a big-endian u32 byte length followed
//! by exactly that many UTF-8 JSON bytes. There are no notifications or batches.
use serde::de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;

pub const VERSION: u32 = 2;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const METHODS: &[&str] = &[
    "create_task_session",
    "verify_task_session",
    "select_task_session",
    "close_task_session",
    "set_task_status",
    "expand_task_session",
    "place_coordinator",
    "set_agent_metadata",
    "open_markdown",
];

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

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub protocol_version: u32,
    pub id: u64,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Success {
    pub protocol_version: u32,
    pub id: u64,
    pub result: Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub protocol_version: u32,
    pub id: u64,
    pub error: RemoteError,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Failed,
    NotCreated,
    OutcomeUnknown,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Success(Success),
    Failure(Failure),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub protocol_version: u32,
    pub host_api: u32,
    pub plugin_id: String,
    pub capabilities: Vec<String>,
    pub inherited_workspace: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::from_slice;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Envelope {
        result: serde_json::Value,
    }

    #[test]
    fn strict_frame_parser_rejects_duplicate_keys_at_every_depth() {
        for raw in [
            br#"{"result":{"complete":false,"complete":true}}"#.as_slice(),
            br#"{"result":{"workspaces":{"task":1,"task":2}}}"#.as_slice(),
            br#"{"result":[{"outer":{"id":"a","id":"b"}}]}"#.as_slice(),
        ] {
            assert!(from_slice::<Envelope>(raw).is_err());
        }
        let parsed: Envelope = from_slice(br#"{"result":{"complete":true}}"#).unwrap();
        assert_eq!(parsed.result["complete"], true);
    }
}
