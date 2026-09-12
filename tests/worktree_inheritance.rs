//! Worktree creation and agent-configuration inheritance.
//!
//! A task worktree must receive the parent checkout's complete agent
//! configuration as it stands at submission — additions, modifications,
//! deletions, ignored files and all — while unrelated dirty source files stay
//! behind in the original checkout.

mod common;

use ahu::{git, snapshot};
use common::TestRepo;

#[test]
fn the_snapshot_covers_native_paths_for_every_supported_harness() {
    let repo = TestRepo::new();
    repo.write("CLAUDE.md", "repository guidance\n");
    repo.write("AGENTS.md", "other harness guidance\n");
    repo.write(".claude/settings.json", "{}\n");
    repo.write(".claude/skills/review/SKILL.md", "review skill\n");
    repo.write(".claude/agents/chris.md", "---\nname: chris\n---\nbody\n");
    repo.write(".agents/skills/shared/SKILL.md", "shared skill\n");
    repo.write(".mcp.json", "{\"mcpServers\":{}}\n");
    repo.write("docs/nested/CLAUDE.md", "nested guidance\n");
    repo.write("src/main.rs", "fn main() {}\n");

    let taken = snapshot::collect(repo.path()).unwrap();
    let paths: Vec<&str> = taken.entries.iter().map(|e| e.path.as_str()).collect();
    for expected in [
        "CLAUDE.md",
        "AGENTS.md",
        ".claude/settings.json",
        ".claude/skills/review/SKILL.md",
        ".claude/agents/chris.md",
        ".agents/skills/shared/SKILL.md",
        ".mcp.json",
        "docs/nested/CLAUDE.md",
    ] {
        assert!(paths.contains(&expected), "missing {expected} in {paths:?}");
    }
    // Source files are not agent configuration.
    assert!(!paths.contains(&"src/main.rs"));
    assert!(!paths.contains(&"README.md"));
}

#[test]
fn the_snapshot_digest_changes_with_any_configuration_change() {
    let repo = TestRepo::new();
    repo.write("CLAUDE.md", "one\n");
    let before = snapshot::collect(repo.path()).unwrap().digest();
    repo.write("CLAUDE.md", "two\n");
    let after = snapshot::collect(repo.path()).unwrap().digest();
    assert_ne!(before, after);
    // A source-only change does not move the configuration digest.
    repo.write("src/lib.rs", "// unrelated\n");
    assert_eq!(snapshot::collect(repo.path()).unwrap().digest(), after);
}

#[test]
fn a_worktree_inherits_additions_modifications_deletions_and_ignored_config() {
    let repo = TestRepo::new();
    repo.write("CLAUDE.md", "committed guidance\n");
    repo.write(".claude/settings.json", "{\"committed\":true}\n");
    repo.write(".claude/skills/old/SKILL.md", "old skill\n");
    repo.write(".gitignore", ".claude/settings.local.json\n");
    repo.write("src/main.rs", "fn main() {}\n");
    repo.commit("configuration");

    // Now diverge the working tree without committing anything.
    repo.write("CLAUDE.md", "edited guidance\n"); // modified
    repo.write(".claude/skills/new/SKILL.md", "new skill\n"); // added, untracked
    repo.write(".claude/settings.local.json", "{\"local\":true}\n"); // ignored
    repo.remove(".claude/skills/old/SKILL.md"); // deleted
    repo.write("src/main.rs", "fn main() { todo!() }\n"); // unrelated dirty source

    let discovered = git::discover(repo.path()).unwrap();
    let taken = snapshot::collect(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt");
    git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/inherit",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    let report = snapshot::materialize(repo.path(), &taken, &worktree).unwrap();

    assert_eq!(
        std::fs::read_to_string(worktree.join("CLAUDE.md")).unwrap(),
        "edited guidance\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join(".claude/skills/new/SKILL.md")).unwrap(),
        "new skill\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join(".claude/settings.local.json")).unwrap(),
        "{\"local\":true}\n",
        "ignored native configuration must still be inherited"
    );
    assert!(
        !worktree.join(".claude/skills/old/SKILL.md").exists(),
        "a deleted skill must not reappear in the task worktree"
    );
    assert!(
        report
            .removed
            .contains(&".claude/skills/old/SKILL.md".to_string()),
        "{report:?}"
    );

    // Unrelated dirty source stays in the original checkout.
    assert_eq!(
        std::fs::read_to_string(worktree.join("src/main.rs")).unwrap(),
        "fn main() {}\n"
    );

    // The parent checkout is untouched: same working tree, same index, same branch.
    assert_eq!(repo.read("CLAUDE.md"), "edited guidance\n");
    assert_eq!(
        common::git(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
        "main"
    );
    let staged = common::git(repo.path(), &["diff", "--cached", "--name-only"]);
    assert!(staged.is_empty(), "ahu must never stage anything: {staged}");
}

#[test]
fn materializing_twice_changes_nothing_the_second_time() {
    let repo = TestRepo::new();
    repo.write("CLAUDE.md", "guidance\n");
    repo.commit("config");
    repo.write(".claude/settings.local.json", "{}\n");

    let discovered = git::discover(repo.path()).unwrap();
    let taken = snapshot::collect(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt");
    git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/idempotent",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    let first = snapshot::materialize(repo.path(), &taken, &worktree).unwrap();
    assert!(!first.is_empty());
    let second = snapshot::materialize(repo.path(), &taken, &worktree).unwrap();
    assert!(second.is_empty(), "second pass changed {second:?}");
}

#[test]
fn a_configuration_edit_during_preparation_is_disclosed() {
    let repo = TestRepo::new();
    repo.write("CLAUDE.md", "original\n");
    repo.commit("config");
    let discovered = git::discover(repo.path()).unwrap();
    let taken = snapshot::collect(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt");
    git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/concurrent",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    // Someone edits the file after the snapshot was taken.
    repo.write("CLAUDE.md", "changed underneath us\n");

    let report = snapshot::materialize(repo.path(), &taken, &worktree).unwrap();
    assert!(
        report
            .concurrently_modified
            .contains(&"CLAUDE.md".to_string()),
        "{report:?}"
    );
}

#[test]
fn every_worktree_of_one_repository_shares_a_single_identity() {
    let repo = TestRepo::new();
    repo.commit("base");
    let discovered = git::discover(repo.path()).unwrap();
    let worktree = repo.state_path().join("second");
    git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/identity",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    let from_worktree = git::discover(&worktree).unwrap();
    assert_eq!(
        discovered.identity(),
        from_worktree.identity(),
        "launches from different worktrees must land in one cmux group"
    );
    assert_eq!(discovered.display_name(), from_worktree.display_name());
    assert_ne!(discovered.root, from_worktree.root);
}

#[test]
fn scan_exclusions_are_disclosed_rather_than_silently_dropped() {
    let repo = TestRepo::new();
    repo.write("node_modules/some-pkg/.claude/settings.json", "{}\n");
    let taken = snapshot::collect(repo.path()).unwrap();
    assert!(
        !taken
            .entries
            .iter()
            .any(|e| e.path.contains("node_modules")),
        "the scan should not descend into node_modules"
    );
}

#[test]
fn a_repository_with_no_agent_configuration_produces_an_empty_snapshot() {
    let repo = TestRepo::new();
    let taken = snapshot::collect(repo.path()).unwrap();
    assert!(taken.entries.is_empty());
    assert_eq!(taken.digest().len(), 64);
}
