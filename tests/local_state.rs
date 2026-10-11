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

#[test]
#[cfg(unix)]
fn oversized_configuration_is_refused_without_being_parsed() {
    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.path().join(".agents/ahu")).unwrap();
    std::fs::write(
        repo.path().join(".agents/ahu/config.toml"),
        vec![0u8; 1048577],
    )
    .unwrap();
    let error = ahu::config::load(repo.path())
        .expect_err("oversized configuration must fail")
        .to_string();
    assert!(error.contains("cannot read"), "{error}");
    assert!(
        error.contains("file is larger than 1048576 bytes"),
        "{error}"
    );
}
