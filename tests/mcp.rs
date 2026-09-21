mod common;

use std::io::Write;
use std::process::Stdio;

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
    assert_eq!(rows[1]["result"]["tools"].as_array().unwrap().len(), 3);
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
fn setup_refuses_to_write_through_a_skills_symlink() {
    let repo = common::TestRepo::new();
    let external = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
    std::os::unix::fs::symlink(external.path(), repo.path().join(".agents/skills")).unwrap();
    let output = common::ahu()
        .args(["mcp", "setup"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("refusing to act through a symlink"),
        "{}",
        stderr
    );
    assert_eq!(std::fs::read_dir(external.path()).unwrap().count(), 0);
}

#[test]
fn setup_preserves_existing_non_utf8_skill_bytes() {
    let repo = common::TestRepo::new();
    let path = repo
        .path()
        .join(".agents/skills/discover-requirements/SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = b"local skill\n\xff\xfe";
    std::fs::write(&path, original).unwrap();
    let output = common::ahu()
        .args(["mcp", "setup"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("refusing to overwrite changed skill")
    );
}

#[test]
fn setup_materializes_the_bundled_skill_trees_without_overwriting_changes() {
    let repo = common::TestRepo::new();
    let output = common::ahu()
        .args(["mcp", "setup"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let agents = repo.read(".agents/skills/discover-requirements/SKILL.md");
    assert!(!repo.path().join(".claude/skills").exists());
    assert!(agents.contains("# Discover requirements"));
    let hygiene = repo.read(".agents/skills/context-hygiene/SKILL.md");
    assert!(hygiene.contains("# Context hygiene"), "{hygiene}");

    let repeated = common::ahu()
        .args(["mcp", "setup"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(repeated.status.success());
    assert!(repeated.stdout.is_empty());

    repo.write(
        ".agents/skills/discover-requirements/SKILL.md",
        "local change\n",
    );
    let refused = common::ahu()
        .args(["mcp", "setup"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("refusing to overwrite changed skill")
    );
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
            "jsonrpc":"2.0","id":1,"method":"server/discover","params":{}
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
                "_meta":{"io.modelcontextprotocol/clientCapabilities":{
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
            "params":{"taskId":"not-a-task"}
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
use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant};

const EXT: &str = "io.modelcontextprotocol/tasks";
fn modern(mut params: Value) -> Value {
    params["_meta"] = json!({"io.modelcontextprotocol/clientCapabilities":{
        "extensions":{EXT:{}}, "elicitation":{"form":{}}
    }});
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
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn asynchronous_handles_resume_after_process_loss_and_keep_results() {
    let repo = common::TestRepo::new();
    let mut first = Client::new(&repo, "alice");
    let id = first.create("ahu_agents_list", json!({}));
    let initial = first.task("tasks/get", &id)["result"].clone();
    first.stop();
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
    let failed = client.until(&id, "failed");
    assert!(failed["error"]["code"].is_number());
    client.stop();
    let mut reconnected = Client::new(&repo, "alice");
    assert_eq!(reconnected.task("tasks/get", &id)["result"], failed);
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
            alice.call(method, json!({"taskId":id}))["error"]["code"],
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
        "2025-06-18"
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
            json!({"notifications":{"taskIds":[id]}})
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
    let cancelled = client.next();
    assert_eq!(cancelled["params"]["status"], "cancelled");
    assert_eq!(cancelled["params"], other.task("tasks/get", &id)["result"]);
}
