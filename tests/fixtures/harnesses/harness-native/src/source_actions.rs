//! Passive definition parsing and bounded project discovery.
use crate::{Error, Result, files};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Parse {
        format: String,
        text: String,
        path: String,
    },
    Discover {
        project_dir: PathBuf,
    },
}
pub(crate) fn call(harness: &str, value: &Value) -> Result<Value> {
    match crate::decode(value)? {
        Action::Parse { format, text, path } => {
            if path.is_empty() || path.contains('\0') {
                return Err(Error::invalid());
            }
            parse(harness, &format, &text)
        }
        Action::Discover { project_dir } => discover(harness, &project_dir),
    }
}
fn formats(harness: &str) -> Result<&'static [(&'static str, &'static str, &'static str)]> {
    Ok(match harness {
        "codex" => &[(".codex/agents", "toml", "codex-agent")],
        "claude-code" => &[(".claude/agents", "md", "claude-agent")],
        "opencode" => &[
            (".opencode/agents", "md", "opencode-agent"),
            (".opencode/agent", "md", "opencode-agent"),
        ],
        "antigravity" => &[(".agents/agents", "md", "antigravity-agent")],
        "kiro" => &[
            (".kiro/agents", "json", "kiro-json"),
            (".kiro/agents", "md", "kiro-markdown"),
        ],
        _ => return Err(Error::invalid()),
    })
}
fn parse(harness: &str, format: &str, text: &str) -> Result<Value> {
    if text.len() > files::LIMIT || !formats(harness)?.iter().any(|(_, _, f)| *f == format) {
        return Err(Error::invalid());
    }
    let mut settings = BTreeMap::<String, String>::new();
    let mut model = None;
    let instructions = if format == "codex-agent" {
        // Native TOML is delivered verbatim; a TOML model key does not select
        // the ahu manifest's fixed identity.
        text.to_owned()
    } else if format == "kiro-json" {
        let value: Value = serde_json::from_str(text)?;
        let object = value.as_object().ok_or_else(Error::invalid)?;
        for (key, value) in object {
            settings.insert(
                key.clone(),
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string()),
            );
        }
        model = object
            .get("model")
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(Error::invalid))
            .transpose()?;
        object
            .get("prompt")
            .and_then(Value::as_str)
            .filter(|s| !s.starts_with("file://"))
            .ok_or_else(Error::invalid)?
            .to_owned()
    } else if let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    {
        let mut end = None;
        let mut offset = 0;
        for line in rest.split_inclusive('\n') {
            if line.trim_end() == "---" {
                end = Some((offset, line.len()));
                break;
            }
            offset += line.len();
        }
        if let Some((offset, length)) = end {
            let value: Value =
                yaml_serde::from_str(&rest[..offset]).map_err(|_| Error::invalid())?;
            let object = value.as_object().ok_or_else(Error::invalid)?;
            for (key, value) in object {
                settings.insert(
                    key.clone(),
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                );
            }
            model = object
                .get("model")
                .map(|v| v.as_str().map(str::to_owned).ok_or_else(Error::invalid))
                .transpose()?;
            rest[offset + length..].trim_start_matches('\n').to_owned()
        } else {
            text.to_owned()
        }
    } else {
        text.to_owned()
    };
    if instructions.trim().is_empty() {
        return Err(Error::invalid());
    }
    Ok(json!({"instructions":instructions,"native_model":model,"native_settings":settings}))
}
fn discover(harness: &str, root: &Path) -> Result<Value> {
    crate::directory(root)?;
    if root.canonicalize()? != root {
        return Err(Error::invalid());
    }
    let mut candidates = Vec::new();
    let mut scanned = 0;
    for (relative, extension, format) in formats(harness)? {
        let directory = root.join(relative);
        files::check_components(&directory)?;
        let entries = match std::fs::read_dir(&directory) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(Error::invalid()),
        };
        for entry in entries {
            scanned += 1;
            if scanned > 1024 {
                return Err(Error::invalid());
            }
            let entry = entry?;
            let mut path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Err(Error::invalid());
            }
            let name = if kind.is_dir() && harness == "antigravity" {
                path = path.join("agent.md");
                entry
                    .file_name()
                    .to_str()
                    .ok_or_else(Error::invalid)?
                    .to_owned()
            } else {
                if !kind.is_file() || path.extension().and_then(|x| x.to_str()) != Some(extension) {
                    continue;
                }
                path.file_stem()
                    .and_then(|x| x.to_str())
                    .ok_or_else(Error::invalid)?
                    .to_owned()
            };
            let Some(bytes) = files::read(&path)? else {
                continue;
            };
            let text = std::str::from_utf8(&bytes).map_err(|_| Error::invalid())?;
            let parsed = parse(harness, format, text);
            let (model, fields, description, blockers) = match parsed {
                Ok(v) => (
                    v["native_model"].clone(),
                    v["native_settings"]
                        .as_object()
                        .ok_or_else(Error::invalid)?
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>(),
                    v["native_settings"]["description"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                    vec![],
                ),
                Err(_) => (
                    Value::Null,
                    vec![],
                    String::new(),
                    vec!["native definition cannot be safely parsed"],
                ),
            };
            candidates.push(json!({"name":name,"path":path.strip_prefix(root).map_err(|_|Error::invalid())?,"format":format,"native_model":model,"blockers":blockers,"preserved_fields":fields,"description":description}));
            if candidates.len() > 256 {
                return Err(Error::invalid());
            }
        }
    }
    candidates.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Ok(json!(candidates))
}
