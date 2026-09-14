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

/// A repository must never be able to direct ahu's writes outside the worktree.
///
/// The full chain, using only real Git operations: an attacker commits a symlink
/// at a configuration path, the victim clones it, any tool rewrites that path
/// with the ordinary atomic temp-file-plus-rename pattern (which replaces the
/// symlink with a real file in the working tree while HEAD keeps the symlink),
/// and the victim launches a task.
#[test]
fn a_committed_symlink_cannot_redirect_writes_outside_the_worktree() {
    let outside = tempfile::TempDir::new().unwrap();
    let victim_file = outside.path().join("zshrc");
    std::fs::write(&victim_file, "# victim's real file\n").unwrap();

    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        &victim_file,
        repo.path().join(".claude/settings.local.json"),
    )
    .unwrap();
    repo.commit("attacker: symlinked configuration path");

    // A tool rewrites the path atomically, replacing the symlink in the working
    // tree. HEAD still holds the symlink, so the fresh worktree will have one.
    let settings = repo.path().join(".claude/settings.local.json");
    let temp = repo.path().join(".claude/settings.local.json.tmp");
    std::fs::write(&temp, "{\"permissions\":{}}\n").unwrap();
    std::fs::rename(&temp, &settings).unwrap();
    assert!(
        !std::fs::symlink_metadata(&settings)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let discovered = git::discover(repo.path()).unwrap();
    let taken = snapshot::collect(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt");
    git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/symlink",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();
    assert!(
        std::fs::symlink_metadata(worktree.join(".claude/settings.local.json"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the fresh worktree should carry the committed symlink"
    );

    let report = snapshot::materialize(repo.path(), &taken, &worktree).unwrap();

    assert_eq!(
        std::fs::read_to_string(&victim_file).unwrap(),
        "# victim's real file\n",
        "the file outside the worktree must be untouched"
    );
    // The hostile link is removed rather than followed, and disclosed.
    assert!(
        report
            .removed_symlinks
            .contains(&".claude/settings.local.json".to_string()),
        "{report:?}"
    );
    let landed = worktree.join(".claude/settings.local.json");
    assert!(
        !std::fs::symlink_metadata(&landed)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the worktree must hold a real file, not the link"
    );
}

/// The same protection for a symlinked *directory*, which would otherwise let a
/// repository redirect every write under it.
#[test]
fn a_committed_directory_symlink_cannot_redirect_writes_outside_the_worktree() {
    let outside = tempfile::TempDir::new().unwrap();
    std::fs::write(outside.path().join("existing.txt"), "untouched\n").unwrap();

    let repo = TestRepo::new();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), repo.path().join(".claude")).unwrap();
    repo.commit("attacker: .claude is a symlink to a directory outside the repository");

    // The victim replaces it with a real directory locally without committing.
    std::fs::remove_file(repo.path().join(".claude")).unwrap();
    repo.write(".claude/settings.json", "{\"victim\":\"local\"}\n");

    let discovered = git::discover(repo.path()).unwrap();
    let taken = snapshot::collect(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt");
    git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/dirsymlink",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    let report = snapshot::materialize(repo.path(), &taken, &worktree).unwrap();

    assert!(
        report.removed_symlinks.contains(&".claude".to_string()),
        "the symlinked directory must be removed and disclosed: {report:?}"
    );
    assert_eq!(
        std::fs::read_to_string(outside.path().join("existing.txt")).unwrap(),
        "untouched\n"
    );
    assert!(
        !outside.path().join("settings.json").exists(),
        "no file may be created outside the worktree"
    );
}

/// The mode-sync path must not be able to chmod outside the worktree either.
#[test]
fn a_committed_symlink_cannot_redirect_a_mode_only_change() {
    let outside = tempfile::TempDir::new().unwrap();
    let victim_file = outside.path().join("target.txt");
    std::fs::write(&victim_file, "identical content\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&victim_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&victim_file, repo.path().join(".claude/settings.json")).unwrap();
    repo.commit("attacker: symlinked configuration path");

    // Same bytes, different mode: the copy is skipped and only sync_mode runs.
    std::fs::remove_file(repo.path().join(".claude/settings.json")).unwrap();
    repo.write(".claude/settings.json", "identical content\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            repo.path().join(".claude/settings.json"),
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
    }

    let discovered = git::discover(repo.path()).unwrap();
    let taken = snapshot::collect(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt");
    git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/mode-symlink",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    let _ = snapshot::materialize(repo.path(), &taken, &worktree);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&victim_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the outside file's mode must be untouched");
    }
}

/// Task worktrees live in the repository, and must be invisible to Git.
#[test]
fn task_worktrees_live_under_a_self_ignoring_directory_in_the_repository() {
    let repo = TestRepo::new();
    repo.commit("base");
    let discovered = git::discover(repo.path()).unwrap();

    let root = ahu::state::ensure_worktrees_root(&discovered.root).unwrap();
    assert_eq!(root, discovered.root.join(".worktrees"));
    assert_eq!(
        std::fs::read_to_string(root.join(".gitignore")).unwrap(),
        ahu::state::WORKTREES_GITIGNORE
    );

    let worktree = ahu::state::worktree_dir(&discovered.root, "task0001").unwrap();
    git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/inrepo",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();
    assert!(worktree.join("README.md").is_file());

    // The whole directory ignores itself, so nothing ahu creates can be
    // committed by accident and `git status` stays clean.
    let status = common::git(repo.path(), &["status", "--porcelain"]);
    assert!(
        !status.contains(".worktrees"),
        "task worktrees must not show up in git status: {status}"
    );

    // And the configuration scan must not descend into a task's own checkout.
    repo.write("CLAUDE.md", "guidance\n");
    let taken = snapshot::collect(&discovered.root).unwrap();
    assert!(
        !taken.entries.iter().any(|e| e.path.contains(".worktrees")),
        "the snapshot must not inventory a task worktree: {:?}",
        taken.entries
    );
}

/// A source that becomes a symlink between collection and copying must not be
/// followed. The window is the whole time the user spends reading the preview.
#[test]
fn a_source_swapped_for_a_symlink_after_collection_is_not_copied() {
    let outside = tempfile::TempDir::new().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "SYNTHETIC SECRET\n").unwrap();

    let repo = TestRepo::new();
    repo.write(".claude/settings.json", "{\"harmless\":true}\n");
    repo.commit("config");

    let discovered = git::discover(repo.path()).unwrap();
    let taken = snapshot::collect(&discovered.root).unwrap();
    let worktree = repo.state_path().join("wt");
    git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/sourceswap",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    // The attacker swaps the collected regular file for a link to a secret.
    let source = discovered.root.join(".claude/settings.json");
    std::fs::remove_file(&source).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, &source).unwrap();

    let report = snapshot::materialize(&discovered.root, &taken, &worktree).unwrap();

    assert!(
        report
            .refused_sources
            .contains(&".claude/settings.json".to_string()),
        "the swap must be refused and disclosed: {report:?}"
    );
    assert!(
        report
            .concurrently_modified
            .contains(&".claude/settings.json".to_string()),
        "{report:?}"
    );
    let landed = worktree.join(".claude/settings.json");
    if let Ok(contents) = std::fs::read_to_string(&landed) {
        assert!(
            !contents.contains("SYNTHETIC SECRET"),
            "a file outside the repository must never be read into the worktree"
        );
    }
}

