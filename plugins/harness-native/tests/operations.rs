use ahu_harness_native::call;
use serde_json::{Value, json};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::{fs, path::PathBuf};
struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    exe: PathBuf,
}
impl Fixture {
    fn new(body: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        fs::create_dir(&home).unwrap();
        let exe = root.join("native");
        fs::write(&exe, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _tmp: tmp,
            root,
            home,
            exe,
        }
    }
    fn probe(&self) -> Value {
        json!({"project":self.root,"home":self.home,"native_executable":self.exe,"environment":{}})
    }
    fn setup(&self) -> Value {
        json!({"project":self.root,"home":self.home,"ahu_executable":self.exe,"skills":[]})
    }
    fn admission(&self) -> Value {
        json!({"project":self.root,"home":self.home,"native_executable":self.exe,"environment":{},"version":"2.29.0","model":"claude-sonnet-4.5","permissions":"auto","mode":"headless","work_mode":false,"overrides":[],"managed_claude":self.root.join("managed-claude.json"),"managed_opencode":self.root.join("managed-opencode"),"managed_preferences":null,"system_codex":self.root.join("system-codex"),"codex_managed_preferences":"not_applicable","trust_opencode_native_state":false,"helpers":{"policy":"disabled","helper_model":null,"helper_role":"reviewer","max_concurrent":1,"max_depth":1,"budget_usd":5.0,"assignment_writes":true}})
    }
}
#[test]
fn probe_returns_only_version_and_bounded_evidence() {
    let f = Fixture::new("printf 'private-sentinel codex-cli 0.160.0\\n'");
    let result = call("codex", "probe", &f.probe()).unwrap();
    assert_eq!(result["version"], "0.160.0");
    assert_eq!(result["reviewed_version"], true);
    assert!(!result.to_string().contains("private-sentinel"));
    assert!(
        call(
            "codex",
            "probe",
            &json!({"project":f.root,"home":f.home,"native_executable":"native","environment":{}})
        )
        .is_err()
    );
    let mut p = f.probe();
    p["unexpected"] = json!(true);
    assert!(call("codex", "probe", &p).is_err());
    assert!(call("unknown", "probe", &f.probe()).is_err());
    assert!(call("codex", "shell", &f.probe()).is_err());
}
#[test]
fn suggested_and_native_models_have_distinct_provenance() {
    let f = Fixture::new(
        "case \"$1\" in whoami) printf '{\"accountType\":\"SocialGitHub\"}';; chat) printf '{\"models\":[{\"model_id\":\"claude-sonnet-4.5\"}]}';; models) printf 'ollama/glm-5.3:cloud\\n';; *) exit 99;; esac",
    );
    assert_eq!(
        call("codex", "models", &f.probe()).unwrap()["source"],
        "suggested"
    );
    assert_eq!(
        call("claude-code", "models", &f.probe()).unwrap()["source"],
        "suggested"
    );
    for h in ["kiro", "opencode", "antigravity"] {
        let v = call(h, "models", &f.probe()).unwrap();
        assert_eq!(v["source"], "native");
        assert_eq!(v["models"].as_array().unwrap().len(), 1);
    }
}
#[test]
fn kiro_listing_requires_sign_in_before_list_command() {
    let f = Fixture::new("case \"$1\" in whoami) printf '{}';; *) touch should-not-run;; esac");
    assert!(call("kiro", "models", &f.probe()).is_err());
    assert!(!f.root.join("should-not-run").exists());
}
#[test]
fn native_probe_mutations_are_detected_without_rollback_or_replay() {
    let f = Fixture::new(
        "mkdir -p .agents; printf 'changed' > .agents/config; printf 'codex-cli 0.160.0\\n'",
    );
    let e = call("codex", "probe", &f.probe()).unwrap_err();
    assert!(e.effects_unknown);
    assert_eq!(
        fs::read_to_string(f.root.join(".agents/config")).unwrap(),
        "changed"
    );
}
#[test]
fn setup_proposes_all_provider_formats_and_never_writes() {
    let f = Fixture::new("exit 99");
    for h in ["codex", "claude-code", "antigravity", "opencode", "kiro"] {
        let mut p = f.setup();
        p["skills"] = json!([{"name":"test-skill","text":"# synthetic\n"}]);
        let result = call(h, "setup", &p).unwrap();
        let changes = result["changes"].as_array().unwrap();
        assert!(changes.len() >= 2);
        for change in changes {
            assert!(change["expected_before_sha256"].is_null());
            assert!(!PathBuf::from(change["path"].as_str().unwrap()).exists());
        }
        let text = changes[0]["text"].as_str().unwrap();
        if h == "codex" {
            let v: toml::Value = toml::from_str(text).unwrap();
            assert_eq!(
                v["mcp_servers"]["ahu"]["tools"]["ahu_agents_list"]["approval_mode"].as_str(),
                Some("approve")
            );
        } else {
            let v: Value = serde_json::from_str(text).unwrap();
            let key = if h == "opencode" { "mcp" } else { "mcpServers" };
            assert!(v[key]["ahu"].is_object());
            if h == "kiro" {
                assert_eq!(v[key]["ahu"]["env"]["AHU_TASK_ID"], "${AHU_TASK_ID}");
            }
        }
    }
}
#[test]
fn setup_retains_existing_fields_and_detects_conflicts() {
    let f = Fixture::new("exit 99");
    let path = f.root.join(".mcp.json");
    let before = b"{\"other\":42,\"mcpServers\":{\"other\":{\"command\":\"keep\"}}}";
    fs::write(&path, before).unwrap();
    let r = call("claude-code", "setup", &f.setup()).unwrap();
    assert!(
        r["changes"][0]["expected_before_sha256"]
            .as_str()
            .unwrap()
            .len()
            == 64
    );
    let v: Value = serde_json::from_str(r["changes"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(v["other"], 42);
    assert_eq!(v["mcpServers"]["other"]["command"], "keep");
    assert_eq!(fs::read(&path).unwrap(), before);
    fs::write(
        &path,
        b"{\"mcpServers\":{\"ahu\":{\"command\":\"conflict\"}}}",
    )
    .unwrap();
    assert!(call("claude-code", "setup", &f.setup()).is_err());
    fs::write(f.root.join("opencode.jsonc"), b"{} // retain").unwrap();
    assert!(call("opencode", "setup", &f.setup()).is_err());
    fs::remove_file(&path).unwrap();
    symlink(f.root.join("native"), &path).unwrap();
    assert!(call("claude-code", "setup", &f.setup()).is_err());
}
#[test]
fn codex_setup_preserves_comments_and_explicit_tool_approval() {
    let f = Fixture::new("exit 99");
    let dir = f.root.join(".codex");
    fs::create_dir(&dir).unwrap();
    let text = format!(
        "# preserve\n[mcp_servers.ahu]\ncommand = {}\nargs=['mcp','serve']\n[mcp_servers.ahu.tools.ahu_agents_list]\napproval_mode='prompt' # preserve too\n",
        json!(f.exe)
    );
    fs::write(dir.join("config.toml"), &text).unwrap();
    let r = call("codex", "setup", &f.setup()).unwrap();
    let text = r["changes"][0]["text"].as_str().unwrap();
    assert!(text.contains("# preserve too"));
    let v: toml::Value = toml::from_str(text).unwrap();
    assert_eq!(
        v["mcp_servers"]["ahu"]["tools"]["ahu_agents_list"]["approval_mode"].as_str(),
        Some("prompt")
    );
}
#[test]
fn auth_returns_stable_fingerprint_without_identity_or_raw_output() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new(
        "printf '{\"loggedIn\":true,\"email\":\"fixture@example.invalid\",\"orgId\":\"synthetic-org\",\"authMethod\":\"oauth\",\"apiProvider\":\"anthropic\",\"token\":\"private-sentinel\"}'",
    );
    let result = call("claude-code", "auth", &f.probe()).unwrap();
    let identity = json!({"harness":"claude-code","identity_kind":"oauth/anthropic","principal":"fixture@example.invalid","organization":"synthetic-org","workspace":null});
    let expected = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&identity).unwrap())
    );
    assert_eq!(result["fingerprint"], expected);
    assert!(!result.to_string().contains("fixture@"));
    assert!(!result.to_string().contains("private-sentinel"));
    for h in ["claude-code", "antigravity", "kiro", "opencode"] {
        assert_eq!(
            call(h, "budget", &f.probe()).unwrap()["status"],
            "unsupported"
        );
    }
}
#[test]
fn kiro_api_keys_never_bind_cached_social_identity() {
    let f = Fixture::new(
        "printf '{\"accountType\":\"SocialGitHub\",\"email\":\"fixture@example.invalid\"}'",
    );
    assert_eq!(
        call("kiro", "auth", &f.probe()).unwrap()["status"],
        "available"
    );
    let mut p = f.probe();
    p["environment"] = json!({"KIRO_API_KEY":"synthetic-only"});
    assert!(call("kiro", "auth", &p).is_err());
}
#[test]
fn opencode_identity_is_explicitly_routed_or_unsupported() {
    let f = Fixture::new("exit 99");
    let mut p = f.probe();
    p["model"] = json!("other/model");
    assert_eq!(
        call("opencode", "auth", &p).unwrap()["status"],
        "unsupported"
    );
    p["model"] = json!("ollama/glm-5.3:cloud");
    p["ollama_host"] = json!("https://example.invalid");
    assert!(call("opencode", "auth", &p).is_err());
}
#[test]
fn admission_distinguishes_unknown_hooks_work_mode_and_guarded_controls() {
    let f = Fixture::new("exit 99");
    let mut p = f.admission();
    let clean = call("kiro", "admission", &p).unwrap();
    assert_eq!(clean["allowed"], true);
    fs::create_dir_all(f.root.join(".kiro/hooks")).unwrap();
    fs::write(f.root.join(".kiro/hooks/synthetic.json"), b"{}").unwrap();
    let unknown = call("kiro", "admission", &p).unwrap();
    assert_eq!(unknown["allowed"], false);
    p["work_mode"] = json!(true);
    let work = call("kiro", "admission", &p).unwrap();
    assert_eq!(work["guarded_checks"], "intentionally_skipped");
    assert_eq!(work["conformance"], "untested");
    p["permissions"] = json!("accept-edits");
    assert!(call("kiro", "admission", &p).is_err());
    p["permissions"] = json!("auto");
    p["work_mode"] = json!(false);
    p["version"] = json!("2.1.302");
    fs::create_dir(f.root.join(".claude")).unwrap();
    fs::write(
        f.root.join(".claude/settings.json"),
        br#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"unreviewed"}]}]}}"#,
    )
    .unwrap();
    let guarded = call("claude-code", "admission", &p).unwrap();
    assert_eq!(guarded["allowed"], true);
    assert_eq!(guarded["invocation_profile"]["args"][0], "--settings");
}
#[test]
fn bounded_helpers_require_the_reviewed_version_and_read_only_assignment() {
    let f = Fixture::new("exit 99");
    let mut p = f.admission();
    p["version"] = json!("2.1.270");
    p["helpers"]["policy"] = json!("bounded");
    assert!(call("claude-code", "admission", &p).is_err());
    p["helpers"]["assignment_writes"] = json!(false);
    let result = call("claude-code", "admission", &p).unwrap();
    assert_eq!(
        result["helper_profile"]["helper_tools"],
        json!(["Read", "Grep", "Glob"])
    );
    p["version"] = json!("2.1.302");
    assert!(call("claude-code", "admission", &p).is_err());
    assert!(call("kiro", "admission", &p).is_err());
}
#[test]
fn inventory_and_import_are_passive_and_reject_path_escape() {
    let f = Fixture::new("touch activated");
    let source = f.root.join("agent.md");
    fs::write(
        &source,
        "---\nmodel: exact-model\nhooks:\n  start: do-not-run\n---\nRead only.\n",
    )
    .unwrap();
    let p =
        json!({"project":f.root,"home":f.home,"import":{"path":source,"format":"claude-agent"}});
    let result = call("claude-code", "native_sources", &p).unwrap();
    assert_eq!(result["imported"]["instructions"], "Read only.\n");
    assert_eq!(result["imported"]["settings_applied"], false);
    assert!(!f.root.join("activated").exists());
    let mut p = p;
    p["import"]["path"] = json!(f.root.join("../outside"));
    assert!(call("claude-code", "native_sources", &p).is_err());
}
#[test]
fn public_trust_calls_reconcile_only_owned_receipts() {
    let f = Fixture::new("exit 99");
    let task = f.root.join("task");
    let worktree = f.root.join("worktree");
    fs::create_dir(&task).unwrap();
    fs::create_dir(&worktree).unwrap();
    let mut p = json!({"overrides":[],"task_dir":task,"worktree":worktree,"home":f.home,"permissions":"auto","mode":"interactive"});
    for h in ["codex", "claude-code", "antigravity"] {
        call(h, "trust_prepare", &p).unwrap();
        assert!(task.join("native-trust.json").exists());
        call(h, "trust_cleanup", &p).unwrap();
        assert!(!task.join("native-trust.json").exists());
    }
    p["permissions"] = json!("prompt");
    assert_eq!(
        call("codex", "trust_prepare", &p).unwrap()["status"],
        "not_applicable"
    );
    fs::write(task.join("native-trust.json"), "{}").unwrap();
    assert!(call("codex", "trust_prepare", &p).is_err());
}
#[test]
fn setup_skill_conflicts_and_invalid_typed_fields_refuse_without_mutation() {
    let f = Fixture::new("exit 99");
    let mut p = f.setup();
    p["skills"] = json!([{"name":"../escape","text":"synthetic"}]);
    assert!(call("kiro", "setup", &p).is_err());
    assert!(!f.root.join(".kiro").exists());
    let mut p = f.admission();
    p["work_mode"] = json!("true");
    assert!(call("kiro", "admission", &p).is_err());
}

