//! Local MCP server for repository-scoped ahu inspection.
//!
//! Inspection tools use the existing repository and task resolution paths.
//! Modern clients can receive durable asynchronous inspection handles; legacy
//! initialization retains synchronous, read-only inspection.

use serde_json::{Value, json};
use std::io::{BufRead, Write};

use crate::git::Repo;
use crate::util::{Error, Result};

#[path = "mcp_tasks.rs"]
mod task_protocol;

const PROTOCOL_VERSION: &str = "2025-06-18";
const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
const TASKS_EXTENSION: &str = "io.modelcontextprotocol/tasks";
const SERVER_NAME: &str = "ahu";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const CONTEXT_HYGIENE_SKILL: &str = "context-hygiene";

pub const BUNDLED_SKILLS: &[(&str, &str)] = &[
    (
        "discover-requirements",
        include_str!("../.agents/skills/discover-requirements/SKILL.md"),
    ),
    (
        "direct-agents",
        include_str!("../.agents/skills/direct-agents/SKILL.md"),
    ),
    (
        CONTEXT_HYGIENE_SKILL,
        include_str!("../.agents/skills/context-hygiene/SKILL.md"),
    ),
    (
        "ahu-architecture",
        include_str!("../.agents/skills/ahu-architecture/SKILL.md"),
    ),
];

pub fn skill_path(name: &str) -> String {
    format!(".agents/skills/{name}/SKILL.md")
}

/// Serve newline-delimited JSON-RPC messages on stdin/stdout.
pub fn serve(repo: &Repo) -> Result<i32> {
    let (send, receive) = std::sync::mpsc::sync_channel(32);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
    let mut session = task_protocol::Session::new()?;
    loop {
        match receive.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(line) => {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let request: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(error) => {
                        write_response(
                            &mut stdout,
                            &rpc_error(&Value::Null, -32700, error.to_string()),
                        )?;
                        continue;
                    }
                };
                if let Some(response) = handle(repo, &request, &mut session) {
                    write_response(&mut stdout, &response)?;
                }
                // Work starts only after the durable handle has been flushed.
                session.start_worker(repo);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        for notification in session.notifications(repo)? {
            write_response(&mut stdout, &notification)?;
        }
    }
    Ok(0)
}

fn write_response(output: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn rpc_error(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message.into()}})
}

fn handle(repo: &Repo, request: &Value, session: &mut task_protocol::Session) -> Option<Value> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str)?;
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    if request.get("id").is_none() && method.starts_with("notifications/") {
        return None;
    }
    if let Some(result) = session.handle(repo, &id, method, &params) {
        return Some(result);
    }
    match method {
        "notifications/initialized" | "notifications/cancelled" => None,
        "initialize" => Some(response(
            &id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
                "instructions": "ahu exposes repository-scoped agent and task inspection. Task identity, ownership and permissions remain controlled by ahu."
            }),
        )),
        "ping" => Some(response(&id, json!({}))),
        "server/discover" => Some(response(
            &id,
            json!({
                "protocolVersion": MODERN_PROTOCOL_VERSION,
                "capabilities": {"extensions": {TASKS_EXTENSION: {}}},
                "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
            }),
        )),
        "tools/list" => Some(response(&id, json!({"tools": tools()}))),
        "tools/call" => Some(call_response(repo, &id, &params)),
        _ => Some(rpc_error(
            &id,
            -32601,
            format!("method not found: {method}"),
        )),
    }
}

fn tools() -> Vec<Value> {
    vec![
        json!({
            "name":"ahu_agents_list",
            "description":"List launchable ahu agents registered in this repository.",
            "inputSchema":{"type":"object","properties":{},"additionalProperties":false}
        }),
        json!({
            "name":"ahu_tasks_list",
            "description":"List ahu tasks belonging to this repository, including canonical IDs and verified @name handles.",
            "inputSchema":{"type":"object","properties":{},"additionalProperties":false}
        }),
        json!({
            "name":"ahu_task_get",
            "description":"Inspect one ahu task by canonical ID, unique prefix, or exact @name handle.",
            "inputSchema":{"type":"object","properties":{"task":{"type":"string"}},"required":["task"],"additionalProperties":false}
        }),
    ]
}

fn call_response(repo: &Repo, id: &Value, params: &Value) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return rpc_error(id, -32602, "tools/call requires a string params.name");
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = match name {
        "ahu_agents_list" => agents(repo),
        "ahu_tasks_list" => tasks(repo),
        "ahu_task_get" => task_get(repo, &arguments),
        _ => Err(Error::new(format!("unknown ahu MCP tool: {name}"))),
    };
    match result {
        Ok(value) => response(
            id,
            json!({"content":[{"type":"text","text":serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into())}],"structuredContent":value}),
        ),
        Err(error) => rpc_error(id, -32000, error.to_string()),
    }
}

