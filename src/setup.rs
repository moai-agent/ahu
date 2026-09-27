//! One first-run workflow for project policy, harness MCP clients, skills, and
//! ahu developer agents.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::catalog;
use crate::config;
use crate::git::Repo;
use crate::launcher::Console;
use crate::selection;
use crate::util::{Error, Result};

struct Detected {
    id: &'static str,
    executable: String,
    version: Option<String>,
}

/// Configure all project-local inputs needed before future ahu launches.
pub fn run(console: &mut Console<'_>, repo: &Repo) -> Result<i32> {
    if !console.interactive {
        return Err(Error::new(
            "`ahu setup` needs a terminal to ask for project harness and model choices. Nothing was changed.",
        )
        .with_kind(crate::util::ErrorKind::Prerequisite));
    }

    let detected: Vec<_> = catalog::HARNESSES
        .iter()
        .filter(|harness| harness.adapter_available)
        .filter_map(|harness| {
            let prerequisite = selection::check_prerequisite(harness.id);
            prerequisite.satisfied().then_some(Detected {
                id: harness.id,
                executable: prerequisite.executable,
                version: prerequisite.version,
            })
        })
        .collect();
    if detected.is_empty() {
        return Err(Error::new(
            "no supported ahu harness was found on PATH. Install Claude Code, Codex, Antigravity CLI, or OpenCode, then run `ahu setup` again.",
        )
        .with_kind(crate::util::ErrorKind::Prerequisite));
    }
    if selection::resolve_executable("ahu").is_none() {
        return Err(Error::new(
            "the `ahu` executable is not on PATH, so harnesses could not start `ahu mcp serve`. Install ahu or add it to PATH, then run `ahu setup` again. Nothing was changed.",
        )
        .with_kind(crate::util::ErrorKind::Prerequisite));
    }

    console.say("Detected harnesses:\n")?;
    for h in &detected {
        console.say(&format!(
            "  {}{} ({})\n",
            h.id,
            h.version
                .as_deref()
                .map(|v| format!(" {v}"))
                .unwrap_or_default(),
            h.executable
        ))?;
    }

    // Establish project-wide selection policy only when this project has none.
    // This remains a shared, explicit choice, separate from the per-harness
    // developer agents created below.
    let new_config = if config::load(&repo.root)?.is_none() {
        let Some(config) = crate::launcher::run_setup(console)? else {
            console.say("Cancelled. Nothing was changed.\n")?;
            return Ok(1);
        };
        Some(config)
    } else {
        None
    };

    let mut chosen_models: Vec<(&'static str, String)> = Vec::new();
    for h in &detected {
        if let Some(model) = new_config
            .as_ref()
            .and_then(|config| config.model_rankings.get(h.id))
            .and_then(|models| models.first())
        {
            console.say(&format!(
                "\nUsing your selected project model {model} for dev-{}.\n",
                h.id
            ))?;
            chosen_models.push((h.id, model.clone()));
            continue;
        }
        let models = catalog::models_for(h.id);
        if models.is_empty() {
            return Err(Error::new(format!(
                "the ahu model catalog has no selectable models for detected harness {}. Nothing was changed.",
                h.id
            )));
        }
        console.say(&format!(
            "\nModels listed for {} by this ahu release:\n",
            h.id
        ))?;
        for (index, model) in models.iter().enumerate() {
            console.say(&format!(
                "  {}. {} — {}\n",
                index + 1,
                model.model,
                model.display_name
            ))?;
        }
        let Some(answer) = console.ask(&format!("Choose the model for dev-{}: ", h.id))? else {
            console.say("Cancelled. Nothing was changed.\n")?;
            return Ok(1);
        };
        let index = answer.trim().parse::<usize>().ok().filter(|n| *n > 0);
        let Some(model) = index.and_then(|n| models.get(n - 1)) else {
            return Err(Error::new(format!(
                "choose a listed model number for {}; nothing was changed.",
                h.id
            ))
            .with_kind(crate::util::ErrorKind::Usage));
        };
        chosen_models.push((h.id, model.model.to_string()));
    }

    // Make sure the exact server binary configured in the project can start and
    // speak MCP before writing any project files.
    check_mcp_server(repo)?;

    let mut writes: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    if let Some(config) = new_config {
        writes.push((
            safe_new_path(&repo.root, config::CONFIG_RELATIVE_PATH)?,
            config::render(&config).into_bytes(),
        ));
    }
    for &(name, content) in crate::mcp::BUNDLED_SKILLS {
        writes.push((
            safe_new_path(&repo.root, &format!(".agents/skills/{name}/SKILL.md"))?,
            content.as_bytes().to_vec(),
        ));
        if detected.iter().any(|h| h.id == "claude-code") {
            writes.push((
                safe_new_path(&repo.root, &format!(".claude/skills/{name}/SKILL.md"))?,
                content.as_bytes().to_vec(),
            ));
        }
    }
    for (harness, model) in &chosen_models {
        let agent = dev_agent(*harness, model);
        let path = safe_new_path(&repo.root, &format!(".agents/ahu/agents/dev-{harness}.md"))?;
        crate::agent::parse_manifest(&agent, &path)?;
        writes.push((path, agent.into_bytes()));
    }
    add_client_configurations(&repo.root, &detected, &mut writes)?;

    let existing_agents = crate::agent::load_all(&repo.root)?;
    for (harness, _) in &chosen_models {
        if existing_agents
            .iter()
            .any(|agent| agent.manifest.name == format!("dev-{harness}"))
        {
            return Err(Error::new(format!(
                "an ahu agent named `dev-{harness}` already exists at another path; setup will not create an ambiguous agent"
            )));
        }
    }

    // Conflict-check the complete plan before changing anything.
    for (path, bytes) in &writes {
        match std::fs::read(path) {
            Ok(existing) if existing == *bytes => {}
            Ok(_) => {
                return Err(Error::new(format!(
                    "setup will not overwrite existing project file {}; review or move it, then rerun setup. Nothing was changed.",
                    path.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(Error::new(format!(
                    "cannot inspect {}: {e}",
                    path.display()
                )));
            }
        }
    }

    apply_plan(console, &writes)?;

    // Read back native config and agent manifests, then refresh the context lock
    // so every setup-created input is fingerprinted. Launch still requires the
    // user to commit these project files and the lock.
    crate::agent::load_all(&repo.root)?;
    config::load(&repo.root)?;
    verify_client_configurations(&repo.root, &detected)?;
    let snapshot = crate::snapshot::collect(&repo.root)?;
    let lock = crate::context_lock::refresh(repo, &snapshot)?;
    console.say(&format!(
        "\nMCP stdio handshake passed; ahu MCP tools responded.\nRefreshed {}. Review and commit the setup files and lock before running ahu agents. Claude Code asks you to approve project MCP servers; Codex loads project MCP settings only for a trusted repository.\n",
        lock.display()
    ))?;
    Ok(0)
}

fn apply_plan(console: &mut Console<'_>, writes: &[(PathBuf, Vec<u8>)]) -> Result<()> {
    let mut created = Vec::new();
    for (path, bytes) in writes {
        if path.exists() {
            continue;
        }
        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            rollback(&created);
            return Err(Error::new(format!(
                "cannot create {}: {error}",
                parent.display()
            )));
        }
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            file.write_all(bytes)?;
            file.sync_all()
        })();
        if let Err(error) = result {
            rollback(&created);
            return Err(Error::new(format!(
                "cannot create {}: {error}",
                path.display()
            )));
        }
        created.push(path.clone());
        console.say(&format!("Created {}\n", path.display()))?;
    }
    Ok(())
}

fn rollback(paths: &[PathBuf]) {
    for path in paths.iter().rev() {
        let _ = std::fs::remove_file(path);
    }
}

fn dev_agent(harness: &str, model: &str) -> String {
    format!(
        "---\nokf_version: 0.2\ntype: ahu:agent\ntitle: dev-{harness}\nversion: 1.0.0\ndescription: Development agent for {harness}\nharness: {harness}\nmodel: {model}\npermissions: prompt\nstatus: stable\n---\n\nYou are the project's development agent for the {harness} harness. Follow the committed repository instructions and skills that your harness exposes. Use the ahu MCP tools when available: ahu_agents_list, ahu_tasks_list, ahu_task_get, and ahu_typed_decide. Do not claim a skill was loaded merely because its file exists. Ask for approval when an action crosses the permissions available to you."
    )
}

fn safe_new_path(root: &Path, relative: &str) -> Result<PathBuf> {
    crate::util::resolve_within(root, relative, true)
}

fn add_client_configurations(
    root: &Path,
    detected: &[Detected],
    writes: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<()> {
    for harness in detected {
        match harness.id {
            "claude-code" => add_json_server(root, ".mcp.json", "mcpServers", "claude", writes)?,
            "antigravity" => add_json_server(
                root,
                ".agents/mcp_config.json",
                "mcpServers",
                "antigravity",
                writes,
            )?,
            "opencode" => add_json_server(root, "opencode.json", "mcp", "opencode", writes)?,
            "codex" => add_codex_server(root, writes)?,
            _ => {}
        }
    }
    Ok(())
}

fn add_json_server(
    root: &Path,
    relative: &str,
    key: &str,
    shape: &str,
    writes: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<()> {
    let path = safe_new_path(root, relative)?;
    if shape == "opencode" && root.join("opencode.jsonc").exists() {
        return Err(Error::new(
            "opencode.jsonc exists; setup will not create a competing opencode.json or discard JSONC comments. Add the ahu MCP entry to opencode.jsonc, then rerun setup.",
        ));
    }
    let mut value = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| {
            Error::new(format!(
                "cannot safely update {relative}: invalid JSON ({e}); setup made no changes"
            ))
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(Error::new(format!("cannot read {relative}: {e}"))),
    };
    let root_obj = value
        .as_object_mut()
        .ok_or_else(|| Error::new(format!("{relative} must contain a JSON object")))?;
    let servers = root_obj.entry(key).or_insert_with(|| serde_json::json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| Error::new(format!("{relative}: {key} must be an object")))?;
    let server = if shape == "opencode" {
        serde_json::json!({"type":"local","command":["ahu","mcp","serve"],"enabled":true})
    } else {
        serde_json::json!({"command":"ahu","args":["mcp","serve"],"env":{}})
    };
    match servers.get("ahu") {
        Some(existing) if *existing == server => return Ok(()),
        Some(_) => {
            return Err(Error::new(format!(
                "{relative} already defines a different MCP server named `ahu`; setup will not replace it"
            )));
        }
        None => {
            servers.insert("ahu".into(), server);
        }
    }
    writes.push((
        path,
        serde_json::to_vec_pretty(&value).map_err(|e| Error::new(e.to_string()))?,
    ));
    Ok(())
}

fn add_codex_server(root: &Path, writes: &mut Vec<(PathBuf, Vec<u8>)>) -> Result<()> {
    let relative = ".codex/config.toml";
    let path = safe_new_path(root, relative)?;
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::new(format!("cannot read {relative}: {e}"))),
    };
    let parsed: toml::Value = if existing.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(&existing).map_err(|e| {
            Error::new(format!(
                "cannot safely update {relative}: invalid TOML ({e})"
            ))
        })?
    };
    if let Some(server) = parsed.get("mcp_servers").and_then(|v| v.get("ahu")) {
        let command_matches = server.get("command").and_then(toml::Value::as_str) == Some("ahu");
        let args_match = server
            .get("args")
            .and_then(toml::Value::as_array)
            .is_some_and(|a| {
                a == &[
                    toml::Value::String("mcp".into()),
                    toml::Value::String("serve".into()),
                ]
            });
        if command_matches && args_match {
            return Ok(());
        }
        return Err(Error::new(
            ".codex/config.toml already defines a different MCP server named `ahu`; setup will not replace it",
        ));
    }
    let mut output = existing;
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    output.push_str("\n[mcp_servers.ahu]\ncommand = \"ahu\"\nargs = [\"mcp\", \"serve\"]\n");
    writes.push((path, output.into_bytes()));
    Ok(())
}

