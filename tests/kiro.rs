mod common;

#[test]
fn shortcut_and_capabilities_are_explicit() {
    assert_eq!(ahu::cli::parse(["kiro"]).unwrap(), ahu::cli::Command::Kiro);
    assert!(ahu::cli::parse(["kiro", "--model", "auto"]).is_err());
    let entry = ahu::catalog::harness("kiro").unwrap();
    assert!(entry.supports(ahu::catalog::Feature::InteractiveLaunch));
    assert!(entry.supports(ahu::catalog::Feature::HeadlessLaunch));
    assert!(!entry.supports(ahu::catalog::Feature::AcceptEditsApprovals));
    assert!(!entry.supports(ahu::catalog::Feature::HookInventory));
    let repo = tempfile::tempdir().unwrap();
    let status = ahu::native_admission::inspect_in(
        repo.path(),
        "kiro",
        &ahu::native_admission::Locations::default(),
    )
    .with_version(Some("kiro-cli 2.28.0"));
    assert!(!status.headless.allowed);
}

#[test]
#[cfg(unix)]
fn coordinator_checks_exact_model_before_starting_and_withholds_native_errors() {
    let repo = common::TestRepo::new();
    repo.write(".agents/ahu/config.toml", &format!(
        "schema_version = 1\nharness_preferences = [\"kiro\"]\nmodel_selection = \"project-ranked\"\ncatalog_version = \"{}\"\n[model_rankings]\nkiro = [\"claude-sonnet-4.5\"]\n",
        ahu::catalog::CATALOG_VERSION
    ));
    repo.commit("Kiro policy");
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["kiro-cli"], |_| {
        scratch.path().join("unused")
    });
    std::fs::write(
        bin.join("kiro-cli"),
        r#"#!/bin/sh
if [ "$1" = whoami ]; then
  test "$AHU_TEST_KIRO" != signed-out || exit 1
  printf '%s\n' '{"accountType":"SocialGitHub","email":"fixture@example.invalid"}'
  exit 0
fi
if [ "$3" = --list-models ]; then
  case "$AHU_TEST_KIRO" in
    malformed) printf 'private-marker';;
    unavailable) printf '%s\n' '{"models":[{"model_id":"auto"}]}' ;;
    *) printf '%s\n' '{"models":[{"model_id":"claude-sonnet-4.5"}]}' ;;
  esac
  exit 0
fi
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
exit 7
"#,
    )
    .unwrap();
    for mode in ["available", "unavailable", "malformed", "signed-out"] {
        let args_path = scratch.path().join(mode);
        let result = common::ahu()
            .arg("kiro")
            .current_dir(repo.path())
            .env_remove("CMUX_WORKSPACE_ID")
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("AHU_TEST_KIRO", mode)
            .env("AHU_TEST_ARGS", &args_path)
            .output()
            .unwrap();
        let diagnostic = String::from_utf8_lossy(&result.stderr);
        assert!(!diagnostic.contains("private-marker"));
        if mode == "available" {
            assert_eq!(result.status.code(), Some(7), "{diagnostic}");
            assert_eq!(
                std::fs::read_to_string(args_path).unwrap(),
                "chat\n--v3\n--model\nclaude-sonnet-4.5\n--trust-all-tools\n"
            );
        } else {
            assert!(!result.status.success());
            assert!(
                !args_path.exists(),
                "must refuse before starting the interactive session"
            );
        }
    }
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

