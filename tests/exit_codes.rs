mod common;

use std::io::Write;
use std::process::{Command, Stdio};

use common::TestRepo;

#[test]
fn command_exit_codes_distinguish_success_cancellation_and_failure_categories() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("reviewer", "1.0.0", "claude-opus-5");
    repo.write("assignment.txt", "Review the fixture.");
    repo.commit("fixture");
    let outside = tempfile::tempdir().unwrap();
    let malformed = TestRepo::new();
    malformed.write(".agents/ahu/config.toml", "not valid TOML");
    let bin = common::fake_harness(repo.state_path(), &repo.state_path().join("argv"));
    let cases: &[(&str, &std::path::Path, &[&str], &str, i32)] = &[
        ("success", repo.path(), &["help"], "", 0),
        ("cancelled", repo.path(), &[], "@reviewer\n.cancel\n", 1),
        ("usage", repo.path(), &["not-a-command"], "", 2),
        (
            "unknown agent",
            repo.path(),
            &[
                "launch",
                "@absent",
                "--prompt-file",
                "assignment.txt",
                "--dry-run",
            ],
            "",
            3,
        ),
        ("missing repository", outside.path(), &["agents"], "", 4),
        (
            "missing cmux",
            repo.path(),
            &["launch", "@reviewer", "--prompt-file", "assignment.txt"],
            "",
            4,
        ),
        (
            "invalid configuration",
            malformed.path(),
            &["launch", "@reviewer", "--prompt", "Review", "--dry-run"],
            "",
            5,
        ),
    ];
    for (label, cwd, args, input, expected) in cases {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ahu"))
            .current_dir(cwd)
            .args(*args)
            .env("NO_COLOR", "1")
            .env("AHU_STATE_DIR", repo.state_path())
            .env("AHU_CMUX_BIN", repo.state_path().join("missing-cmux"))
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(*expected),
            "{label}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if *expected >= 2 {
            assert!(!output.stderr.is_empty(), "{label} needs a diagnostic");
        }
    }
    assert_eq!(
        common::git(repo.path(), &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1
    );
    assert_eq!(common::git(repo.path(), &["branch", "--list", "ahu/*"]), "");
}
