//! Local MCP server for repository-scoped ahu inspection.
//!
//! Inspection tools use the existing repository and task resolution paths.
//! Modern clients can receive durable asynchronous inspection handles; legacy
//! initialization retains synchronous, read-only inspection.

use serde_json::{Value, json};
use std::io::{self, BufRead, Read, Write};

use crate::git::Repo;
use crate::util::{Error, Result};

#[path = "mcp_tasks.rs"]
mod task_protocol;

#[path = "mcp_decisions.rs"]
mod decisions;

const LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";
const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
const TASKS_EXTENSION: &str = "io.modelcontextprotocol/tasks";
const SERVER_NAME: &str = "ahu";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const CLIENT_CAPABILITIES_META: &str = "io.modelcontextprotocol/clientCapabilities";
const PROTOCOL_VERSION_META: &str = "io.modelcontextprotocol/protocolVersion";
const SERVER_INFO_META: &str = "io.modelcontextprotocol/serverInfo";
const MAX_FRAME_BYTES: usize = 1024 * 1024;

pub const AGENT_CONTEXT_CRITIC_SKILL: &str = "agent-context-critic";

/// Skills `ahu setup` installs in a user's repository, and the set doctor
/// verifies. `.agents/skills/` in this repository also holds skills about
/// building and releasing ahu itself; those are for ahu's own agents and are
/// deliberately not shipped to other projects.
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
        AGENT_CONTEXT_CRITIC_SKILL,
        include_str!("../.agents/skills/agent-context-critic/SKILL.md"),
    ),
];

pub fn skill_path(name: &str) -> String {
    format!(".agents/skills/{name}/SKILL.md")
}

/// Compare materialized bundled skills with the exact bytes shipped in this
/// binary. Missing and locally changed files are reported separately; this is
/// read-only and refuses symlink traversal.
pub fn verify_bundled_skills(repo_root: &std::path::Path) -> Result<(usize, usize, usize)> {
    const MAX_SKILL_BYTES: u64 = 1024 * 1024;
    let mut verified = 0;
    let mut missing = 0;
    let mut changed = 0;
    for &(name, expected) in BUNDLED_SKILLS {
        let relative = skill_path(name);
        let Some(path) = crate::util::resolve_existing_within(repo_root, &relative)? else {
            missing += 1;
            continue;
        };
        let metadata = std::fs::metadata(&path)
            .map_err(|error| Error::new(format!("cannot inspect {relative}: {error}")))?;
        if !metadata.is_file() || metadata.len() > MAX_SKILL_BYTES {
            changed += 1;
            continue;
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        std::fs::File::open(&path)
            .and_then(|file| file.take(MAX_SKILL_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|error| Error::new(format!("cannot read {relative}: {error}")))?;
        if bytes.as_slice() == expected.as_bytes() {
            verified += 1;
        } else {
            changed += 1;
        }
    }
    Ok((verified, missing, changed))
}

/// Serve newline-delimited JSON-RPC messages on stdin/stdout.
pub fn serve(repo: &Repo) -> Result<i32> {
    let telemetry = crate::config::load(&repo.root)?
        .map(|loaded| loaded.config.telemetry)
        .unwrap_or_default();
    crate::telemetry::initialize_mcp(&telemetry)?;
    let (send, receive) = std::sync::mpsc::sync_channel(32);
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        loop {
            match read_frame(&mut input) {
                Ok(Some(frame)) => {
                    if send.send(Ok(frame)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let recoverable = error.kind() == io::ErrorKind::InvalidData;
                    if send.send(Err(error)).is_err() || !recoverable {
                        break;
                    }
                }
            }
        }
    });
    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
    let mut session = task_protocol::Session::new()?;
    loop {
        match receive.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(Ok(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                let request: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(error) => {
                        let mut span = crate::telemetry::mcp_request_span(&Value::Null);
                        let failure = rpc_error(&Value::Null, -32700, error.to_string());
                        crate::telemetry::finish_mcp_request_span(
                            &mut span,
                            &Value::Null,
                            Some(&failure),
                        );
                        write_response(&mut stdout, &failure)?;
                        continue;
                    }
                };
                let mut span = crate::telemetry::mcp_request_span(&request);
                let mut response = validate_envelope(&request);
                if response.is_none() && request.get("id").is_some() {
                    response = validate_request(&request, &mut session);
                    if response.is_none() {
                        response = handle(repo, &request, &mut session);
                        // Work starts only after the durable handle has been flushed.
                        session.start_worker(repo);
                    }
                }
                crate::telemetry::finish_mcp_request_span(&mut span, &request, response.as_ref());
                if let Some(response) = response {
                    write_response(&mut stdout, &response)?;
                }
            }
            Ok(Err(error)) => {
                crate::telemetry::mcp_transport_error();
                let failure = rpc_error(&Value::Null, -32600, error.to_string());
                write_response(&mut stdout, &failure)?;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        for notification in session.notifications(repo)? {
            write_response(&mut stdout, &notification)?;
        }
    }
    crate::telemetry::mcp_session_summary(telemetry.enabled);
    Ok(0)
}

fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut frame = Vec::new();
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                String::from_utf8(frame)
                    .map(Some)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
            };
        }
        let take = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |position| position + 1);
        let terminated = take < buffer.len() || buffer[take - 1] == b'\n';
        if frame.len() + take > MAX_FRAME_BYTES {
            reader.consume(take);
            if !terminated {
                discard_frame(reader)?;
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("MCP frame exceeds {MAX_FRAME_BYTES} bytes"),
            ));
        }
        frame.extend_from_slice(&buffer[..take]);
        reader.consume(take);
        if terminated {
            return String::from_utf8(frame)
                .map(Some)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
        }
    }
}

