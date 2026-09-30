mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Stdio;

#[cfg(unix)]
#[test]
fn mcp_sigterm_flushes_a_complete_session_without_stdin_eof() {
    mcp_shutdown_signal_fixture(libc::SIGTERM);
}

#[cfg(unix)]
#[test]
fn mcp_sigint_flushes_a_complete_session_without_stdin_eof() {
    mcp_shutdown_signal_fixture(libc::SIGINT);
}

#[cfg(unix)]
fn mcp_shutdown_signal_fixture(signal: libc::c_int) {
    let repo = common::TestRepo::new();
    let receiver = ahu::eval_otel::Receiver::start().unwrap();
    let mut child = common::ahu()
        .args(["mcp", "serve"])
        .current_dir(repo.path())
        .env("AHU_EVAL_OTEL_ENDPOINT", receiver.endpoint())
        .env(
            "AHU_MCP_RESOURCE_ATTRIBUTES",
            "ahu.task.id=signal-fixture,ahu.task.attempt=1",
        )
        .env_remove("OTEL_RESOURCE_ATTRIBUTES")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    writeln!(input, "{}", serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}})).unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"],
        1
    );
    // The harness may close its read end before terminating the server.
    drop(output);
    // This PID belongs to the unreaped child created above. Keep stdin open:
    // real harnesses can terminate MCP children before closing their pipes.
    assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("MCP shutdown timed out");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(status.success(), "{status}");
    let observed = receiver
        .task("signal-fixture", 1)
        .expect("missing session summary");
    assert_eq!(observed.coverage().as_str(), "complete_session");
    assert_eq!(observed.session_summaries, 1);
    assert_eq!(observed.tool_calls, 0);
}

#[test]
fn typed_decision_tool_posts_model_neutral_contract_to_local_service() {
    let repo = common::TestRepo::new();
    let receiver = ahu::eval_otel::Receiver::start().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/v1/decisions", listener.local_addr().unwrap());
    let service = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            headers.push_str(&line);
        }
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).unwrap();
        let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(request["questions"]["route"]["type"], "choice");
        let response = serde_json::json!({
            "answers":{"route":{"value":"billing","confidence":0.91}},
            "service":{
                "backend":"ollama",
                "model":"fixture-model",
                "prompt_tokens":12,
                "generated_tokens":3,
                "duration_ms":25
            }
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        )
        .unwrap();
        request
    });

    let mut child = common::ahu()
        .args(["mcp", "serve"])
        .current_dir(repo.path())
        .env("AHU_DECISION_URL", endpoint)
        .env("AHU_EVAL_OTEL_ENDPOINT", receiver.endpoint())
        .env(
            "OTEL_RESOURCE_ATTRIBUTES",
            "ahu.task.id=decision-fixture,ahu.task.attempt=1",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let meta = serde_json::json!({
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientCapabilities":{
            "extensions":{"io.modelcontextprotocol/tasks":{}}
        }
    });
    writeln!(
        input,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":meta}
        })
    )
    .unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"ahu_typed_decide",
                "arguments":{
                    "state":{"body":"Please refund the duplicate charge."},
                    "questions":{"route":{
                        "type":"choice","instructions":"Which team handles this?",
                        "telemetry_key":"routing",
                        "options":{"billing":"Invoices and refunds","other":"Everything else"}
                    }}
                },
                "_meta":{
                    "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities":{
                        "extensions":{"io.modelcontextprotocol/tasks":{}}
                    }
                }
            }
        })
    )
    .unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let rows: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[1]["result"]["resultType"], "complete");
    assert_eq!(
        rows[1]["result"]["structuredContent"]["answers"]["route"]["value"],
        "billing"
    );
    let request = service.join().unwrap();
    assert_eq!(
        request["state"]["body"],
        "Please refund the duplicate charge."
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let telemetry = loop {
        if let Some(telemetry) = receiver.task("decision-fixture", 1)
            && telemetry.session_summaries == 1
        {
            break telemetry;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "MCP OTel spans were not exported"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(
        telemetry.coverage(),
        ahu::eval_otel::Coverage::CompleteSession
    );
    assert!(telemetry.mcp_observed);
    assert_eq!(telemetry.tool_calls, 1);
    assert_eq!(telemetry.typed_decision_calls, 1);
    assert_eq!(telemetry.tool_calls_by_name["ahu_typed_decide"], 1);
}

#[test]
fn stdio_server_negotiates_and_lists_repository_agents_and_tasks() {
    let repo = common::TestRepo::new();
    repo.add_agent("reviewer", "1.0.0", "claude-opus-5");
    let mut child = common::ahu()
        .args(["mcp", "serve"])
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for request in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ahu_agents_list","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"ahu_tasks_list","arguments":{}}}"#,
    ] {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let rows: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["result"]["serverInfo"]["name"], "ahu");
    assert_eq!(rows[1]["result"]["tools"].as_array().unwrap().len(), 5);
    assert_eq!(
        rows[2]["result"]["structuredContent"]["agents"][0]["name"],
        "@reviewer"
    );
    assert_eq!(
        rows[3]["result"]["structuredContent"]["tasks"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn modern_stdio_conformance_requires_metadata_and_uses_discovery_shape() {
    let repo = common::TestRepo::new();
    let mut child = common::ahu()
        .args(["mcp", "serve"])
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"tools/list","params":{}
        })
    )
    .unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"server/discover",
            "params":{"_meta":{
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientCapabilities":{}
            }}
        })
    )
    .unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":3,"method":"tools/list",
            "params":{"_meta":{
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientCapabilities":{}
            }}
        })
    )
    .unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":4,"method":"initialize","params":{}
        })
    )
    .unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let rows: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["error"]["code"], -32022);
    assert_eq!(rows[0]["error"]["data"]["supported"][0], "2026-07-28");
    assert_eq!(rows[1]["result"]["resultType"], "complete");
    assert_eq!(rows[1]["result"]["supportedVersions"][0], "2026-07-28");
    assert_eq!(
        rows[1]["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "ahu"
    );
    assert_eq!(rows[2]["result"]["resultType"], "complete");
    assert_eq!(rows[3]["error"]["code"], -32601);
}

