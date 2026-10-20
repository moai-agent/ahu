//! Offline tests of the actual executable and public framed protocol.
use ahu_host_api::protocol::{self, ErrorCode, Request, Response};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output, Stdio};

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let cli = root.path().join("fake-cmux");
        std::fs::write(&cli, include_str!("fixtures/cmux.py")).unwrap();
        std::fs::set_permissions(cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }

    fn run(&self, mode: &str, bytes: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ahu-cmux-host"))
            .arg("--ahu-host-protocol=1")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("AHU_CMUX_BIN", self.0.path().join("fake-cmux"))
            .env("FAKE_ROOT", self.0.path())
            .env("FAKE_MODE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        child.wait_with_output().unwrap()
    }

    fn has(&self, name: &str) -> bool {
        self.0.path().join(name).exists()
    }
}

fn raw_frame(bytes: &[u8]) -> Vec<u8> {
    let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(bytes);
    frame
}

fn frame(id: u64, method: &str, params: Value) -> Vec<u8> {
    raw_frame(
        &serde_json::to_vec(&Request {
            protocol_version: 1,
            id,
            method: method.into(),
            params,
        })
        .unwrap(),
    )
}

fn hello_params() -> Value {
    json!({"protocol_versions":[1], "host_api":1, "plugin_id":"cmux-local", "required_capabilities":protocol::METHODS})
}

fn hello() -> Vec<u8> {
    frame(1, "hello", hello_params())
}

fn create_params() -> Value {
    json!({"group_id":"group", "window_id":"window", "title":"title", "summary":"summary",
        "cwd":"/synthetic/task", "startup_command":"literal $(not-run); 'quoted'", "focus":false})
}

fn responses(output: &Output) -> Vec<Response> {
    let mut input = output.stdout.as_slice();
    let mut replies = Vec::new();
    while !input.is_empty() {
        let mut header = [0; 4];
        input.read_exact(&mut header).expect("only framed stdout");
        let size = u32::from_be_bytes(header) as usize;
        assert!((1..=protocol::MAX_FRAME_BYTES).contains(&size));
        let mut body = vec![0; size];
        input.read_exact(&mut body).unwrap();
        replies.push(protocol::from_slice(&body).unwrap());
    }
    replies
}

fn success(response: &Response) -> &Value {
    match response {
        Response::Success(reply) => {
            assert_eq!(reply.protocol_version, 1);
            &reply.result
        }
        response => panic!("expected success: {response:?}"),
    }
}