fn discard_frame(reader: &mut impl BufRead) -> io::Result<()> {
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(());
        }
        let take = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |position| position + 1);
        let terminated = take < buffer.len() || buffer[take - 1] == b'\n';
        reader.consume(take);
        if terminated {
            return Ok(());
        }
    }
}

// MCP request IDs are strings or integers; null and fractional IDs are invalid.
fn validate_envelope(request: &Value) -> Option<Value> {
    let valid_id = |id: &Value| id.is_string() || id.is_i64() || id.is_u64();
    if !request.is_object()
        || request["jsonrpc"] != "2.0"
        || !request["method"].is_string()
        || request.get("id").is_some_and(|id| !valid_id(id))
        || request.get("result").is_some()
        || request.get("error").is_some()
    {
        return Some(rpc_error(&Value::Null, -32600, "invalid JSON-RPC request"));
    }
    None
}

fn validate_request(request: &Value, session: &mut task_protocol::Session) -> Option<Value> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str)?;
    if request
        .get("params")
        .is_some_and(|params| !params.is_object())
    {
        return Some(rpc_error(&id, -32602, "params must be an object"));
    }
    if method.starts_with("notifications/") {
        return Some(rpc_error(
            &id,
            -32601,
            format!("method not found: {method}"),
        ));
    }
    if method == "initialize" {
        if session.modern() {
            return Some(rpc_error(&id, -32601, "method not found: initialize"));
        }
        return None;
    }
    if session.legacy() {
        if method == "server/discover" {
            return Some(rpc_error(&id, -32601, "method not found: server/discover"));
        }
        return None;
    }
    let meta = request.get("params").and_then(|params| params.get("_meta"));
    let version = meta.and_then(|meta| meta.get(PROTOCOL_VERSION_META));
    let capabilities = meta.and_then(|meta| meta.get(CLIENT_CAPABILITIES_META));
    if version.and_then(Value::as_str) != Some(MODERN_PROTOCOL_VERSION)
        || !capabilities.is_some_and(Value::is_object)
    {
        let mut error = rpc_error(
            &id,
            -32022,
            "Unsupported protocol version or missing request metadata",
        );
        error["error"]["data"] = json!({
            "supported": [MODERN_PROTOCOL_VERSION, LEGACY_PROTOCOL_VERSION],
            "requested": version.cloned().unwrap_or(Value::Null),
        });
        return Some(error);
    }
    if method == "server/discover"
        && request["params"]
            .as_object()
            .is_some_and(|params| params.keys().any(|key| key != "_meta"))
    {
        return Some(rpc_error(
            &id,
            -32602,
            "server/discover accepts only standard request metadata",
        ));
    }
    session.mark_modern();
    None
}

