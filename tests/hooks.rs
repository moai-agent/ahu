//! Hook discovery, reporting, and the guarantees ahu makes about them.

mod common;

use std::path::{Path, PathBuf};

use ahu::hooks::{self, HookInventory, Locations, Scope};
use ahu::{inventory, snapshot};
use common::TestRepo;

/// Locations pointing at a throwaway home, so a developer's own
/// `~/.claude/settings.json` can never change a test's outcome.
fn locations(home: &Path, cmux_wrapper: bool) -> Locations {
    Locations {
        home: Some(home.to_path_buf()),
        managed: None,
        cmux_wrapper,
    }
}

fn settings_with_hooks(event: &str, matcher: Option<&str>, command: &str) -> String {
    let matcher = match matcher {
        Some(matcher) => format!("\"matcher\": {matcher:?}, "),
        None => String::new(),
    };
    format!(
        "{{\"hooks\": {{{event:?}: [{{{matcher}\"hooks\": [{{\"type\": \"command\", \"command\": {command:?}}}]}}]}}}}"
    )
}

#[test]
fn doctor_separates_project_hook_inventory_from_all_harness_integration_status() {
    for (claude_preference, claude_agent) in [(false, false), (true, false), (false, true)] {
        let repo = TestRepo::new();
        repo.init_config();
        if !claude_preference {
            let config = repo
                .read(".agents/ahu/config.toml")
                .replace("claude-code", "codex")
                .replace("\"claude-opus-5\", \"claude-sonnet-5\"", "\"gpt-6-astra\"");
            repo.write(".agents/ahu/config.toml", &config);
        }
        if claude_agent {
            repo.add_agent("chris", "1.0.0", "claude-opus-5");
        }
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".claude")).unwrap();
        let settings = home.path().join(".claude/settings.json");
        let used = claude_preference || claude_agent;
        // Generic inventory follows project harnesses; integration status reports
        // all native scopes, including unrelated malformed settings.
        std::fs::write(
            &settings,
            if used {
                settings_with_hooks("Stop", None, "notify-me")
            } else {
                "invalid unrelated settings".to_string()
            },
        )
        .unwrap();
        let output = common::ahu()
            .arg("doctor")
            .current_dir(repo.path())
            .env("HOME", home.path())
            .env("AHU_CMUX_BIN", home.path().join("missing-cmux"))
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(!text.contains(hooks::NON_PROJECT_HOOK_WARNING), "{text}");
        for harness in ["claude-code", "codex", "opencode", "antigravity"] {
            assert!(
                text.contains(&format!("cmux integration {harness}:")),
                "{text}"
            );
        }
        if used {
            assert!(
                text.contains("hooks        Claude Code: 1 configured"),
                "{text}"
            );
            assert!(text.contains("user         Stop → notify-me"), "{text}");
        } else {
            assert!(!text.contains("hooks        "), "{text}");
            assert!(text.contains("settings.json"), "{text}");
        }
    }
}

#[test]
fn project_hooks_are_found_and_marked_as_travelling() {
    let repo = TestRepo::new();
    let home = tempfile::TempDir::new().unwrap();
    repo.write(
        ".claude/settings.json",
        &settings_with_hooks("PreToolUse", Some("Bash"), "./.claude/hooks/guard.sh"),
    );

    let found = hooks::collect_in(repo.path(), &locations(home.path(), false)).unwrap();
    assert_eq!(found.hooks.len(), 1);
    let hook = &found.hooks[0];
    assert_eq!(hook.event, "PreToolUse");
    assert_eq!(hook.matcher.as_deref(), Some("Bash"));
    assert_eq!(hook.kind, "command");
    assert_eq!(hook.scope, Scope::Project);
    assert!(hook.scope.is_project_policy());
    assert!(hook.scope.travels_into_worktree());
    assert_eq!(found.travelling().len(), 1);
    assert!(
        found.outside_project_policy().is_empty(),
        "a project hook is project policy"
    );
    assert!(hook.label().contains("PreToolUse:Bash"), "{}", hook.label());
}

#[test]
fn hooks_outside_the_repository_are_flagged_and_do_not_travel() {
    let repo = TestRepo::new();
    let home = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::write(
        home.path().join(".claude/settings.json"),
        settings_with_hooks("Stop", None, "notify-me"),
    )
    .unwrap();

    let found = hooks::collect_in(repo.path(), &locations(home.path(), false)).unwrap();
    assert_eq!(found.hooks.len(), 1);
    let hook = &found.hooks[0];
    assert_eq!(hook.scope, Scope::User);
    assert!(!hook.scope.is_project_policy());
    assert!(!hook.scope.travels_into_worktree());
    assert_eq!(found.outside_project_policy().len(), 1);
    assert!(found.travelling().is_empty());
}

