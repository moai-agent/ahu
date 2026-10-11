//! Offline stdio contract tests. No harness, provider, telemetry, or network.
mod common;

use ahu::mcp::{BUNDLED_SKILLS, skill_path};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const EXT: &str = "io.modelcontextprotocol/skills";
const CLIENT: &str = "io.modelcontextprotocol/clientCapabilities";
const VERSION: &str = "io.modelcontextprotocol/protocolVersion";

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
        let input = self.input.as_mut().unwrap();
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0", "id":self.id, "method":method, "params":params})
        )
        .unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], self.id);
        response
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