fn write_response(output: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn response(id: &Value, result: Value) -> Value {
    let mut result = result;
    if let Some(object) = result.as_object_mut() {
        object.entry("_meta").or_insert_with(
            || json!({SERVER_INFO_META: {"name": SERVER_NAME, "version": SERVER_VERSION}}),
        );
    }
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn rpc_error(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message.into()}})
}

fn handle(repo: &Repo, request: &Value, session: &mut task_protocol::Session) -> Option<Value> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str)?;
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    if let Some(result) = session.handle(repo, &id, method, &params) {
        return Some(result);
    }
    match method {
        "initialize" => Some(response(
            &id,
            json!({
                "protocolVersion": legacy_protocol_version(&params),
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
                "instructions": "ahu exposes repository-scoped agent and task inspection. Task identity, ownership and permissions remain controlled by ahu."
            }),
        )),
        "ping" => {
            let result = if session.modern() {
                json!({"resultType":"complete"})
            } else {
                json!({})
            };
            Some(response(&id, result))
        }
        "server/discover" => Some(response(
            &id,
            json!({
                "resultType": "complete",
                "supportedVersions": [MODERN_PROTOCOL_VERSION, LEGACY_PROTOCOL_VERSION],
                "capabilities": {"tools": {}, "extensions": {TASKS_EXTENSION: {}}},
                "_meta": {SERVER_INFO_META: {"name": SERVER_NAME, "version": SERVER_VERSION}},
                "instructions": "ahu exposes repository-scoped agent and task inspection.",
                "ttlMs": 0,
                "cacheScope": "private",
            }),
        )),
        "tools/list" => {
            let mut result = json!({"tools": tools()});
            if session.modern() {
                result["resultType"] = json!("complete");
            }
            Some(response(&id, result))
        }
        "tools/call" => Some(call_response(repo, &id, &params, session.modern())),
        _ => Some(rpc_error(
            &id,
            -32601,
            format!("method not found: {method}"),
        )),
    }
}

fn legacy_protocol_version(params: &Value) -> &'static str {
    match params.get("protocolVersion").and_then(Value::as_str) {
        Some("2025-11-25") => "2025-11-25",
        Some("2025-06-18") => "2025-06-18",
        Some("2025-03-26") => "2025-03-26",
        Some("2024-11-05") => "2024-11-05",
        _ => LEGACY_PROTOCOL_VERSION,
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
        decisions::tool_definition(),
    ]
}

/// Every read-only tool name an ahu MCP session can expose.
///
/// Evaluation case tool expectations and evaluation record validation are both
/// bounded by this list, so neither can name a tool that does not exist.
pub const TOOL_NAMES: [&str; 4] = [
    "ahu_agents_list",
    "ahu_tasks_list",
    "ahu_task_get",
    "ahu_typed_decide",
];

/// SHA-256 over the served tool definitions, descriptions and schemas included.
///
/// A comparison between agents is only a comparison of the agents when the
/// tools they were offered were the same, so the definitions are fingerprinted
/// rather than assumed stable across ahu builds.
pub fn tool_definitions_digest() -> String {
    crate::util::digest_bytes(&serde_json::to_vec(&tools()).unwrap_or_default())
}

fn validate_tool_call(params: &Value, inspection_adapter: bool) -> Result<()> {
    let name = params["name"]
        .as_str()
        .ok_or_else(|| Error::new("tools/call requires a string params.name"))?;
    let selector = match name {
        "ahu_agents_list" | "ahu_tasks_list" | "ahu_typed_decide" => false,
        "ahu_task_get" => true,
        "ahu_task_inspect" if inspection_adapter => true,
        _ => return Err(Error::new(format!("unknown ahu MCP tool: {name}"))),
    };
    let empty = json!({});
    let arguments = params.get("arguments").unwrap_or(&empty);
    if name == "ahu_typed_decide" {
        return decisions::validate_arguments(arguments);
    }
    let object = arguments
        .as_object()
        .ok_or_else(|| Error::new("arguments must be an object"))?;
    if serde_json::to_vec(arguments)?.len() > 8192
        || object.keys().any(|key| !selector || key != "task")
    {
        return Err(Error::new("invalid inspection arguments"));
    }
    match object.get("task") {
        Some(value)
            if !value
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 256) =>
        {
            return Err(Error::new("invalid task selector"));
        }
        None if name == "ahu_task_get" => {
            return Err(Error::new("ahu_task_get requires arguments.task"));
        }
        _ => {}
    }
    Ok(())
}