// --- Disclosure of what the scan did not look at -----------------------------
//
// The worktree is created by `git worktree add <base>`, so every *committed*
// file is physically present in it whether or not the snapshot scanned it. A
// directory the scan refuses to descend into is therefore not a directory whose
// contents stay behind; it is a directory whose contents arrive unannounced.
// The inventory has to say so for every skipped directory, not just the ones
// that happen to be configuration paths themselves.

#[test]
fn every_skipped_directory_is_recorded_not_just_configuration_ones() {
    let repo = TestRepo::new();
    repo.write("vendor/CLAUDE.md", "planted guidance\n");
    repo.write("build/AGENTS.md", "planted guidance\n");
    repo.write("node_modules/some-pkg/.claude/settings.json", "{}\n");

    let taken = snapshot::collect(repo.path()).unwrap();

    assert!(
        !taken.entries.iter().any(|e| e.path.contains("vendor")),
        "the scan should still not descend into vendor"
    );
    for expected in ["vendor", "build", "node_modules"] {
        assert!(
            taken.skipped_directories.iter().any(|d| d == expected),
            "{expected} was skipped without being recorded: {:?}",
            taken.skipped_directories
        );
    }
}

#[test]
fn configuration_directly_inside_a_skipped_directory_is_named() {
    let repo = TestRepo::new();
    repo.write("vendor/CLAUDE.md", "planted guidance\n");
    repo.write("node_modules/.claude/settings.json", "{}\n");

    let taken = snapshot::collect(repo.path()).unwrap();

    assert!(
        taken
            .unscanned_config
            .iter()
            .any(|p| p == "vendor/CLAUDE.md"),
        "a planted CLAUDE.md under a skipped directory must be named: {:?}",
        taken.unscanned_config
    );
    assert!(
        taken
            .unscanned_config
            .iter()
            .any(|p| p == "node_modules/.claude"),
        "a configuration directory under a skipped directory must be named: {:?}",
        taken.unscanned_config
    );
}

