//! Passive inventory and explicit import; never runs native code.
use crate::{Error, Result, files};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    project: PathBuf,
    home: PathBuf,
    import: Option<Import>,
    #[serde(default)]
    settings: Settings,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    #[serde(default)]
    environment_names: Vec<String>,
    view: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    path: PathBuf,
    format: String,
}
pub(crate) fn call(harness: &str, params: &Value) -> Result<Value> {
    let p: Params = crate::decode(params)?;
    crate::directory(&p.project)?;
    crate::directory(&p.home)?;
    if !crate::valid_environment_names(&p.settings.environment_names) {
        return Err(Error::invalid());
    }
    let overrides = p
        .settings
        .environment_names
        .iter()
        .any(|name| crate::ambient::auth_override(harness, name));
    match p.settings.view.as_deref() {
        Some("hooks") => {
            let locations = crate::hook_inventory::Locations {
                home: Some(p.home.clone()),
                managed: Some(PathBuf::from(if cfg!(target_os = "macos") {
                    "/Library/Application Support/ClaudeCode/managed-settings.json"
                } else {
                    "/etc/claude-code/managed-settings.json"
                })),
            };
            let mut inventory =
                crate::hook_inventory::collect_for(&p.project, harness, &locations)?;
            if overrides {
                inventory.unscanned_harness = Some(harness.into());
                inventory.unreadable.push("native environment changes configuration scope; default-scope inventory is incomplete".into());
            }
            for facts in &mut inventory.settings {
                facts.approvals_widened = Some(facts.widens_approvals());
            }
            for server in &mut inventory.mcp_servers {
                server.command = format!(
                    "<native command sha256:{}>",
                    crate::util::digest_bytes(server.command.as_bytes())
                );
            }
            return Ok(serde_json::to_value(inventory)?);
        }
        Some("private_fingerprints") => {
            if overrides {
                return Err(Error::invalid());
            }
            let paths: &[&str] = match harness {
                "antigravity" => &[".gemini/config/mcp_config.json"],
                "codex" => &[".codex/config.toml"],
                "claude-code" => &[".claude/settings.json"],
                "kiro" => &[".kiro/settings/mcp.json"],
                "opencode" => &[".config/opencode/opencode.json"],
                _ => return Err(Error::invalid()),
            };
            let mut fingerprints = serde_json::Map::new();
            for (index, relative) in paths.iter().enumerate() {
                let path = p.home.join(relative);
                if let Some(bytes) = crate::files::read(&path)? {
                    fingerprints.insert(
                        format!("native:{harness}:{index}"),
                        json!(crate::util::digest_bytes(&bytes)),
                    );
                }
            }
            return Ok(json!({"fingerprints":fingerprints}));
        }
        None | Some("inventory") => (),
        _ => return Err(Error::invalid()),
    }

    let (context, agents, skills, configs): (&[&str], &[&str], &[&str], &[&str]) = match harness {
        "claude-code" => (
            &["CLAUDE.md", ".claude/CLAUDE.md"],
            &[".claude/agents"],
            &[".claude/skills", ".agents/skills"],
            &[
                ".claude/settings.json",
                ".claude/settings.local.json",
                ".mcp.json",
            ],
        ),
        "codex" => (
            &["AGENTS.md", "AGENTS.override.md"],
            &[".codex/agents"],
            &[".agents/skills"],
            &[".codex/config.toml", ".codex/hooks.json"],
        ),
        "antigravity" => (
            &["GEMINI.md", ".agents/rules"],
            &[".agents/agents"],
            &[".agents/skills"],
            &[".agents/mcp_config.json"],
        ),
        "opencode" => (
            &["AGENTS.md"],
            &[".opencode/agents", ".opencode/agent"],
            &[".opencode/skills", ".agents/skills"],
            &["opencode.json", "opencode.jsonc"],
        ),
        "kiro" => (
            &["AGENTS.md", ".kiro/steering", ".kiroignore"],
            &[".kiro/agents"],
            &[".kiro/skills", ".agents/skills"],
            &[".kiro/settings/cli.json", ".kiro/settings/mcp.json"],
        ),
        _ => return Err(Error::invalid()),
    };
    let mut entries = Vec::new();
    for (kind, paths) in [
        ("context", context),
        ("agents", agents),
        ("skills", skills),
        ("configuration", configs),
    ] {
        for relative in paths {
            let path = p.project.join(relative);
            files::check_components(&path)?;
            let present = match std::fs::symlink_metadata(&path) {
                Ok(_) => true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(_) => return Err(Error::invalid()),
            };
            entries.push(json!({"kind":kind,"path":path,"present":present,"loaded":"unknown"}));
        }
    }
    let global: &[&str] = match harness {
        "codex" => &[
            ".codex/config.toml",
            ".codex/hooks.json",
            ".codex/AGENTS.md",
            ".codex/agents",
            ".agents/skills",
        ],
        "claude-code" => &[
            ".claude/settings.json",
            ".claude/CLAUDE.md",
            ".claude/agents",
            ".claude/skills",
        ],
        "antigravity" => &[
            ".gemini/config/mcp_config.json",
            ".gemini/config/hooks.json",
            ".gemini/antigravity-cli/settings.json",
        ],
        "opencode" => &[
            ".config/opencode/opencode.json",
            ".config/opencode/opencode.jsonc",
            ".config/opencode/agents",
            ".config/opencode/skills",
        ],
        "kiro" => &[
            ".kiro/settings/cli.json",
            ".kiro/settings/mcp.json",
            ".kiro/agents",
            ".kiro/skills",
            ".kiro/steering",
        ],
        _ => &[],
    };
    for relative in global {
        let path = p.home.join(relative);
        files::check_components(&path)?;
        let present = match std::fs::symlink_metadata(&path) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err(Error::invalid()),
        };
        entries
            .push(json!({"kind":"native_global","path":path,"present":present,"loaded":"unknown"}));
    }
    let imported = if let Some(import) = p.import {
        if !import.path.starts_with(&p.project) || import.path == p.project {
            return Err(Error::invalid());
        }
        Some(import_agent(harness, &import.path, &import.format)?)
    } else {
        None
    };
    Ok(
        json!({"sources":entries,"imported":imported,"activation":"none","coverage":"recognized project and native home paths; synchronized and overridden context remains unknown"}),
    )
}
fn import_agent(harness: &str, path: &Path, format: &str) -> Result<Value> {
    let bytes = files::read(path)?.ok_or_else(Error::invalid)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| Error::invalid())?;
    let (body, model, settings) = match (harness, format) {
        ("codex", "codex-agent") => (text.to_owned(), Value::Null, json!({})),
        ("kiro", "kiro-json") => {
            let v: Value = serde_json::from_str(text)?;
            let body = v
                .get("prompt")
                .and_then(Value::as_str)
                .filter(|s| !s.starts_with("file://"))
                .ok_or_else(Error::invalid)?
                .to_owned();
            let model = v.get("model").cloned().unwrap_or(Value::Null);
            (body, model, v)
        }
        ("claude-code", "claude-agent")
        | ("opencode", "opencode-agent")
        | ("kiro", "kiro-markdown")
        | ("antigravity", "antigravity-agent") => {
            if let Some(rest) = text.strip_prefix("---\n") {
                let (front, body) = rest.split_once("\n---\n").ok_or_else(Error::invalid)?;
                let settings: Value = yaml_serde::from_str(front).map_err(|_| Error::invalid())?;
                if !settings.is_object() {
                    return Err(Error::invalid());
                }
                let model = settings.get("model").cloned().unwrap_or(Value::Null);
                (body.to_owned(), model, settings)
            } else {
                (text.to_owned(), Value::Null, json!({}))
            }
        }
        _ => return Err(Error::invalid()),
    };
    if body.trim().is_empty() || (!model.is_null() && !model.is_string()) {
        return Err(Error::invalid());
    }
    // Settings are reported as declared fields only, never interpreted as grants.
    let fields = settings
        .as_object()
        .ok_or_else(Error::invalid)?
        .keys()
        .collect::<Vec<_>>();
    Ok(
        json!({"path":path,"format":format,"instructions":body,"model":model,"declared_setting_fields":fields,"sha256":crate::util::digest_bytes(&bytes),"settings_applied":false}),
    )
}