#[test]
fn legacy_initialize_echoes_a_supported_handshake_version() {
    let repo = common::TestRepo::new();
    let mut child = common::ahu()
        .args(["mcp", "serve"])
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}
        })
    )
    .unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let line = output.stdout.split(|byte| *byte == b'\n').next().unwrap();
    let response: serde_json::Value = serde_json::from_slice(line).unwrap();
    assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn legacy_initialize_echoes_each_protocol_version_still_supported_by_ahu() {
    let repo = common::TestRepo::new();
    let versions = ["2025-06-18", "2025-03-26", "2024-11-05"];
    let requests = versions
        .iter()
        .enumerate()
        .map(|(index, version)| {
            json!({"jsonrpc":"2.0","id":index + 1,"method":"initialize","params":{"protocolVersion":version}}).to_string()
        })
        .collect::<Vec<_>>();
    let rows = exchange(&repo, &requests);
    for (row, version) in rows.iter().zip(versions) {
        assert_eq!(row["result"]["protocolVersion"], version);
    }
}

#[test]
fn mcp_setup_subcommand_is_replaced_by_the_single_setup_command() {
    let repo = common::TestRepo::new();
    let output = common::ahu()
        .args(["mcp", "setup"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("expected ahu mcp serve"));
    assert!(!repo.path().join(".agents/skills").exists());
}

#[test]
fn tasks_extension_returns_a_durable_handle_and_rejects_legacy_calls() {
    let repo = common::TestRepo::new();
    let mut child = common::ahu()
        .args(["mcp", "serve"])
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"server/discover",
            "params":{"_meta":{
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientCapabilities":{}
            }}
        })
    )
    .unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{
                "name":"ahu_agents_list","arguments":{},
                "_meta":{
                    "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities":{
                    "extensions":{"io.modelcontextprotocol/tasks":{}}
                }}
            }
        })
    )
    .unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":3,"method":"tasks/get",
            "params":{
                "taskId":"not-a-task",
                "_meta":{
                    "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities":{}
                }
            }
        })
    )
    .unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let rows: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        rows[0]["result"]["capabilities"]["extensions"]["io.modelcontextprotocol/tasks"],
        serde_json::json!({})
    );
    assert_eq!(rows[1]["result"]["resultType"], "task");
    assert!(rows[1]["result"]["taskId"].as_str().unwrap().contains('-'));
    assert_eq!(rows[1]["result"]["status"], "working");
    assert_eq!(rows[2]["error"]["code"], -32021);
    let task_files = std::fs::read_dir(
        repo.path()
            .join(".ahu/state")
            .join("repos")
            .join(ahu::git::discover(repo.path()).unwrap().identity())
            .join("mcp/tasks"),
    )
    .unwrap()
    .count();
    assert_eq!(task_files, 1);
}