#[test]
fn antigravity_account_banner_is_observed_without_input_or_approval() {
    let f = Fixture::new(
        "printf 'fixture@example.invalid Agent Platform GCP Project: synthetic-workspace\\n'; sleep 5",
    );
    let result = call("antigravity", "auth", &f.probe()).unwrap();
    assert_eq!(result["status"], "available");
    assert!(!result.to_string().contains("fixture@example"));
    fs::write(
        &f.exe,
        "#!/bin/sh\nprintf 'Do you trust this folder?\\n'; sleep 5\n",
    )
    .unwrap();
    assert!(call("antigravity", "auth", &f.probe()).is_err());
}
#[test]
fn codex_capacity_includes_account_fingerprint_and_rejects_partial_windows() {
    for secondary in ["null", r#"{"usedPercent":200,"windowDurationMins":60}"#] {
        let script = format!(
            r#"read -r initialize
printf '%s\n' '{{"id":1,"result":{{}}}}'
read -r initialized
read -r account
printf '%s\n' '{{"id":2,"result":{{"account":{{"type":"chatgpt","email":"fixture@example.invalid"}}}}}}'
read -r limits
printf '%s\n' '{{"id":3,"result":{{"rateLimits":{{"primary":{{"usedPercent":25,"windowDurationMins":300}},"secondary":{secondary}}}}}}}'
"#
        );
        let f = Fixture::new(&script);
        let result = call("codex", "budget", &f.probe()).unwrap();
        assert_eq!(result["fingerprint"].as_str().unwrap().len(), 64);
        assert!(!result.to_string().contains("fixture@"));
        if secondary == "null" {
            assert_eq!(result["status"], "available");
            assert_eq!(result["windows"][0]["remaining_percent"], 75.0);
        } else {
            assert_eq!(result["status"], "unknown");
            assert_eq!(result["windows"], json!([]));
        }
    }
}
#[test]
fn setup_refuses_writable_native_parent_and_inexact_codex_arguments() {
    let f = Fixture::new("exit 99");
    fs::create_dir(f.home.join(".gemini")).unwrap();
    fs::set_permissions(f.home.join(".gemini"), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(call("antigravity", "setup", &f.setup()).is_err());
    fs::create_dir(f.root.join(".codex")).unwrap();
    fs::write(
        f.root.join(".codex/config.toml"),
        format!(
            "[mcp_servers.ahu]\ncommand={}\nargs=['mcp',1,'serve']\n",
            json!(f.exe)
        ),
    )
    .unwrap();
    assert!(call("codex", "setup", &f.setup()).is_err());
}

#[test]
fn ollama_cloud_route_uses_explicit_loopback_identity_with_no_raw_output() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::Duration,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let start = std::time::Instant::now();
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        && start.elapsed() < Duration::from_secs(5) =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("synthetic server failed: {e}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buffer[..n]);
            assert!(request.len() < 8192);
        }
        assert!(request.starts_with(b"POST /api/me HTTP/1.1"));
        let body = r#"{"email":"fixture@example.invalid","id":"synthetic-account","token":"private-sentinel"}"#;
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
    });
    let f = Fixture::new("exit 99");
    let mut params = f.probe();
    params["model"] = json!("ollama/glm-5.3:cloud");
    params["ollama_host"] = json!(endpoint);
    let result = call("opencode", "auth", &params).unwrap();
    server.join().unwrap();
    assert_eq!(result["binding_harness"], "ollama");
    assert_eq!(result["effective_provider_address_verified"], false);
    assert!(!result.to_string().contains("private-sentinel"));
    assert!(!result.to_string().contains("fixture@"));
}
#[test]
fn kiro_declarative_agents_are_accepted_but_executable_unknown_fields_refuse() {
    let f = Fixture::new("exit 99");
    fs::create_dir_all(f.root.join(".kiro/agents")).unwrap();
    let path = f.root.join(".kiro/agents/reviewer.json");
    fs::write(
        &path,
        br#"{"name":"reviewer","prompt":"Read only.","model":"claude-sonnet-4.5","hooks":[]}"#,
    )
    .unwrap();
    assert_eq!(
        call("kiro", "admission", &f.admission()).unwrap()["allowed"],
        true
    );
    fs::write(
        &path,
        br#"{"name":"reviewer","hooks":[{"command":"do-not-run"}]}"#,
    )
    .unwrap();
    assert_eq!(
        call("kiro", "admission", &f.admission()).unwrap()["allowed"],
        false
    );
}