fn kiro_event(events: &mut ahu::headless::Events, event: serde_json::Value) {
    events.observe("kiro", &serde_json::to_vec(&event).unwrap());
}
fn finished() -> serde_json::Value {
    serde_json::json!({"type":"runFinished","data":{"sessionId":"sess_fixture","status":"success","stopReason":"end_turn","finalText":"private response","finalTextTruncated":false}})
}
fn kiro_started() -> ahu::headless::Events {
    let mut events = ahu::headless::Events::default();
    kiro_event(
        &mut events,
        serde_json::json!({"type":"runStarted","data":{"engine":"v3","payloadSchema":"acp","acpProtocolVersion":1}}),
    );
    events
}
#[test]
fn kiro_requires_final_run_result_and_preserves_denials_and_identity_failures() {
    use serde_json::json;
    let mut events = kiro_started();
    kiro_event(
        &mut events,
        json!({"type":"sessionUpdate","data":{"sessionId":"sess_fixture","update":{"sessionUpdate":"session_info_update","_meta":{"kiro":{"kind":"turn_end","stopReason":"end_turn"}}}}}),
    );
    assert!(!events.terminal);
    kiro_event(&mut events, finished());
    assert!(events.terminal && !events.failed);
    assert_eq!(events.session.as_deref(), Some("sess_fixture"));
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("private response")
    );
    assert!(events.cost.usd.is_none());
    kiro_event(&mut events, finished());
    assert!(events.failed);
    for (index, update) in [
        json!({"sessionUpdate":"tool_call_update","toolCallId":"a","status":"failed","rawOutput":{"message":"private denial"},"_meta":{"kiro":{"policyDenial":{"effect":"deny"}}}}),
        json!({"sessionUpdate":"session_info_update"}),
    ].into_iter().enumerate() {
        let mut events = kiro_started();
        kiro_event(
            &mut events,
            json!({"type":"sessionUpdate","data":{"sessionId":if index==0 {"sess_fixture"} else {"sess_other"},"update":update}}),
        );
        kiro_event(&mut events, finished());
        assert!(events.failed);
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("private denial")
        );
    }
    for (field, value) in [
        ("status", json!("interrupted")),
        ("stopReason", json!("max_tokens")),
        ("finalTextTruncated", json!(true)),
        ("sessionId", json!("--bad")),
    ] {
        let mut event = finished();
        event["data"][field] = value;
        let mut events = kiro_started();
        kiro_event(&mut events, event);
        assert!(events.failed, "{field}");
    }
    let mut events = kiro_started();
    kiro_event(
        &mut events,
        json!({"type":"runError","data":{"sessionId":"sess_fixture","stage":"prompt","message":"private provider error"}}),
    );
    assert!(events.failed && events.terminal);
    assert_eq!(events.session.as_deref(), Some("sess_fixture"));
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("private provider error")
    );
    let mut events = kiro_started();
    events.observe("kiro", b"{broken");
    assert!(events.failed && !events.terminal);
}

#[test]
fn kiro_headless_inspection_refuses_unreviewed_sources_without_reading_credentials() {
    use ahu::native_admission::{Locations, inspect_in};
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let locations = Locations {
        home: Some(home.path().canonicalize().unwrap()),
        ..Default::default()
    };
    assert!(
        inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations)
            .headless
            .allowed
    );
    for relative in [
        "hooks/start.json",
        "agents/custom.json",
        "extensions/custom.json",
        "powers/custom.json",
        "workflows/custom.json",
        "settings/cli.json",
        "web-session/config.json",
        "cloud-cache/user/config/state.json",
        "cloud-cache/user/config/hooks/test.json",
    ] {
        let path = home.path().join(".kiro").join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{\"private-marker\":true}").unwrap();
        let status = inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations);
        assert!(!status.headless.allowed, "{relative}");
        assert!(
            !serde_json::to_string(&status)
                .unwrap()
                .contains("private-marker")
        );
        std::fs::remove_file(path).unwrap();
    }
    assert!(
        inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations)
            .headless
            .allowed
    );
    let mut override_locations = locations.clone();
    override_locations
        .overrides
        .insert("KIRO_ACP_RECORD_PATH".into(), "private-marker".into());
    assert!(
        !inspect_in(
            &repo.path().canonicalize().unwrap(),
            "kiro",
            &override_locations
        )
        .headless
        .allowed
    );
}

#[test]
fn kiro_events_feed_partial_acp_call_evidence_without_inferred_usage() {
    use ahu::eval::trajectory::{Coverage, Source};
    use serde_json::json;
    let mut events = ahu::headless::Events::default();
    kiro_event(
        &mut events,
        json!({"type":"runStarted","data":{"engine":"v3","payloadSchema":"acp"}}),
    );
    kiro_event(
        &mut events,
        json!({"type":"sessionUpdate","data":{"sessionId":"sess_fixture","update":{"sessionUpdate":"tool_call","toolCallId":"fixture-call","title":"Read File","status":"pending"}}}),
    );
    kiro_event(
        &mut events,
        json!({"type":"sessionUpdate","data":{"sessionId":"sess_fixture","update":{"sessionUpdate":"tool_call_update","toolCallId":"fixture-call","status":"completed"}}}),
    );
    kiro_event(&mut events, finished());
    let observation = &events.trajectory.observation;
    assert_eq!(observation.source, Source::KiroSessionUpdates);
    assert_eq!(observation.coverage, Coverage::Partial);
    assert_eq!(observation.tool_calls, Some(1));
    assert_eq!(observation.completed_tool_calls, Some(1));
    assert_eq!(observation.tool_errors, Some(0));
    assert_eq!(observation.steps, None);
    assert_eq!(observation.repeated_tool_calls, None);
    assert!(events.usage.input.is_none());
    assert!(events.cost.usd.is_none());
}