use serde_json::{Value, json};
use std::time::{Duration, Instant};

const EXT: &str = "io.modelcontextprotocol/tasks";
fn modern(mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientCapabilities":{
        "extensions":{EXT:{}}, "elicitation":{"form":{}}
    }});
    params
}
fn modern_without_tasks(mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientCapabilities":{}
    });
    params
}
struct Client {
    child: std::process::Child,
    input: Option<std::process::ChildStdin>,
    output: std::sync::mpsc::Receiver<Value>,
    sequence: u64,
    notifications: Vec<Value>,
}
impl Client {
    fn new(repo: &common::TestRepo, owner: &str) -> Self {
        let mut child = common::ahu()
            .args(["mcp", "serve"])
            .current_dir(repo.path())
            .env("AHU_MCP_CALLER", owner)
            .env("AHU_MCP_TASKS_ADAPTER", "inspection-v1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = child.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if sender
                    .send(serde_json::from_str(&line.unwrap()).unwrap())
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            output: receiver,
            sequence: 0,
            notifications: Vec::new(),
        }
    }
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.sequence += 1;
        writeln!(
            self.input.as_mut().unwrap(),
            "{}",
            json!({"jsonrpc":"2.0","id":self.sequence,"method":method,"params":params})
        )
        .unwrap();
        loop {
            let message = self.next();
            if message.get("id").is_some() {
                assert_eq!(message["id"], self.sequence);
                return message;
            }
            self.notifications.push(message);
        }
    }
    fn next(&self) -> Value {
        self.output
            .recv_timeout(Duration::from_secs(10))
            .expect("server message")
    }
    fn task(&mut self, method: &str, id: &str) -> Value {
        self.call(method, modern(json!({"taskId":id})))
    }
    fn create(&mut self, name: &str, arguments: Value) -> String {
        let value = self.call(
            "tools/call",
            modern(json!({"name":name,"arguments":arguments})),
        );
        assert_eq!(value["result"]["status"], "working", "{value}");
        assert_eq!(value["result"]["resultType"], "task");
        value["result"]["taskId"].as_str().unwrap().into()
    }
    fn until(&mut self, id: &str, status: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let value = self.task("tasks/get", id);
            if value["result"]["status"] == status {
                return value["result"].clone();
            }
            assert!(Instant::now() < deadline, "expected {status}: {value}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    fn stop(&mut self) {
        // EOF lets the server persist its normal shutdown summary and flush
        // LLVM coverage counters. Tests that specifically model a crash call
        // `crash` instead.
        self.input.take();
        let status = self.child.wait().unwrap();
        assert!(status.success(), "MCP server exited with {status}");
    }
    fn crash(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.wait();
    }
}

#[test]
fn asynchronous_handles_resume_after_process_loss_and_keep_results() {
    let repo = common::TestRepo::new();
    let mut first = Client::new(&repo, "alice");
    let id = first.create("ahu_agents_list", json!({}));
    let initial = first.task("tasks/get", &id)["result"].clone();
    first.crash();
    let mut second = Client::new(&repo, "alice");
    let restored = second.task("tasks/get", &id)["result"].clone();
    assert_eq!(restored["createdAt"], initial["createdAt"]);
    assert_eq!(restored["ttlMs"], Value::Null);
    let completed = second.until(&id, "completed");
    assert!(completed["result"]["structuredContent"]["agents"].is_array());
    assert!(completed.get("error").is_none());
    second.stop();
    let mut third = Client::new(&repo, "alice");
    assert_eq!(third.task("tasks/get", &id)["result"], completed);
    assert_eq!(
        third.task("tasks/cancel", &id)["result"]["resultType"],
        "complete"
    );
    assert_eq!(third.task("tasks/get", &id)["result"], completed);
}

