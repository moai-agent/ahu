//! Synthetic native configuration and executable fixtures; no real cmux install.
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use ahu::cmux::integration::{self, Isolation, Locations, Registration};
use serde_json::{Value, json};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        std::fs::create_dir(&home).unwrap();
        let bin = root.join("native ; $literal");
        Self {
            _temp: temp,
            root,
            home,
            bin,
        }
    }
    fn write(&self, path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) {
        let path = path.as_ref();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    fn locations(&self) -> Locations {
        Locations {
            home: Some(self.home.clone()),
            ..Locations::default()
        }
    }
    fn inspect(&self, harness: &str) -> integration::Status {
        integration::inspect_in(&self.root, harness, &self.locations())
    }
    fn native(&self, code: i32, version: &str) {
        use std::os::unix::fs::PermissionsExt;
        self.write(
            &self.bin,
            format!(
                r#"#!/bin/sh
if [ "$1" = --version ]; then printf '%s\n' '{}'; exit 0; fi
printf '%s\n' "$@" > "$HOME/argv"
printf 'native confirmation preserved\n' >&2
read reply
printf '%s' "$reply" > "$HOME/reply"
exit {code}
"#,
                version
            ),
        );
        std::fs::set_permissions(&self.bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    fn command(&self) -> Command {
        let mut command = common::ahu();
        command
            .current_dir(&self.root)
            .env("AHU_CMUX_BIN", &self.bin)
            .env("HOME", &self.home);
        for key in [
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "XDG_CONFIG_HOME",
            "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_DIR",
            "OPENCODE_CONFIG_CONTENT",
            "GEMINI_CLI_HOME",
            "AGY_CONFIG_DIR",
        ] {
            command.env_remove(key);
        }
        command
    }
}

#[test]
fn absence_is_scoped_and_inspection_never_installs() {
    let f = Fixture::new();
    for harness in ["codex", "claude-code", "opencode", "antigravity"] {
        let result = f.inspect(harness);
        assert!(result.headless.allowed, "{result:?}");
        assert!(
            result
                .components
                .iter()
                .all(|c| c.registration == Registration::Missing)
        );
        assert!(!result.gaps.is_empty());
    }
    assert_eq!(std::fs::read_dir(&f.home).unwrap().count(), 0);
}

#[test]
fn orphan_named_file_and_guard_substrings_do_not_prove_registration() {
    let f = Fixture::new();
    let script = f
        .home
        .join(".cmux/hooks/cmux-codex-hook-persistent-stop.sh");
    f.write(
        &script,
        "#!/bin/sh\n# CMUX_SURFACE_ID CMUX_CODEX_HOOKS_DISABLED\necho synthetic\n",
    );
    assert!(f.inspect("codex").headless.allowed);
    let registration = json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":script}]}]}});
    f.write(f.home.join(".codex/hooks.json"), registration.to_string());
    let status = f.inspect("codex");
    assert!(!status.headless.allowed);
    assert!(
        !status
            .components
            .iter()
            .any(|c| c.registration == Registration::Installed)
    );
}

#[test]
fn malformed_unreadable_special_and_oversize_files_are_unknown() {
    let f = Fixture::new();
    let path = f.home.join(".codex/hooks.json");
    for bytes in [
        "{",
        "[]",
        r#"{"hooks":null}"#,
        r#"{"hooks":{"Stop":{}}}"#,
        r#"{"hooks":{"Stop":[{}]}}"#,
    ] {
        f.write(&path, bytes);
        assert!(!f.inspect("codex").headless.allowed, "{bytes}");
    }
    f.write(&path, vec![b' '; 1024 * 1024 + 1]);
    assert!(!f.inspect("codex").headless.allowed);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(!f.inspect("codex").headless.allowed);
}

#[test]
fn symlink_and_dangling_parent_cannot_become_missing_evidence() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    symlink(f.root.join("absent"), f.home.join(".codex")).unwrap();
    assert!(!f.inspect("codex").headless.allowed);
    std::fs::remove_file(f.home.join(".codex")).unwrap();
    std::fs::create_dir(f.home.join(".codex")).unwrap();
    symlink(f.root.join("absent"), f.home.join(".codex/hooks.json")).unwrap();
    assert!(!f.inspect("codex").headless.allowed);
}

#[test]
fn overrides_are_not_reported_as_default_absence() {
    let f = Fixture::new();
    for (harness, name) in [
        ("codex", "CODEX_HOME"),
        ("claude-code", "CLAUDE_CONFIG_DIR"),
        ("opencode", "OPENCODE_CONFIG_CONTENT"),
        ("antigravity", "GEMINI_CLI_HOME"),
    ] {
        let mut locations = f.locations();
        locations
            .overrides
            .insert(name.into(), "synthetic confidential config".into());
        let status = integration::inspect_in(&f.root, harness, &locations);
        assert!(!status.headless.allowed);
        assert!(
            !serde_json::to_string(&status)
                .unwrap()
                .contains("synthetic confidential config")
        );
    }
}

#[test]
fn partial_plugins_changed_feed_and_declared_modules_refuse_headless() {
    let f = Fixture::new();
    let feed = f.home.join(".config/opencode/plugins/cmux-feed.js");
    f.write(
        &feed,
        "// cmux-feed-plugin-marker v1\nconnectToFallbackSocket();\n",
    );
    let status = f.inspect("opencode");
    assert!(!status.headless.allowed);
    assert!(status.next_action.contains("uninstall"));
    assert!(
        status
            .components
            .iter()
            .any(|c| c.isolation == Isolation::Unknown)
    );
    std::fs::remove_file(feed).unwrap();
    f.write(
        f.root.join("opencode.json"),
        r#"{"plugin":["arbitrary-native-module"]}"#,
    );
    assert!(!f.inspect("opencode").headless.allowed);
}

#[test]
fn duplicate_registration_is_not_deduplicated_or_silently_trusted() {
    let f = Fixture::new();
    let entry = json!({"type":"command", "command":"cmux hooks codex stop"});
    f.write(
        f.home.join(".codex/hooks.json"),
        json!({"hooks":{"Stop":[{"hooks":[entry, entry]}]}}).to_string(),
    );
    let status = f.inspect("codex");
    assert_eq!(
        status
            .components
            .iter()
            .filter(|c| c.name.starts_with("Stop registration"))
            .count(),
        2
    );
    assert!(!status.headless.allowed);
}

#[test]
fn native_trust_data_does_not_imply_activation_and_profile_notify_is_unknown() {
    let f = Fixture::new();
    let path = f.home.join(".codex/config.toml");
    f.write(
        &path,
        "[features]\nhooks = true\n[hooks.state.synthetic]\ntrusted_hash = 'sha256:synthetic'\n",
    );
    let status = f.inspect("codex");
    assert!(status.headless.allowed, "{status:?}");
    assert!(
        !status
            .components
            .iter()
            .any(|c| c.activation == integration::Activation::Enabled)
    );
    f.write(&path, "[profiles.synthetic]\nnotify = ['cmux', 'notify']\n");
    assert!(!f.inspect("codex").headless.allowed);
}

#[test]
fn native_installer_receives_fixed_argv_and_preserves_confirmation_and_exit() {
    use std::io::Write;
    use std::process::Stdio;
    let f = Fixture::new();
    f.native(23, "cmux 0.64.22 (102) [ddd4a01bc]");
    let mut child = f
        .command()
        .args(["cmux", "install", "--harness", "opencode"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"native yes\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(f.home.join("argv")).unwrap(),
        "hooks\nopencode\ninstall\n"
    );
    assert_eq!(
        std::fs::read_to_string(f.home.join("reply")).unwrap(),
        "native yes"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Native operation:"));
    assert!(stderr.contains("native confirmation preserved"));
    assert!(stderr.contains("cmux integration opencode"));
}

#[test]
fn dry_run_claude_guidance_unknown_version_and_parser_never_install() {
    let f = Fixture::new();
    f.native(0, "0.64.22 (102) [ddd4a01bc]");
    let output = f
        .command()
        .args(["cmux", "install", "--harness", "codex", "--dry-run"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["argv"], json!(["hooks", "codex", "install"]));
    assert_eq!(plan["available"], true);
    let output = f
        .command()
        .args(["cmux", "install", "--harness", "claude-code"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("wrapper-managed"));
    f.native(0, "0.64.220 (102) [ddd4a01bc]");
    let output = f
        .command()
        .args(["cmux", "install", "--harness", "codex"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    for args in [
        vec!["cmux", "install", "--harness", "codex; false"],
        vec!["cmux", "install", "--harness", "codex", "--yes"],
        vec!["cmux", "install", "--harness", "codex", "--project"],
        vec!["cmux", "status", "--output", "text"],
    ] {
        assert!(!f.command().args(args).output().unwrap().status.success());
    }
    assert!(!f.home.join("argv").exists());
}

#[test]
fn human_json_status_agree_without_socket_or_mutation() {
    let f = Fixture::new();
    f.native(0, "0.64.22 (102) [ddd4a01bc]");
    let output = f
        .command()
        .args(["cmux", "status", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["integrations"].as_array().unwrap().len(), 4);
    let human = f.command().args(["cmux", "status"]).output().unwrap();
    let text = String::from_utf8(human.stdout).unwrap();
    for status in value["integrations"].as_array().unwrap() {
        assert!(text.contains(&format!(
            "cmux integration {}:",
            status["harness"].as_str().unwrap()
        )));
    }
    assert!(!f.home.join("argv").exists());
    assert!(!f.home.join(".codex").exists());
    assert!(!f.home.join(".config").exists());
}

#[test]
fn sanitation_removes_routing_and_sets_native_disable_controls() {
    let mut command = Command::new("synthetic");
    let policy = integration::HeadlessPolicy {
        allowed: true,
        disable_variable: Some("CMUX_CODEX_HOOKS_DISABLED".into()),
        reasons: vec![],
        evidence_digest: String::new(),
    };
    integration::sanitize(&mut command, Some(&policy));
    let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
    assert_eq!(
        env[std::ffi::OsStr::new("CMUX_CODEX_HOOKS_DISABLED")],
        Some(std::ffi::OsStr::new("1"))
    );
    let mut absent = Command::new("synthetic");
    integration::sanitize(&mut absent, None);
    assert!(
        absent
            .get_envs()
            .filter(|(key, _)| key.to_string_lossy().starts_with("CMUX_"))
            .all(|(_, value)| value.is_none())
    );
}

#[test]
fn wrapper_evidence_does_not_spill_into_other_harnesses_and_old_digests_roundtrip() {
    let f = Fixture::new();
    let locations = ahu::hooks::Locations {
        home: Some(f.home.clone()),
        managed: None,
        cmux_wrapper: true,
    };
    let found = ahu::hooks::collect_for(&f.root, "codex", &locations).unwrap();
    assert!(!found.wrapper_injected);
    let old = ahu::hooks::HookInventory {
        wrapper_injected: true,
        ..Default::default()
    };
    let digest = old.digest();
    let serialized = serde_json::to_string(&old).unwrap();
    let restored: ahu::hooks::HookInventory = serde_json::from_str(&serialized).unwrap();
    assert_eq!(digest, restored.digest());
    assert!(restored.wrapper_injected);
}

#[test]
fn ancestor_project_hooks_are_not_mistaken_for_a_clean_leaf() {
    let f = Fixture::new();
    let project = f.root.join("parent/leaf");
    std::fs::create_dir_all(&project).unwrap();
    f.write(
        f.root.join("parent/.codex/hooks.json"),
        json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"synthetic-unknown"}]}]}})
            .to_string(),
    );
    let status = integration::inspect_in(&project, "codex", &f.locations());
    assert!(!status.headless.allowed);
    assert!(
        status
            .components
            .iter()
            .any(|c| c.registration == Registration::Unknown
                && c.evidence[0].scope == "project ancestor")
    );
}

#[test]
fn unsupported_install_scope_and_missing_cli_have_reviewable_plans() {
    let f = Fixture::new();
    let cli = integration::NativeCli {
        executable: Some(f.bin.clone()),
        version: Some("0.64.22 (102) [ddd4a01bc]".into()),
        installer_supported: true,
        detail: "synthetic".into(),
    };
    let mut locations = f.locations();
    locations.overrides.insert(
        "CODEX_HOME".into(),
        f.root.join("custom").display().to_string(),
    );
    let plan = integration::installation_plan_in("codex", &cli, &locations).unwrap();
    assert!(!plan.available);
    assert_eq!(plan.unresolved_overrides, ["CODEX_HOME"]);
    let plan = integration::installation_plan_in("codex", &cli, &f.locations()).unwrap();
    assert_eq!(
        plan.affected_paths,
        [
            f.home.join(".codex/hooks.json"),
            f.home.join(".codex/config.toml"),
            f.home.join(".cmux/hooks")
        ]
    );
    let output = f
        .command()
        .args(["cmux", "status", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["cli"]["installer_supported"], false);
}

#[test]
fn native_cancellation_status_is_not_replaced_with_success() {
    let f = Fixture::new();
    f.native(130, "0.64.22 (102) [ddd4a01bc]");
    let output = f
        .command()
        .args(["cmux", "install", "--harness", "antigravity"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert_eq!(
        std::fs::read_to_string(f.home.join("argv")).unwrap(),
        "hooks\nantigravity\ninstall\n"
    );
}

#[test]
fn file_evidence_is_bounded_and_terminal_paths_are_escaped() {
    let f = Fixture::new();
    let plugin = f.home.join(".config/opencode/plugins/\x1b[31m.js");
    f.write(plugin, "synthetic native plugin");
    let status = f.inspect("opencode");
    assert!(!integration::render(&status).contains('\x1b'));
    let json = serde_json::to_value(&status).unwrap();
    assert!(!json.to_string().contains("synthetic native plugin"));
    assert!(
        status
            .components
            .iter()
            .any(|c| c.evidence.iter().any(|e| e.digest.is_some()))
    );
}

#[test]
fn headless_dry_run_uses_the_same_status_and_refuses_unknown_registration() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let repo = common::TestRepo::new();
    repo.init_config();
    repo.add_agent_on("worker", "1.0.0", "claude-code", "claude-opus-5");
    repo.commit("synthetic agent configuration");
    let executable = f.root.join("bin/claude");
    f.write(
        &executable,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 2.1.270; exit 0; fi\nexit 99\n",
    );
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let launch = || {
        f.command()
            .current_dir(repo.path())
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", f.root.join("bin").display()),
            )
            .env("AHU_RUNTIME_DIR", f.root.join("runtime"))
            .args([
                "launch",
                "@worker",
                "--headless",
                "--dry-run",
                "--output",
                "json",
                "--prompt",
                "synthetic task",
            ])
            .output()
            .unwrap()
    };
    let result = launch();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let preview: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(preview["cmux_integration"]["harness"], "claude-code");
    assert_eq!(preview["cmux_integration"]["headless"]["allowed"], true);
    let digest = preview["cmux_integration"]["headless"]["evidence_digest"]
        .as_str()
        .unwrap();
    assert!(
        preview["capabilities"]["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains(digest))
    );
    assert!(!Path::new(preview["worktree"].as_str().unwrap()).exists());
    f.write(
        f.home.join(".claude/settings.json"),
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"opaque-native-hook"}]}]}}"#,
    );
    let result = launch();
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("headless cmux isolation is unverified")
    );
    assert!(!Path::new(preview["worktree"].as_str().unwrap()).exists());
}

#[test]
fn legacy_user_plugin_scope_refuses_opaque_feed_and_inventory_is_not_isolation() {
    let f = Fixture::new();
    let path = f.home.join(".opencode/plugins/cmux-feed.js");
    f.write(
        &path,
        "// cmux-feed-plugin-marker v1\nconnectToFallbackSocket();\n",
    );
    let inventory = ahu::hooks::collect_for(
        &f.root,
        "opencode",
        &ahu::hooks::Locations {
            home: Some(f.home.clone()),
            managed: None,
            cmux_wrapper: false,
        },
    )
    .unwrap();
    // The former admission check looked only at hook.command. Plugin paths
    // reside in declared_plugins and cannot establish their isolation.
    assert!(
        inventory
            .hooks
            .iter()
            .all(|h| h.command.as_deref().is_none_or(|c| !c.contains("cmux")))
    );
    assert!(
        inventory
            .declared_plugins
            .iter()
            .any(|p| p.source == path.to_string_lossy())
    );
    let status = f.inspect("opencode");
    assert!(!status.headless.allowed);
    assert!(
        status
            .components
            .iter()
            .any(|c| c.evidence[0].path == path && c.registration == Registration::Unknown)
    );
}