#[test]
fn a_directory_past_the_depth_cap_is_disclosed_rather_than_dropped() {
    let repo = TestRepo::new();
    // Deeper than the scan's depth cap, so the walk stops before reading it.
    let deep = "d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14";
    repo.write(&format!("{deep}/CLAUDE.md"), "planted guidance\n");

    let taken = snapshot::collect(repo.path()).unwrap();

    assert!(
        !taken.entries.iter().any(|e| e.path.contains("/d13/")),
        "the scan should still stop at the depth cap"
    );
    assert!(
        taken
            .skipped_directories
            .iter()
            .any(|d| d.starts_with("d1/d2/")),
        "the depth cut-off recorded nothing: {:?}",
        taken.skipped_directories
    );
}

/// Committed content under a skipped path *is* inherited — it arrives with the
/// checkout rather than with the configuration copy. Saying "not inherited"
/// tells the reader the opposite of the truth.
#[test]
fn the_disclosure_does_not_claim_skipped_configuration_is_absent() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.write("vendor/CLAUDE.md", "planted guidance\n");
    repo.commit("fixture");

    let loaded = ahu::config::load(repo.path()).unwrap().unwrap();
    let taken = snapshot::collect(repo.path()).unwrap();
    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let enforcement = adapter
        .enforcement("claude-opus-5", Default::default())
        .unwrap();
    let found = ahu::hooks::collect(repo.path(), "claude-code").unwrap();
    let built = ahu::inventory::build(&ahu::inventory::Subject {
        repo_root: repo.path(),
        loaded_config: &loaded,
        snapshot: &taken,
        agent: None,
        harness: "claude-code",
        model: "claude-opus-5",
        enforcement: &enforcement,
        hooks: &found,
        prompt: None,
    })
    .unwrap();

    let gaps = built.coverage_gaps.join("\n");
    assert!(gaps.contains("vendor"), "{gaps}");
    assert!(
        !gaps.contains("nor inherited"),
        "the inventory claims skipped configuration is not inherited: {gaps}"
    );
    assert!(
        gaps.contains("still present in the task worktree"),
        "the inventory must say committed files under a skipped path are present: {gaps}"
    );
}

// --- `.worktrees` must really ignore itself ----------------------------------

#[test]
fn a_committed_worktrees_gitignore_that_ignores_nothing_is_refused() {
    let repo = TestRepo::new();
    repo.write(".worktrees/.gitignore", "# nothing ignored here\n");
    repo.commit("hostile gitignore");

    let error = ahu::state::ensure_worktrees_root(repo.path())
        .expect_err("a .gitignore that ignores nothing must be refused");
    let message = error.to_string();
    assert!(message.contains(".worktrees"), "{message}");
    assert!(message.contains(".gitignore"), "{message}");
}

