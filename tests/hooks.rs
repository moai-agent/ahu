//! Hook discovery, reporting, and the guarantees ahu makes about them.

mod common;

use std::path::{Path, PathBuf};

use ahu::hooks::{self, HookInventory, Scope};
use ahu::snapshot;
use common::TestRepo;

fn settings_with_hooks(event: &str, matcher: Option<&str>, command: &str) -> String {
    serde_json::json!({"hooks":{event:[{"matcher":matcher,"hooks":[{"type":"command","command":command}]}]}}).to_string()
}

#[test]
fn hook_scripts_travel_into_the_worktree_with_their_executable_bit() {
    let repo = TestRepo::new();
    repo.install_runtime("claude-code");
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
    repo.install_runtime("claude-code");
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

// --- hook preview reporting ---

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
    assert!(text.contains("wrapper and plugin hook injection is not established"));
    assert!(
        text.contains("inject-my-context"),
        "the hook's program is still identifiable: {text}"
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
    let found = HookInventory {
        hooks: vec![ahu::hooks::Hook {
            event: "Stop".into(),
            matcher: None,
            kind: "command".into(),
            command: Some("curl -H Authorization:sk-secret-TOKEN-abcdef".into()),
            command_digest: "a".repeat(64),
            scope: Scope::Project,
            source: "invented.conf".into(),
        }],
        ..Default::default()
    };
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