#[test]
fn a_project_local_hook_travels_but_is_still_not_project_policy() {
    let repo = TestRepo::new();
    let home = tempfile::TempDir::new().unwrap();
    // `.claude/settings.local.json` is inside the repository but conventionally
    // ignored by Git, so it reaches the worktree without being shared or reviewed.
    repo.write(
        ".claude/settings.local.json",
        &settings_with_hooks("UserPromptSubmit", None, "inject-context"),
    );

    let found = hooks::collect_in(repo.path(), &locations(home.path(), false)).unwrap();
    let hook = &found.hooks[0];
    assert_eq!(hook.scope, Scope::ProjectLocal);
    assert!(hook.scope.travels_into_worktree());
    assert!(!hook.scope.is_project_policy());
    assert_eq!(found.outside_project_policy().len(), 1);
    assert!(
        hook.scope.why_not_project_policy().contains("does travel"),
        "a project-local hook must not be described as non-travelling"
    );
}

#[test]
fn an_unreadable_settings_file_reports_unknown_hooks_rather_than_none() {
    let repo = TestRepo::new();
    let home = tempfile::TempDir::new().unwrap();
    repo.write(".claude/settings.json", "{ this is not json");

    let found = hooks::collect_in(repo.path(), &locations(home.path(), false)).unwrap();
    assert!(found.hooks.is_empty());
    assert_eq!(found.unreadable.len(), 1, "{found:?}");
    assert!(found.unreadable[0].contains("settings.json"));
}

#[test]
fn a_hooks_key_ahu_does_not_understand_is_reported_not_ignored() {
    let repo = TestRepo::new();
    let home = tempfile::TempDir::new().unwrap();
    // A shape ahu cannot walk must never be reported as "no hooks".
    repo.write(
        ".claude/settings.json",
        "{\"hooks\": {\"PreToolUse\": \"nope\"}}",
    );

    let found = hooks::collect_in(repo.path(), &locations(home.path(), false)).unwrap();
    assert!(found.hooks.is_empty());
    assert_eq!(found.unreadable.len(), 1, "{found:?}");
}

#[test]
fn settings_without_hooks_are_read_cleanly() {
    let repo = TestRepo::new();
    let home = tempfile::TempDir::new().unwrap();
    repo.write(".claude/settings.json", "{\"model\": \"claude-opus-5\"}");

    let found = hooks::collect_in(repo.path(), &locations(home.path(), false)).unwrap();
    assert!(found.hooks.is_empty());
    assert!(found.unreadable.is_empty());
    assert!(!found.checked.is_empty(), "the file was actually examined");
}

#[test]
fn the_cmux_wrapper_is_reported_as_an_unreadable_hook_source() {
    let repo = TestRepo::new();
    let home = tempfile::TempDir::new().unwrap();
    let found = hooks::collect_in(repo.path(), &locations(home.path(), true)).unwrap();
    assert!(found.wrapper_injected);
    // It also changes the digest, so a launch inside cmux is not presented as
    // having the same effective hooks as one outside it.
    let outside = hooks::collect_in(repo.path(), &locations(home.path(), false)).unwrap();
    assert_ne!(found.digest(), outside.digest());
}

#[test]
fn the_hook_digest_covers_scopes_the_config_snapshot_cannot_see() {
    let repo = TestRepo::new();
    let home = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    let user_settings = home.path().join(".claude/settings.json");
    std::fs::write(&user_settings, settings_with_hooks("Stop", None, "one")).unwrap();

    let before = hooks::collect_in(repo.path(), &locations(home.path(), false))
        .unwrap()
        .digest();
    let snapshot_before = snapshot::collect(repo.path()).unwrap().digest();

    std::fs::write(&user_settings, settings_with_hooks("Stop", None, "two")).unwrap();

    let after = hooks::collect_in(repo.path(), &locations(home.path(), false))
        .unwrap()
        .digest();
    assert_ne!(before, after, "a user hook change must show as drift");
    assert_eq!(
        snapshot_before,
        snapshot::collect(repo.path()).unwrap().digest(),
        "the repository snapshot cannot see it, which is why the hook digest exists"
    );
}

#[test]
fn reading_hooks_never_modifies_a_settings_file() {
    let repo = TestRepo::new();
    let home = tempfile::TempDir::new().unwrap();
    let body = settings_with_hooks("PreToolUse", Some("Bash"), "guard");
    repo.write(".claude/settings.json", &body);
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    let user_settings = home.path().join(".claude/settings.json");
    std::fs::write(&user_settings, settings_with_hooks("Stop", None, "notify")).unwrap();

    for _ in 0..3 {
        hooks::collect_in(repo.path(), &locations(home.path(), true)).unwrap();
    }
    assert_eq!(repo.read(".claude/settings.json"), body);
    assert_eq!(
        std::fs::read_to_string(&user_settings).unwrap(),
        settings_with_hooks("Stop", None, "notify"),
        "ahu must never write a hook"
    );
}

