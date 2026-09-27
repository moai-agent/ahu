mod common;

use common::TestRepo;

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
