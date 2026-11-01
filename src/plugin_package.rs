//! Installed plugin packages and committed bindings.
//!
//! Discovery reads package metadata only; executable activation is a separate,
//! explicit step owned by the plugin runtimes.
pub(crate) mod local;
pub mod project;

use crate::util::{Error, ErrorKind, Result};
use ahu_host_api::protocol;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What a package declares about its executable.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Executable {
    pub path: String,
    pub protocol_version: u32,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    5000
}

impl Executable {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.protocol_version != protocol::VERSION || !(100..=30_000).contains(&self.timeout_ms)
        {
            return Err(Error::new(
                "executable requires protocol_version = 2 and timeout_ms between 100 and 30000",
            )
            .with_kind(ErrorKind::Usage));
        }
        Ok(())
    }
}

/// One discovered package, before authority, source verification or readiness.
#[derive(Debug, Serialize)]
pub struct PluginStatus {
    pub id: String,
    pub package: String,
    pub host_api: u32,
    pub selected: bool,
    pub selection_source: &'static str,
    pub origin: &'static str,
    pub manifest: Option<PathBuf>,
    pub cargo_manifest: Option<PathBuf>,
    pub executable: Option<Executable>,
    pub entrypoint: Option<crate::plugin_exec::Entrypoint>,
    pub activation_blocker: Option<&'static str>,
    pub metadata: BTreeMap<String, Value>,
}