#[test]
fn input_round_trip_is_durable_bounded_and_capability_checked() {
    let repo = common::TestRepo::new();
    let mut client = Client::new(&repo, "alice");
    let id = client.create("ahu_task_inspect", json!({}));
    let pending = client.until(&id, "input_required");
    assert_eq!(
        pending["inputRequests"]["task-selection"]["method"],
        "elicitation/create"
    );
    client.stop();
    let mut client = Client::new(&repo, "alice");
    assert_eq!(client.task("tasks/get", &id)["result"], pending);
    let valid = json!({"taskId":id,"inputResponses":{"task-selection":{"action":"accept","content":{"task":"@missing"}}}});
    let mut no_form = modern(valid.clone());
    no_form["_meta"]["io.modelcontextprotocol/clientCapabilities"]
        .as_object_mut()
        .unwrap()
        .remove("elicitation");
    assert_eq!(
        client.call("tasks/update", no_form)["error"]["code"],
        -32602
    );
    for field in ["owner", "permissions", "repository", "name", "status"] {
        let mut injection = valid.clone();
        injection[field] = json!("override");
        assert_eq!(
            client.call("tasks/update", modern(injection))["error"]["code"],
            -32602
        );
    }
    let mut injection = valid.clone();
    injection["inputResponses"]["task-selection"]["content"]["permissions"] = json!("auto");
    assert_eq!(
        client.call("tasks/update", modern(injection))["error"]["code"],
        -32602
    );
    let mut oversized = valid.clone();
    oversized["inputResponses"]["task-selection"]["content"]["task"] = json!("x".repeat(9000));
    assert_eq!(
        client.call("tasks/update", modern(oversized))["error"]["code"],
        -32602
    );
    assert_eq!(client.task("tasks/get", &id)["result"], pending);
    assert_eq!(
        client.call("tasks/update", modern(valid.clone()))["result"]["resultType"],
        "complete"
    );
    // A repeated response is harmless and never changes the accepted selector.
    assert_eq!(
        client.call("tasks/update", modern(valid))["result"]["resultType"],
        "complete"
    );
    let tool_error = client.until(&id, "completed");
    assert_eq!(tool_error["result"]["isError"], true);
    client.stop();
    let mut reconnected = Client::new(&repo, "alice");
    assert_eq!(reconnected.task("tasks/get", &id)["result"], tool_error);
}

#[test]
fn cancellation_is_durable_idempotent_and_not_overwritten_by_workers() {
    let repo = common::TestRepo::new();
    for input_required in [false, true] {
        let mut client = Client::new(&repo, "alice");
        let id = client.create("ahu_task_inspect", json!({}));
        if input_required {
            client.until(&id, "input_required");
        }
        assert_eq!(
            client.task("tasks/cancel", &id)["result"]["resultType"],
            "complete"
        );
        let cancelled = client.until(&id, "cancelled");
        assert!(cancelled.get("inputRequests").is_none());
        assert_eq!(
            client.task("tasks/cancel", &id)["result"]["resultType"],
            "complete"
        );
        assert_eq!(client.task("tasks/get", &id)["result"], cancelled);
        client.stop();
        let mut client = Client::new(&repo, "alice");
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(client.task("tasks/get", &id)["result"], cancelled);
    }
}

