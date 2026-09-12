mod common;

use common::{TestRepo, git};
use std::path::Path;

fn review(checkout: &Path, old_state: &Path) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .args(["hygiene", "@chris"])
        .current_dir(checkout)
        .env_remove("AHU_STATE_DIR")
        .env("XDG_STATE_HOME", old_state)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let identity = ahu::git::discover(checkout).unwrap().identity();
    assert!(
        checkout
            .join(".ahu/state/repos")
            .join(identity)
            .join("hygiene.json")
            .is_file()
    );
    assert!(git(checkout, &["status", "--porcelain"]).is_empty());
    assert_eq!(git(checkout, &["check-ignore", ".ahu/state"]), ".ahu/state");
    assert!(!old_state.join("ahu").exists());
}

#[test]
fn sessions_store_state_locally_and_nested_launches_create_sibling_worktrees() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    review(repo.path(), repo.state_path());

    let main = ahu::git::discover(repo.path()).unwrap();
    ahu::state::ensure_worktrees_root(&main.root).unwrap();
    let first = ahu::state::worktree_dir(&main.root, "first").unwrap();
    ahu::git::add_worktree(&main, &first, "first", "HEAD").unwrap();
    review(&first, repo.state_path());

    let linked = ahu::git::discover(&first).unwrap();
    let second = ahu::state::worktree_dir(&linked.root, "second").unwrap();
    assert_eq!(second, main.root.join(".worktrees/second"));
    ahu::state::ensure_worktrees_root(&linked.root).unwrap();
    ahu::git::add_worktree(&linked, &second, "second", "HEAD").unwrap();
    ahu::state::verify_worktree_inside_repo(&linked.root, &second).unwrap();
    review(&second, repo.state_path());
    assert!(!first.join(".worktrees").exists());
    assert!(!second.join(".worktrees").exists());
}

#[test]
#[cfg(unix)]
fn local_state_refuses_symlinks_and_non_ignoring_policy() {
    let repo = TestRepo::new();
    std::os::unix::fs::symlink(repo.state_path(), repo.path().join(".ahu")).unwrap();
    assert!(ahu::state::ensure_checkout_state(repo.path()).is_err());
    std::fs::remove_file(repo.path().join(".ahu")).unwrap();
    std::fs::create_dir(repo.path().join(".ahu")).unwrap();
    std::fs::write(repo.path().join(".ahu/.gitignore"), "# nothing ignored\n").unwrap();
    assert!(ahu::state::ensure_checkout_state(repo.path()).is_err());
    assert!(
        std::fs::read_dir(repo.state_path())
            .unwrap()
            .next()
            .is_none()
    );
}
