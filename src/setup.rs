//! One first-run workflow for project policy, harness MCP clients, skills, and
//! ahu developer agents.

pub mod native_mcp;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::catalog;
use crate::config;
use crate::git::Repo;
use crate::launcher::Console;
use crate::selection;
use crate::style::{self, Role};
use crate::util::{Error, Result, display_path};

struct Detected {
    id: &'static str,
    executable: String,
    version: Option<String>,
}

// Codex filters the environment of stdio MCP children. Forward names only;
// credentials are resolved by the server, never read or persisted by setup.
const CODEX_MCP_ENV: &[&str] = &[
    "AHU_TASK_ID",
    "AHU_TASK_DIR",
    "AHU_EVAL_OTEL_ENDPOINT",
    "AHU_MCP_RESOURCE_ATTRIBUTES",
    "OTEL_RESOURCE_ATTRIBUTES",
    "TYPESAFE_API_KEY",
    "AHU_DECISION_URL",
    "AHU_DECISION_MODEL",
    "AHU_OLLAMA_MODEL",
    "AHU_OLLAMA_URL",
];

fn codex_env_forwarding(existing: &str, parsed: &toml::Value) -> Result<Option<String>> {
    let server = &parsed["mcp_servers"]["ahu"];
    let mut values = match server.get("env_vars") {
        Some(toml::Value::Array(values)) => values.clone(),
        None => Vec::new(),
        _ => {
            return Err(Error::new(
                "ahu MCP env_vars must be an array of variable names",
            ));
        }
    };
    for name in CODEX_MCP_ENV {
        if !values.iter().any(|v| {
            v.as_str() == Some(name)
                || (v.get("name").and_then(toml::Value::as_str) == Some(name)
                    && v.get("source")
                        .and_then(toml::Value::as_str)
                        .unwrap_or("local")
                        == "local")
        }) {
            values.push(toml::Value::String((*name).into()));
        }
    }
    let wanted = toml::Value::Array(values);
    if server.get("env_vars") == Some(&wanted) {
        return Ok(None);
    }
    #[derive(serde::Deserialize)]
    struct Location {
        args: toml::Spanned<toml::Value>,
        env_vars: Option<toml::Spanned<toml::Value>>,
    }
    #[derive(serde::Deserialize)]
    struct Locations {
        mcp_servers: std::collections::BTreeMap<String, Location>,
    }
    let locations: Locations = toml::from_str(existing).map_err(|_| {
        Error::new("cannot locate ahu MCP configuration for environment forwarding")
    })?;
    let location = &locations.mcp_servers["ahu"];
    let encoded = wanted.to_string();
    let mut expected = parsed.clone();
    expected["mcp_servers"]["ahu"]
        .as_table_mut()
        .unwrap()
        .insert("env_vars".into(), wanted);
    let mut candidates = Vec::new();
    if let Some(value) = &location.env_vars {
        let mut candidate = existing.to_owned();
        candidate.replace_range(value.span(), &encoded);
        candidates.push(candidate);
    } else {
        let end = location.args.span().end;
        let line_end = existing[end..]
            .find('\n')
            .map_or(existing.len(), |offset| end + offset + 1);
        let mut candidate = existing.to_owned();
        let separator = if line_end == existing.len() && !existing.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        candidate.insert_str(line_end, &format!("{separator}env_vars = {encoded}\n"));
        candidates.push(candidate);
        // Inline tables need a comma rather than a new assignment line.
        let mut candidate = existing.to_owned();
        candidate.insert_str(end, &format!(", env_vars = {encoded}"));
        candidates.push(candidate);
    }
    candidates
        .into_iter()
        .find(|text| toml::from_str::<toml::Value>(text).ok().as_ref() == Some(&expected))
        .map(Some)
        .ok_or_else(|| {
            Error::new("cannot safely update ahu MCP env_vars without changing other configuration")
        })
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

    let native_mcp = if detected.iter().any(|h| h.id == "antigravity") {
        let path = crate::native_mcp::antigravity_config_path()?.ok_or_else(|| {
            Error::new("Antigravity interactive MCP setup needs an absolute native home")
        })?;
        let executable = selection::resolve_executable("ahu")
            .ok_or_else(|| Error::new("ahu executable disappeared before MCP setup"))?;
        let executable = Path::new(&executable).canonicalize()?;
        Some(native_mcp::plan(&path, &executable)?)
    } else {
        None
    };
    run_detected_with_native(
        console,
        repo,
        &detected,
        crate::launcher::run_setup_with_available_models,
        check_mcp_server,
        native_mcp,
    )
}

