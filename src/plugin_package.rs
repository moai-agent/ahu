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

/// Inspect discovered local packages and committed bindings without verifying
/// authority or sources and without executing anything.
pub fn inventory() -> Result<Vec<PluginStatus>> {
    let repo = crate::git::discover(&std::env::current_dir()?);
    let mut plugins = local::discover()?;
    if let Ok(repo) = &repo {
        for status in project::inventory(&repo.root)?.bindings {
            let binding = status.binding;
            // A discovery path takes precedence, just as it does at activation.
            if plugins.iter().any(|p| p.id == binding.id) {
                continue;
            }
            let metadata = (|| -> Result<PluginStatus> {
                let store = crate::state::checkout_root(&repo.primary_root()?)?.join("plugins");
                let installation = crate::plugin_install::installed(&store, &binding)?;
                crate::state::confine_existing_dir(&installation)?;
                let metadata = local::read_package(&installation.join("package"))?;
                if metadata.id != binding.id {
                    return Err(Error::new("installed package metadata identity mismatch"));
                }
                Ok(metadata)
            })();
            // Missing or invalid installed metadata is reported, never promoted
            // to verified authority.
            let mut plugin = metadata.unwrap_or_else(|_| PluginStatus {
                id: binding.id,
                package: "unknown".into(),
                host_api: binding.compatibility.host_api,
                selected: false,
                selection_source: "not-selected",
                origin: "committed-binding",
                manifest: None,
                cargo_manifest: None,
                executable: None,
                entrypoint: None,
                activation_blocker: Some(
                    "Installed package metadata unavailable; inspect the binding and installation.",
                ),
                metadata: BTreeMap::new(),
            });
            if plugin.manifest.is_some() {
                plugin.origin = "installed-manifest";
            }
            plugins.push(plugin);
        }
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(plugins)
}

pub fn list(output_json: bool) -> Result<()> {
    let plugins = inventory()?;
    if output_json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({"schema_version":1,"plugins":plugins}))?
        );
    } else {
        for status in plugins {
            println!("{} ({}, {})", status.id, status.package, status.origin);
            if let Some(blocker) = status.activation_blocker {
                println!("  {blocker}");
            }
        }
        println!(
            "AHU_PLUGIN_PATH lists absolute local package directories. Inventory does not verify authority or sources, execute plugins, or probe a harness."
        );
    }
    Ok(())
}
