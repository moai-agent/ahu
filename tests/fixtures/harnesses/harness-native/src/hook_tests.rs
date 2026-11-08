use crate::hook_inventory::{self as hooks, Locations, Scope};
use std::path::{Path, PathBuf};
struct TestRepo(tempfile::TempDir);
impl TestRepo {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    fn path(&self) -> &Path {
        self.0.path()
    }
    fn write(&self, path: &str, text: &str) -> PathBuf {
        let path = self.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.path().join(path)).unwrap()
    }
}
/// Locations pointing at a throwaway home, so a developer's own
/// `~/.claude/settings.json` can never change a test's outcome.
fn locations(home: &Path) -> Locations {
    Locations {
        home: Some(home.to_path_buf()),
        managed: None,
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
fn project_hooks_are_found_and_marked_as_travelling() {
    let repo = TestRepo::new();
    let home = tempfile::TempDir::new().unwrap();
    repo.write(
        ".claude/settings.json",
        &settings_with_hooks("PreToolUse", Some("Bash"), "./.claude/hooks/guard.sh"),
    );

    let found = hooks::collect_in(repo.path(), &locations(home.path())).unwrap();
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

    let found = hooks::collect_in(repo.path(), &locations(home.path())).unwrap();
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

    let found = hooks::collect_in(repo.path(), &locations(home.path())).unwrap();
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

    let found = hooks::collect_in(repo.path(), &locations(home.path())).unwrap();
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

    let found = hooks::collect_in(repo.path(), &locations(home.path())).unwrap();
    assert!(found.hooks.is_empty());
    assert_eq!(found.unreadable.len(), 1, "{found:?}");
}

#[test]
fn settings_without_hooks_are_read_cleanly() {
    let repo = TestRepo::new();
    let home = tempfile::TempDir::new().unwrap();
    repo.write(".claude/settings.json", "{\"model\": \"claude-opus-5\"}");

    let found = hooks::collect_in(repo.path(), &locations(home.path())).unwrap();
    assert!(found.hooks.is_empty());
    assert!(found.unreadable.is_empty());
    assert!(!found.checked.is_empty(), "the file was actually examined");
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
        hooks::collect_in(repo.path(), &locations(home.path())).unwrap();
    }
    assert_eq!(repo.read(".claude/settings.json"), body);
    assert_eq!(
        std::fs::read_to_string(&user_settings).unwrap(),
        settings_with_hooks("Stop", None, "notify"),
        "ahu must never write a hook"
    );
}

// --- executable configuration travelling into a worktree ---
