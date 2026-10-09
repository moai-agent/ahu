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
    let status = ahu::cmux::integration::inspect_in(
        repo.path(),
        "kiro",
        &ahu::cmux::integration::Locations::default(),
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
            .env("AHU_CMUX_BIN", scratch.path().join("missing-cmux"))
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
#[test]
fn kiro_requires_final_run_result_and_preserves_denials_and_identity_failures() {
    use ahu::headless::Events;
    use serde_json::json;
    let mut events = Events::default();
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
        json!({"sessionUpdate":"tool_call_update","status":"failed","rawOutput":{"message":"private denial"}}),
        json!({"sessionUpdate":"session_info_update"}),
    ].into_iter().enumerate() {
        let mut events = Events::default();
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
        let mut events = Events::default();
        kiro_event(&mut events, event);
        assert!(events.failed, "{field}");
    }
    let mut events = Events::default();
    kiro_event(
        &mut events,
        json!({"type":"runError","data":{"sessionId":"sess_fixture","stage":"prompt","message":"private provider error"}}),
    );
    assert!(events.failed && !events.terminal);
    assert_eq!(events.session.as_deref(), Some("sess_fixture"));
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("private provider error")
    );
    let mut events = Events::default();
    events.observe("kiro", b"{broken");
    assert!(events.failed && !events.terminal);
}

#[test]
fn kiro_headless_inspection_refuses_unreviewed_sources_without_reading_credentials() {
    use ahu::cmux::integration::{Locations, inspect_in};
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