#[test]
fn the_gitignore_ahu_writes_itself_is_accepted_on_every_later_launch() {
    let repo = TestRepo::new();
    ahu::state::ensure_worktrees_root(repo.path()).expect("first call writes it");
    ahu::state::ensure_worktrees_root(repo.path()).expect("second call accepts it");
    assert_eq!(
        std::fs::read_to_string(repo.path().join(".worktrees/.gitignore")).unwrap(),
        ahu::state::WORKTREES_GITIGNORE
    );
}

// --- Containment is not allowed to degrade into a textual prefix test --------

#[test]
fn a_worktree_path_that_cannot_be_resolved_is_refused() {
    let repo = TestRepo::new();
    let discovered = git::discover(repo.path()).unwrap();
    // Textually inside the repository, actually outside it, and nonexistent —
    // so `canonicalize` fails and there is nothing to compare components of.
    let escaping = discovered.root.join(".worktrees/../../outside-the-repo");

    let error = ahu::state::verify_worktree_inside_repo(&discovered.root, &escaping)
        .expect_err("an unresolvable worktree path must be refused");
    assert!(
        error.to_string().contains("outside-the-repo") || error.to_string().contains("cannot"),
        "{error}"
    );
}

#[test]
fn a_prepared_worktree_inside_the_repository_still_verifies() {
    let repo = TestRepo::new();
    let discovered = git::discover(repo.path()).unwrap();
    let worktree = ahu::state::worktree_dir(&discovered.root, "task0001").unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    ahu::state::verify_worktree_inside_repo(&discovered.root, &worktree)
        .expect("a real worktree inside the repository verifies");
}

// --- Positional arguments to git stay positional -----------------------------

#[test]
fn a_worktree_path_is_never_read_as_a_git_option() {
    let repo = TestRepo::new();
    let discovered = git::discover(repo.path()).unwrap();
    let base = discovered.head.clone().expect("the fixture has a commit");
    // A leading hyphen is the whole point: without a `--` delimiter git reads
    // this as a bundle of short options instead of a path.
    let hostile = std::path::Path::new("-weird-worktree");

    git::add_worktree(&discovered, hostile, "ahu/test/hyphen", &base)
        .expect("a hyphen-leading path must be treated as a path");
    assert!(
        repo.path().join("-weird-worktree").is_dir(),
        "the worktree was not created where the path said"
    );

    git::remove_worktree(&discovered, hostile, "ahu/test/hyphen")
        .expect("removal must treat it as a path too");
    assert!(!repo.path().join("-weird-worktree").exists());
}

/// A source directory swapped for a symlink after collection must not be copied
/// through.
///
/// `materialize` checked only the final component with `symlink_metadata`, and
/// that call follows symlinks in ancestor directories. So replacing `.claude`
/// with a symlink between the preview and the copy — the window in which the
/// user is reading the preview — passed the check for `.claude/settings.json`
/// and copied an external file into the task worktree. `safe_target` protects
/// the destination; this is about the source.
#[test]
fn an_ancestor_directory_swapped_for_a_symlink_is_refused_not_followed() {
    let repo = TestRepo::new();
    repo.write(".claude/settings.json", "{\"benign\": true}\n");
    repo.commit("fixture");

    // Collected while everything is a real file in a real directory.
    let taken = ahu::snapshot::collect(repo.path()).unwrap();
    assert!(
        taken
            .entries
            .iter()
            .any(|e| e.path == ".claude/settings.json"),
        "the fixture must be collected as a regular file"
    );

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt");
    ahu::git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/ancestor",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    // Now the ancestor is replaced, with a matching file name behind it.
    let outside = tempfile::TempDir::new().unwrap();
    std::fs::write(
        outside.path().join("settings.json"),
        "SYNTHETIC_EXTERNAL_SECRET\n",
    )
    .unwrap();
    std::fs::rename(
        repo.path().join(".claude"),
        repo.path().join(".claude-real"),
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.path(), repo.path().join(".claude")).unwrap();

    let report = ahu::snapshot::materialize(repo.path(), &taken, &worktree).unwrap();

    let landed = worktree.join(".claude/settings.json");
    let contents = std::fs::read_to_string(&landed).unwrap_or_default();
    assert!(
        !contents.contains("SYNTHETIC_EXTERNAL_SECRET"),
        "an external file was copied in through a swapped ancestor: {contents:?}"
    );
    assert!(
        report
            .refused_sources
            .contains(&".claude/settings.json".to_string()),
        "the refusal must be disclosed, not silent: {report:?}"
    );
    assert!(
        report
            .concurrently_modified
            .contains(&".claude/settings.json".to_string()),
        "{report:?}"
    );
}

