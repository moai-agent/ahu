#[test]
fn kiro_headless_inspection_refuses_unreviewed_sources_without_reading_credentials() {
    use crate::admission::{Locations, inspect_in};
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
fn kiro_admission_retains_228_and_inspects_229_without_admitting_unknown_versions() {
    use crate::admission::{Locations, inspect_in};
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
    assert_eq!(
        crate::compatibility::isolation_profile("kiro")
            .unwrap()
            .headless_verified_versions,
        &["2.28.0", "2.29.0"]
    );
}

#[test]
fn kiro_admits_declarative_native_agents_in_each_discovered_scope() {
    use crate::admission::{Locations, inspect_in};
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
    use crate::admission::{Locations, inspect_in};
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
    use crate::admission::{Locations, inspect_in};
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
    use crate::admission::{Locations, inspect_in};
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

#[test]
fn kiro_mcp_admission_uses_structure_without_provider_name_policy() {
    use crate::admission::{Locations, inspect_in};
    use serde_json::json;
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let locations = Locations {
        home: Some(home.path().canonicalize().unwrap()),
        ..Default::default()
    };
    let agents = repo.path().join(".kiro/agents");
    std::fs::create_dir_all(&agents).unwrap();
    for command in ["cmux", "CMUX", "other-provider"] {
        let profile =
            json!({"name":"fixture","mcpServers":{"fixture":{"command":command,"args":[]}}});
        std::fs::write(agents.join("fixture.json"), profile.to_string()).unwrap();
        let status = inspect_in(&repo.path().canonicalize().unwrap(), "kiro", &locations);
        assert!(status.headless.allowed, "{command}: {status:?}");
        assert!(
            status
                .gaps
                .iter()
                .any(|gap| gap.contains("arbitrary MCP subprocess behavior is not inspected"))
        );
    }
}