/// Finish configuration for an already detected set of harnesses. Keeping
/// environment discovery at the edge makes the setup transaction testable
/// without faking PATH or launching harness binaries.
#[cfg(test)]
fn run_detected(
    console: &mut Console<'_>,
    repo: &Repo,
    detected: &[Detected],
    configure_project: fn(&mut Console<'_>) -> Result<Option<crate::config::ProjectConfig>>,
    check_server: fn(&Repo) -> Result<()>,
) -> Result<i32> {
    run_detected_with_native(
        console,
        repo,
        detected,
        configure_project,
        check_server,
        None,
    )
}

fn run_detected_with_native(
    console: &mut Console<'_>,
    repo: &Repo,
    detected: &[Detected],
    configure_project: fn(&mut Console<'_>) -> Result<Option<crate::config::ProjectConfig>>,
    check_server: fn(&Repo) -> Result<()>,
    native_mcp: Option<native_mcp::Plan>,
) -> Result<i32> {
    console.say(&style::stdout().paint(Role::Heading, "Detected harnesses\n"))?;
    for h in detected {
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
    let loaded = config::load(&repo.root)?;
    let (new_config, existing_config) = if loaded.is_none() {
        let Some(config) = configure_project(console)? else {
            console.say("Cancelled. Nothing was changed.\n")?;
            return Ok(1);
        };
        (Some(config), None)
    } else {
        (None, loaded.map(|config| config.config))
    };

    let mut chosen_models: Vec<(&'static str, String)> = Vec::new();
    for h in detected {
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
        let options = crate::models::for_harness(h.id);
        let models = options.models;
        if models.is_empty() {
            return Err(Error::new(format!(
                "no currently available model for {} is in ahu's validated compatibility catalog. Nothing was changed.",
                h.id
            )));
        }
        let source = match options.source {
            crate::models::Source::Harness => "listed by this harness",
            crate::models::Source::Catalog => {
                "listed in the ahu catalog (account availability is not checked)"
            }
        };
        console.say(&style::stdout().paint(
            Role::Heading,
            &format!("\nModels for {} ({source}):\n", h.id),
        ))?;
        for (index, model) in models.iter().enumerate() {
            console.say(&format!(
                "  {}. {} — {}\n",
                index + 1,
                model.model,
                model.display_name
            ))?;
        }
        let preferred = existing_config
            .as_ref()
            .and_then(|config| config.model_rankings.get(h.id))
            .and_then(|ranking| ranking.first())
            .and_then(|preferred| models.iter().position(|model| model.model == preferred));
        let prompt = match preferred {
            Some(index) => format!(
                "Choose the model for dev-{} [{}] (blank keeps it): ",
                h.id,
                index + 1
            ),
            None => format!("Choose the model for dev-{} [1]: ", h.id),
        };
        let Some(answer) = console.ask(&prompt)? else {
            console.say("Cancelled. Nothing was changed.\n")?;
            return Ok(1);
        };
        let index = if answer.trim().is_empty() {
            Some(preferred.unwrap_or(0))
        } else {
            answer
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|n| *n > 0)
                .map(|n| n - 1)
        };
        let Some(model) = index.and_then(|n| models.get(n)) else {
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
    check_server(repo)?;

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
        let agent = dev_agent(harness, model);
        let path = safe_new_path(&repo.root, &format!(".agents/ahu/agents/dev-{harness}.md"))?;
        crate::agent::parse_manifest(&agent, &path)?;
        writes.push((path, agent.into_bytes()));
    }
    add_client_configurations(&repo.root, detected, &mut writes)?;

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

    if native_mcp.is_some() {
        console.say("Antigravity: registering the cwd-sensitive ahu server in native user MCP configuration for interactive sessions; project configuration remains available for print mode. Other native servers are preserved.\n")?;
    }
    apply_plan(console, &writes)?;
    if let Some(plan) = native_mcp {
        plan.apply()?;
    }

    // Read back native config and agent manifests, then refresh shared context.
    // A first setup initializes private local-context acceptance; later setup
    // runs must not silently accept changes to an existing user's settings.
    crate::agent::load_all(&repo.root)?;
    config::load(&repo.root)?;
    verify_client_configurations(&repo.root, detected)?;
    let snapshot = crate::snapshot::collect(&repo.root)?;
    let lock = crate::context_lock::refresh_for_setup(repo, &snapshot)?;
    console.say(&format!(
        "\nMCP stdio handshake passed; ahu MCP tools responded.\nRefreshed {} for shared context. Review and commit setup files and the lock before running agents. Local settings are accepted on first setup; later changes require `ahu lock --update` and stay in private Ahu state. Claude Code asks you to approve project MCP servers; Codex loads project MCP settings only for a trusted repository.\n",
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
        console.say(&format!(
            "{} {}\n",
            style::stdout().paint(Role::Success, "created"),
            display_path(path)
        ))?;
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
        "---\nokf_version: 0.2\ntype: ahu:agent\ntitle: dev-{harness}\nversion: 1.0.0\ndescription: Development agent for {harness}\nharness: {harness}\nmodel: {model}\npermissions: prompt\nstatus: stable\n---\n\nYou are the project's development agent for the {harness} harness. Follow the committed repository instructions and skills that your harness exposes. Use the ahu MCP tools when available: ahu_agents_list, ahu_auth_budget, ahu_tasks_list, ahu_task_get, ahu_typed_decide, and ahu_request_approval for an explicit operator checkpoint before a consequential operation. Use ahu_auth_budget before delegating costly work when useful; optionally pass minimum_remaining_percent to find registered agent/model candidates whose every reported provider window meets that threshold. It reports rate-limit percentages, not token counts or costs, and agents on one account share capacity. Unknown or unsupported windows are never eligible. Eligibility does not reserve capacity or confirm model availability. The approval tool only pauses and records bounded request details; it never performs the requested operation. Do not claim a skill was loaded merely because its file exists. Ask for approval when an action crosses the permissions available to you."
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
            if let Some(updated) = codex_env_forwarding(&existing, &parsed)? {
                writes.push((path, updated.into_bytes()));
            }
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
    output.push_str(&format!(
        "env_vars = {}\n",
        toml::Value::Array(
            CODEX_MCP_ENV
                .iter()
                .map(|v| toml::Value::String((*v).into()))
                .collect()
        )
    ));
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
                    || codex_env_forwarding(&text, &value)?.is_some()
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
    check_mcp_server_at(repo, Path::new(&executable), Duration::from_secs(10))
}

// Keep executable discovery at the edge so the protocol can be checked with
// local deterministic servers without changing process-wide PATH.
fn check_mcp_server_at(repo: &Repo, executable: &Path, timeout: Duration) -> Result<()> {
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
    let response = rx.recv_timeout(timeout);
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

    use super::{
        CODEX_MCP_ENV, Detected, add_client_configurations, add_codex_server, add_json_server,
        apply_plan, codex_env_forwarding, dev_agent, run, run_detected, safe_new_path,
        verify_client_configurations,
    };

    fn detected(id: &'static str) -> Detected {
        Detected {
            id,
            executable: id.into(),
            version: None,
        }
    }

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

    #[test]
    fn json_server_preserves_existing_settings_and_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".mcp.json"), r#"{"custom":{"keep":true}}"#).unwrap();
        let mut plan = Vec::new();
        add_json_server(root.path(), ".mcp.json", "mcpServers", "claude", &mut plan).unwrap();
        assert_eq!(plan.len(), 1);
        let value: serde_json::Value = serde_json::from_slice(&plan[0].1).unwrap();
        assert_eq!(value["custom"]["keep"], true);
        assert_eq!(value["mcpServers"]["ahu"]["args"][1], "serve");
        std::fs::write(&plan[0].0, &plan[0].1).unwrap();
        let mut second_plan = Vec::new();
        add_json_server(
            root.path(),
            ".mcp.json",
            "mcpServers",
            "claude",
            &mut second_plan,
        )
        .unwrap();
        assert!(second_plan.is_empty());
    }

    #[test]
    fn json_server_rejects_unsafe_or_conflicting_configuration() {
        let root = tempfile::tempdir().unwrap();
        let mut plan = Vec::new();
        std::fs::write(root.path().join("opencode.jsonc"), "{} // comment\n").unwrap();
        assert!(
            add_json_server(root.path(), "opencode.json", "mcp", "opencode", &mut plan)
                .unwrap_err()
                .to_string()
                .contains("opencode.jsonc")
        );
        std::fs::remove_file(root.path().join("opencode.jsonc")).unwrap();

        std::fs::write(root.path().join("config.json"), "not json").unwrap();
        assert!(
            add_json_server(root.path(), "config.json", "mcp", "claude", &mut plan)
                .unwrap_err()
                .to_string()
                .contains("invalid JSON")
        );
        std::fs::write(root.path().join("config.json"), "[]").unwrap();
        assert!(
            add_json_server(root.path(), "config.json", "mcp", "claude", &mut plan)
                .unwrap_err()
                .to_string()
                .contains("JSON object")
        );
        std::fs::write(root.path().join("config.json"), r#"{"mcp":[]}"#).unwrap();
        assert!(
            add_json_server(root.path(), "config.json", "mcp", "claude", &mut plan)
                .unwrap_err()
                .to_string()
                .contains("must be an object")
        );
        std::fs::write(
            root.path().join("config.json"),
            r#"{"mcp":{"ahu":{"command":"other"}}}"#,
        )
        .unwrap();
        assert!(
            add_json_server(root.path(), "config.json", "mcp", "claude", &mut plan)
                .unwrap_err()
                .to_string()
                .contains("will not replace it")
        );
    }

    #[test]
    fn codex_server_handles_empty_existing_and_conflicting_toml() {
        let root = tempfile::tempdir().unwrap();
        let mut plan = Vec::new();
        add_codex_server(root.path(), &mut plan).unwrap();
        assert!(
            String::from_utf8(plan.pop().unwrap().1)
                .unwrap()
                .contains("[mcp_servers.ahu]")
        );

        std::fs::create_dir_all(root.path().join(".codex")).unwrap();
        let path = root.path().join(".codex/config.toml");
        std::fs::write(&path, "model = 'x'\n").unwrap();
        let mut plan = Vec::new();
        add_codex_server(root.path(), &mut plan).unwrap();
        assert!(
            String::from_utf8(plan[0].1.clone())
                .unwrap()
                .starts_with("model = 'x'")
        );
        std::fs::write(&path, &plan[0].1).unwrap();
        let mut idempotent = Vec::new();
        add_codex_server(root.path(), &mut idempotent).unwrap();
        assert!(idempotent.is_empty());

        std::fs::write(&path, "[mcp_servers.ahu]\ncommand='other'\n").unwrap();
        assert!(
            add_codex_server(root.path(), &mut Vec::new())
                .unwrap_err()
                .to_string()
                .contains("will not replace it")
        );
        std::fs::write(&path, "invalid = [\n").unwrap();
        assert!(
            add_codex_server(root.path(), &mut Vec::new())
                .unwrap_err()
                .to_string()
                .contains("invalid TOML")
        );
    }

    #[test]
    fn codex_mcp_upgrade_forwards_names_and_preserves_unrelated_config() {
        for text in [
            "# keep this\nmodel = 'x'\n[mcp_servers.ahu]\ncommand = 'ahu'\nargs = ['mcp','serve'] # keep this too\n[features]\nhooks = true\n",
            "[mcp_servers.ahu]\ncommand = 'ahu'\nargs = ['mcp','serve']",
            "mcp_servers.ahu = {command = 'ahu', args = ['mcp','serve']}\n",
            "[mcp_servers.ahu]\ncommand = 'ahu'\nargs = ['mcp','serve']\nenv_vars = ['CUSTOM_NAME'] # retained comment\n",
        ] {
            let parsed: toml::Value = toml::from_str(text).unwrap();
            let updated = codex_env_forwarding(text, &parsed).unwrap().unwrap();
            let value: toml::Value = toml::from_str(&updated).unwrap();
            for name in ["AHU_OLLAMA_MODEL", "AHU_OLLAMA_URL"]
                .iter()
                .chain(CODEX_MCP_ENV.iter())
            {
                assert!(
                    value["mcp_servers"]["ahu"]["env_vars"]
                        .as_array()
                        .unwrap()
                        .contains(&toml::Value::String((*name).into()))
                );
            }
            for comment in ["# keep this", "# keep this too", "# retained comment"] {
                if text.contains(comment) {
                    assert!(updated.contains(comment));
                }
            }
            if text.contains("CUSTOM_NAME") {
                assert!(updated.contains("CUSTOM_NAME"));
            }
            assert!(codex_env_forwarding(&updated, &value).unwrap().is_none());
        }
        let invalid: toml::Value =
            toml::from_str("[mcp_servers.ahu]\nargs = ['mcp','serve']\nenv_vars = 'bad'\n")
                .unwrap();
        assert!(codex_env_forwarding("", &invalid).is_err());
    }

    #[test]
    fn setup_plan_creates_files_and_rolls_back_partial_failure() {
        let root = tempfile::tempdir().unwrap();
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        let mut console = crate::launcher::Console {
            input: &mut input,
            output: &mut output,
            interactive: true,
        };
        let created = root.path().join("nested/first.txt");
        apply_plan(&mut console, &[(created.clone(), b"first".to_vec())]).unwrap();
        assert_eq!(std::fs::read(&created).unwrap(), b"first");

        let blocking_file = root.path().join("blocker");
        std::fs::write(&blocking_file, "not a directory").unwrap();
        let before_failure = root.path().join("before-failure.txt");
        let blocked = blocking_file.join("child.txt");
        assert!(
            apply_plan(
                &mut console,
                &[
                    (before_failure.clone(), b"temporary".to_vec()),
                    (blocked, b"fail".to_vec())
                ]
            )
            .is_err()
        );
        assert!(!before_failure.exists());
    }

    #[test]
    fn safe_new_path_rejects_parent_escape_and_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        assert!(safe_new_path(root.path(), "../outside").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = tempfile::tempdir().unwrap();
            symlink(outside.path(), root.path().join("escape")).unwrap();
            assert!(safe_new_path(root.path(), "escape/file").is_err());
        }
    }

    #[test]
    fn client_configuration_verification_reports_missing_or_malformed_entries() {
        let root = tempfile::tempdir().unwrap();
        assert!(verify_client_configurations(root.path(), &[detected("claude-code")]).is_err());
        std::fs::write(root.path().join(".mcp.json"), "not json").unwrap();
        assert!(verify_client_configurations(root.path(), &[detected("claude-code")]).is_err());

        std::fs::create_dir_all(root.path().join(".codex")).unwrap();
        std::fs::write(root.path().join(".codex/config.toml"), "model='x'\n").unwrap();
        assert!(
            verify_client_configurations(root.path(), &[detected("codex")])
                .unwrap_err()
                .to_string()
                .contains("did not read back")
        );
        std::fs::write(root.path().join(".codex/config.toml"), "invalid = [\n").unwrap();
        assert!(verify_client_configurations(root.path(), &[detected("codex")]).is_err());
    }

    #[test]
    fn unknown_harnesses_do_not_create_client_configuration() {
        let root = tempfile::tempdir().unwrap();
        let mut plan = Vec::new();
        add_client_configurations(root.path(), &[detected("future-harness")], &mut plan).unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn noninteractive_setup_fails_before_inspecting_harnesses_or_writing_files() {
        let root = tempfile::tempdir().unwrap();
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        let mut console = crate::launcher::Console {
            input: &mut input,
            output: &mut output,
            interactive: false,
        };
        let repo = crate::git::Repo {
            root: root.path().to_path_buf(),
            common_dir: root.path().join(".git"),
            head: None,
        };
        let error = run(&mut console, &repo).unwrap_err();
        assert!(error.to_string().contains("needs a terminal"));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn setup_plan_skips_identical_existing_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("already.md");
        std::fs::write(&path, "same").unwrap();
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        let mut console = crate::launcher::Console {
            input: &mut input,
            output: &mut output,
            interactive: true,
        };
        apply_plan(&mut console, &[(path.clone(), b"same".to_vec())]).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"same");
        assert!(output.is_empty());
    }

    #[test]
    fn detected_harness_setup_writes_a_complete_consistent_project_plan() {
        fn configure(
            _: &mut crate::launcher::Console<'_>,
        ) -> crate::util::Result<Option<crate::config::ProjectConfig>> {
            Ok(Some(project_config()))
        }
        fn mcp_handshake_ok(_: &crate::git::Repo) -> crate::util::Result<()> {
            Ok(())
        }

        let root = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(init.success());
        let repo = crate::git::discover(root.path()).unwrap();

        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        let mut console = crate::launcher::Console {
            input: &mut input,
            output: &mut output,
            interactive: true,
        };
        assert_eq!(
            run_detected(
                &mut console,
                &repo,
                &[detected("codex")],
                configure,
                mcp_handshake_ok
            )
            .unwrap(),
            0
        );
        assert!(
            root.path()
                .join(crate::config::CONFIG_RELATIVE_PATH)
                .is_file()
        );
        assert!(
            root.path()
                .join(".agents/ahu/agents/dev-codex.md")
                .is_file()
        );
        assert!(root.path().join(".codex/config.toml").is_file());
        assert!(
            root.path()
                .join(".agents/skills/ahu-direct-agents/SKILL.md")
                .is_file()
        );
        assert!(root.path().join(crate::context_lock::LOCK_PATH).is_file());
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("MCP stdio handshake passed"));
        assert!(output.contains("Refreshed"));
    }

    #[test]
    fn detected_harness_setup_stops_before_writes_on_mcp_handshake_failure() {
        fn configure(
            _: &mut crate::launcher::Console<'_>,
        ) -> crate::util::Result<Option<crate::config::ProjectConfig>> {
            Ok(Some(project_config()))
        }
        fn mcp_handshake_fails(_: &crate::git::Repo) -> crate::util::Result<()> {
            Err(crate::util::Error::new("injected MCP handshake failure"))
        }

        let root = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(init.success());
        let repo = crate::git::discover(root.path()).unwrap();
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        let mut console = crate::launcher::Console {
            input: &mut input,
            output: &mut output,
            interactive: true,
        };
        assert!(
            run_detected(
                &mut console,
                &repo,
                &[detected("codex")],
                configure,
                mcp_handshake_fails
            )
            .unwrap_err()
            .to_string()
            .contains("injected MCP handshake failure")
        );
        assert!(!root.path().join(".codex/config.toml").exists());
        assert!(
            !root
                .path()
                .join(crate::config::CONFIG_RELATIVE_PATH)
                .exists()
        );
        assert!(!root.path().join(".agents/ahu/agents/dev-codex.md").exists());
    }

    #[test]
    fn detected_setup_cancellation_leaves_the_project_untouched() {
        fn configure(
            _: &mut crate::launcher::Console<'_>,
        ) -> crate::util::Result<Option<crate::config::ProjectConfig>> {
            Ok(None)
        }
        fn mcp_handshake_ok(_: &crate::git::Repo) -> crate::util::Result<()> {
            Ok(())
        }

        let root = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(init.success());
        let repo = crate::git::discover(root.path()).unwrap();
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        let mut console = crate::launcher::Console {
            input: &mut input,
            output: &mut output,
            interactive: true,
        };
        assert_eq!(
            run_detected(
                &mut console,
                &repo,
                &[detected("codex")],
                configure,
                mcp_handshake_ok
            )
            .unwrap(),
            1
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        assert!(String::from_utf8(output).unwrap().contains("Cancelled"));
    }

    fn setup_repo() -> (tempfile::TempDir, crate::git::Repo) {
        let root = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(root.path())
                .status()
                .unwrap()
                .success()
        );
        let repo = crate::git::discover(root.path()).unwrap();
        (root, repo)
    }

    fn configure_project(
        _: &mut crate::launcher::Console<'_>,
    ) -> crate::util::Result<Option<crate::config::ProjectConfig>> {
        Ok(Some(project_config()))
    }

    fn keep_existing_project(
        _: &mut crate::launcher::Console<'_>,
    ) -> crate::util::Result<Option<crate::config::ProjectConfig>> {
        panic!("existing project policy must not be configured again")
    }

    fn handshake_ok(_: &crate::git::Repo) -> crate::util::Result<()> {
        Ok(())
    }

    fn handshake_unreachable(_: &crate::git::Repo) -> crate::util::Result<()> {
        panic!("invalid or cancelled model selection must not start MCP")
    }

    fn setup_with_input(
        repo: &crate::git::Repo,
        harnesses: &[Detected],
        input: &str,
        configure: fn(
            &mut crate::launcher::Console<'_>,
        ) -> crate::util::Result<Option<crate::config::ProjectConfig>>,
        handshake: fn(&crate::git::Repo) -> crate::util::Result<()>,
    ) -> (crate::util::Result<i32>, String) {
        let mut input = std::io::Cursor::new(input.as_bytes());
        let mut output = Vec::new();
        let result = run_detected(
            &mut crate::launcher::Console {
                input: &mut input,
                output: &mut output,
                interactive: true,
            },
            repo,
            harnesses,
            configure,
            handshake,
        );
        (result, String::from_utf8(output).unwrap())
    }

    #[test]
    fn existing_project_model_preference_and_explicit_choice_are_respected() {
        let models = crate::catalog::models_for("codex");
        assert!(models.len() > 1);
        for (answer, expected) in [("\n", models[1].model), (" 1 \n", models[0].model)] {
            let (root, repo) = setup_repo();
            let mut config = project_config();
            config
                .model_rankings
                .insert("codex".into(), vec![models[1].model.into()]);
            let path = crate::config::write_new(root.path(), &config).unwrap();
            let original = std::fs::read(&path).unwrap();
            let mut harness = detected("codex");
            harness.version = Some("test-version".into());
            let (result, output) = setup_with_input(
                &repo,
                &[harness],
                answer,
                keep_existing_project,
                handshake_ok,
            );
            assert_eq!(result.unwrap(), 0);
            assert!(output.contains("codex test-version (codex)"));
            assert!(output.contains("[2] (blank keeps it)"));
            assert!(output.contains("account availability is not checked"));
            let agents = crate::agent::load_all(root.path()).unwrap();
            assert_eq!(agents.len(), 1);
            assert_eq!(agents[0].manifest.model, expected);
            assert_eq!(std::fs::read(path).unwrap(), original);
            assert!(root.path().join(crate::context_lock::LOCK_PATH).is_file());
        }
    }

    #[test]
    fn unranked_harness_defaults_to_first_model_and_installs_claude_skills() {
        let (root, repo) = setup_repo();
        let (result, output) = setup_with_input(
            &repo,
            &[detected("claude-code")],
            "\n",
            configure_project,
            handshake_ok,
        );
        assert_eq!(result.unwrap(), 0);
        assert!(output.contains("Choose the model for dev-claude-code [1]"));
        let agents = crate::agent::load_all(root.path()).unwrap();
        assert_eq!(
            agents[0].manifest.model,
            crate::catalog::models_for("claude-code")[0].model
        );
        for &(name, content) in crate::mcp::BUNDLED_SKILLS {
            for directory in [".agents", ".claude"] {
                assert_eq!(
                    std::fs::read_to_string(
                        root.path()
                            .join(format!("{directory}/skills/{name}/SKILL.md"))
                    )
                    .unwrap(),
                    content
                );
            }
        }
        verify_client_configurations(root.path(), &[detected("claude-code")]).unwrap();
    }

    #[test]
    fn invalid_model_numbers_and_eof_leave_existing_policy_untouched() {
        for answer in [
            "0\n",
            "-1\n",
            "no\n",
            "999999\n",
            "184467440737095516160\n",
            "",
        ] {
            let (root, repo) = setup_repo();
            let path = crate::config::write_new(root.path(), &project_config()).unwrap();
            let original = std::fs::read(&path).unwrap();
            let (result, output) = setup_with_input(
                &repo,
                &[detected("codex")],
                answer,
                keep_existing_project,
                handshake_unreachable,
            );
            if answer.is_empty() {
                assert_eq!(result.unwrap(), 1);
                assert!(output.contains("Cancelled. Nothing was changed."));
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind(), crate::util::ErrorKind::Usage);
                assert!(
                    error
                        .to_string()
                        .contains("choose a listed model number for codex")
                );
            }
            assert_eq!(std::fs::read(path).unwrap(), original);
            assert!(!root.path().join(".agents/skills").exists());
            assert!(!root.path().join(".codex").exists());
            assert!(!root.path().join(crate::context_lock::LOCK_PATH).exists());
        }
    }

    #[test]
    fn unsupported_model_catalog_stops_before_handshake_or_writes() {
        let (root, repo) = setup_repo();
        let (result, _) = setup_with_input(
            &repo,
            &[detected("future-harness")],
            "",
            configure_project,
            handshake_unreachable,
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no currently available model for future-harness")
        );
        assert!(!root.path().join(".agents").exists());
    }

    #[test]
    fn setup_conflicts_are_detected_before_any_planned_write() {
        for directory in [false, true] {
            let (root, repo) = setup_repo();
            let path = root
                .path()
                .join(".agents/skills/ahu-direct-agents/SKILL.md");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            if directory {
                std::fs::create_dir(&path).unwrap();
            } else {
                std::fs::write(&path, "user-authored skill").unwrap();
            }
            let (result, _) = setup_with_input(
                &repo,
                &[detected("codex")],
                "",
                configure_project,
                handshake_ok,
            );
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains(if directory {
                    "cannot inspect"
                } else {
                    "will not overwrite existing project file"
                }),
                "{error}"
            );
            if directory {
                assert!(path.is_dir());
            } else {
                assert_eq!(
                    std::fs::read_to_string(path).unwrap(),
                    "user-authored skill"
                );
            }
            assert!(
                !root
                    .path()
                    .join(crate::config::CONFIG_RELATIVE_PATH)
                    .exists()
            );
            // Planning may create parent directories, but must not write files.
            assert!(!root.path().join(".codex/config.toml").exists());
            assert!(!root.path().join(".agents/ahu/agents/dev-codex.md").exists());
            assert!(!root.path().join(crate::context_lock::LOCK_PATH).exists());
        }
    }

    #[test]
    fn setup_keeps_identical_bundled_skill_and_refuses_existing_developer_agent() {
        let (root, repo) = setup_repo();
        let (name, content) = crate::mcp::BUNDLED_SKILLS[0];
        let skill = root.path().join(format!(".agents/skills/{name}/SKILL.md"));
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(&skill, content).unwrap();
        let (result, _) = setup_with_input(
            &repo,
            &[detected("codex")],
            "",
            configure_project,
            handshake_ok,
        );
        assert_eq!(result.unwrap(), 0);
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), content);
        let agent = root.path().join(".agents/ahu/agents/dev-codex.md");
        let original = std::fs::read(&agent).unwrap();
        let (result, _) = setup_with_input(
            &repo,
            &[detected("codex")],
            "\n",
            keep_existing_project,
            handshake_ok,
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("already exists at another path")
        );
        assert_eq!(std::fs::read(agent).unwrap(), original);
    }

    #[test]
    fn native_config_read_failures_do_not_add_writes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".mcp.json")).unwrap();
        std::fs::create_dir_all(root.path().join(".codex/config.toml")).unwrap();
        let mut plan = Vec::new();
        assert!(
            add_json_server(root.path(), ".mcp.json", "mcpServers", "claude", &mut plan)
                .unwrap_err()
                .to_string()
                .contains("cannot read .mcp.json")
        );
        assert!(
            add_codex_server(root.path(), &mut plan)
                .unwrap_err()
                .to_string()
                .contains("cannot read .codex/config.toml")
        );
        assert!(plan.is_empty());
    }

    #[test]
    fn codex_append_preserves_unterminated_existing_line() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".codex")).unwrap();
        let path = root.path().join(".codex/config.toml");
        std::fs::write(&path, "model = 'x'").unwrap();
        let mut plan = Vec::new();
        add_codex_server(root.path(), &mut plan).unwrap();
        let text = std::str::from_utf8(&plan[0].1).unwrap();
        assert!(text.starts_with("model = 'x'\n\n[mcp_servers.ahu]"));
        let parsed: toml::Value = toml::from_str(text).unwrap();
        assert_eq!(parsed["model"].as_str(), Some("x"));
        std::fs::write(path, text).unwrap();
        verify_client_configurations(root.path(), &[detected("codex")]).unwrap();
    }

    #[test]
    fn verification_rejects_wrong_native_commands_and_argument_shapes() {
        let root = tempfile::tempdir().unwrap();
        for (harness, path, key, entries) in [
            (
                "claude-code",
                ".mcp.json",
                "mcpServers",
                vec![
                    serde_json::json!({}),
                    serde_json::json!({"command":"other", "args":["mcp","serve"]}),
                    serde_json::json!({"command":"ahu", "args":"mcp serve"}),
                    serde_json::json!({"command":"ahu", "args":["serve","mcp"]}),
                ],
            ),
            (
                "opencode",
                "opencode.json",
                "mcp",
                vec![
                    serde_json::json!({"type":"remote", "command":["ahu","mcp","serve"]}),
                    serde_json::json!({"type":"local", "command":"ahu mcp serve"}),
                    serde_json::json!({"type":"local", "command":["other","mcp","serve"]}),
                ],
            ),
        ] {
            for entry in entries {
                std::fs::write(
                    root.path().join(path),
                    serde_json::to_vec(&serde_json::json!({key: {"ahu": entry}})).unwrap(),
                )
                .unwrap();
                assert!(
                    verify_client_configurations(root.path(), &[detected(harness)])
                        .unwrap_err()
                        .to_string()
                        .contains("did not read back the ahu MCP entry")
                );
            }
        }
        std::fs::create_dir(root.path().join(".codex")).unwrap();
        for server in [
            "command='other'\nargs=['mcp','serve']",
            "command='ahu'\nargs='mcp serve'",
            "command='ahu'\nargs=['serve','mcp']",
        ] {
            std::fs::write(
                root.path().join(".codex/config.toml"),
                format!("[mcp_servers.ahu]\n{server}\n"),
            )
            .unwrap();
            assert!(
                verify_client_configurations(root.path(), &[detected("codex")])
                    .unwrap_err()
                    .to_string()
                    .contains("did not read back as expected")
            );
        }
        verify_client_configurations(root.path(), &[detected("future-harness")]).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn setup_plan_rolls_back_when_create_new_refuses_a_dangling_symlink() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let blocked = root.path().join("blocked");
        let target = root.path().join("missing");
        std::os::unix::fs::symlink(&target, &blocked).unwrap();
        let mut input = std::io::Cursor::new(Vec::new());
        let mut output = Vec::new();
        let mut console = crate::launcher::Console {
            input: &mut input,
            output: &mut output,
            interactive: true,
        };
        let error = apply_plan(
            &mut console,
            &[
                (first.clone(), b"first".to_vec()),
                (blocked.clone(), b"blocked".to_vec()),
            ],
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot create"));
        assert!(!first.exists());
        assert!(!target.exists());
        assert_eq!(std::fs::read_link(blocked).unwrap(), target);
    }

    #[cfg(unix)]
    fn mcp_fixture(root: &Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = root.join("fixture-server");
        // Checking arguments, working directory, and both requests makes this
        // exercise the actual subprocess contract, not only response parsing.
        let prefix = r#"#!/bin/sh
[ "$#" -eq 2 ] && [ "$1" = mcp ] && [ "$2" = serve ] || exit 10
[ -f fixture-server ] || exit 11
IFS= read -r initialize || exit 12
IFS= read -r list || exit 13
case "$initialize" in
  *'"id":1,"method":"initialize"'*'"protocolVersion":"2025-11-25"'*) ;;
  *) exit 14 ;;