#[test]
fn provider_selected_settings_keep_inventory_distinct_from_admission() {
    let f = Fixture::new(
        "case \"$1\" in whoami) printf '{\"accountType\":\"SocialGitHub\"}';; chat) printf '{\"models\":[{\"model_id\":\"claude-sonnet-4.5\"}]}';; *) exit 99;; esac",
    );
    let mut p = f.probe();
    p["environment_names"] = json!(["PATH", "synthetic_lowercase"]);
    p["settings"] = json!({"inspection_only":true});
    let inventory = call("kiro", "admission", &p).unwrap();
    assert_eq!(inventory["guarded_checks"], "inventory_only");
    assert_eq!(inventory["allowed"], false);
    p["settings"] = json!({});
    p["model"] = json!("claude-sonnet-4.5");
    p["version"] = json!("1.2.3");
    p["permissions"] = json!("auto");
    p["mode"] = json!("interactive");
    let admitted = call("kiro", "admission", &p).unwrap();
    assert_eq!(admitted["allowed"], true);
    p["mode"] = json!("headless");
    p["work_mode"] = json!(true);
    let work = call("kiro", "admission", &p).unwrap();
    assert_eq!(work["guarded_checks"], "intentionally_skipped");
    p["settings"]["unknown"] = json!(true);
    assert!(call("kiro", "admission", &p).is_err());
}

