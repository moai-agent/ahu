#![cfg(unix)]

mod common;

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::TestRepo;

fn executable(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn quote(path: &Path) -> String {
    ahu::util::shell_single_quote(&path.to_string_lossy())
}

struct Fixture {
    repo: TestRepo,
    outside: tempfile::TempDir,
    bin: PathBuf,
    alias: PathBuf,
    git_marker: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let repo = TestRepo::new();
        repo.init_config();
        repo.add_agent("sable", "1.0.0", "claude-sonnet-5");
        repo.write("assignment.txt", "review the launcher\n");
        repo.write("nested/placeholder", "fixture\n");
        let outside = tempfile::tempdir().unwrap();
        let bin = common::fake_harness(outside.path(), &outside.path().join("harness-argv"));
        let git_marker = outside.path().join("repository-git-ran");
        executable(
            &repo.path().join("git"),
            &format!(": > {}\nexit 1", quote(&git_marker)),
        );
        executable(
            &bin.join("git"),
            &format!(
                ": > {}\nexec /usr/bin/git \"$@\"",
                quote(&outside.path().join("external-git-ran"))
            ),
        );
        let alias = outside.path().join("aliases");
        std::fs::create_dir(&alias).unwrap();
        symlink(repo.path().join("git"), alias.join("git")).unwrap();
        symlink(repo.path(), outside.path().join("directory-alias")).unwrap();
        repo.commit("fixture");
        Self {
            repo,
            outside,
            bin,
            alias,
            git_marker,
        }
    }

    fn command(&self, cwd: &Path, path: &str) -> Command {
        let mut command = common::ahu();
        command
            .current_dir(cwd)
            .env("PATH", path)
            .env("HOME", self.outside.path())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        command
    }

    fn assert_unexecuted(&self, result: &Output) {
        assert!(!self.git_marker.exists(), "repository Git ran: {result:?}");
    }
}

#[test]
fn repository_utilities_are_excluded_before_discovery_and_launch() {
    let fixture = Fixture::new();
    let prefixes = [
        ".".to_string(),
        String::new(),
        fixture.repo.path().display().to_string(),
        fixture.alias.display().to_string(),
        fixture
            .outside
            .path()
            .join("directory-alias")
            .display()
            .to_string(),
    ];
    for prefix in prefixes {
        let path = format!("{prefix}:{}:/usr/bin:/bin", fixture.bin.display());
        for args in [
            vec!["agents"],
            vec!["doctor"],
            vec!["@sable", "--prompt-file", "assignment.txt", "--dry-run"],
            vec!["@sable", "--prompt-file", "assignment.txt"],
        ] {
            let result = fixture
                .command(fixture.repo.path(), &path)
                .args(&args)
                .output()
                .unwrap();
            fixture.assert_unexecuted(&result);
            assert!(fixture.outside.path().join("external-git-ran").exists());
            if args == ["agents"]
                || args.contains(&"--dry-run")
                || (args == ["doctor"])
                || (args.first() == Some(&"@sable"))
            {
                assert!(result.status.success(), "{args:?}: {result:?}");
                if args.first() == Some(&"@sable") && !args.contains(&"--dry-run") {
                    assert!(fixture.outside.path().join("harness-argv").exists());
                }
            } else {
                assert!(!result.status.success(), "{args:?}: {result:?}");
            }
            std::fs::remove_file(fixture.outside.path().join("external-git-ran")).unwrap();
        }
    }
}

#[test]
fn bootstrap_inspects_ancestors_and_worktree_git_files() {
    let fixture = Fixture::new();
    let sibling = fixture.outside.path().join("linked");
    common::git(
        fixture.repo.path(),
        &["worktree", "add", "--detach", sibling.to_str().unwrap()],
    );
    assert!(sibling.join(".git").is_file());
    // The committed marker scripts are copied to the linked fixture worktree.
    for root in [fixture.repo.path(), sibling.as_path()] {
        let path = format!("{}:{}:/usr/bin:/bin", root.display(), fixture.bin.display());
        let result = fixture
            .command(&root.join("nested"), &path)
            .arg("doctor")
            .output()
            .unwrap();
        fixture.assert_unexecuted(&result);
        assert!(fixture.outside.path().join("external-git-ran").exists());
    }
}

