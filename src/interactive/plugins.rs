//! Compiled and local plugin inventory and per-process selection.
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
    pub compiled: bool,
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
    let compiled = cfg!(feature = "cmux-plugin");
    let mut plugins = vec![PluginStatus {
        id: "cmux".into(),
        package: "ahu-cmux-plugin".into(),
        host_api: 1,
        compiled,
        selected: compiled,
        selection_source: "build-default",
        origin: "compiled",
        manifest: None,
        cargo_manifest: None,
        executable: None,
        entrypoint: None,
        activation_blocker: if compiled {
            None
        } else {
            Some("Rebuild ahu with --features cmux-plugin for cmux operations.")
        },
        metadata: BTreeMap::new(),
    }];
    if selection.id == "terminal" {
        plugins.push(PluginStatus {
            id: "terminal".into(),
            package: "ahu-core".into(),
            host_api: 0,
            compiled: true,
            selected: false,
            selection_source: "not-selected",
            origin: "built-in",
            manifest: None,
            cargo_manifest: None,
            executable: None,
            entrypoint: None,
            activation_blocker: None,
            metadata: BTreeMap::new(),
        });
    }
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
                compiled: false,
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
    plugins[1..].sort_by(|a, b| a.id.cmp(&b.id));
    if selection.id != "none" && !plugins.iter().any(|p| p.id == selection.id) {
        return Err(Error::new(
            "AHU_INTERACTIVE_PLUGIN must be cmux, terminal, or none, a committed binding id, or an id discovered through AHU_PLUGIN_PATH. No other backend was selected.",
        ).with_kind(ErrorKind::Usage));
    }
    for plugin in &mut plugins {
        plugin.selected = selection.id != "none"
            && plugin.id == selection.id
            && (selection.source != "build-default" || plugin.compiled);
        plugin.selection_source = selection.source;
    }
    Ok(plugins)
}

/// The selected package, or the unselected compiled entry when hosting is off.
pub fn status() -> Result<PluginStatus> {
    let mut plugins = inventory()?;
    let index = plugins.iter().position(|p| p.selected).unwrap_or(0);
    Ok(plugins.remove(index))
}

pub fn require_cmux() -> Result<()> {
    let status = status()?;
    if status.id != "cmux" {
        return Err(Error::new("The selected local package is not the compiled cmux host. Use AHU_INTERACTIVE_PLUGIN=cmux for this operation; no backend was substituted.").with_kind(ErrorKind::Prerequisite));
    }
    if !status.compiled {
        return Err(Error::new(
            "cmux session plugin is not included in this build. Rebuild ahu with --features cmux-plugin for cmux operations. Headless tasks and current-terminal coordinators remain available; no backend was substituted.",
        ).with_kind(ErrorKind::Prerequisite));
    }
    if !status.selected {
        return Err(Error::new(
            "cmux session plugin is disabled by AHU_INTERACTIVE_PLUGIN=none. Set AHU_INTERACTIVE_PLUGIN=cmux for this operation. No backend was substituted.",
        ).with_kind(ErrorKind::Prerequisite));
    }
    Ok(())
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
                "{} ({}, Host V{}): compiled={}, selected={} ({}, {})",
                status.id,
                status.package,
                status.host_api,
                status.compiled,
                status.selected,
                status.selection_source,
                status.origin
            );
            if let Some(blocker) = status.activation_blocker {
                println!("  {blocker}");
            }
        }
        println!(
            "Selection: AHU_INTERACTIVE_PLUGIN=cmux|terminal|none|<binding-or-local-id>; otherwise a single enabled committed binding or the build default. `terminal` runs the task in the caller's terminal; `none` disables interactive launch. AHU_PLUGIN_PATH lists absolute local package directories. Inventory does not verify authority or sources, execute plugins, or probe cmux. Native hook integration is separate."
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
    let workspaces = host.workspaces()?;
    let report = serde_json::json!({
        "schema_version": 1, "plugin_id": id,
        "protocol_version": host.persistence_key().map(|_| 1),
        "capabilities": capabilities, "workspaces": workspaces,
        "persistent_sessions": host.check_persistence().is_ok(),
    });
    if output_json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!(
            "{}: activated; {} capabilities, {} workspaces; durable sessions supported={}",
            id,
            capabilities.len(),
            workspaces.len(),
            host.check_persistence().is_ok()
        );
    }
    Ok(())
}