#[test]
fn executable_negotiates_and_dispatches_every_host_v1_method() {
    let fixture = Fixture::new();
    let operations = [
        ("list_groups", json!({"window_id":"window"})),
        (
            "find_group",
            json!({"group_id":"group", "window_id":"window"}),
        ),
        ("current_window", json!({})),
        ("window_for_workspace", json!({"workspace_id":"existing"})),
        (
            "add_workspace_to_group",
            json!({"group_id":"group", "workspace_id":"existing", "window_id":"window"}),
        ),
        ("current_workspace", json!({})),
        (
            "create_anchor_workspace",
            json!({"group_id":"group", "title":"anchor title", "cwd":"/synthetic/repo"}),
        ),
        (
            "set_anchor",
            json!({"group_id":"group", "workspace_id":"anchor"}),
        ),
        (
            "create_group",
            json!({"name":"repo", "cwd":"/synthetic/repo"}),
        ),
        ("expand_group", json!({"group_id":"group"})),
        ("create_task_workspace", create_params()),
        (
            "open_markdown",
            json!({"path":"/synthetic/read me.md", "focus":true}),
        ),
        ("select_workspace", json!({"workspace_id":"task"})),
        ("close_workspace", json!({"workspace_id":"task"})),
        (
            "set_status",
            json!({"workspace_id":"task", "value":"running"}),
        ),
        (
            "set_agent_metadata",
            json!({"workspace_id":"task", "agent":"agent", "harness":"harness", "model":"model"}),
        ),
        ("workspaces", json!({})),
        ("workspaces_in_window", json!({"window_id":null})),
    ];
    assert_eq!(
        operations
            .iter()
            .map(|(method, _)| *method)
            .collect::<Vec<_>>(),
        protocol::METHODS
    );
    let mut input = hello();
    for (index, (method, params)) in operations.into_iter().enumerate() {
        input.extend(frame(index as u64 + 2, method, params));
    }
    input.extend(frame(
        20,
        "workspaces_in_window",
        json!({"window_id":"window"}),
    ));
    input.extend(frame(
        21,
        "find_group",
        json!({"group_id":"absent", "window_id":null}),
    ));
    let output = fixture.run("normal", &input);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let replies = responses(&output);
    assert_eq!(replies.len(), 21);
    for (index, reply) in replies.iter().enumerate() {
        success(reply);
        if let Response::Success(reply) = reply {
            assert_eq!(reply.id, index as u64 + 1);
        }
    }
    assert_eq!(
        success(&replies[0]),
        &json!({"protocol_version":1, "host_api":1, "plugin_id":"cmux-local", "capabilities":protocol::METHODS, "inherited_workspace":null})
    );
    assert_eq!(success(&replies[1])["complete"], true);
    assert_eq!(success(&replies[3]), "window");
    assert_eq!(success(&replies[6]), "existing");
    assert_eq!(
        success(&replies[11]),
        &json!({"workspace_id":"task", "window_id":"window", "group_id":"group"})
    );
    assert_eq!(success(&replies[12]), "surface");
    for index in [5, 8, 10, 13, 14, 15, 16] {
        assert!(success(&replies[index]).is_null());
    }
    for index in [17, 18] {
        assert_eq!(success(&replies[index])["complete"], true);
        assert_eq!(
            success(&replies[index])["workspaces"]
                .as_object()
                .unwrap()
                .len(),
            2
        );
    }
    assert_eq!(
        success(&replies[19])["workspaces"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert!(success(&replies[20]).is_null());
    assert!(fixture.has("created"));
}

#[test]
fn creation_outcomes_survive_the_wire_without_cleanup_or_retry() {
    for (mode, code, created) in [
        ("preflight-failure", ErrorCode::NotCreated, false),
        ("spawn-failure", ErrorCode::NotCreated, false),
        ("command-failure", ErrorCode::OutcomeUnknown, true),
        ("postflight-failure", ErrorCode::OutcomeUnknown, true),
    ] {
        let fixture = Fixture::new();
        let output = fixture.run(
            mode,
            &[hello(), frame(2, "create_task_workspace", create_params())].concat(),
        );
        assert!(output.status.success());
        let replies = responses(&output);
        assert_eq!(replies.len(), 2);
        let Response::Failure(reply) = &replies[1] else {
            panic!("{mode}: {:?}", replies[1]);
        };
        assert_eq!(reply.error.code, code, "{mode}");
        assert!(reply.error.message.chars().count() <= 512);
        assert_eq!(fixture.has("created"), created, "{mode}");
        assert!(!fixture.has("closed"));
    }
}

#[test]
fn incomplete_inventory_is_an_error_and_oversized_output_never_emits_a_partial_frame() {
    for method in ["workspaces", "workspaces_in_window"] {
        let params = if method == "workspaces" {
            json!({})
        } else {
            json!({"window_id":null})
        };
        let input = [hello(), frame(2, method, params)].concat();
        let output = Fixture::new().run("inventory-failure", &input);
        assert!(output.status.success());
        assert!(
            matches!(&responses(&output)[1], Response::Failure(reply) if reply.error.code == ErrorCode::Failed)
        );
        let output = Fixture::new().run("oversized-result", &input);
        assert!(!output.status.success());
        assert_eq!(responses(&output).len(), 1);
    }
}

#[test]
fn prerequisite_failures_are_bounded_protocol_errors() {
    for mode in ["ping-failure", "missing-capabilities"] {
        let fixture = Fixture::new();
        let output = fixture.run(mode, &hello());
        assert!(!output.status.success());
        assert!(
            matches!(&responses(&output)[0], Response::Failure(reply) if reply.id == 1 && reply.error.code == ErrorCode::Failed)
        );
        assert!(!fixture.has("dispatched"));
    }
}

#[test]
fn invalid_negotiation_never_connects_to_cmux() {
    let mut bad_hellos = vec![
        frame(1, "workspaces", json!({})),
        frame(2, "hello", hello_params()),
    ];
    for (field, value) in [
        ("plugin_id", json!("impostor")),
        ("host_api", json!(2)),
        ("protocol_versions", json!([2])),
        ("required_capabilities", json!(["unknown"])),
        ("required_capabilities", json!(["workspaces", "workspaces"])),
        ("extra", json!(true)),
    ] {
        let mut params = hello_params();
        params[field] = value;
        bad_hellos.push(frame(1, "hello", params));
    }
    for input in bad_hellos {
        let fixture = Fixture::new();
        let output = fixture.run("normal", &input);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!fixture.has("connected"));
    }
}

#[test]
fn strict_frames_and_parameters_fail_before_dispatch() {
    let mut invalid = vec![
        vec![0, 0, 0, 0], ((protocol::MAX_FRAME_BYTES + 1) as u32).to_be_bytes().to_vec(),
        vec![0, 0], vec![0, 0, 0, 2, b'{'], raw_frame(b"[]"), raw_frame(&[0xff]),
        raw_frame(br#"{"protocol_version":1,"id":2,"method":"workspaces","params":{},"extra":true}"#),
        raw_frame(br#"{"protocol_version":1,"id":2,"id":3,"method":"workspaces","params":{}}"#),
        raw_frame(br#"{"protocol_version":1,"id":2,"method":"list_groups","params":{"window_id":null,"window_id":"window"}}"#),
        raw_frame(br#"{"protocol_version":2,"id":2,"method":"workspaces","params":{}}"#),
        frame(1, "workspaces", json!({})), frame(0, "workspaces", json!({})),
        frame(2, "hello", hello_params()), frame(2, "unknown", json!({})),
        frame(2, "workspaces", json!({"extra":true})), frame(2, "workspaces", json!(null)),
        frame(2, "list_groups", json!({})), frame(2, "list_groups", json!({"window_id":3})),
        frame(2, "close_workspace", json!({"workspace_id":""})),
        frame(2, "close_workspace", json!({"workspace_id":"bad\nidentifier"})),
        frame(2, "close_workspace", json!({"workspace_id":"x".repeat(257)})),
    ];
    for (field, value) in [
        ("cwd", json!("relative")),
        ("focus", json!("false")),
        ("extra", json!(true)),
    ] {
        let mut params = create_params();
        params[field] = value;
        invalid.push(frame(2, "create_task_workspace", params));
    }
    for tail in invalid {
        let fixture = Fixture::new();
        let output = fixture.run("normal", &[hello(), tail].concat());
        assert!(!output.status.success());
        assert_eq!(responses(&output).len(), 1);
        assert!(!fixture.has("dispatched"));
    }
}

#[test]
fn empty_input_and_unsupported_arguments_never_connect() {
    let fixture = Fixture::new();
    let output = fixture.run("normal", &[]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!fixture.has("connected"));
    for args in [
        vec![],
        vec!["--ahu-host-protocol=2"],
        vec!["--ahu-host-protocol=1", "extra"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ahu-cmux-host"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("usage:"));
    }
}