fn verify_client_configurations(root: &Path, detected: &[Detected]) -> Result<()> {
    for harness in detected {
        match harness.id {
            "codex" => {
                let text = std::fs::read_to_string(root.join(".codex/config.toml"))?;
                let value: toml::Value = toml::from_str(&text).map_err(|error| {
                    Error::new(format!("cannot read back Codex MCP config: {error}"))
                })?;
                let server = value.get("mcp_servers").and_then(|v| v.get("ahu"));
                let args_match = server
                    .and_then(|v| v.get("args"))
                    .and_then(toml::Value::as_array)
                    .is_some_and(|a| {
                        a == &[
                            toml::Value::String("mcp".into()),
                            toml::Value::String("serve".into()),
                        ]
                    });
                if server
                    .and_then(|v| v.get("command"))
                    .and_then(toml::Value::as_str)
                    != Some("ahu")
                    || !args_match
                {
                    return Err(Error::new(
                        "Codex MCP project configuration did not read back as expected",
                    ));
                }
            }
            "claude-code" => verify_json_server(root, ".mcp.json", "mcpServers", false)?,
            "antigravity" => {
                verify_json_server(root, ".agents/mcp_config.json", "mcpServers", false)?
            }
            "opencode" => verify_json_server(root, "opencode.json", "mcp", true)?,
            _ => {}
        }
    }
    Ok(())
}