#[test]
fn utility_resolution_fails_closed_without_an_external_installation() {
    let fixture = Fixture::new();
    for path in [
        ".".to_string(),
        String::new(),
        fixture.repo.path().display().to_string(),
        fixture.alias.display().to_string(),
    ] {
        let result = fixture
            .command(fixture.repo.path(), &path)
            .arg("doctor")
            .output()
            .unwrap();
        fixture.assert_unexecuted(&result);
        assert!(!result.status.success());
        let text = String::from_utf8_lossy(&result.stdout);
        assert!(text.contains("git was not found"), "{result:?}");
    }
}

#[test]
fn utility_candidates_in_primary_and_sibling_worktrees_are_never_bootstrap_tools() {
    let fixture = Fixture::new();
    let first = fixture.outside.path().join("first-linked");
    let second = fixture.outside.path().join("second-linked");
    for linked in [&first, &second] {
        common::git(
            fixture.repo.path(),
            &["worktree", "add", "--detach", linked.to_str().unwrap()],
        );
    }
    let roots = [fixture.repo.path(), first.as_path(), second.as_path()];
    for (index, candidate_root) in roots.iter().enumerate() {
        let aliases = fixture
            .outside
            .path()
            .join(format!("worktree-aliases-{index}"));
        std::fs::create_dir(&aliases).unwrap();
        symlink(candidate_root.join("git"), aliases.join("git")).unwrap();
        let directory_alias = fixture
            .outside
            .path()
            .join(format!("worktree-directory-{index}"));
        symlink(candidate_root, &directory_alias).unwrap();
        for cwd in roots.iter().filter(|cwd| *cwd != candidate_root) {
            for prefix in [
                *candidate_root,
                aliases.as_path(),
                directory_alias.as_path(),
            ] {
                let path = format!(
                    "{}:{}:/usr/bin:/bin",
                    prefix.display(),
                    fixture.bin.display()
                );
                for command in ["agents", "doctor"] {
                    let result = fixture
                        .command(&cwd.join("nested"), &path)
                        .arg(command)
                        .output()
                        .unwrap();
                    fixture.assert_unexecuted(&result);
                    assert!(
                        fixture.outside.path().join("external-git-ran").exists(),
                        "{result:?}"
                    );
                    assert!(result.status.success(), "{result:?}");
                    std::fs::remove_file(fixture.outside.path().join("external-git-ran")).unwrap();
                }
            }
        }
    }
}

#[test]
fn utility_marker_inspection_does_not_follow_git_pointers_or_unbounded_ancestry() {
    let fixture = Fixture::new();
    let marked = fixture.outside.path().join("unopened-tree");
    std::fs::create_dir(&marked).unwrap();
    executable(
        &marked.join("git"),
        &format!(": > {}\nexit 1", quote(&fixture.git_marker)),
    );
    let mut deep = fixture.outside.path().join("deep");
    for _ in 0..256 {
        deep.push("d");
    }
    std::fs::create_dir_all(&deep).unwrap();
    executable(
        &deep.join("git"),
        &format!(": > {}\nexit 1", quote(&fixture.git_marker)),
    );
    // A self-referential marker must be rejected through lstat, not followed.
    symlink(".git", marked.join(".git")).unwrap();
    for prefix in [&marked, &deep] {
        let path = format!(
            "{}:{}:/usr/bin:/bin",
            prefix.display(),
            fixture.bin.display()
        );
        let result = fixture
            .command(fixture.repo.path(), &path)
            .arg("agents")
            .output()
            .unwrap();
        fixture.assert_unexecuted(&result);
        assert!(result.status.success(), "{result:?}");
        assert!(fixture.outside.path().join("external-git-ran").exists());
        std::fs::remove_file(fixture.outside.path().join("external-git-ran")).unwrap();
    }
}

/// Installed version evidence is metadata, not a core version allowlist.
#[test]
fn observed_versions_are_evidence_not_an_allowlist() {
    if !common::in_harness_fixture("observed_versions_are_evidence_not_an_allowlist") {
        return;
    }
    let repo = common::TestRepo::new();
    repo.install_runtime("sixth");
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let descriptor = ahu::harness_runtime::discover(&discovered)
        .unwrap()
        .remove(0);
    assert_eq!(descriptor.tested_versions, ["1.2.3"]);
    let prerequisite = ahu::selection::check_prerequisite(&discovered, "sixth").unwrap();
    assert_eq!(prerequisite.version.as_deref(), Some("1.2.3"));
    ahu::catalog::check_version("9.8.7", None).unwrap();
    assert!(ahu::catalog::check_version("9.8.7", Some("1.2.3")).is_err());
}
