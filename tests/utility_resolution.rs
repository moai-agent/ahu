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
    cmux_marker: PathBuf,
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
        let cmux_marker = outside.path().join("repository-cmux-ran");
        for (name, marker) in [("git", &git_marker), ("cmux", &cmux_marker)] {
            executable(
                &repo.path().join(name),
                &format!(": > {}\nexit 1", quote(marker)),
            );
        }
        executable(
            &bin.join("git"),
            &format!(
                ": > {}\nexec /usr/bin/git \"$@\"",
                quote(&outside.path().join("external-git-ran"))
            ),
        );
        // Stop launch/doctor at a synthetic cmux failure, never a live session.
        executable(
            &bin.join("cmux"),
            &format!(
                ": > {}\nprintf 'synthetic cmux unavailable\\n' >&2\nexit 1",
                quote(&outside.path().join("external-cmux-ran"))
            ),
        );
        let alias = outside.path().join("aliases");
        std::fs::create_dir(&alias).unwrap();
        for name in ["git", "cmux"] {
            symlink(repo.path().join(name), alias.join(name)).unwrap();
        }
        symlink(repo.path(), outside.path().join("directory-alias")).unwrap();
        repo.commit("fixture");
        Self {
            repo,
            outside,
            bin,
            alias,
            git_marker,
            cmux_marker,
        }
    }

    fn command(&self, cwd: &Path, path: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ahu"));
        command
            .current_dir(cwd)
            .env("PATH", path)
            .env("HOME", self.outside.path())
            .env("AHU_STATE_DIR", self.repo.state_path())
            .env_remove("AHU_CMUX_BIN")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        command
    }

    fn assert_unexecuted(&self, result: &Output) {
        assert!(
            !self.git_marker.exists() && !self.cmux_marker.exists(),
            "repository utility ran: {result:?}"
        );
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
            vec![
                "launch",
                "@sable",
                "--prompt-file",
                "assignment.txt",
                "--dry-run",
            ],
            vec!["launch", "@sable", "--prompt-file", "assignment.txt"],
        ] {
            let result = fixture
                .command(fixture.repo.path(), &path)
                .args(&args)
                .output()
                .unwrap();
            fixture.assert_unexecuted(&result);
            assert!(fixture.outside.path().join("external-git-ran").exists());
            if args == ["agents"] || args.contains(&"--dry-run") {
                assert!(result.status.success(), "{args:?}: {result:?}");
            } else {
                assert!(!result.status.success(), "{args:?}: {result:?}");
                assert!(fixture.outside.path().join("external-cmux-ran").exists());
                std::fs::remove_file(fixture.outside.path().join("external-cmux-ran")).unwrap();
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
        assert!(fixture.outside.path().join("external-cmux-ran").exists());
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
        assert!(text.contains("cmux was not found"), "{result:?}");
    }
}

#[test]
fn explicit_cmux_overrides_retain_user_selected_command_semantics() {
    let fixture = Fixture::new();
    let path = format!(
        "{}:{}:/usr/bin:/bin",
        fixture.repo.path().display(),
        fixture.bin.display()
    );
    for override_path in [
        PathBuf::from("cmux"),
        PathBuf::from("./cmux"),
        fixture.repo.path().join("cmux"),
    ] {
        let result = fixture
            .command(fixture.repo.path(), &path)
            .env("AHU_CMUX_BIN", override_path)
            .arg("doctor")
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!fixture.git_marker.exists());
        assert!(fixture.cmux_marker.exists(), "{result:?}");
        std::fs::remove_file(&fixture.cmux_marker).unwrap();
    }
}

#[test]
fn default_cmux_pins_the_external_target_across_rpc_calls() {
    let fixture = Fixture::new();
    let real = fixture.outside.path().join("cmux-installation");
    let marker = fixture.outside.path().join("capabilities-ran");
    executable(
        &real,
        &format!(
            "case \"$1\" in\nping) /bin/ln -sf {} {} ;;\ncapabilities) : > {}; printf '{{\"capabilities\":[]}}\\n' ;;\nesac",
            quote(&fixture.repo.path().join("cmux")),
            quote(&fixture.bin.join("cmux")),
            quote(&marker),
        ),
    );
    std::fs::remove_file(fixture.bin.join("cmux")).unwrap();
    symlink(real, fixture.bin.join("cmux")).unwrap();
    let path = format!("{}:/usr/bin:/bin", fixture.bin.display());
    let result = fixture
        .command(fixture.repo.path(), &path)
        .arg("doctor")
        .output()
        .unwrap();
    fixture.assert_unexecuted(&result);
    assert!(
        marker.exists(),
        "the pinned external cmux must receive capabilities: {result:?}"
    );
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
        for name in ["git", "cmux"] {
            symlink(candidate_root.join(name), aliases.join(name)).unwrap();
        }
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
                    if command == "agents" {
                        assert!(result.status.success(), "{result:?}");
                    } else {
                        assert!(
                            fixture.outside.path().join("external-cmux-ran").exists(),
                            "{result:?}"
                        );
                        std::fs::remove_file(fixture.outside.path().join("external-cmux-ran"))
                            .unwrap();
                    }
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
