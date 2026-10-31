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
    /// Context whose acceptance belongs only in owner-local state.
    #[serde(default)]
    pub private_context_paths: Vec<String>,
    #[serde(default)]
    pub skill_paths: Vec<String>,
    #[serde(default)]
    pub native_source_paths: Vec<String>,
    /// Explicit native configuration routing. Values are transient request inputs.
    #[serde(default)]
    pub environment_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub protocol_version: u32,
    /// Packaged executable. Empty declares a generic harness: core answers the
    /// two operations below itself and no plugin code exists or runs.
    #[serde(default)]
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
/// Native routing keys cannot override core identity, tracing or process loading.
pub fn environment_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && value.as_bytes()[0].is_ascii_uppercase()
        && !value.starts_with("AHU_")
        && !value.starts_with("OTEL_")
        && !value.starts_with("LD_")
        && !value.starts_with("DYLD_")
        && !matches!(
            value,
            "HOME"
                | "PATH"
                | "ENV"
                | "BASH_ENV"
                | "SHELLOPTS"
                | "PYTHONPATH"
                | "PYTHONHOME"
                | "NODE_OPTIONS"
        )
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
            || self.environment_keys.len() > 64
            || !unique(&self.environment_keys)
            || !self.environment_keys.iter().all(|key| environment_key(key))
        {
            return Err(ValidationError);
        }
        if self.private_context_paths.len() > 64
            || self.private_context_paths.iter().any(|path| {
                let plain = path.strip_suffix('/').unwrap_or(path);
                plain.len() > 4096
                    || plain.split('/').any(|part| {
                        part.is_empty()
                            || part == "."
                            || part == ".."
                            || part.eq_ignore_ascii_case(".git")
                            || part.contains(['\\', ':'])
                            || part.chars().any(char::is_control)
                    })
                    || !self.context_paths.iter().any(|context| {
                        context == path || (context.ends_with('/') && path.starts_with(context))
                    })
            })
        {
            return Err(ValidationError);
        }
        for values in [
            &self.models,
            &self.capabilities,
            &self.tested_versions,
            &self.context_paths,
            &self.private_context_paths,
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
    /// A harness without a plugin, integrated only over MCP. Core supports a
    /// direct interactive session and a version probe for it, nothing else.
    pub fn is_generic(&self) -> bool {
        self.path.is_empty()
    }
    fn generic_shape(&self) -> bool {
        self.descriptor.modes == [Mode::Direct]
            && self
                .descriptor
                .operations
                .iter()
                .all(|op| matches!(op, Operation::Probe | Operation::Launch))
            && self.descriptor.capabilities == ["mcp-only"]
    }
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.is_generic() {
            if self.protocol_version != PROTOCOL_VERSION
                || !(100..=30_000).contains(&self.timeout_ms)
                || !self.generic_shape()
            {
                return Err(ValidationError);
            }
            return self.descriptor.validate();
        }
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
    /// The native CLI is waiting for its own folder-trust decision.
    NeedsNativeTrust,
    /// The native CLI reports no signed-in account.
    NotSignedIn,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn generic_declaration() -> Declaration {
        serde_json::from_value(serde_json::json!({
            "protocol_version": 1,
            "timeout_ms": 5000,
            "descriptor": {
                "id": "sixth", "display_name": "Sixth", "program": "sixth",
                "modes": ["direct"], "operations": ["probe", "launch"],
                "capabilities": ["mcp-only"]
            }
        }))
        .unwrap()
    }

    #[test]
    fn a_generic_declaration_has_no_executable_and_a_narrow_shape() {
        let declaration = generic_declaration();
        assert!(declaration.is_generic());
        assert!(declaration.validate().is_ok());
        for mutate in [
            |d: &mut Declaration| d.descriptor.modes = vec![Mode::Headless],
            |d: &mut Declaration| d.descriptor.operations.push(Operation::Auth),
            |d: &mut Declaration| d.descriptor.capabilities.clear(),
        ] {
            let mut invalid = generic_declaration();
            mutate(&mut invalid);
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn private_paths_are_bounded_relative_and_inside_declared_context() {
        let mut descriptor:Descriptor=serde_json::from_value(serde_json::json!({"id":"invented","display_name":"Invented","program":"invented","modes":["direct"],"operations":["native_sources"],"context_paths":[".invented/"]})).unwrap();
        descriptor.private_context_paths =
            vec![".invented/local.json".into(), ".invented/private/".into()];
        descriptor.validate().unwrap();
        for path in [
            "../escape",
            "/absolute",
            ".invented/../escape",
            ".git/config",
            ".other/local",
            ".invented//local",
        ] {
            descriptor.private_context_paths = vec![path.into()];
            assert!(descriptor.validate().is_err(), "{path}");
        }
        descriptor.private_context_paths = (0..65).map(|i| format!(".invented/{i}")).collect();
        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn routing_keys_are_explicit_bounded_and_cannot_override_host_identity() {
        let mut descriptor: Descriptor = serde_json::from_value(serde_json::json!({
            "id":"invented", "display_name":"Invented", "program":"invented-native",
            "modes":["direct"], "operations":["probe"]
        }))
        .unwrap();
        descriptor.environment_keys = vec!["INVENTED_CONFIG_DIR".into(), "XDG_CONFIG_HOME".into()];
        descriptor.validate().unwrap();
        for key in [
            "HOME",
            "PATH",
            "AHU_TASK_ID",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "BASH_ENV",
            "bad-key",
            "",
            "1BAD",
        ] {
            descriptor.environment_keys = vec![key.into()];
            assert!(descriptor.validate().is_err(), "{key}");
        }
        descriptor.environment_keys = vec!["REPEAT".into(), "REPEAT".into()];
        assert!(descriptor.validate().is_err());
        descriptor.environment_keys = (0..65).map(|n| format!("CONFIG_{n}")).collect();
        assert!(descriptor.validate().is_err());
    }
}
