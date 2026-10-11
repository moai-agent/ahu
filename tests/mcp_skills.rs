//! Offline stdio contract tests. No harness, provider, telemetry, or network.
mod common;

use ahu::mcp::{BUNDLED_SKILLS, skill_path};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const EXT: &str = "io.modelcontextprotocol/skills";
const CLIENT: &str = "io.modelcontextprotocol/clientCapabilities";
const VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const FRAME_LIMIT: usize = 1024 * 1024;

fn bundled_repo() -> common::TestRepo {
    let repo = common::TestRepo::new();
    for &(name, text) in BUNDLED_SKILLS {
        repo.write(&skill_path(name), text);
    }
    repo
}

struct Server {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    id: u64,
}

impl Server {
    fn new(repo: &common::TestRepo) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ahu"))
            .args(["mcp", "serve"])
            .current_dir(repo.path())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            id: 0,
        }
    }

    fn call(&mut self, method: &str, mut params: Value) -> Value {
        params["_meta"] = json!({VERSION: "2026-07-28", CLIENT: {"extensions": {EXT: {}}}});
        self.raw(method, params)
    }

    fn raw(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let (response, _) = self.raw_request(json!({
            "jsonrpc":"2.0", "id":self.id, "method":method, "params":params
        }));
        assert_eq!(response["id"], self.id);
        response
    }

    fn raw_request(&mut self, request: Value) -> (Value, usize) {
        let input = self.input.as_mut().unwrap();
        input
            .write_all(&serde_json::to_vec(&request).unwrap())
            .unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        (response, line.len())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.input.take();
        assert!(self.child.wait().unwrap().success());
    }
}

#[test]
fn skills_capabilities_require_available_content_and_per_request_negotiation() {
    let repo = common::TestRepo::new();
    let mut server = Server::new(&repo);
    let discovered = server.call("server/discover", json!({}));
    assert!(discovered["result"]["capabilities"]["extensions"][EXT].is_null());
    assert!(discovered["result"]["capabilities"]["resources"].is_null());
    assert_eq!(
        server.call("skills/list", json!({}))["result"]["skills"],
        json!([])
    );
    let (name, text) = BUNDLED_SKILLS[0];
    repo.write(&skill_path(name), text);
    let discovered = server.call("server/discover", json!({}));
    assert_eq!(
        discovered["result"]["capabilities"]["extensions"][EXT],
        json!({})
    );
    assert_eq!(discovered["result"]["capabilities"]["resources"], json!({}));
    assert!(!discovered.to_string().contains("directoryRead"));
    for method in ["skills/list", "skills/get"] {
        for caps in [
            json!({}),
            json!({"extensions": {EXT: true}}),
            json!({"extensions": {EXT: null}}),
        ] {
            let result = server.raw(
                method,
                json!({"_meta": {VERSION:"2026-07-28", CLIENT:caps}}),
            );
            assert_eq!(result["error"]["code"], -32021);
            assert_eq!(
                result["error"]["data"]["requiredCapabilities"]["extensions"][EXT],
                json!({})
            );
        }
    }
    assert_eq!(
        server.raw("skills/list", json!({}))["error"]["code"],
        -32022
    );
    assert_eq!(
        server.raw("skills/list", json!({"_meta":{VERSION:"wrong", CLIENT:{}}}))["error"]["code"],
        -32022
    );
    assert_eq!(
        server.call("resources/directory/read", json!({}))["error"]["code"],
        -32601
    );
}

#[test]
fn legacy_does_not_advertise_or_enable_skills_with_modern_metadata() {
    let repo = bundled_repo();
    let mut server = Server::new(&repo);
    let initialized = server.raw(
        "initialize",
        json!({"protocolVersion":"2025-11-25", "capabilities":{"extensions":{EXT:{}}}}),
    );
    assert!(initialized["result"]["capabilities"]["extensions"].is_null());
    for method in [
        "skills/list",
        "skills/get",
        "resources/list",
        "resources/read",
    ] {
        assert_eq!(server.call(method, json!({}))["error"]["code"], -32601);
    }
}

