//! External plugin inventory and per-process selection.
//! Discovery reads metadata only; executable activation is explicitly separate.
pub(crate) mod local;
pub mod project;

use crate::util::{Error, ErrorKind, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

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
    pub executable: Option<super::external::Executable>,
    pub entrypoint: Option<crate::plugin_exec::Entrypoint>,
    pub activation_blocker: Option<&'static str>,
    pub metadata: BTreeMap<String, Value>,
}

/// Inspect selection intent independently of authority, source verification and readiness.
pub fn inventory() -> Result<Vec<PluginStatus>> {
    let repo = crate::git::discover(&std::env::current_dir()?);
    let selection = super::selection(repo.as_ref().ok())?;
    let mut plugins = Vec::new();
    plugins.push(PluginStatus {
        id: "terminal".into(),
        package: "ahu-core".into(),
        host_api: 0,
        selected: selection.id == "terminal",
        selection_source: "not-selected",
        origin: "built-in",
        manifest: None,
        cargo_manifest: None,
        executable: None,
        entrypoint: None,
        activation_blocker: None,
        metadata: BTreeMap::new(),
    });
    plugins.extend(local::discover()?);
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
            // Missing or invalid installed metadata does not change selection.
            // It also must not turn declared policy into verified authority.
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
                activation_blocker: Some("Installed package metadata unavailable; inspect the binding and installation. Selection does not establish authority or readiness."),
                metadata: BTreeMap::new(),
            });
            if plugin.manifest.is_some() {
                plugin.origin = "installed-manifest";
            }
            plugins.push(plugin);
        }
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));
    if selection.id != "none" && !plugins.iter().any(|p| p.id == selection.id) {
        return Err(Error::new(
            "AHU_INTERACTIVE_PLUGIN must be terminal, none, a committed binding id, or an id discovered through AHU_PLUGIN_PATH. No other backend was selected.",
        ).with_kind(ErrorKind::Usage));
    }
    for plugin in &mut plugins {
        plugin.selected = selection.id != "none" && plugin.id == selection.id;
        plugin.selection_source = selection.source;
    }
    Ok(plugins)
}

/// The selected package, if one exists.
pub fn status() -> Result<PluginStatus> {
    let mut plugins = inventory()?;
    let index = plugins
        .iter()
        .position(|p| p.selected)
        .ok_or_else(|| Error::new("no external session host is selected"))?;
    Ok(plugins.remove(index))
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
            println!(
                "{} ({}, Host V{}): selected={} ({}, {})",
                status.id,
                status.package,
                status.host_api,
                status.selected,
                status.selection_source,
                status.origin
            );
            if let Some(blocker) = status.activation_blocker {
                println!("  {blocker}");
            }
        }
        println!(
            "Selection: AHU_INTERACTIVE_PLUGIN=terminal|none|<binding-or-local-id>; otherwise a single enabled committed binding or the current terminal. `terminal` runs the task in the caller's terminal; `none` disables interactive launch. AHU_PLUGIN_PATH lists absolute local package directories. Inventory does not verify authority or sources, execute plugins, or probe a provider."
        );
    }
    Ok(())
}

/// Explicit activation is separate from read-only package discovery.
pub fn probe(output_json: bool) -> Result<()> {
    let host = super::discover()?;
    let repo = crate::git::discover(&std::env::current_dir()?);
    let id = super::selection(repo.as_ref().ok())?.id;
    let capabilities = host.check_capabilities()?;
    let report = serde_json::json!({
        "schema_version": 1, "plugin_id": id,
        "protocol_version": host.persistence_key().map(|_| 2),
        "capabilities": capabilities,
        "persistent_sessions": host.check_persistence().is_ok(),
    });
    if output_json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!(
            "{}: activated; {} provider-neutral capabilities; durable sessions supported={}",
            id,
            capabilities.len(),
            host.check_persistence().is_ok()
        );
    }
    Ok(())
}