fn call_response(repo: &Repo, id: &Value, params: &Value, modern: bool) -> Value {
    if let Err(error) = validate_tool_call(params, false) {
        return rpc_error(id, -32602, error.to_string());
    }
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
        "ahu_typed_decide" => decisions::call(&arguments),
        _ => Err(Error::new(format!("unknown ahu MCP tool: {name}"))),
    };
    match result {
        Ok(value) => {
            let mut result = json!({"content":[{"type":"text","text":serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into())}],"structuredContent":value});
            if modern {
                result["resultType"] = json!("complete");
            }
            response(id, result)
        }
        Err(error) => {
            let mut result =
                json!({"isError":true,"content":[{"type":"text","text":error.to_string()}]});
            if modern {
                result["resultType"] = json!("complete");
            }
            response(id, result)
        }
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
    let (dir, record) = crate::commands::inspect_task(repo, input)?;
    let listing = crate::task::list(repo)?;
    let workspaces = crate::commands::liveness_workspaces(&listing.records);
    crate::commands::task_summary(&dir, &record, workspaces.as_ref())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Cursor;

    use super::{
        AGENT_CONTEXT_CRITIC_SKILL, BUNDLED_SKILLS, MAX_FRAME_BYTES, TOOL_NAMES, read_frame,
        skill_path, tool_definitions_digest, tools, validate_envelope, validate_tool_call,
        verify_bundled_skills,
    };

    #[test]
    fn frame_reader_accepts_exact_limit_and_recovers_after_oversize() {
        let mut exact = vec![b'x'; MAX_FRAME_BYTES - 1];
        exact.push(b'\n');
        let mut exact_reader = Cursor::new(exact);
        assert_eq!(
            read_frame(&mut exact_reader).unwrap().unwrap().len(),
            MAX_FRAME_BYTES
        );

        let mut oversized = vec![b'x'; MAX_FRAME_BYTES + 1];
        oversized.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\"}\n");
        let mut reader = Cursor::new(oversized);
        assert!(read_frame(&mut reader).is_err());
        assert_eq!(
            read_frame(&mut reader).unwrap().as_deref(),
            Some("{\"jsonrpc\":\"2.0\"}\n")
        );

        let mut unterminated_oversize = Cursor::new(vec![b'x'; MAX_FRAME_BYTES + 1]);
        assert!(read_frame(&mut unterminated_oversize).is_err());
        assert!(read_frame(&mut unterminated_oversize).unwrap().is_none());

        let streamed = Cursor::new(vec![b'x'; MAX_FRAME_BYTES + 8192]);
        let mut streamed = std::io::BufReader::with_capacity(4096, streamed);
        assert!(read_frame(&mut streamed).is_err());
        assert!(read_frame(&mut streamed).unwrap().is_none());
    }

    #[test]
    fn frame_reader_rejects_invalid_utf8_and_returns_an_unterminated_final_frame() {
        let mut invalid = Cursor::new(vec![0xff, b'\n']);
        assert_eq!(
            read_frame(&mut invalid).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        let mut final_frame = Cursor::new(b"{}".to_vec());
        assert_eq!(read_frame(&mut final_frame).unwrap().as_deref(), Some("{}"));
        assert!(read_frame(&mut final_frame).unwrap().is_none());
    }

    #[test]
    fn envelope_and_tool_argument_validation_reject_malformed_requests() {
        for request in [
            serde_json::json!(null),
            serde_json::json!({"jsonrpc":"1.0","method":"ping","id":1}),
            serde_json::json!({"jsonrpc":"2.0","method":1,"id":1}),
            serde_json::json!({"jsonrpc":"2.0","method":"ping","id":null}),
            serde_json::json!({"jsonrpc":"2.0","method":"ping","id":1.5}),
            serde_json::json!({"jsonrpc":"2.0","method":"ping","id":1,"result":{}}),
        ] {
            assert!(validate_envelope(&request).is_some(), "accepted {request}");
        }
        assert!(
            validate_envelope(&serde_json::json!({"jsonrpc":"2.0","method":"ping","id":"ok"}))
                .is_none()
        );

        for params in [
            serde_json::json!({}),
            serde_json::json!({"name":"ahu_task_get"}),
            serde_json::json!({"name":"ahu_task_get","arguments":{"task":""}}),
            serde_json::json!({"name":"ahu_task_get","arguments":{"task":"x","extra":true}}),
            serde_json::json!({"name":"not_a_tool","arguments":{}}),
        ] {
            assert!(
                validate_tool_call(&params, false).is_err(),
                "accepted {params}"
            );
        }
        assert!(
            validate_tool_call(
                &serde_json::json!({"name":"ahu_task_get","arguments":{"task":"t-1"}}),
                false
            )
            .is_ok()
        );
        assert!(
            validate_tool_call(
                &serde_json::json!({"name":"ahu_task_inspect","arguments":{"task":"t-1"}}),
                true
            )
            .is_ok()
        );
        assert!(
            validate_tool_call(
                &serde_json::json!({"name":"ahu_task_inspect","arguments":{"task":"t-1"}}),
                false
            )
            .is_err()
        );
    }

    #[test]
    fn read_only_tools_are_explicit_and_bounded() {
        let names: Vec<_> = tools()
            .into_iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "ahu_agents_list",
                "ahu_tasks_list",
                "ahu_task_get",
                "ahu_typed_decide"
            ]
        );
        // The exported name list is what bounds evaluation tool expectations,
        // so it has to stay the served set rather than a copy of it.
        assert_eq!(names, TOOL_NAMES);
        let digest = tool_definitions_digest();
        assert_eq!(digest.len(), 64);
        assert_eq!(digest, tool_definitions_digest());
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

    #[test]
    fn the_bundle_ships_only_skills_meant_for_other_repositories() {
        // `ahu-architecture` and `release` stay in this repository's skill tree
        // for ahu's own agents; setup and doctor must not carry them into a
        // user's repository.
        let bundled: Vec<&str> = BUNDLED_SKILLS.iter().map(|&(name, _)| name).collect();
        assert_eq!(
            bundled,
            vec![
                "discover-requirements",
                "direct-agents",
                AGENT_CONTEXT_CRITIC_SKILL
            ]
        );
    }

    #[test]
    fn skill_verification_distinguishes_missing_verified_and_changed_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        assert_eq!(
            verify_bundled_skills(root).unwrap(),
            (0, BUNDLED_SKILLS.len(), 0)
        );

        let (name, content) = BUNDLED_SKILLS[0];
        let path = root.join(skill_path(name));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        assert_eq!(
            verify_bundled_skills(root).unwrap(),
            (1, BUNDLED_SKILLS.len() - 1, 0)
        );

        std::fs::write(path, "locally changed\n").unwrap();
        assert_eq!(
            verify_bundled_skills(root).unwrap(),
            (0, BUNDLED_SKILLS.len() - 1, 1)
        );
    }

    #[test]
    fn skill_verification_marks_non_file_entries_as_changed() {
        let temp = tempfile::tempdir().unwrap();
        let (name, _) = BUNDLED_SKILLS[0];
        let path = temp.path().join(skill_path(name));
        std::fs::create_dir_all(path).unwrap();
        let (_, _, changed) = verify_bundled_skills(temp.path()).unwrap();
        assert_eq!(changed, 1);
    }

    #[cfg(unix)]
    #[test]
    fn skill_verification_refuses_symlinked_sources() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (name, content) = BUNDLED_SKILLS[0];
        let path = root.join(skill_path(name));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let outside = temp.path().join("outside");
        std::fs::write(&outside, content).unwrap();
        std::os::unix::fs::symlink(outside, &path).unwrap();
        assert!(
            verify_bundled_skills(root)
                .unwrap_err()
                .to_string()
                .contains("symlink")
        );
    }
}