esac
case "$list" in
  *'"id":2,"method":"tools/list"'*) ;;
  *) exit 15 ;;
esac
"#;
        std::fs::write(&script, format!("{prefix}{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        script
    }

    #[cfg(unix)]
    #[test]
    fn mcp_handshake_accepts_initialize_and_tools_in_either_response_order() {
        for responses in [
            r#"printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{}}' '{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}'"#,
            r#"printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"ahu_agents_list"}]}}' '{"jsonrpc":"2.0","id":1,"result":{}}'"#,
        ] {
            let (root, repo) = setup_repo();
            let executable = mcp_fixture(root.path(), responses);
            super::check_mcp_server_at(&repo, &executable, std::time::Duration::from_secs(2))
                .unwrap();
            assert!(!root.path().join(".agents").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn mcp_handshake_rejects_eof_malformed_errors_and_wrong_response_ids() {
        for responses in [
            "exit 0",
            r#"printf '%s\n' 'not json' '{}'"#,
            r#"printf '%s\n' '{"id":1,"result":{}}'"#,
            r#"printf '%s\n' '{"id":1,"error":{"code":-32603}}' '{"id":2,"result":{"tools":[]}}'"#,
            r#"printf '%s\n' '{"id":3,"result":{}}' '{"id":2,"result":{"tools":[]}}'"#,
            r#"printf '%s\n' '{"id":1,"result":{}}' '{"id":3,"result":{"tools":[]}}'"#,
            r#"printf '%s\n' '{"id":1,"result":{}}' '{"id":2,"result":{"tools":{}}}'"#,
            r#"printf '%s\n' '{"id":1,"result":{}}' '{"id":2,"result":{}}'"#,
        ] {
            let (root, repo) = setup_repo();
            let executable = mcp_fixture(root.path(), responses);
            let error =
                super::check_mcp_server_at(&repo, &executable, std::time::Duration::from_secs(2))
                    .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("did not complete initialize and tools/list"),
                "{responses}: {error}"
            );
            assert!(!root.path().join(".agents").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn mcp_handshake_times_out_and_reports_spawn_failure() {
        let (root, repo) = setup_repo();
        // A shell builtin loop has no grandchildren that could retain a pipe
        // after the handshake kills and reaps the server process.
        let executable = mcp_fixture(root.path(), "while :; do :; done");
        let error =
            super::check_mcp_server_at(&repo, &executable, std::time::Duration::from_millis(50))
                .unwrap_err();
        assert_eq!(error.to_string(), "ahu MCP handshake timed out");
        std::fs::remove_file(&executable).unwrap();
        assert!(
            super::check_mcp_server_at(&repo, &executable, std::time::Duration::from_secs(2))
                .is_err()
        );
    }

    fn project_config() -> crate::config::ProjectConfig {
        crate::config::ProjectConfig {
            schema_version: crate::config::SUPPORTED_SCHEMA_VERSION,
            harness_preferences: vec!["codex".into()],
            model_selection: "project-ranked".into(),
            catalog_version: crate::catalog::CATALOG_VERSION.into(),
            harness_version_pins: Default::default(),
            model_rankings: std::collections::BTreeMap::from([(
                "codex".into(),
                vec![crate::catalog::models_for("codex")[0].model.into()],
            )]),
            knowledge: Default::default(),
            telemetry: Default::default(),
        }
    }
}