// --- executable configuration travelling into a worktree ---

#[test]
fn hook_scripts_travel_into_the_worktree_with_their_executable_bit() {
    let repo = TestRepo::new();
    let script = repo.write(".claude/hooks/guard.sh", "#!/bin/sh\nexit 2\n");
    make_executable(&script);
    repo.write(
        ".claude/settings.json",
        &settings_with_hooks(
            "PreToolUse",
            Some("Bash"),
            "$CLAUDE_PROJECT_DIR/.claude/hooks/guard.sh",
        ),
    );
    repo.commit("hooks");

    let taken = snapshot::collect(repo.path()).unwrap();
    let entry = taken
        .entries
        .iter()
        .find(|e| e.path == ".claude/hooks/guard.sh")
        .expect("the hook script is agent configuration");
    assert!(entry.executable, "the mode must be recorded");
    assert_eq!(taken.executable_count(), 1);

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt");
    ahu::git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/hooks",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();
    snapshot::materialize(repo.path(), &taken, &worktree).unwrap();

    let copied = worktree.join(".claude/hooks/guard.sh");
    assert!(copied.is_file());
    assert!(
        is_executable(&copied),
        "a hook that is not executable in the worktree silently stops running"
    );
}

#[test]
fn a_mode_only_change_is_drift_and_is_synced_into_the_worktree() {
    let repo = TestRepo::new();
    let script = repo.write(".claude/hooks/guard.sh", "#!/bin/sh\nexit 0\n");
    make_executable(&script);
    repo.commit("hooks");

    let executable = snapshot::collect(repo.path()).unwrap();
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt");
    ahu::git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/mode",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();
    snapshot::materialize(repo.path(), &executable, &worktree).unwrap();
    assert!(is_executable(&worktree.join(".claude/hooks/guard.sh")));

    // Take the bit off in the parent. Contents are unchanged, so only the mode
    // distinguishes the two — and that changes whether the hook runs at all.
    set_mode(&script, 0o644);
    let plain = snapshot::collect(repo.path()).unwrap();
    assert_ne!(
        executable.digest(),
        plain.digest(),
        "the executable bit must be part of the snapshot digest"
    );

    let report = snapshot::materialize(repo.path(), &plain, &worktree).unwrap();
    assert!(
        report
            .written
            .contains(&".claude/hooks/guard.sh".to_string()),
        "{report:?}"
    );
    assert!(!is_executable(&worktree.join(".claude/hooks/guard.sh")));
}

// --- inventory and preview reporting ---

#[test]
fn the_inventory_lists_hooks_as_their_own_category_with_scope() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.write(
        ".claude/settings.json",
        &settings_with_hooks("PreToolUse", Some("Bash"), "guard"),
    );
    repo.commit("fixture");

    let loaded = ahu::config::load(repo.path()).unwrap().unwrap();
    let agent = ahu::agent::find(repo.path(), "chris").unwrap();
    let taken = snapshot::collect(repo.path()).unwrap();
    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let enforcement = adapter
        .enforcement("claude-opus-5", Default::default())
        .unwrap();
    let home = tempfile::TempDir::new().unwrap();
    let found = hooks::collect_in(repo.path(), &locations(home.path(), true)).unwrap();

    let built = inventory::build(&inventory::Subject {
        repo_root: repo.path(),
        loaded_config: &loaded,
        snapshot: &taken,
        agent: Some(&agent),
        harness: "claude-code",
        model: "claude-opus-5",
        enforcement: &enforcement,
        hooks: &found,
        prompt: None,
    })
    .unwrap();

    let hook_items: Vec<_> = built.items_in(inventory::Category::Hook).collect();
    assert_eq!(hook_items.len(), 2, "the project hook and the cmux wrapper");
    let project = hook_items
        .iter()
        .find(|i| i.scope == "project")
        .expect("project hook is inventoried");
    assert_eq!(project.visibility, inventory::Visibility::Available);
    assert!(
        project
            .notes
            .iter()
            .any(|n| n.contains("runs on PreToolUse"))
    );
    assert!(project.notes.iter().any(|n| n.contains("task worktree")));

    let wrapper = hook_items
        .iter()
        .find(|i| i.visibility == inventory::Visibility::Opaque)
        .expect("the cmux wrapper is inventoried as unreadable");
    assert!(wrapper.name.contains("cmux"));

    let rendered = inventory::render(&built);
    assert!(
        rendered.contains("Hooks (executable, run by the harness)"),
        "{rendered}"
    );
    assert!(
        built.coverage_gaps.iter().any(|g| g.contains("plugins")),
        "plugin hooks are a known gap"
    );
}