#[test]
fn masked_native_overrides_never_probe_the_default_account() {
    let f = Fixture::new("touch should-not-run; exit 99");
    for (harness, variable) in [
        ("codex", "CODEX_HOME"),
        ("claude-code", "ANTHROPIC_API_KEY"),
        ("opencode", "OLLAMA_HOST"),
        ("kiro", "KIRO_HOME"),
        ("antigravity", "GEMINI_API_KEY"),
    ] {
        let mut p = f.probe();
        p["environment_names"] = json!([variable]);
        assert!(call(harness, "auth", &p).is_err());
        assert!(call(harness, "budget", &p).is_err());
    }
    assert!(!f.root.join("should-not-run").exists());
}

#[test]
fn native_hook_view_redacts_commands_and_reports_unknown_scope() {
    let f = Fixture::new("exit 99");
    fs::create_dir(f.root.join(".claude")).unwrap();
    fs::write(f.root.join(".claude/settings.json"), r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"tool private-sentinel"}]}]},"permissions":{"defaultMode":"bypassPermissions"}}"#).unwrap();
    let mut p =
        json!({"project":f.root,"home":f.home,"settings":{"view":"hooks","environment_names":[]}});
    let v = call("claude-code", "native_sources", &p).unwrap();
    assert_eq!(v["hooks"].as_array().unwrap().len(), 1);
    assert!(!v.to_string().contains("private-sentinel"));
    assert_eq!(v["settings"][0]["approvals_widened"], true);
    p["settings"]["environment_names"] = json!(["CLAUDE_CONFIG_DIR"]);
    let v = call("claude-code", "native_sources", &p).unwrap();
    assert_eq!(v["unscanned_harness"], "claude-code");
    assert!(!v["unreadable"].as_array().unwrap().is_empty());
}

