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
    pub activation_blocker: Option<&'static str>,
    pub metadata: BTreeMap<String, Value>,
}

/// Selection is explicit process configuration, never a discovery side effect.
/// Absence retains the default build's historical cmux behavior.
pub fn inventory() -> Result<Vec<PluginStatus>> {
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
        activation_blocker: if compiled {
            None
        } else {
            Some("Rebuild ahu with --features cmux-plugin for cmux operations.")
        },
        metadata: BTreeMap::new(),
    }];
    plugins.extend(local::discover()?);
    let selection = match std::env::var("AHU_INTERACTIVE_PLUGIN") {
        Err(std::env::VarError::NotPresent) => return Ok(plugins),
        Ok(value) if value == "none" || plugins.iter().any(|p| p.id == value) => value,
        _ => {
            return Err(Error::new(
                "AHU_INTERACTIVE_PLUGIN must be cmux or none, or an id discovered through AHU_PLUGIN_PATH; unset it to use this build's default. No other backend was selected.",
            ).with_kind(ErrorKind::Usage));
        }
    };
    for plugin in &mut plugins {
        plugin.selected = plugin.id == selection;
        plugin.selection_source = "environment";
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
    if status.origin == "local-manifest" {
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
            "Selection: AHU_INTERACTIVE_PLUGIN=cmux|none|<local-id>. AHU_PLUGIN_PATH lists absolute local package directories. Inventory does not execute plugins or probe cmux. Native hook integration is separate."
        );
    }
    Ok(())
}

/// Explicit activation is separate from read-only package discovery.
pub fn probe(output_json: bool) -> Result<()> {
    let status = status()?;
    if !status.selected {
        return Err(Error::new("no session host is selected").with_kind(ErrorKind::Prerequisite));
    }
    let host = super::discover()?;
    let capabilities = host.check_capabilities()?;
    let workspaces = host.workspaces()?;
    let report = serde_json::json!({
        "schema_version": 1, "plugin_id": status.id,
        "protocol_version": status.executable.as_ref().map(|e| e.protocol_version),
        "capabilities": capabilities, "workspaces": workspaces,
        "persistent_sessions": host.check_persistence().is_ok(),
    });
    if output_json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!(
            "{}: activated; {} capabilities, {} workspaces; durable sessions supported={}",
            status.id,
            capabilities.len(),
            workspaces.len(),
            host.check_persistence().is_ok()
        );
    }
    Ok(())
}