#[test]
fn the_preview_warns_about_hooks_that_are_not_project_policy() {
    let mut found = HookInventory::default();
    let text = hooks::render_for_preview(&found, 0);
    assert!(text.contains("none found"), "{text}");

    found.hooks.push(ahu::hooks::Hook {
        event: "PreToolUse".to_string(),
        matcher: Some("Bash".to_string()),
        kind: "command".to_string(),
        command_digest: String::new(),
        command: Some("./.claude/hooks/guard.sh".to_string()),
        scope: Scope::Project,
        source: ".claude/settings.json".to_string(),
    });
    found.hooks.push(ahu::hooks::Hook {
        event: "UserPromptSubmit".to_string(),
        matcher: None,
        kind: "command".to_string(),
        command_digest: String::new(),
        command: Some("inject-my-context".to_string()),
        scope: Scope::User,
        source: "/home/someone/.claude/settings.json".to_string(),
    });
    found.hooks.push(ahu::hooks::Hook {
        event: "PostToolUse".to_string(),
        matcher: None,
        kind: "command".to_string(),
        command_digest: String::new(),
        command: Some("local-only-audit".to_string()),
        scope: Scope::ProjectLocal,
        source: ".claude/settings.local.json".to_string(),
    });
    found.wrapper_injected = true;

    let text = hooks::render_for_preview(&found, 2);
    assert!(
        text.contains("travel into the task worktree and run there"),
        "{text}"
    );
    assert!(
        text.contains("2 inherited configuration file(s) are executable"),
        "{text}"
    );
    assert!(text.contains(hooks::NON_PROJECT_HOOK_WARNING), "{text}");
    assert!(
        text.contains("inject-my-context"),
        "the hook's program is still identifiable: {text}"
    );
    assert!(
        text.contains("cmux injects its own Claude Code hooks"),
        "{text}"
    );
    assert!(
        text.contains("does not add, edit, or remove hooks"),
        "the preview must say ahu will not fix this for you: {text}"
    );
    // The reason must be right per scope: a project-local hook does travel.
    assert!(
        text.contains("does travel into the task worktree, but this file is"),
        "{text}"
    );
    assert!(
        text.contains("does not travel into the task worktree and"),
        "{text}"
    );
}

fn make_executable(path: &Path) {
    set_mode(path, 0o755);
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

fn is_executable(path: &PathBuf) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

/// A repository must not be able to paint over ahu's own disclosure.
#[test]
fn control_characters_from_repository_content_never_reach_the_terminal_raw() {
    const ESC: char = '\u{1b}';
    let mut found = HookInventory::default();
    found.hooks.push(ahu::hooks::Hook {
        event: format!("PreToolUse{ESC}[2J"),
        matcher: Some(format!("Bash{ESC}[4A")),
        kind: "command".to_string(),
        command_digest: String::new(),
        command: Some(format!(
            "{ESC}[4A{ESC}[2K  none found in the settings files"
        )),
        scope: Scope::Project,
        source: format!(".claude/settings{ESC}[2K.json"),
    });

    let text = hooks::render_for_preview(&found, 0);
    assert!(
        !text.contains(ESC),
        "no raw escape may survive into ahu's output: {text:?}"
    );
    assert!(
        text.contains("\\x1b"),
        "escapes are shown, not dropped: {text}"
    );
}

/// Hook arguments routinely carry credentials; only the program is shown.
#[test]
fn hook_arguments_are_not_printed_or_persisted() {
    let repo = TestRepo::new();
    let home = tempfile::TempDir::new().unwrap();
    repo.write(
        ".claude/settings.json",
        &settings_with_hooks(
            "Stop",
            None,
            "curl -H \"Authorization: Bearer sk-secret-TOKEN-abcdef\" https://example.invalid",
        ),
    );

    let found = hooks::collect_in(repo.path(), &locations(home.path(), false)).unwrap();
    let rendered = hooks::render_for_preview(&found, 0);
    assert!(
        !rendered.contains("sk-secret-TOKEN-abcdef"),
        "a token in a hook argument must not be printed: {rendered}"
    );
    assert!(
        rendered.contains("curl"),
        "the program is still identifiable: {rendered}"
    );

    // Nor may it be written into the task record.
    let serialized = serde_json::to_string(&found).unwrap();
    assert!(
        !serialized.contains("sk-secret-TOKEN-abcdef"),
        "a token must not be persisted in a task record: {serialized}"
    );
    assert!(
        !found.hooks[0].command_digest.is_empty(),
        "the digest still identifies the command for drift"
    );
}