/// The leaf case still holds: this is the regression the ancestor case escaped.
#[test]
fn a_leaf_swapped_for_a_symlink_is_still_refused() {
    let repo = TestRepo::new();
    repo.write(".claude/settings.json", "{\"benign\": true}\n");
    repo.commit("fixture");
    let taken = ahu::snapshot::collect(repo.path()).unwrap();

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt-leaf");
    ahu::git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/leaf",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    let outside = tempfile::TempDir::new().unwrap();
    let external = outside.path().join("elsewhere.json");
    std::fs::write(&external, "SYNTHETIC_EXTERNAL_SECRET\n").unwrap();
    std::fs::remove_file(repo.path().join(".claude/settings.json")).unwrap();
    std::os::unix::fs::symlink(&external, repo.path().join(".claude/settings.json")).unwrap();

    let report = ahu::snapshot::materialize(repo.path(), &taken, &worktree).unwrap();
    let contents =
        std::fs::read_to_string(worktree.join(".claude/settings.json")).unwrap_or_default();
    assert!(
        !contents.contains("SYNTHETIC_EXTERNAL_SECRET"),
        "{contents:?}"
    );
    assert!(
        report
            .refused_sources
            .contains(&".claude/settings.json".to_string()),
        "{report:?}"
    );
}

/// A source deleted between collection and the copy is a concurrent change, not
/// a refusal — the distinction matters because only one of them is an attempt.
#[test]
fn a_source_deleted_after_collection_is_reported_as_concurrently_modified() {
    let repo = TestRepo::new();
    repo.write(".claude/settings.json", "{\"benign\": true}\n");
    repo.commit("fixture");
    let taken = ahu::snapshot::collect(repo.path()).unwrap();

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt-gone");
    ahu::git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/gone",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    std::fs::remove_file(repo.path().join(".claude/settings.json")).unwrap();
    let report = ahu::snapshot::materialize(repo.path(), &taken, &worktree).unwrap();
    assert!(
        report
            .concurrently_modified
            .contains(&".claude/settings.json".to_string()),
        "{report:?}"
    );
    assert!(report.refused_sources.is_empty(), "{report:?}");
}

/// The bytes ahu digests must be the bytes it copies.
///
/// `materialize` used to open the source path twice: once for the digest, once
/// for `std::fs::copy`. It now hashes and copies from one descriptor, so the
/// content that reaches the worktree is the content whose digest was compared.
#[test]
fn the_copied_bytes_are_the_digested_bytes() {
    let repo = TestRepo::new();
    let script = repo.write(".claude/hooks/guard.sh", "#!/bin/sh\nexit 0\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    repo.commit("fixture");
    let taken = ahu::snapshot::collect(repo.path()).unwrap();

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt-bytes");
    ahu::git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/bytes",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    // Change the contents after collection: the copy takes the new bytes, and
    // says so, rather than copying one version and reporting another's digest.
    repo.write(".claude/hooks/guard.sh", "#!/bin/sh\nexit 3\n");
    let report = ahu::snapshot::materialize(repo.path(), &taken, &worktree).unwrap();
    assert!(
        report
            .concurrently_modified
            .contains(&".claude/hooks/guard.sh".to_string()),
        "{report:?}"
    );
    let copied = worktree.join(".claude/hooks/guard.sh");
    assert_eq!(
        std::fs::read_to_string(&copied).unwrap(),
        "#!/bin/sh\nexit 3\n"
    );
    assert_eq!(
        ahu::util::digest_file(&copied).unwrap(),
        ahu::util::digest_file(&repo.path().join(".claude/hooks/guard.sh")).unwrap(),
        "the worktree must hold exactly what was read"
    );
    // And the mode came across with it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(std::fs::metadata(&copied).unwrap().permissions().mode() & 0o111 != 0);
    }
}