#[test]
fn kiro_admission_retains_228_and_inspects_229_without_admitting_unknown_versions() {
    use ahu::native_admission::{Locations, inspect_in};
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let locations = Locations {
        home: Some(home.path().canonicalize().unwrap()),
        ..Default::default()
    };
    for version in ["kiro-cli 2.28.0", "kiro-cli 2.29.0"] {
        let status = inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations)
            .with_version(Some(version));
        assert!(
            status.headless.allowed,
            "{version}: {:?}",
            status.headless.reasons
        );
    }
    for version in [
        None,
        Some("unknown"),
        Some("2.27.0"),
        Some("2.29.1"),
        Some("2.29.0-beta.1"),
        Some("3.0.0"),
    ] {
        let status = inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations)
            .with_version(version);
        assert!(!status.headless.allowed, "{version:?}");
    }
    let entry = ahu::catalog::harness("kiro").unwrap();
    assert_eq!(entry.headless_verified_versions, &["2.28.0", "2.29.0"]);
}

#[test]
fn kiro_admits_declarative_native_agents_in_each_discovered_scope() {
    use ahu::native_admission::{Locations, inspect_in};
    use serde_json::json;
    let home = tempfile::tempdir().unwrap();
    let ancestor = tempfile::tempdir().unwrap();
    let repo = ancestor.path().join("project");
    std::fs::create_dir(&repo).unwrap();
    let repo = repo.canonicalize().unwrap();
    let locations = Locations {
        home: Some(home.path().canonicalize().unwrap()),
        ..Default::default()
    };
    let profile = json!({
        "name": "fixture-reviewer",
        "description": "Review cmux integration source",
        "prompt": "Read the requested source and report findings.",
        "model": "claude-sonnet-4.5",
        "tools": ["read", "shell", "@fixture"],
        "excludedTools": ["subagent"],
        "resources": ["file://README.md", "skill://.kiro/skills/**/SKILL.md"],
        "includeMcpJson": true,
        "includePowers": false,
        "permissions": {"rules": [{"capability":"shell", "match":["git diff *"], "effect":"ask"}]},
        "mcpServers": {"fixture": {"command":"fixture-mcp", "args":[], "env":{"FIXTURE":"synthetic"}, "disabled":true}},
        "hooks": []
    });
    for base in [home.path(), ancestor.path(), repo.as_path()] {
        let agents = base.join(".kiro/agents/team");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("reviewer.json"), profile.to_string()).unwrap();
        std::fs::write(agents.join("writer.md"), "---\nname: fixture-writer\ndescription: Edit requested code\ntools: read, write\nincludeMcpJson: true\npermissions:\n  rules:\n    - capability: fs_write\n      match: ['src/**']\n      effect: ask\nhooks: {}\n---\nOnly edit requested files.\n").unwrap();
    }
    let status = inspect_in(&repo, "kiro", &locations).with_version(Some("2.29.0"));
    assert!(status.headless.allowed, "{:?}", status.headless.reasons);
    let profiles: Vec<_> = status
        .components
        .iter()
        .filter(|c| c.name == "Kiro native agent")
        .collect();
    assert_eq!(profiles.len(), 6);
    assert!(profiles.iter().all(|c| c.evidence[0].digest.is_some()));
    assert!(
        status
            .gaps
            .iter()
            .any(|gap| gap.contains("not proof of selection"))
    );
    // Native policy remains untouched; inspection is not an installer.
    assert_eq!(
        std::fs::read_to_string(repo.join(".kiro/agents/team/reviewer.json")).unwrap(),
        profile.to_string()
    );
}