fn verify_json_server(root: &Path, relative: &str, key: &str, opencode: bool) -> Result<()> {
    let text = std::fs::read(root.join(relative))?;
    let value: serde_json::Value = serde_json::from_slice(&text)?;
    let server = value.pointer(&format!("/{key}/ahu"));
    let valid = if opencode {
        server.is_some_and(|v| {
            v.get("type").and_then(serde_json::Value::as_str) == Some("local")
                && v.get("command")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|a| a == &["ahu", "mcp", "serve"])
        })
    } else {
        server.is_some_and(|v| {
            v.get("command").and_then(serde_json::Value::as_str) == Some("ahu")
                && v.get("args")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|a| a == &["mcp", "serve"])
        })
    };
    if !valid {
        return Err(Error::new(format!(
            "{relative} did not read back the ahu MCP entry"
        )));
    }
    Ok(())
}

fn check_mcp_server(repo: &Repo) -> Result<()> {
    let executable =
        selection::resolve_executable("ahu").ok_or_else(|| Error::new("ahu is not on PATH"))?;
    let mut child = Command::new(executable)
        .args(["mcp", "serve"])
        .current_dir(&repo.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let input = child.stdin.take().expect("piped stdin");
    let output = child.stdout.take().expect("piped stdout");
    let requests = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"ahu-setup\",\"version\":\"1\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n"
    );
    std::thread::spawn(move || {
        let mut input = input;
        let _ = input.write_all(requests.as_bytes());
        let _ = input.flush();
    });
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(output);
        let mut lines = Vec::new();
        for _ in 0..2 {
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() || line.is_empty() {
                break;
            }
            lines.push(line);
        }
        let _ = tx.send(lines);
    });
    let response = rx.recv_timeout(Duration::from_secs(10));
    let _ = child.kill();
    let _ = child.wait();
    let lines = response.map_err(|_| Error::new("ahu MCP handshake timed out"))?;
    let parsed: Vec<serde_json::Value> = lines
        .iter()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    if parsed
        .iter()
        .any(|v| v.get("id") == Some(&serde_json::json!(1)) && v.get("result").is_some())
        && parsed.iter().any(|v| {
            v.get("id") == Some(&serde_json::json!(2))
                && v.pointer("/result/tools")
                    .and_then(serde_json::Value::as_array)
                    .is_some()
        })
    {
        Ok(())
    } else {
        Err(Error::new(
            "ahu MCP server did not complete initialize and tools/list; nothing was changed",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Detected, add_client_configurations, dev_agent, verify_client_configurations};

    #[test]
    fn each_supported_harness_gets_its_native_project_mcp_shape() {
        let root = tempfile::tempdir().unwrap();
        let detected = [
            Detected {
                id: "claude-code",
                executable: "claude".into(),
                version: None,
            },
            Detected {
                id: "codex",
                executable: "codex".into(),
                version: None,
            },
            Detected {
                id: "antigravity",
                executable: "agy".into(),
                version: None,
            },
            Detected {
                id: "opencode",
                executable: "opencode".into(),
                version: None,
            },
        ];
        let mut plan = Vec::new();
        add_client_configurations(root.path(), &detected, &mut plan).unwrap();
        for (path, bytes) in plan {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        verify_client_configurations(root.path(), &detected).unwrap();
    }

    #[test]
    fn generated_agent_is_a_valid_prompt_permission_manifest() {
        for (harness, model) in [
            ("claude-code", "claude-opus-5"),
            ("codex", "gpt-6-astra"),
            ("antigravity", "gemini-3.1-pro-high"),
            ("opencode", "ollama/glm-5.3:cloud"),
        ] {
            let text = dev_agent(harness, model);
            let path = Path::new(".agents/ahu/agents/generated.md");
            let (manifest, body) = crate::agent::parse_manifest(&text, path).unwrap();
            assert_eq!(manifest.name, format!("dev-{harness}"));
            assert_eq!(manifest.model, model);
            assert_eq!(manifest.permissions, crate::agent::Permissions::Prompt);
            assert!(body.contains("ahu MCP tools"));
        }
    }
}