/// An OpenCode agent's configuration travels the same way every other
/// harness's does.
///
/// OpenCode reads a project `opencode.json`/`.jsonc` and a `.opencode/`
/// directory, takes its rules from `AGENTS.md` with the Claude files as a
/// fallback, and discovers skills on demand — including duplicate skill trees
/// under both `.agents/skills` and `.claude/skills`. All of it has to be in the
/// task worktree before OpenCode looks for it, because ahu never tells OpenCode
/// where to look.
///
/// An `opencode.json` may name `plugin` modules OpenCode installs and runs at
/// startup, so this is executable configuration: it is carried and digested,
/// never rewritten, and never disabled with `--pure`.
#[test]
fn the_snapshot_covers_opencode_configuration_rules_and_duplicate_skills() {
    let repo = TestRepo::new();
    repo.write(
        "opencode.json",
        "{\"$schema\":\"https://opencode.ai/config.json\",\"plugin\":[\"example-plugin\"]}\n",
    );
    repo.write("nested/opencode.jsonc", "{ /* comment */ }\n");
    repo.write(
        ".opencode/agent/reviewer.md",
        "---\nmode: primary\n---\nbody\n",
    );
    repo.write(".opencode/command/ship.md", "ship it\n");
    repo.write("AGENTS.md", "repository rules for OpenCode\n");
    repo.write("CLAUDE.md", "the Claude-compatible fallback\n");
    repo.write(".agents/skills/review/SKILL.md", "agents-tree skill\n");
    repo.write(".claude/skills/review/SKILL.md", "claude-tree skill\n");
    let hook = repo.write(".opencode/plugin/startup.js", "export default () => {}\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    repo.write("src/main.rs", "fn main() {}\n");

    let taken = snapshot::collect(repo.path()).unwrap();
    let by_path = taken.by_path();
    for expected in [
        "opencode.json",
        "nested/opencode.jsonc",
        ".opencode/agent/reviewer.md",
        ".opencode/command/ship.md",
        "AGENTS.md",
        "CLAUDE.md",
        ".agents/skills/review/SKILL.md",
        ".claude/skills/review/SKILL.md",
        ".opencode/plugin/startup.js",
    ] {
        assert!(
            by_path.contains_key(expected),
            "missing {expected} in {:?}",
            by_path.keys().collect::<Vec<_>>()
        );
    }
    // The two skill trees are distinct files, not one deduplicated by name.
    assert_ne!(
        by_path[".agents/skills/review/SKILL.md"].digest,
        by_path[".claude/skills/review/SKILL.md"].digest
    );
    assert!(!by_path.contains_key("src/main.rs"));

    // Executable configuration is recorded as executable, and the mode is part
    // of the digest: a plugin gaining its executable bit changes behaviour.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(by_path[".opencode/plugin/startup.js"].executable);
        assert_eq!(taken.executable_count(), 1);
        let before = taken.digest();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o644)).unwrap();
        let after = snapshot::collect(repo.path()).unwrap();
        assert!(!after.by_path()[".opencode/plugin/startup.js"].executable);
        assert_ne!(before, after.digest(), "the mode is part of the digest");
    }

    // And it all reaches a task worktree with its bytes and its mode intact.
    let discovered = git::discover(repo.path()).unwrap();
    repo.commit("opencode configuration");
    let taken = snapshot::collect(repo.path()).unwrap();
    let worktree = repo.state_path().join("wt-opencode");
    git::add_worktree(
        &discovered,
        &worktree,
        "ahu/test/opencode",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();
    snapshot::materialize(repo.path(), &taken, &worktree).unwrap();
    assert_eq!(
        std::fs::read_to_string(worktree.join("opencode.json")).unwrap(),
        "{\"$schema\":\"https://opencode.ai/config.json\",\"plugin\":[\"example-plugin\"]}\n",
        "ahu must carry the user's OpenCode configuration unchanged"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join(".agents/skills/review/SKILL.md")).unwrap(),
        "agents-tree skill\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join(".claude/skills/review/SKILL.md")).unwrap(),
        "claude-tree skill\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("AGENTS.md")).unwrap(),
        "repository rules for OpenCode\n"
    );
}
