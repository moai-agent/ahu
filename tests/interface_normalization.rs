mod common;

use ahu::cli::extract_repository;
use common::TestRepo;

#[test]
fn repository_selection_does_not_consume_command_payloads() {
    for args in [
        vec!["message", "abc123", "--repo=elsewhere"],
        vec!["launch", "@worker", "--prompt", "--repo"],
        vec!["launch", "@worker", "--prompt-file", "--repo=elsewhere"],
    ] {
        let original: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (remaining, repository) = extract_repository(original.clone()).unwrap();
        assert_eq!(remaining, original);
        assert_eq!(repository, None);
    }
    for args in [
        vec!["--repo"],
        vec!["--repo="],
        vec!["--repo", "--help"],
        vec!["--repo=a", "--repo", "b", "tasks"],
    ] {
        assert!(extract_repository(args.into_iter().map(str::to_owned).collect()).is_err());
    }
}

#[test]
fn explicit_repository_works_outside_git_and_composes_with_color() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("selected-worker", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let outside = tempfile::tempdir().unwrap();
    for prefix in [
        vec!["--repo".to_owned(), repo.path().display().to_string()],
        vec![
            "--color=never".to_owned(),
            format!("--repo={}", repo.path().display()),
        ],
    ] {
        let output = common::ahu()
            .args(prefix)
            .arg("agents")
            .current_dir(outside.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("selected-worker"));
    }
}