#[test]
fn kiro_native_agent_controls_fail_closed_without_echoing_values() {
    use ahu::native_admission::{Locations, inspect_in};
    use serde_json::json;
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let locations = Locations {
        home: Some(home.path().canonicalize().unwrap()),
        ..Default::default()
    };
    let agents = repo.path().join(".kiro/agents");
    std::fs::create_dir_all(&agents).unwrap();
    let path = agents.join("fixture.json");
    for fields in [
        json!({"hooks":[{"name":"start", "trigger":"SessionStart", "action":{"type":"command","command":"private-marker"}}]}),
        json!({"hooks":{"agentSpawn":[{"command":"private-marker"}]}}),
        json!({"hooks":[{"name":"stop", "enabled":false,"trigger":"Stop","action":{"type":"agent","prompt":"private-marker"}}]}),
        json!({"futureControl":"private-marker"}),
        json!({"includePowers":true}),
        json!({"mcpServers":{"fixture":{"command":"CMUX", "args":[]}}}),
        json!({"mcpServers":{"fixture":{"futureControl":"private-marker"}}}),
        json!({"permissions":{"rules":[],"futureControl":"private-marker"}}),
        json!({"permissions":{"rules":[{"capability":"shell","effect":"allow","futureControl":"private-marker"}]}}),
        json!({"resources":[{"type":"knowledgeBase","source":"file://docs","autoUpdate":true}]}),
        json!({"allowedTools":["read"]}),
        json!({"tools":123}),
        json!({"hooks":null}),
    ] {
        let mut profile = json!({"name":"fixture"});
        profile
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        std::fs::write(&path, profile.to_string()).unwrap();
        let status = inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations);
        assert!(!status.headless.allowed, "{fields}");
        assert!(
            !serde_json::to_string(&status)
                .unwrap()
                .contains("private-marker")
        );
    }
    for malformed in ["{broken", "[]", "{\"name\":\"fixture\", // comment\n}"] {
        std::fs::write(&path, malformed).unwrap();
        assert!(
            !inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations)
                .headless
                .allowed
        );
    }
    std::fs::remove_file(&path).unwrap();
    for markdown in [
        "No frontmatter\n",
        "---\nname: fixture\nunknown: private-marker\n---\nPrompt\n",
        "---\nname: fixture\nhooks:\n  agentSpawn:\n    - command: private-marker\n---\nPrompt\n",
    ] {
        std::fs::write(agents.join("fixture.md"), markdown).unwrap();
        assert!(
            !inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations)
                .headless
                .allowed
        );
    }
}

#[test]
fn kiro_cli_settings_allow_only_inspected_preferences_and_types() {
    use ahu::native_admission::{Locations, inspect_in};
    use serde_json::json;
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let locations = Locations {
        home: Some(home.path().canonicalize().unwrap()),
        ..Default::default()
    };
    let settings = repo.path().join(".kiro/settings/cli.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    for (value, allowed) in [
        (json!({}), true),
        (
            json!({"chat.defaultAgent":"fixture", "chat.defaultModel":"claude-sonnet-4.5", "chat.greeting.enabled":false,"chat.disableWrap":true, "chat.preserveScrollback":false,"chat.startFullscreen":false}),
            true,
        ),
        (
            json!({"chat.defaultAgent":{ "path":"private-marker" }}),
            false,
        ),
        (json!({"chat.greeting.enabled":"false"}), false),
        (json!({"chat.enableNotifications":true}), false),
        (json!({"chat.enableWorkflows":true}), false),
        (json!({"futureControl":"private-marker"}), false),
    ] {
        std::fs::write(&settings, value.to_string()).unwrap();
        let status = inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations);
        assert_eq!(
            status.headless.allowed, allowed,
            "{value}: {:?}",
            status.headless.reasons
        );
    }
    std::fs::remove_file(settings).unwrap();
    for key in ["KAS_BUNDLE_PATH", "KIRO_TEST_WORKSPACE_SETTINGS_PATH"] {
        let mut overridden = locations.clone();
        overridden
            .overrides
            .insert(key.into(), "private-marker".into());
        let status = inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &overridden);
        assert!(!status.headless.allowed, "{key}");
        assert!(
            !serde_json::to_string(&status)
                .unwrap()
                .contains("private-marker")
        );
    }
}

#[test]
#[cfg(unix)]
fn kiro_agent_inspection_refuses_symlinks_and_bounded_scan_overflow() {
    use ahu::native_admission::{Locations, inspect_in};
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let locations = Locations {
        home: Some(home.path().canonicalize().unwrap()),
        ..Default::default()
    };
    let agents = repo.path().join(".kiro/agents");
    std::fs::create_dir_all(&agents).unwrap();
    let target = home.path().join("ordinary.json");
    std::fs::write(&target, "{\"name\":\"fixture\"}").unwrap();
    symlink(&target, agents.join("alias.json")).unwrap();
    assert!(
        !inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations)
            .headless
            .allowed
    );
    std::fs::remove_file(agents.join("alias.json")).unwrap();
    symlink(home.path(), agents.join("linked")).unwrap();
    assert!(
        !inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations)
            .headless
            .allowed
    );
    std::fs::remove_file(agents.join("linked")).unwrap();
    std::fs::write(agents.join("large.json"), vec![b' '; 1024 * 1024 + 1]).unwrap();
    assert!(
        !inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations)
            .headless
            .allowed
    );
    std::fs::remove_file(agents.join("large.json")).unwrap();
    for index in 0..257 {
        std::fs::write(
            agents.join(format!("fixture-{index}.json")),
            "{\"name\":\"fixture\"}",
        )
        .unwrap();
    }
    assert!(
        !inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations)
            .headless
            .allowed
    );
}
