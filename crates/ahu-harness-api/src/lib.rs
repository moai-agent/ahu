//! Data contract only: no provider implementations, execution or ambient discovery.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub mod json;

pub const PROTOCOL_VERSION: u32 = 1;
/// Includes the terminating newline.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Probe,
    Models,
    Launch,
    Admission,
    Setup,
    NativeSources,
    Auth,
    Budget,
    TrustPrepare,
    TrustCleanup,
    DecodeStart,
    DecodeEvent,
    DecodeFinish,
    Recover,
}
impl Operation {
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        serde_json::from_value(Value::String(value.into())).map_err(|_| ValidationError)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Interactive,
    Headless,
    Direct,
}

/// Installed suggestions and compatibility evidence, not account availability or policy.
/// Path entries are provider-owned location descriptions; core must validate any
/// concrete filesystem action separately. Model identifiers are exact suggestions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub id: String,
    pub display_name: String,
    pub program: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub models: Vec<String>,
    pub modes: Vec<Mode>,
    pub operations: Vec<Operation>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub tested_versions: Vec<String>,
    #[serde(default)]
    pub context_paths: Vec<String>,
    #[serde(default)]
    pub skill_paths: Vec<String>,
    #[serde(default)]
    pub native_source_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub protocol_version: u32,
    pub path: String,
    pub timeout_ms: u64,
    pub descriptor: Descriptor,
}

#[derive(Debug, Clone, Copy)]
pub struct ValidationError;
impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid harness runtime contract")
    }
}
impl std::error::Error for ValidationError {}

pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !value.ends_with('-')
}
fn text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control)
}
fn unique<T: Ord>(values: &[T]) -> bool {
    values.len() <= 256 && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}
impl Descriptor {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !identifier(&self.id)
            || !text(&self.display_name)
            || !text(&self.program)
            || self.program.starts_with('-')
            || self.program.contains(['/', '\\', ':'])
            || !self.aliases.iter().all(|s| identifier(s) && s != &self.id)
            || !unique(&self.aliases)
            || self.operations.is_empty()
            || !unique(&self.operations)
            || !unique(&self.modes)
        {
            return Err(ValidationError);
        }
        for values in [
            &self.models,
            &self.capabilities,
            &self.tested_versions,
            &self.context_paths,
            &self.skill_paths,
            &self.native_source_paths,
        ] {
            if !unique(values) || !values.iter().all(|s| text(s)) {
                return Err(ValidationError);
            }
        }
        Ok(())
    }
}
impl Declaration {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.protocol_version != PROTOCOL_VERSION
            || !(100..=30_000).contains(&self.timeout_ms)
            || self.path.len() > 1024
            || self.path.split('/').count() > 32
            || self.path.split('/').any(|part| {
                part.is_empty()
                    || part == "."
                    || part == ".."
                    || part.eq_ignore_ascii_case(".git")
                    || part.contains(['\\', ':'])
                    || part.chars().any(char::is_control)
            })
        {
            return Err(ValidationError);
        }
        self.descriptor.validate()
    }
}

// Payloads deliberately lack Debug: they may contain native events or private paths.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub protocol_version: u32,
    pub id: u64,
    pub operation: Operation,
    pub params: Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Success {
    pub protocol_version: u32,
    pub id: u64,
    pub result: Value,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteError {
    Failed,
    InvalidInput,
    Unsupported,
    OutcomeUnknown,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub protocol_version: u32,
    pub id: u64,
    pub error: RemoteError,
}
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Success(Success),
    Failure(Failure),
}