#[test]
fn task_authorization_legacy_isolation_and_invalid_ids() {
    let repo = common::TestRepo::new();
    let mut alice = Client::new(&repo, "alice");
    let id = alice.create("ahu_task_inspect", json!({}));
    let mut bob = Client::new(&repo, "bob");
    for method in ["tasks/get", "tasks/update", "tasks/cancel"] {
        for invalid in [
            &id,
            "../outside",
            "",
            "NOT-A-UUID",
            "00000000-0000-7000-8000-000000000000",
        ] {
            let mut request = modern(json!({"taskId":invalid,"inputResponses":{}}));
            request["_meta"]["caller"] = json!("alice");
            assert_eq!(bob.call(method, request)["error"]["code"], -32602);
        }
        assert_eq!(
            alice.call(method, modern_without_tasks(json!({"taskId":id})))["error"]["code"],
            -32021
        );
    }
    assert_eq!(
        bob.call(
            "subscriptions/listen",
            modern(json!({"notifications":{"taskIds":[id]}}))
        )["error"]["code"],
        -32602
    );
    assert!(bob.notifications.is_empty());
    let mut legacy = Client::new(&repo, "alice");
    assert_eq!(
        legacy.call("initialize", json!({}))["result"]["protocolVersion"],
        "2025-11-25"
    );
    legacy.call("server/discover", json!({}));
    for method in [
        "tasks/get",
        "tasks/update",
        "tasks/cancel",
        "subscriptions/listen",
    ] {
        assert_eq!(
            legacy.call(method, modern(json!({"taskId":id})))["error"]["code"],
            -32021
        );
    }
    let value = legacy.call("tools/call", modern(json!({"name":"ahu_agents_list"})));
    assert!(value["result"]["structuredContent"].is_object());
    assert!(value["result"].get("taskId").is_none());
    for method in ["tasks/list", "tasks/result"] {
        assert_eq!(
            alice.call(method, modern(json!({"taskId":id})))["error"]["code"],
            -32601
        );
    }
}