#[test]
fn exact_native_source_actions_preserve_delivery_and_model_conflicts() {
    let f = Fixture::new("exit 99");
    let text = "---\r\nmodel: exact-model\r\ntools: Read, Grep\r\n---\r\nDo the work.\n";
    let parsed=call("claude-code","native_sources",&json!({"action":"parse","format":"claude-agent","text":text,"path":".claude/agents/reviewer.md"})).unwrap();
    assert_eq!(
        parsed,
        json!({"instructions":"Do the work.\n","native_model":"exact-model","native_settings":{"model":"exact-model","tools":"Read, Grep"}})
    );
    let toml = "model = \"not-the-manifest-model\"\ninstructions = \"Keep all TOML\"\n";
    assert_eq!(call("codex","native_sources",&json!({"action":"parse","format":"codex-agent","text":toml,"path":".codex/agents/a.toml"})).unwrap(),json!({"instructions":toml,"native_model":null,"native_settings":{}}));
    assert!(
        call(
            "codex",
            "native_sources",
            &json!({"action":"parse","format":"claude-agent","text":text,"path":"x"})
        )
        .is_err()
    );
    fs::create_dir_all(f.root.join(".claude/agents")).unwrap();
    fs::write(f.root.join(".claude/agents/reviewer.md"), text).unwrap();
    let discovered = call(
        "claude-code",
        "native_sources",
        &json!({"action":"discover","project_dir":f.root}),
    )
    .unwrap();
    assert_eq!(discovered[0]["path"], ".claude/agents/reviewer.md");
    assert_eq!(discovered[0]["native_model"], "exact-model");
    assert_eq!(discovered[0].as_object().unwrap().len(), 7);
    symlink(f.exe, f.root.join(".claude/agents/linked.md")).unwrap();
    assert!(
        call(
            "claude-code",
            "native_sources",
            &json!({"action":"discover","project_dir":f.root})
        )
        .is_err()
    );
}
#[test]
fn exact_native_profile_freezes_controls_and_rejects_identity_substitution() {
    let mut p = json!({"action":"native_profile","request":{"harness":"codex","harness_version":"0.160.0","policy":"disabled","session_model":"gpt-6-astra","helper_model":null,"helper_role":"reviewer","max_concurrent":1,"max_depth":1,"budget_usd":null,"assignment_writes":true}});
    let profile = call("codex", "admission", &p).unwrap();
    let args = profile["args"].as_array().unwrap();
    assert!(args.iter().any(|v| v == "agents.enabled=false"));
    assert!(args.iter().any(|v| v == "features.hooks=true"));
    assert!(call("kiro", "admission", &p).is_err());
    p["request"]["policy"] = json!("bounded");
    assert!(call("codex", "admission", &p).is_err());
}
#[test]
fn cleanup_reconciles_owned_receipt_after_eligibility_changes() {
    let f = Fixture::new("exit 99");
    let task = f.root.join("task");
    let tree = f.root.join("tree");
    fs::create_dir(&task).unwrap();
    fs::create_dir(&tree).unwrap();
    let mut p = json!({"task_dir":task,"worktree":tree,"home":f.home,"permissions":"prompt","mode":"interactive","overrides":[]});
    for permission in ["prompt", "accept-edits"] {
        p["permissions"] = json!(permission);
        assert_eq!(
            call("codex", "trust_prepare", &p).unwrap()["status"],
            "not_applicable"
        );
    }
    p["permissions"] = json!("auto");
    call("codex", "trust_prepare", &p).unwrap();
    assert!(task.join("native-trust.json").exists());
    p["permissions"] = json!("prompt");
    p["mode"] = json!("headless");
    call("codex", "trust_cleanup", &p).unwrap();
    assert!(!task.join("native-trust.json").exists());
}
#[test]
fn bounded_recovery_validates_export_and_never_replays() {
    use sha2::{Digest, Sha256};
    let export = json!({"info":{"id":"ses_synthetic123","outcome":"succeeded","time":{"created":1010,"updated":1900,"idle":1900}},"messages":[{"id":"prompt","type":"user","time":{"created":1011}},{"id":"answer","type":"assistant","finish":"stop","content":[],"time":{"created":1012,"completed":1800}},{"id":"idle","type":"idle","outcome":"succeeded","time":{"created":1900}}]});
    let f = Fixture::new(&format!(
        "test \"$1 $2 $3 $4 $5\" = 'session export --sanitize --standalone ses_synthetic123' || exit 98\nprintf '%s' '{}'",
        export
    ));
    let digest = format!("{:x}", Sha256::digest(fs::read(&f.exe).unwrap()));
    let mut p = json!({"harness":"opencode","native_version":"2.0.32","native_executable":f.exe,"expected_executable_digest":digest,"worktree":f.root,"session":"ses_synthetic123","attempt_started_ms":1000});
    assert_eq!(
        call("opencode", "recover", &p).unwrap(),
        json!({"recovered":true})
    );
    p["attempt_started_ms"] = json!(2000);
    assert_eq!(
        call("opencode", "recover", &p).unwrap(),
        json!({"recovered":false})
    );
    p["expected_executable_digest"] = json!("a".repeat(64));
    assert!(call("opencode", "recover", &p).is_err());
    p["native_version"] = json!("2.0.33");
    assert_eq!(
        call("opencode", "recover", &p).unwrap(),
        json!({"recovered":false})
    );
}
