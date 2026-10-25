use serde::Deserialize;
use serde_json::{Value, json};
const PROMPT: &str = "<prompt: see prompt.txt>";
#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Interactive,
    Headless,
    Direct,
}
#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Permissions {
    Prompt,
    AcceptEdits,
    Auto,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    mode: Mode,
    model: String,
    permissions: Permissions,
    work_mode: bool,
    session: Option<String>,
    native_version: String,
    timeout_seconds: u64,
    cwd: String,
    broker_dir: Option<String>,
    native_profile: Option<Value>,
    native_controls: Vec<String>,
}
fn identity(value: &str, limit: usize, alphabet: &[u8]) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || alphabet.contains(&b))
}
pub(super) fn plan(harness: &str, params: Value) -> Result<Value, ()> {
    let r: Request = serde_json::from_value(params).map_err(|_| ())?;
    if !identity(&r.model, 256, b"-_.:/+")
        || !(1..=604800).contains(&r.timeout_seconds)
        || r.session.as_ref().is_some_and(|s| !identity(s, 128, b"-_"))
        || !std::path::Path::new(&r.cwd).is_absolute()
        || r.cwd.contains('\0')
        || r.work_mode && r.permissions != Permissions::Auto
        || r.mode != Mode::Headless && (r.session.is_some() || r.work_mode)
        || r.native_controls.len() > 128
    {
        return Err(());
    }
    // Version-specific flags belong to the package and cannot be guessed by core.
    let reviewed = match harness {
        "codex" => ["0.157.1", "0.160.0", "0.163.0"].contains(&r.native_version.as_str()),
        "claude-code" => ["2.1.283", "2.1.288", "2.1.289", "2.1.302", "2.1.310"]
            .contains(&r.native_version.as_str()),
        "antigravity" => r.native_version == "1.4.7",
        "opencode" => r.native_version == "2.0.32",
        "kiro" => ["2.28.0", "2.29.0", "2.34.0"].contains(&r.native_version.as_str()),
        _ => false,
    };
    if !reviewed {
        return Err(());
    }
    if harness == "opencode"
        && !r
            .model
            .split_once('/')
            .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty() && !b.contains('/'))
    {
        return Err(());
    }
    let program = match harness {
        "claude-code" => "claude",
        "antigravity" => "agy",
        "kiro" => "kiro-cli",
        other => other,
    };
    let mut args: Vec<String> = Vec::new();
    let mut add = |values: &[&str]| args.extend(values.iter().map(|s| (*s).to_owned()));
    if r.mode == Mode::Direct && r.permissions == Permissions::Auto {
        match harness {
            "codex" => add(&["-m", &r.model, "--dangerously-bypass-approvals-and-sandbox"]),
            "claude-code" => add(&["--model", &r.model, "--dangerously-skip-permissions"]),
            "antigravity" => add(&["--model", &r.model, "--dangerously-skip-permissions"]),
            "kiro" => add(&["chat", "--v3", "--model", &r.model, "--trust-all-tools"]),
            // The reviewed mini CLI has no unattended permission override.
            "opencode" => return Err(()),
            _ => return Err(()),
        }
        return Ok(
            json!({"program":program,"args":args,"prompt_arg":null,"native_eval_otlp":harness != "kiro"}),
        );
    }
    if r.mode == Mode::Headless {
        match harness {
            "codex" => {
                add(&["exec"]);
                if r.session.is_some() {
                    add(&["resume"]);
                }
                add(&[
                    "--model",
                    &r.model,
                    "--json",
                    "-c",
                    "approval_policy=\"never\"",
                ]);
                if !r.work_mode {
                    add(&["-c", "agents.enabled=false"]);
                }
                match r.permissions {
                    Permissions::Prompt => add(&["-c", "sandbox_mode=\"read-only\""]),
                    Permissions::Auto => add(&[
                        "-c",
                        if r.work_mode {
                            "sandbox_mode=\"danger-full-access\""
                        } else {
                            "sandbox_mode=\"workspace-write\""
                        },
                    ]),
                    Permissions::AcceptEdits if r.session.is_none() => add(&["--approve-for-me"]),
                    _ => return Err(()),
                }
                if r.session.is_none() {
                    add(&["--color", "never"]);
                }
                if !r.work_mode && ["0.157.1", "0.160.0"].contains(&r.native_version.as_str()) {
                    let id = format!("codex-{}-effective-guarded-hooks-v1", r.native_version);
                    if !r.native_controls.contains(&id) {
                        return Err(());
                    }
                    add(&[
                        "--strict-config",
                        "-c",
                        "features.hooks=true",
                        "-c",
                        "features.plugins=false",
                        "-c",
                        "features.remote_plugin=false",
                        "-c",
                        "notify=[]",
                    ]);
                }
                if let Some(dir) = &r.broker_dir {
                    if !std::path::Path::new(dir).is_absolute() || dir.contains('\0') {
                        return Err(());
                    }
                    let quoted = serde_json::to_string(dir).map_err(|_| ())?;
                    add(&[
                        "-c",
                        &format!("sandbox_workspace_write.writable_roots=[{quoted}]"),
                    ]);
                }
                add(&["--"]);
                if let Some(session) = &r.session {
                    add(&[session]);
                }
            }
            "claude-code" => {
                add(&[
                    "--print",
                    "--model",
                    &r.model,
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--permission-prompts",
                    "none",
                ]);
                if !r.work_mode {
                    let id = format!("claude-{}-nonmanaged-hooks-disabled-v1", r.native_version);
                    if ["2.1.283", "2.1.288", "2.1.289", "2.1.302"]
                        .contains(&r.native_version.as_str())
                    {
                        if !r.native_controls.contains(&id) {
                            return Err(());
                        }
                        add(&["--settings", "{\"disableAllHooks\":true}"]);
                    }
                    if let Some(profile) = &r.native_profile {
                        let values: Vec<String> =
                            serde_json::from_value(profile.get("args").ok_or(())?.clone())
                                .map_err(|_| ())?;
                        if values.len() > 64
                            || values.iter().any(|s| s.len() > 65536 || s.contains('\0'))
                        {
                            return Err(());
                        }
                        add(&values.iter().map(String::as_str).collect::<Vec<_>>());
                    } else {
                        add(&["--disallowedTools", "Agent,Task,TeamCreate,TeamDelete"]);
                    }
                }
                match r.permissions {
                    Permissions::Prompt => (),
                    Permissions::Auto => add(&["--permission-mode", "auto"]),
                    Permissions::AcceptEdits => add(&["--permission-mode", "acceptEdits"]),
                }
                if let Some(session) = &r.session {
                    add(&["--resume", session]);
                }
                add(&["--"]);
            }
            "antigravity" => {
                add(&[
                    "--model",
                    &r.model,
                    "--output-format",
                    "stream-json",
                    "--print-timeout",
                    &format!("{}s", r.timeout_seconds),
                ]);
                match r.permissions {
                    Permissions::Prompt => (),
                    Permissions::Auto => add(&["--dangerously-skip-permissions"]),
                    Permissions::AcceptEdits => add(&["--mode", "accept-edits"]),
                }
                if let Some(session) = &r.session {
                    add(&["--conversation", session]);
                }
                add(&["--print"]);
            }
            "opencode" => {
                add(&[
                    "run",
                    "--standalone",
                    "--format",
                    "json",
                    "--model",
                    &r.model,
                ]);
                match r.permissions {
                    Permissions::Prompt => (),
                    Permissions::Auto => add(&["--auto"]),
                    Permissions::AcceptEdits => return Err(()),
                }
                if let Some(session) = &r.session {
                    add(&["--session", session]);
                }
                add(&["--"]);
            }
            "kiro" => {
                add(&[
                    "chat",
                    "--v3",
                    "--model",
                    &r.model,
                    "--no-interactive",
                    "--output-format",
                    "stream-json",
                    "--require-mcp-startup",
                ]);
                match r.permissions {
                    Permissions::Prompt => (),
                    Permissions::Auto => add(&["--trust-all-tools"]),
                    Permissions::AcceptEdits => return Err(()),
                }
                if let Some(session) = &r.session {
                    add(&["--resume-id", session]);
                }
            }
            _ => return Err(()),
        }
    } else {
        match harness {
            "codex" => {
                add(&["-m", &r.model, "--no-daemon"]);
                match r.permissions {
                    Permissions::Prompt => (),
                    Permissions::AcceptEdits => add(&["--approve-for-me"]),
                    Permissions::Auto => add(&[
                        "--ask-for-approval",
                        "never",
                        "--sandbox",
                        "workspace-write",
                    ]),
                }
                if r.mode == Mode::Interactive {
                    add(&["--"]);
                }
            }
            "claude-code" => {
                add(&["--model", &r.model]);
                match r.permissions {
                    Permissions::Prompt => (),
                    Permissions::AcceptEdits => add(&["--permission-mode", "acceptEdits"]),
                    Permissions::Auto => add(&[
                        "--permission-mode",
                        "auto",
                        "--settings",
                        "{\"enabledMcpjsonServers\":[\"ahu\"]}",
                    ]),
                }
                if r.mode == Mode::Interactive {
                    add(&["--"]);
                }
            }
            "antigravity" => {
                add(&["--model", &r.model]);
                match r.permissions {
                    Permissions::Prompt => (),
                    Permissions::AcceptEdits => add(&["--mode", "accept-edits"]),
                    Permissions::Auto => add(&["--dangerously-skip-permissions"]),
                }
                if r.mode == Mode::Interactive {
                    add(&["--prompt-interactive"]);
                }
            }
            "opencode" => {
                if r.permissions != Permissions::Prompt {
                    return Err(());
                }
                add(&["mini", "--model", &r.model]);
                if r.mode == Mode::Interactive {
                    add(&["--prompt"]);
                }
            }
            "kiro" => {
                add(&["chat", "--v3", "--model", &r.model]);
                match r.permissions {
                    Permissions::Prompt => (),
                    Permissions::Auto => add(&["--trust-all-tools"]),
                    Permissions::AcceptEdits => return Err(()),
                }
            }
            _ => return Err(()),
        }
    }
    let prompt_arg = if r.mode == Mode::Direct {
        None
    } else {
        let i = args.len();
        args.push(PROMPT.into());
        Some(i)
    };
    Ok(
        json!({"program":program,"args":args,"prompt_arg":prompt_arg,"native_eval_otlp":harness != "kiro"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn params(mode: &str, version: &str, model: &str) -> Value {
        json!({"mode":mode,"model":model,"permissions":if mode=="headless" {"auto"}else{"prompt"},"work_mode":mode=="headless","session":null,"native_version":version,"timeout_seconds":1800,"cwd":"/task/worktree","broker_dir":null,"native_profile":null,"native_controls":[]})
    }
    #[test]
    fn all_providers_plan_all_modes_with_literal_prompt_boundary() {
        for (h, v, m, p) in [
            ("codex", "0.163.0", "gpt-6-astra", "codex"),
            ("claude-code", "2.1.310", "claude-opus-5", "claude"),
            ("antigravity", "1.4.7", "model", "agy"),
            ("opencode", "2.0.32", "provider/model", "opencode"),
            ("kiro", "2.29.0", "model", "kiro-cli"),
        ] {
            for mode in ["interactive", "headless", "direct"] {
                let result = plan(h, params(mode, v, m)).unwrap();
                assert_eq!(result["program"], p);
                let args = result["args"].as_array().unwrap();
                if mode == "direct" {
                    assert!(result["prompt_arg"].is_null());
                    assert!(!args.contains(&json!(PROMPT)));
                } else {
                    assert_eq!(args.last(), Some(&json!(PROMPT)));
                    assert_eq!(result["prompt_arg"], args.len() - 1);
                }
            }
        }
    }
    #[test]
    fn direct_automatic_approval_is_never_silently_weakened() {
        for (h,v,flag) in [("codex","0.163.0","--dangerously-bypass-approvals-and-sandbox"),("claude-code","2.1.310","--dangerously-skip-permissions"),("antigravity","1.4.7","--dangerously-skip-permissions"),("kiro","2.29.0","--trust-all-tools")] {
            let mut request=params("direct",v,"model");request["permissions"]=json!("auto");
            let result=plan(h,request).unwrap();assert!(result["args"].as_array().unwrap().contains(&json!(flag)));
            assert!(result["prompt_arg"].is_null());
        }
        let mut request=params("direct","2.0.32","provider/model");request["permissions"]=json!("auto");
        assert!(plan("opencode",request).is_err());
    }
    #[test]
    fn invalid_flags_and_unsupported_permissions_are_refused() {
        for model in ["", "--help", "a b", "a\0b"] {
            assert!(plan("codex", params("headless", "0.163.0", model)).is_err());
        }
        assert!(plan("codex", params("headless", "999.0.0", "model")).is_err());
        assert!(plan("opencode", params("headless", "2.0.32", "model")).is_err());
        for h in ["opencode", "kiro"] {
            let mut p = params(
                "interactive",
                if h == "kiro" { "2.29.0" } else { "2.0.32" },
                "provider/model",
            );
            p["permissions"] = json!("accept-edits");
            assert!(plan(h, p).is_err());
        }
        let mut p = params("headless", "0.163.0", "model");
        p["session"] = json!("--session");
        assert!(plan("codex", p).is_err());
    }
}
