//! Frozen invocation controls must match the version used for admission.
use ahu::agent::Permissions;
use ahu::harness::{LaunchRequest, isolation};
use ahu::headless::{Options, Spec, batch_command};

#[test]
fn decorated_codex_versions_keep_frozen_controls_on_launch_and_resume() {
    let root = tempfile::tempdir().unwrap();
    for version in ["0.157.1", "0.160.0"] {
        let profile = isolation::profile("codex", version).unwrap();
        let decorated = format!("codex-cli v{version}");
        assert_eq!(isolation::profile("codex", &decorated), Some(profile));
        for permissions in [
            Permissions::Prompt,
            Permissions::Auto,
            Permissions::AcceptEdits,
        ] {
            let request = LaunchRequest {
                model: "synthetic-model",
                prompt: "literal prompt",
                cwd: root.path(),
                permissions,
            };
            let mut spec: Spec = serde_json::from_value(serde_json::json!({
                "schema_version": 2, "options": Options::default(),
                "harness_version": decorated, "executable_digest": "synthetic",
                "depth": 0, "attempt": 1, "native_controls": [], "gaps": []
            }))
            .unwrap();
            for session in [None, Some("synthetic-session".into())] {
                spec.session = session;
                spec.native_controls.clear();
                assert!(batch_command("codex", &request, &spec).is_err());
                spec.native_controls.push(profile.id.into());
                let result = batch_command("codex", &request, &spec);
                if permissions == Permissions::AcceptEdits && spec.session.is_some() {
                    assert!(result.is_err());
                    continue;
                }
                let command = result.unwrap();
                assert!(
                    command
                        .args
                        .windows(profile.args.len())
                        .any(|args| args == profile.args)
                );
                assert!(
                    !command
                        .args
                        .iter()
                        .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox")
                );
            }
        }
    }
    for version in ["v0.160.1", "v0.160.0-beta.1", "vv0.160.0"] {
        assert!(isolation::profile("codex", version).is_none());
    }
}

#[test]
fn reviewed_profiles_are_required_on_launch_and_resume_without_relocating_native_config() {
    let root = tempfile::tempdir().unwrap();
    let request = LaunchRequest {
        model: "synthetic-model",
        prompt: "literal prompt",
        cwd: root.path(),
        permissions: Permissions::Prompt,
    };
    for (harness, version, other_version) in [
        ("codex", "0.157.1", "0.160.0"),
        ("codex", "0.160.0", "0.157.1"),
        ("claude-code", "2.1.283", "2.1.288"),
        ("claude-code", "2.1.288", "2.1.283"),
        ("claude-code", "2.1.289 (Claude Code)", "2.1.288"),
        ("claude-code", "2.1.302 (Claude Code)", "2.1.289"),
        ("claude-code", "v2.1.302", "2.1.289"),
    ] {
        let profile = isolation::profile(harness, version).unwrap();
        let other = isolation::profile(harness, other_version).unwrap();
        let mut spec: Spec = serde_json::from_value(serde_json::json!({
            "schema_version": 2,
            "options": Options::default(),
            "harness_version": version,
            "executable_digest": "synthetic",
            "depth": 0,
            "attempt": 1,
            "native_controls": [],
            "gaps": []
        }))
        .unwrap();
        for session in [None, Some("synthetic-session".into())] {
            spec.session = session;
            spec.native_controls.clear();
            assert!(batch_command(harness, &request, &spec).is_err());
            spec.native_controls.push(other.id.into());
            assert!(batch_command(harness, &request, &spec).is_err());
            spec.native_controls = vec![profile.id.into()];
            let command = batch_command(harness, &request, &spec).unwrap();
            assert!(
                command
                    .args
                    .windows(profile.args.len())
                    .any(|args| args == profile.args)
            );
            assert!(
                command
                    .args
                    .windows(2)
                    .any(|args| args == ["--model", request.model])
            );
            assert_eq!(command.args[command.prompt_arg.unwrap()], request.prompt);
            for forbidden in [
                "--bare",
                "--safe-mode",
                "--ignore-user-config",
                "--setting-sources",
                "--strict-mcp-config",
                "--disable-slash-commands",
                "--dangerously-bypass-hook-trust",
                "features.hooks=false",
            ] {
                assert!(!command.args.iter().any(|arg| arg == forbidden));
            }
        }
    }
    for (harness, version) in [
        ("codex", "0.160.1"),
        ("claude-code", "2.1.290"),
        ("claude-code", "2.1.289-beta.1"),
        ("claude-code", "2.1.301"),
        ("claude-code", "2.1.303"),
        ("claude-code", "2.1.302-beta.1"),
        ("claude-code", "2.1.302+build.1"),
        ("opencode", "1.18.34"),
        ("antigravity", "1.2.16"),
    ] {
        assert!(isolation::profile(harness, version).is_none());
    }
}
