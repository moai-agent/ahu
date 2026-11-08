//! Native model and CLI option checks live in the plugin, never the coordinator.
use ahu_harness_native::call;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

fn params(root: &std::path::Path, version: &str) -> Value {
    json!({"project":root.join("project"),"home":root.join("home"),"native_executable":root.join("opencode"),"environment":{},"version":version,"model":"ollama/glm-5.3:cloud","permissions":"prompt","mode":"interactive","work_mode":false,"overrides":[],"managed_claude":null,"managed_opencode":root.join("managed"),"managed_preferences":root.join("managed-preferences"),"system_codex":null,"codex_managed_preferences":"not_applicable","trust_opencode_native_state":false,"helpers":{"policy":"disabled","helper_model":null,"helper_role":"reader","max_concurrent":1,"max_depth":1,"budget_usd":null,"assignment_writes":false}})
}
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("project")).unwrap();
    std::fs::create_dir(tmp.path().join("home")).unwrap();
    tmp
}
fn native(root: &std::path::Path, help: &str, model: &str, exit: u8) {
    // Only fixed synthetic strings are used in these shell fixtures.
    std::fs::write(root.join("opencode"),format!("#!/bin/sh\nif [ \"$1\" = models ]; then printf '%s\\n' '{model}'; exit 0; fi\nprintf '%s\\n' '{help}'\nexit {exit}\n")).unwrap();
    std::fs::set_permissions(
        root.join("opencode"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
}
#[test]
fn interactive_model_absence_refuses_before_native_launch() {
    let tmp = fixture();
    let root = tmp.path().canonicalize().unwrap();
    native(&root, "--model MODEL\n--prompt PROMPT", "ollama/other", 0);
    assert!(call("opencode", "admission", &params(&root, "2.0.32")).is_err());
}
#[test]
fn interactive_help_is_checked_each_time_and_failed_probes_are_not_evidence() {
    let tmp = fixture();
    let root = tmp.path().canonicalize().unwrap();
    for (help, exit, allowed) in [
        ("--model MODEL\n--prompt PROMPT", 0, true),
        ("--prompt PROMPT", 0, false),
        ("--model MODEL\n--prompt PROMPT", 0, true),
        ("--model MODEL\n--prompt PROMPT", 1, false),
    ] {
        native(&root, help, "ollama/glm-5.3:cloud", exit);
        let result = call("opencode", "admission", &params(&root, "2.0.32"));
        assert_eq!(result.is_ok(), allowed, "{result:?}");
    }
}
#[test]
fn mini_without_automatic_approval_cannot_accept_auto_permission() {
    let tmp = fixture();
    let root = tmp.path().canonicalize().unwrap();
    native(
        &root,
        "--model MODEL\n--prompt PROMPT",
        "ollama/glm-5.3:cloud",
        0,
    );
    let mut p = params(&root, "2.0.32");
    p["permissions"] = json!("auto");
    assert!(call("opencode", "admission", &p).is_err());
}
