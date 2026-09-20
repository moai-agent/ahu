//! Local MCP server for repository-scoped ahu inspection.
//!
//! The first MCP surface is deliberately read-only. It exposes the repository
//! agent registry and ahu task records without creating a second identity or
//! storage model. Mutating task tools will build on these same repository and
//! task resolution paths.

use serde_json::{Value, json};
use std::io::{BufRead, Write};

use crate::git::Repo;
use crate::util::{Error, Result};

const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "ahu";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Serve newline-delimited JSON-RPC messages on stdin/stdout.
pub fn serve(repo: &Repo) -> Result<i32> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                write_response(
                    &mut stdout,
                    &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":error.to_string()}}),
                )?;
                continue;
            }
        };
        if let Some(response) = handle(repo, &request) {
            write_response(&mut stdout, &response)?;
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

fn handle(repo: &Repo, request: &Value) -> Option<Value> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str)?;
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
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
            json!({
                "name": format!("@{}", agent.manifest.name),
                "version": agent.manifest.version,
                "description": agent.manifest.description,
                "harness": agent.manifest.harness,
                "model": agent.manifest.model,
                "permissions": agent.manifest.permissions.as_str(),
                "status": agent.manifest.status.as_str(),
                "source": agent.source_path.strip_prefix(&repo.root).unwrap_or(&agent.source_path),
                "identity_digest": agent.identity_digest(),
            })
        })
        .collect::<Vec<_>>();
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
    let files = [(
        "discover-requirements",
        include_str!("../.agents/skills/discover-requirements/SKILL.md"),
    )];
    for (name, content) in files {
        for root in [".agents/skills", ".claude/skills"] {
            let path = repo.root.join(root).join(name).join("SKILL.md");
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
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::tools;

    #[test]
    fn read_only_tools_are_explicit_and_bounded() {
        let names: Vec<_> = tools()
            .into_iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["ahu_agents_list", "ahu_tasks_list", "ahu_task_get"]);
    }
}
