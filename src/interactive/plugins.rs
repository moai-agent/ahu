//! Compiled plugin inventory and per-process activation. No plugin code runs here.
use crate::util::{Error, ErrorKind, Result};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct PluginStatus {
    pub id: &'static str,
    pub package: &'static str,
    pub host_api: u32,
    pub compiled: bool,
    pub selected: bool,
    pub selection_source: &'static str,
}

/// Selection is explicit process configuration, never repository discovery.
/// Absence retains the default build's historical cmux behavior.
pub fn status() -> Result<PluginStatus> {
    let compiled = cfg!(feature = "cmux-plugin");
    let (selected, selection_source) = match std::env::var("AHU_INTERACTIVE_PLUGIN") {
        Err(std::env::VarError::NotPresent) => (compiled, "build-default"),
        Ok(value) if value == "cmux" => (true, "environment"),
        Ok(value) if value == "none" => (false, "environment"),
        _ => {
            return Err(Error::new(
                "AHU_INTERACTIVE_PLUGIN must be cmux or none; unset it to use this build's default. No other backend was selected.",
            ).with_kind(ErrorKind::Usage));
        }
    };
    Ok(PluginStatus {
        id: "cmux",
        package: "ahu-cmux-plugin",
        host_api: 1,
        compiled,
        selected,
        selection_source,
    })
}

pub fn require_cmux() -> Result<()> {
    let status = status()?;
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
    let status = status()?;
    if output_json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({"schema_version":1,"plugins":[status]}))?
        );
    } else {
        println!(
            "{} ({}, Host V{}): compiled={}, selected={} ({})",
            status.id,
            status.package,
            status.host_api,
            status.compiled,
            status.selected,
            status.selection_source
        );
        println!(
            "Selection: AHU_INTERACTIVE_PLUGIN=cmux|none. Inventory does not execute plugins or probe cmux. Native hook integration is separate."
        );
    }
    Ok(())
}