#[test]
fn subscribed_stdio_clients_receive_authorized_durable_transitions() {
    let repo = common::TestRepo::new();
    let mut client = Client::new(&repo, "alice");
    let id = client.create("ahu_task_inspect", json!({}));
    assert_eq!(
        client.call(
            "subscriptions/listen",
            modern_without_tasks(json!({"notifications":{"taskIds":[id]}}))
        )["error"]["code"],
        -32021
    );
    assert_eq!(
        client.call(
            "subscriptions/listen",
            modern(json!({"notifications":{"taskIds":[id]}}))
        )["result"]["resultType"],
        "complete"
    );
    let acknowledgement = client.next();
    assert_eq!(
        acknowledgement["method"],
        "notifications/subscriptions/acknowledged"
    );
    assert_eq!(
        acknowledgement["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        3
    );
    loop {
        let notification = client.next();
        assert_eq!(notification["method"], "notifications/tasks");
        assert_eq!(notification["params"]["taskId"], id);
        if notification["params"]["status"] == "input_required" {
            break;
        }
    }
    // Another authorized connection changes the state; the subscription sees it.
    let mut other = Client::new(&repo, "alice");
    assert_eq!(
        other.task("tasks/cancel", &id)["result"]["resultType"],
        "complete"
    );
    let mut cancelled = client.next();
    assert_eq!(cancelled["params"]["status"], "cancelled");
    assert_eq!(
        cancelled["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        3
    );
    cancelled["params"]["_meta"]
        .as_object_mut()
        .unwrap()
        .remove("io.modelcontextprotocol/subscriptionId");
    assert_eq!(cancelled["params"], other.task("tasks/get", &id)["result"]);
}

#[test]
fn subscriptions_reject_missing_oversized_and_unowned_task_id_lists() {
    let repo = common::TestRepo::new();
    let mut client = Client::new(&repo, "alice");
    for notifications in [
        json!({}),
        json!({"taskIds":vec!["00000000-0000-4000-8000-000000000000"; 65]}),
        json!({"taskIds":[17]}),
        json!({"taskIds":["00000000-0000-4000-8000-000000000000"]}),
    ] {
        let response = client.call(
            "subscriptions/listen",
            modern(json!({"notifications":notifications})),
        );
        assert_eq!(response["error"]["code"], -32602, "{response}");
    }
    client.stop();
}

// EOF bounds the exchange: unexpected notification replies cannot hide behind
// a timeout, and every stdout line must be a JSON-RPC response.
fn exchange(repo: &common::TestRepo, lines: &[String]) -> Vec<Value> {
    let mut child = common::ahu()
        .args(["mcp", "serve"])
        .current_dir(repo.path())
        .env("AHU_MCP_CALLER", "conformance")
        .env_remove("AHU_MCP_TASKS_ADAPTER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for line in lines {
        writeln!(input, "{line}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn conformance_rejects_invalid_envelopes_and_recovers() {
    let repo = common::TestRepo::new();
    let invalid = [
        json!(null),
        json!(42),
        json!("request"),
        json!([]),
        json!([{"jsonrpc":"2.0","id":1,"method":"ping"}]),
        json!({"id":1,"method":"ping"}),
        json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":1}),
        json!({"jsonrpc":"2.0","id":1,"method":12}),
        json!({"jsonrpc":"2.0","id":null,"method":"initialize"}),
        json!({"jsonrpc":"2.0","id":true,"method":"initialize"}),
        json!({"jsonrpc":"2.0","id":{},"method":"initialize"}),
        json!({"jsonrpc":"2.0","id":[],"method":"initialize"}),
        json!({"jsonrpc":"2.0","id":1.5,"method":"initialize"}),
        json!({"jsonrpc":"2.0","id":1,"method":"ping","result":{}}),
        json!({"jsonrpc":"2.0","id":1,"method":"ping","error":{}}),
    ];
    let mut lines = vec!["{".to_string()];
    lines.extend(invalid.iter().map(Value::to_string));
    lines.push(json!({"jsonrpc":"2.0","id":"ok","method":"server/discover","params":modern_without_tasks(json!({}))}).to_string());
    let rows = exchange(&repo, &lines);
    assert_eq!(rows.len(), lines.len());
    assert_eq!(rows[0]["error"]["code"], -32700);
    for row in &rows[1..=invalid.len()] {
        assert_eq!(row["error"]["code"], -32600, "{row}");
        assert_eq!(row["id"], Value::Null);
    }
    assert_eq!(rows.last().unwrap()["id"], "ok");
    assert_eq!(rows.last().unwrap()["result"]["resultType"], "complete");
}

#[test]
fn conformance_notifications_are_silent_and_do_not_select_modes_or_queue_work() {
    let repo = common::TestRepo::new();
    let mut lines: Vec<_> = [
        ("initialize", json!({})),
        ("server/discover", modern(json!({}))),
        ("notifications/initialized", json!({})),
        ("notifications/cancelled", json!({})),
        ("unknown", json!({})),
        ("tools/call", modern(json!({"name":"ahu_agents_list"}))),
        ("tasks/cancel", modern(json!({"taskId":"invalid"}))),
        (
            "subscriptions/listen",
            modern(json!({"notifications":{"taskIds":[]}})),
        ),
        ("ping", json!(false)),
    ]
    .into_iter()
    .map(|(method, params)| json!({"jsonrpc":"2.0","method":method,"params":params}).to_string())
    .collect();
    lines.push(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}).to_string());
    lines.push(json!({"jsonrpc":"2.0","id":2,"method":"notifications/initialized"}).to_string());
    lines.push(json!({"jsonrpc":"2.0","id":3,"method":"ping"}).to_string());
    let rows = exchange(&repo, &lines);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[0]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(rows[1]["error"]["code"], -32601);
    assert!(rows[2]["result"].is_object());
    assert!(
        !repo.path().join(".ahu/state/repos").exists(),
        "notifications must not persist work"
    );
}

#[test]
fn conformance_modes_and_all_tool_list_paths_have_consistent_shapes() {
    let repo = common::TestRepo::new();
    let mut client = Client::new(&repo, "alice");
    for (params, count) in [(modern_without_tasks(json!({})), 5), (modern(json!({})), 6)] {
        let result = client.call("tools/list", params)["result"].clone();
        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["tools"].as_array().unwrap().len(), count);
        assert_eq!(
            result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "ahu"
        );
    }
    assert_eq!(
        client.call("initialize", json!({}))["error"]["code"],
        -32601
    );
    for params in [json!(null), json!([]), json!(false), json!("bad")] {
        assert_eq!(client.call("ping", params)["error"]["code"], -32602);
    }
    assert_eq!(client.call("ping", json!({}))["error"]["code"], -32022);
    assert_eq!(
        client.call("server/discover", modern(json!({"extra":true})))["error"]["code"],
        -32602
    );
    let mut legacy = Client::new(&repo, "legacy");
    legacy.call("initialize", json!({}));
    for (method, params) in [
        ("ping", modern(json!({}))),
        ("tools/list", modern(json!({}))),
        ("tools/call", modern(json!({"name":"ahu_agents_list"}))),
    ] {
        let result = legacy.call(method, params)["result"].clone();
        assert!(result.is_object());
        assert!(result.get("resultType").is_none(), "{result}");
        assert!(result.get("taskId").is_none());
        if method == "tools/list" {
            assert_eq!(result["tools"].as_array().unwrap().len(), 5);
        }
    }
    assert_eq!(
        legacy.call("server/discover", modern(json!({})))["error"]["code"],
        -32601
    );
}

#[test]
fn conformance_tool_errors_and_argument_validation_match_across_modes() {
    let repo = common::TestRepo::new();
    for mode in 0..3 {
        let mut client = Client::new(&repo, "alice");
        if mode == 0 {
            client.call("initialize", json!({}));
        }
        let params = |p| match mode {
            0 => p,
            1 => modern_without_tasks(p),
            _ => modern(p),
        };
        for invalid in [
            json!({}),
            json!({"name":null}),
            json!({"name":"unknown"}),
            json!({"name":"ahu_agents_list","arguments":null}),
            json!({"name":"ahu_agents_list","arguments":[]}),
            json!({"name":"ahu_agents_list","arguments":{"task":"@missing"}}),
            json!({"name":"ahu_tasks_list","arguments":{"extra":true}}),
            json!({"name":"ahu_task_get"}),
            json!({"name":"ahu_task_get","arguments":{"task":4}}),
            json!({"name":"ahu_task_get","arguments":{"task":""}}),
            json!({"name":"ahu_task_get","arguments":{"task":"x".repeat(257)}}),
            json!({"name":"ahu_task_get","arguments":{"task":"@missing","extra":true}}),
        ] {
            let response = client.call("tools/call", params(invalid));
            assert_eq!(response["error"]["code"], -32602, "mode {mode}: {response}");
        }
        let response = client.call(
            "tools/call",
            params(json!({"name":"ahu_task_get","arguments":{"task":"@missing"}})),
        );
        let result = if mode == 2 {
            let id = response["result"]["taskId"].as_str().unwrap();
            let completed = client.until(id, "completed");
            assert!(completed.get("error").is_none());
            completed["result"].clone()
        } else {
            assert!(response.get("error").is_none());
            response["result"].clone()
        };
        assert_eq!(result["isError"], true, "{result}");
        assert!(result["content"][0]["text"].is_string());
        if mode != 0 {
            assert_eq!(result["resultType"], "complete");
        }
    }
}

#[test]
fn conformance_notifications_cannot_mutate_an_existing_task() {
    let repo = common::TestRepo::new();
    let mut client = Client::new(&repo, "alice");
    let id = client.create("ahu_task_inspect", json!({}));
    let pending = client.until(&id, "input_required");
    for (method, params) in [
        ("tasks/cancel", modern(json!({"taskId":id}))),
        (
            "tasks/update",
            modern(json!({"taskId":id,"inputResponses":{"task-selection":{"action":"decline"}}})),
        ),
        (
            "subscriptions/listen",
            modern(json!({"notifications":{"taskIds":[id]}})),
        ),
        ("initialize", json!({})),
        ("notifications/initialized", json!({})),
    ] {
        writeln!(
            client.input.as_mut().unwrap(),
            "{}",
            json!({"jsonrpc":"2.0","method":method,"params":params})
        )
        .unwrap();
    }
    // The request is a processing barrier for all preceding notifications.
    assert_eq!(client.task("tasks/get", &id)["result"], pending);
    assert!(client.notifications.is_empty());
}

#[test]
fn conformance_failed_probes_allow_legacy_and_adapter_requires_both_opt_ins() {
    let repo = common::TestRepo::new();
    let mut client = Client::new(&repo, "alice");
    for meta in [
        json!({}),
        json!({"io.modelcontextprotocol/protocolVersion":"unsupported","io.modelcontextprotocol/clientCapabilities":{}}),
        json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":[]}),
    ] {
        assert_eq!(
            client.call("server/discover", json!({"_meta":meta}))["error"]["code"],
            -32022
        );
    }
    assert_eq!(
        client.call("server/discover", modern(json!({"extra":true})))["error"]["code"],
        -32602
    );
    assert_eq!(
        client.call("initialize", json!({}))["result"]["protocolVersion"],
        "2025-11-25"
    );
    assert_eq!(
        client.call("tools/call", modern(json!({"name":"ahu_task_inspect"})))["error"]["code"],
        -32602
    );
    let mut modern_client = Client::new(&repo, "alice");
    assert_eq!(
        modern_client.call(
            "tools/call",
            modern_without_tasks(json!({"name":"ahu_task_inspect"}))
        )["error"]["code"],
        -32602
    );
    let rows = exchange(&repo, &[
        json!({"jsonrpc":"2.0","id":0,"method":"tools/list","params":modern(json!({}))}).to_string(),
        json!({"jsonrpc":"2.0","id":-1,"method":"tools/call","params":modern(json!({"name":"ahu_task_inspect"}))}).to_string(),
        json!({"jsonrpc":"2.0","id":"","method":"ping","params":modern_without_tasks(json!({}))}).to_string(),
    ]);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["id"], 0);
    assert_eq!(rows[0]["result"]["resultType"], "complete");
    assert_eq!(rows[0]["result"]["tools"].as_array().unwrap().len(), 5);
    assert_eq!(rows[1]["id"], -1);
    assert_eq!(rows[1]["error"]["code"], -32602);
    assert_eq!(rows[2]["id"], "");
    assert_eq!(rows[2]["result"]["resultType"], "complete");
}