fn agents(repo: &Repo) -> Result<Value> {
    let rows = crate::agent::load_all(&repo.root)?
        .into_iter()
        .map(|agent| {
            let agent_ref = crate::agent_ref::ensure(repo, &agent)?;
            Ok::<_, crate::util::Error>(json!({
                "name": format!("@{}", agent.manifest.name),
                "version": agent.manifest.version,
                "description": agent.manifest.description,
                "harness": agent.manifest.harness,
                "model": agent.manifest.model,
                "permissions": agent.manifest.permissions.as_str(),
                "status": agent.manifest.status.as_str(),
                "source": agent.source_path.strip_prefix(&repo.root).unwrap_or(&agent.source_path),
                "identity_digest": agent.identity_digest(),
                "agent_ref": agent_ref,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"schema_version":1,"repository":repo.identity(),"agents":rows}))
}

fn tasks(repo: &Repo) -> Result<Value> {
    let listing = crate::task::list(repo)?;
    let workspaces = crate::commands::liveness_workspaces(&listing.records);
    let rows = listing
        .records
        .iter()
        .map(|(dir, record)| crate::commands::task_summary(dir, record, workspaces.as_ref()))
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "schema_version":1,
        "repository":repo.identity(),
        "tasks":rows,
        "unreadable":listing.unreadable.iter().map(|row| json!({"task_id":row.task_id,"reason":row.reason})).collect::<Vec<_>>(),
        "notes":listing.notes,
    }))
}

fn task_get(repo: &Repo, arguments: &Value) -> Result<Value> {
    let input = arguments
        .get("task")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("ahu_task_get requires arguments.task"))?;
    let id = crate::task_ref::resolve(repo, input)?;
    let listing = crate::task::list(repo)?;
    let (dir, record) = listing
        .records
        .iter()
        .find(|(_, record)| record.task_id == id)
        .ok_or_else(|| Error::new(format!("no task matching {input:?}")))?;
    let workspaces = crate::commands::liveness_workspaces(&listing.records);
    crate::commands::task_summary(dir, record, workspaces.as_ref())
}

/// Copy the bundled skill set into a repository for review and commit.
pub fn setup(repo: &Repo) -> Result<i32> {
    for &(name, content) in BUNDLED_SKILLS {
        let path = crate::util::resolve_within(
            &repo.root,
            &format!(".agents/skills/{name}/SKILL.md"),
            true,
        )?;
        if let Ok(existing) = std::fs::read_to_string(&path) {
            if existing != content {
                return Err(Error::new(format!(
                    "refusing to overwrite changed skill {}",
                    path.display()
                )));
            }
            continue;
        }
        std::fs::create_dir_all(path.parent().expect("skill path has parent"))?;
        std::fs::write(&path, content)?;
        println!("Wrote {}", path.display());
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{BUNDLED_SKILLS, skill_path, tools};

    #[test]
    fn read_only_tools_are_explicit_and_bounded() {
        let names: Vec<_> = tools()
            .into_iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["ahu_agents_list", "ahu_tasks_list", "ahu_task_get"]);
    }

    #[test]
    fn bundled_skills_match_the_skill_tree_contract() {
        let mut seen = std::collections::BTreeSet::new();
        for &(name, content) in BUNDLED_SKILLS {
            assert!(seen.insert(name), "duplicate bundled skill name: {name}");
            assert_eq!(skill_path(name), format!(".agents/skills/{name}/SKILL.md"));
            assert!(!name.is_empty());
            assert!(
                name.chars().next().unwrap().is_ascii_lowercase()
                    || name.chars().next().unwrap().is_ascii_digit()
            );
            assert!(
                name.chars().last().unwrap().is_ascii_lowercase()
                    || name.chars().last().unwrap().is_ascii_digit()
            );
            assert!(
                name.chars()
                    .all(|c| { c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' })
            );
            let lines: Vec<&str> = content.split('\n').collect();
            assert_eq!(
                lines.first(),
                Some(&"---"),
                "missing frontmatter opener: {name}"
            );
            let mut keys: BTreeMap<&str, &str> = BTreeMap::new();
            let mut closed = false;
            let mut body = 0;
            for line in lines.iter().skip(1) {
                if closed {
                    if !line.is_empty() {
                        body += 1;
                    }
                    continue;
                }
                if *line == "---" {
                    closed = true;
                    continue;
                }
                if let Some(value) = line.strip_prefix("name: ") {
                    assert_eq!(value, name, "frontmatter name mismatch for {name}");
                    assert!(
                        keys.insert("name", value).is_none(),
                        "duplicate name key: {name}"
                    );
                } else if let Some(value) = line.strip_prefix("description: ") {
                    assert!(
                        keys.insert("description", value).is_none(),
                        "duplicate description key: {name}"
                    );
                } else {
                    panic!("unsupported frontmatter line in {name}: {line:?}");
                }
            }
            assert!(closed, "unterminated frontmatter: {name}");
            assert_eq!(
                keys.len(),
                2,
                "expected exactly name and description for {name}"
            );
            assert!(!keys["description"].is_empty(), "empty description: {name}");
            assert!(body > 0, "empty skill body: {name}");
        }
    }
}
