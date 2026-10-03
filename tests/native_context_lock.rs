mod common;
use common::TestRepo;

#[test]
fn native_mcp_changes_require_private_acceptance_without_shared_lock_churn() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.write(
        ".agents/mcp_config.json",
        r#"{"mcpServers":{"ahu":{"command":"ahu","args":["mcp","serve"]}}}"#,
    );
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let native = home.path().join(".gemini/config/mcp_config.json");
    std::fs::create_dir_all(native.parent().unwrap()).unwrap();
    std::fs::write(
        &native,
        br#"{"mcpServers":{"ahu":{"command":"SYNTHETIC-PRIVATE-FIRST"}}}"#,
    )
    .unwrap();
    let run = |args: &[&str]| {
        common::ahu()
            .current_dir(repo.path())
            .env("HOME", home.path())
            .env("XDG_STATE_HOME", state.path())
            .args(args)
            .output()
            .unwrap()
    };
    let update = run(&["lock", "--update"]);
    assert!(
        update.status.success(),
        "{}",
        String::from_utf8_lossy(&update.stderr)
    );
    common::git(repo.path(), &["add", ".agents", "ahu.lock"]);
    common::git(repo.path(), &["commit", "-qm", "lock fixture context"]);
    let shared = std::fs::read(repo.path().join("ahu.lock")).unwrap();
    assert!(!String::from_utf8_lossy(&shared).contains("SYNTHETIC-PRIVATE"));
    assert!(!String::from_utf8_lossy(&shared).contains("user:antigravity"));
    assert!(run(&["lock"]).status.success());
    std::fs::write(
        &native,
        br#"{"mcpServers":{"ahu":{"command":"SYNTHETIC-PRIVATE-SECOND"}}}"#,
    )
    .unwrap();
    let changed = run(&["lock"]);
    assert!(!changed.status.success());
    assert!(!String::from_utf8_lossy(&changed.stdout).contains("SYNTHETIC-PRIVATE"));
    assert!(run(&["lock", "--update"]).status.success());
    assert!(run(&["lock"]).status.success());
    assert_eq!(std::fs::read(repo.path().join("ahu.lock")).unwrap(), shared);
    std::fs::remove_file(&native).unwrap();
    assert!(!run(&["lock"]).status.success());
    assert!(run(&["lock", "--update"]).status.success());
    assert!(run(&["lock"]).status.success());
    assert_eq!(std::fs::read(repo.path().join("ahu.lock")).unwrap(), shared);
}