#[test]
fn skill_suggestions_are_local_when_lexical_and_export_only_bounded_metadata() {
    let repo = common::TestRepo::new();
    repo.write(".agents/skills/billing/SKILL.md",
        "---\nname: billing\ndescription: Resolve duplicate charge disputes\n---\nUse the refund checklist.\n");
    repo.commit("synthetic skill");
    let receiver = ahu::eval_otel::Receiver::start().unwrap();
    let mut child = common::ahu()
        .current_dir(repo.path())
        .args(["mcp", "serve"])
        .env("AHU_EVAL_OTEL_ENDPOINT", receiver.endpoint())
        .env(
            "AHU_MCP_RESOURCE_ATTRIBUTES",
            "ahu.task.id=skill-fixture,ahu.task.attempt=1",
        )
        .env_remove("OTEL_RESOURCE_ATTRIBUTES")
        .env_remove("TYPESAFE_API_KEY")
        .env("AHU_DECISION_URL", "http://127.0.0.1:1/never-called")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let requests = [
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":"ahu_skills_suggest","arguments":{"task":"Resolve a duplicate charge","mode":"lexical"}}}),
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
            "name":"ahu_skills_suggest","arguments":{"task":"private-marker","mode":"bogus"}}}),
    ];
    let mut input = child.stdin.take().unwrap();
    for request in requests {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let value = &rows.iter().find(|r| r["id"] == 2).unwrap()["result"]["structuredContent"];
    assert_eq!(value["status"], "suggested");
    assert_eq!(
        value["selected"],
        serde_json::json!([".agents/skills/billing/SKILL.md"])
    );
    assert!(rows.iter().find(|r| r["id"] == 3).unwrap()["error"].is_object());
    let observed = receiver.task("skill-fixture", 1).unwrap();
    assert_eq!(observed.coverage().as_str(), "complete_session");
    assert_eq!(observed.selection_observations, 1);
    assert_eq!(observed.selection_candidate_count, Some(1));
    assert_eq!(observed.selection_selected_count, Some(1));
    assert_eq!(observed.tool_calls_by_name["ahu_skills_suggest"], 2);
    let recorded = serde_json::to_string(&observed).unwrap();
    assert!(!recorded.contains("private-marker"));
    assert!(!recorded.contains("duplicate charge"));
}