#[test]
fn paginated_manifests_get_and_resource_reads_preserve_bytes_metadata_and_origin() {
    let repo = bundled_repo();
    let mut server = Server::new(&repo);
    let mut params = json!({});
    let mut entries = Vec::new();
    loop {
        let response = server.call("skills/list", params.clone());
        let result = &response["result"];
        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["ttlMs"], 0);
        assert_eq!(result["cacheScope"], "private");
        entries.extend(result["skills"].as_array().unwrap().iter().cloned());
        let Some(cursor) = result.get("nextCursor") else {
            break;
        };
        params["cursor"] = cursor.clone();
    }
    assert_eq!(entries.len(), BUNDLED_SKILLS.len());
    for (entry, &(name, text)) in entries.iter().zip(BUNDLED_SKILLS) {
        let uri = format!("ahu://skills/{name}/SKILL.md");
        assert_eq!(entry["uri"], uri);
        let yaml = text
            .strip_prefix("---\n")
            .unwrap()
            .split_once("\n---")
            .unwrap()
            .0;
        assert_eq!(
            entry["frontmatter"],
            yaml_serde::from_str::<Value>(yaml).unwrap()
        );
        assert_eq!(entry["resources"].as_array().unwrap().len(), 1);
        assert_eq!(entry["resources"][0]["uri"], uri);
        assert_eq!(
            entry["resources"][0]["digest"],
            format!("sha256:{}", ahu::util::digest_bytes(text.as_bytes()))
        );
        assert_eq!(entry["resources"][0]["size"], text.len());
        let origin = &entry["_meta"]["io.github.moai-agent.ahu/skillOrigin"];
        assert_eq!(origin["kind"], "ahu-bundled-local");
        assert_eq!(origin["path"], skill_path(name));
        assert_eq!(origin["executionObserved"], false);
        let get = server.call("skills/get", json!({"uri":uri}));
        assert_eq!(get["result"]["skill"], *entry);
        assert_eq!(get["result"]["resultType"], "complete");
        assert_eq!(get["result"]["ttlMs"], 0);
        assert_eq!(get["result"]["cacheScope"], "private");
        // Standard resource reads do not require the Skills capability.
        let read = server.raw(
            "resources/read",
            json!({"uri":uri, "_meta":{VERSION:"2026-07-28",CLIENT:{}}}),
        );
        assert_eq!(
            read["result"]["contents"],
            json!([{"uri":uri,"mimeType":"text/markdown","text":text}])
        );
    }
    let resources = server.call("resources/list", json!({}));
    assert_eq!(
        resources["result"]["resources"][0],
        entries[0]["resources"][0]
    );
    let cursor = resources["result"]["nextCursor"].clone();
    assert_eq!(
        server.call("resources/list", json!({"cursor":cursor}))["result"]["resources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn changed_and_malformed_content_are_removed_and_cursors_become_stale() {
    let repo = bundled_repo();
    let mut server = Server::new(&repo);
    let page = server.call("skills/list", json!({}));
    let cursor = page["result"]["nextCursor"].clone();
    let (name, original) = BUNDLED_SKILLS[0];
    let uri = format!("ahu://skills/{name}/SKILL.md");
    for changed in ["---\nname: broken\n---", "changed", ""] {
        repo.write(&skill_path(name), changed);
        assert_eq!(
            server.call("skills/list", json!({"cursor":cursor}))["error"]["code"],
            -32602
        );
        assert!(
            server
                .call("skills/get", json!({"uri":uri}))
                .get("error")
                .is_some()
        );
        assert!(
            server
                .call("resources/read", json!({"uri":uri}))
                .get("error")
                .is_some()
        );
        assert!(
            !server.call("skills/list", json!({}))["result"]["skills"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["uri"] == uri)
        );
    }
    repo.write(&skill_path(name), original);
    assert_eq!(
        server.call("skills/get", json!({"uri":uri}))["result"]["skill"]["uri"],
        uri
    );
    for cursor in [
        json!(null),
        json!(3),
        json!("../x"),
        json!("0:18446744073709551615"),
    ] {
        assert_eq!(
            server.call("skills/list", json!({"cursor":cursor}))["error"]["code"],
            -32602
        );
    }
}

#[test]
fn unbundled_supporting_files_remote_content_and_path_escapes_are_never_exposed() {
    let repo = bundled_repo();
    let (name, _) = BUNDLED_SKILLS[0];
    repo.write(
        &format!(".agents/skills/{name}/references/local.md"),
        "unbundled supporting file",
    );
    repo.write(
        ".agents/skills/remote/SKILL.md",
        "---\nname: remote\ndescription: Remote shadow.\n---\n",
    );
    let mut server = Server::new(&repo);
    for suffix in [
        "references/local.md",
        "../SKILL.md",
        "%2e%2e/SKILL.md",
        "/SKILL.md",
        "SKILL.md?source=remote",
    ] {
        let uri = format!("ahu://skills/{name}/{suffix}");
        assert!(
            server
                .call("resources/read", json!({"uri":uri}))
                .get("error")
                .is_some()
        );
    }
    for uri in [
        "https://example.invalid/SKILL.md",
        "file:///etc/passwd",
        "ahu://skills/remote/SKILL.md",
    ] {
        for method in ["skills/get", "resources/read"] {
            assert!(
                server
                    .call(method, json!({"uri":uri}))
                    .get("error")
                    .is_some()
            );
        }
    }
    assert_eq!(
        server.call("skills/list", json!({"source":"https://example.invalid"}))["error"]["code"],
        -32602
    );
    assert_eq!(
        server.call(
            "skills/get",
            json!({"uri":format!("ahu://skills/{name}/SKILL.md"),"permissions":"auto"})
        )["error"]["code"],
        -32602
    );
    // An exact-byte symlink target remains untrusted, even within the checkout.
    let path = repo.path().join(skill_path(name));
    let saved = repo.path().join("saved.md");
    std::fs::rename(&path, &saved).unwrap();
    std::os::unix::fs::symlink(saved, path).unwrap();
    assert!(
        server
            .call(
                "resources/read",
                json!({"uri":format!("ahu://skills/{name}/SKILL.md")})
            )
            .get("error")
            .is_some()
    );
}

fn tool_call(server: &mut Server, modern: bool, name: &str, arguments: Value) -> Value {
    let mut params = json!({"name":name,"arguments":arguments});
    if modern {
        // A tool-only client need not know either extension, including Tasks.
        params["_meta"] = json!({VERSION:"2026-07-28", CLIENT:{}});
    }
    server.raw("tools/call", params)
}

#[test]
fn tool_only_clients_discover_and_retrieve_verified_skills() {
    for modern in [false, true] {
        let repo = bundled_repo();
        let mut server = Server::new(&repo);
        if !modern {
            server.raw("initialize", json!({"protocolVersion":"2025-11-25"}));
        }
        let params = if modern {
            json!({"_meta":{VERSION:"2026-07-28",CLIENT:{}}})
        } else {
            json!({})
        };
        let tools = server.raw("tools/list", params);
        for name in ["ahu_skills_list", "ahu_skills_get"] {
            let definition = tools["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == name)
                .expect("compatibility tool advertised");
            assert_eq!(definition["inputSchema"]["additionalProperties"], false);
            assert_eq!(definition["annotations"]["readOnlyHint"], true);
            assert_eq!(definition["annotations"]["openWorldHint"], false);
        }
        let mut arguments = json!({});
        let mut entries = Vec::new();
        loop {
            let response = tool_call(&mut server, modern, "ahu_skills_list", arguments.clone());
            let result = &response["result"];
            assert_eq!(result["ttlMs"], 0);
            assert_eq!(result["cacheScope"], "private");
            assert_eq!(result["resultType"].as_str(), modern.then_some("complete"));
            let value = &result["structuredContent"];
            assert_eq!(value["resultType"], "complete");
            assert_eq!(value["ttlMs"], 0);
            assert_eq!(value["cacheScope"], "private");
            assert_eq!(
                serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap())
                    .unwrap(),
                *value
            );
            assert!(value["skills"].as_array().unwrap().len() <= 2);
            entries.extend(value["skills"].as_array().unwrap().iter().cloned());
            let Some(cursor) = value.get("nextCursor") else {
                break;
            };
            assert!(cursor.as_str().unwrap().len() <= 128);
            arguments["cursor"] = cursor.clone();
        }
        assert_eq!(entries.len(), BUNDLED_SKILLS.len());
        for (entry, &(name, text)) in entries.iter().zip(BUNDLED_SKILLS) {
            let response = tool_call(
                &mut server,
                modern,
                "ahu_skills_get",
                json!({"uri":entry["uri"]}),
            );
            assert!(response.to_string().len() < 1024 * 1024);
            assert!(!response.to_string().contains(repo.path().to_str().unwrap()));
            let result = &response["result"];
            let value = &result["structuredContent"];
            assert_eq!(result["ttlMs"], 0);
            assert_eq!(result["cacheScope"], "private");
            assert_eq!(value["skill"], *entry);
            assert_eq!(
                value["contents"],
                json!([{"uri":entry["uri"],"mimeType":"text/markdown","text":text}])
            );
            assert_eq!(value["skill"]["resources"].as_array().unwrap().len(), 1);
            assert_eq!(
                value["skill"]["resources"][0]["digest"],
                format!("sha256:{}", ahu::util::digest_bytes(text.as_bytes()))
            );
            assert_eq!(value["skill"]["resources"][0]["size"], text.len());
            let origin = &value["skill"]["_meta"]["io.github.moai-agent.ahu/skillOrigin"];
            assert_eq!(origin["path"], skill_path(name));
            assert_eq!(origin["executionObserved"], false);
            if modern {
                assert_eq!(
                    server.call("skills/get", json!({"uri":entry["uri"]}))["result"]["skill"],
                    *entry
                );
                assert_eq!(
                    server.call("resources/read", json!({"uri":entry["uri"]}))["result"]["contents"],
                    value["contents"]
                );
            }
        }
    }
}

#[test]
fn near_limit_tool_request_ids_are_rejected_without_oversized_responses_in_both_modes() {
    let repo = bundled_repo();
    let uri = format!("ahu://skills/{}/SKILL.md", BUNDLED_SKILLS[0].0);
    for modern in [false, true] {
        let mut server = Server::new(&repo);
        if modern {
            server.call("server/discover", json!({}));
        } else {
            server.raw("initialize", json!({"protocolVersion":"2025-11-25"}));
        }
        let mut params = json!({"name":"ahu_skills_get","arguments":{"uri":uri}});
        if modern {
            params["_meta"] = json!({
                VERSION:"2026-07-28",
                CLIENT:{"extensions":{"io.modelcontextprotocol/tasks":{}}}
            });
        }
        let mut request = json!({"jsonrpc":"2.0","id":"","method":"tools/call","params":params});
        let base_len = serde_json::to_vec(&request).unwrap().len() + 1;
        request["id"] = json!("x".repeat(FRAME_LIMIT - base_len));
        assert_eq!(serde_json::to_vec(&request).unwrap().len() + 1, FRAME_LIMIT);

        let (response, response_bytes) = server.raw_request(request);
        assert_eq!(response["error"]["code"], -32600);
        assert_eq!(response["id"], Value::Null);
        assert!(response_bytes <= FRAME_LIMIT);

        let mut valid_params = json!({"name":"ahu_skills_get","arguments":{"uri":uri}});
        if modern {
            valid_params["_meta"] = json!({
                VERSION:"2026-07-28",
                CLIENT:{"extensions":{"io.modelcontextprotocol/tasks":{}}}
            });
        }
        let response = server.raw("tools/call", valid_params);
        assert_eq!(response["result"]["isError"], Value::Null);
        assert!(response["result"]["structuredContent"]["contents"][0]["text"].is_string());
    }
}

#[test]
fn skill_tools_reject_invalid_arguments_and_do_not_cache_stale_content() {
    let repo = bundled_repo();
    let mut server = Server::new(&repo);
    for (name, arguments) in [
        ("ahu_skills_list", json!(null)),
        ("ahu_skills_list", json!([])),
        ("ahu_skills_list", json!({"cursor":null})),
        ("ahu_skills_list", json!({"cursor":3})),
        ("ahu_skills_list", json!({"cursor":""})),
        ("ahu_skills_list", json!({"cursor":"x".repeat(129)})),
        ("ahu_skills_list", json!({"_meta":{}})),
        ("ahu_skills_list", json!({"source":"private-marker"})),
        ("ahu_skills_get", json!({})),
        ("ahu_skills_get", json!({"uri":null})),
        ("ahu_skills_get", json!({"uri":3})),
        ("ahu_skills_get", json!({"uri":""})),
        ("ahu_skills_get", json!({"uri":"x".repeat(257)})),
        (
            "ahu_skills_get",
            json!({"uri":"private-marker","extra":true}),
        ),
    ] {
        let result = tool_call(&mut server, true, name, arguments);
        assert_eq!(result["error"]["code"], -32602, "{result}");
        assert!(!result.to_string().contains("private-marker"));
    }
    let page = tool_call(&mut server, true, "ahu_skills_list", json!({}));
    let cursor = page["result"]["structuredContent"]["nextCursor"].clone();
    let fingerprint = cursor.as_str().unwrap().split_once(':').unwrap().0;
    for invalid in [
        json!("../x"),
        json!("0:18446744073709551615"),
        json!(format!("{fingerprint}:0")),
        json!(format!("{fingerprint}:1")),
        json!(format!("{fingerprint}:999")),
    ] {
        let response = tool_call(
            &mut server,
            true,
            "ahu_skills_list",
            json!({"cursor":invalid}),
        );
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(response["result"]["ttlMs"], 0);
        assert_eq!(response["result"]["cacheScope"], "private");
    }
    let (name, original) = BUNDLED_SKILLS[0];
    let uri = format!("ahu://skills/{name}/SKILL.md");
    for changed in [
        "private-marker".to_string(),
        "---\nname: broken\n---".into(),
        String::new(),
        "x".repeat(64 * 1024 + 1),
    ] {
        repo.write(&skill_path(name), &changed);
        for (name, args) in [
            ("ahu_skills_list", json!({"cursor":cursor})),
            ("ahu_skills_get", json!({"uri":uri})),
        ] {
            let response = tool_call(&mut server, true, name, args);
            assert_eq!(response["result"]["isError"], true);
            assert_eq!(response["result"]["resultType"], "complete");
            assert!(!response.to_string().contains("private-marker"));
        }
        let response = tool_call(&mut server, true, "ahu_skills_list", json!({}));
        assert!(
            !response["result"]["structuredContent"]["skills"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["uri"] == uri)
        );
    }
    repo.write(&skill_path(name), original);
    assert_eq!(
        tool_call(&mut server, true, "ahu_skills_get", json!({"uri":uri}))["result"]["structuredContent"]
            ["contents"][0]["text"],
        original
    );
}

#[test]
fn skill_tools_preserve_local_bundle_privacy_boundary() {
    let repo = bundled_repo();
    let (name, _) = BUNDLED_SKILLS[0];
    repo.write(
        &format!(".agents/skills/{name}/references/local.md"),
        "private-marker",
    );
    repo.write(".agents/skills/unbundled/SKILL.md", "private-marker");
    let mut server = Server::new(&repo);
    for uri in [
        format!("ahu://skills/{name}/references/local.md"),
        format!("ahu://skills/{name}/../SKILL.md"),
        format!("ahu://skills/{name}/%2e%2e/SKILL.md"),
        format!("ahu://skills/{name}//SKILL.md"),
        format!("ahu://skills/{name}/SKILL.md?source=remote"),
        format!("ahu://skills/{name}/SKILL.md#fragment"),
        "ahu://skills/unbundled/SKILL.md".into(),
        "https://example.invalid/SKILL.md".into(),
        "file:///private-marker".into(),
        repo.path().join(skill_path(name)).display().to_string(),
    ] {
        let response = tool_call(&mut server, true, "ahu_skills_get", json!({"uri":uri}));
        assert_eq!(response["error"]["code"], -32602);
        assert!(!response.to_string().contains("private-marker"));
        assert!(!response.to_string().contains(repo.path().to_str().unwrap()));
    }
    let path = repo.path().join(skill_path(name));
    let saved = repo.path().join("saved.md");
    std::fs::rename(&path, &saved).unwrap();
    std::os::unix::fs::symlink(&saved, &path).unwrap();
    let response = tool_call(
        &mut server,
        true,
        "ahu_skills_get",
        json!({"uri":format!("ahu://skills/{name}/SKILL.md")}),
    );
    assert_eq!(response["result"]["isError"], true);
    std::fs::remove_file(path).unwrap();
    for &(name, _) in &BUNDLED_SKILLS[1..] {
        repo.remove(&skill_path(name));
    }
    assert_eq!(
        tool_call(&mut server, true, "ahu_skills_list", json!({}))["result"]["structuredContent"]["skills"],
        json!([])
    );
}

#[test]
fn skill_tools_stay_synchronous_with_tasks_capability_and_default_arguments() {
    let repo = bundled_repo();
    let mut server = Server::new(&repo);
    let metadata =
        json!({VERSION:"2026-07-28", CLIENT:{"extensions":{"io.modelcontextprotocol/tasks":{}}}});
    let list = server.raw(
        "tools/call",
        json!({"name":"ahu_skills_list","_meta":metadata}),
    );
    assert_eq!(list["result"]["resultType"], "complete");
    assert!(list["result"].get("task").is_none());
    assert_eq!(
        server.raw(
            "tools/call",
            json!({"name":"ahu_skills_get","_meta":metadata})
        )["error"]["code"],
        -32602
    );
    let uri = &list["result"]["structuredContent"]["skills"][0]["uri"];
    let get = server.raw(
        "tools/call",
        json!({"name":"ahu_skills_get","arguments":{"uri":uri},"_meta":metadata}),
    );
    assert_eq!(get["result"]["resultType"], "complete");
    assert!(get["result"]["structuredContent"]["contents"][0]["text"].is_string());
    assert_eq!(
        get["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "ahu"
    );
}
