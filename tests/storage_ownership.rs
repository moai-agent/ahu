mod common;
use ahu::storage::HeadlessStore;

#[test]
fn review_regression_fake_git_marker_is_not_verified_ownership() {
    let root = tempfile::tempdir().unwrap();
    let primary = root.path().canonicalize().unwrap();
    let marker = primary.join(".git");
    std::fs::create_dir(&marker).unwrap();
    let identity = ahu::util::digest_bytes(marker.as_os_str().as_encoded_bytes());
    let path = primary
        .join(".ahu/state/repos")
        .join(&identity[..16])
        .join("headless");
    assert!(HeadlessStore::containing(&path).is_err());
}

#[test]
fn review_regression_verified_primary_rechecks_replaced_marker() {
    let root = common::TestRepo::new();
    let repo = ahu::git::discover(root.path()).unwrap();
    let store = HeadlessStore::for_repo(&repo).unwrap();
    assert!(
        HeadlessStore::containing(&store.directory)
            .unwrap()
            .is_some()
    );
    std::fs::rename(root.path().join(".git"), root.path().join("saved-git")).unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    assert!(HeadlessStore::containing(&store.directory).is_err());
}

#[test]
fn separate_git_directory_without_a_discoverable_primary_is_refused() {
    let root = common::TestRepo::new();
    let external = tempfile::tempdir().unwrap();
    let gitdir = external.path().join("git-storage");
    common::git(
        root.path(),
        &["init", "--separate-git-dir", gitdir.to_str().unwrap()],
    );
    let repo = ahu::git::discover(root.path()).unwrap();
    // Git's worktree inventory reports the detached gitdir itself as primary,
    // not this working tree. Do not infer ownership from the invoking checkout.
    let error = HeadlessStore::for_repo(&repo).unwrap_err().to_string();
    assert!(
        error.contains("primary checkout") && error.contains("unsupported"),
        "{error}"
    );
    let wrong = repo
        .root
        .join(".ahu/state/repos")
        .join(repo.identity())
        .join("headless");
    assert!(HeadlessStore::containing(&wrong).is_err());
}

#[test]
fn linked_checkout_discovers_primary_but_cannot_own_coordination() {
    let root = common::TestRepo::new();
    let external = tempfile::tempdir().unwrap();
    let repo = ahu::git::discover(root.path()).unwrap();
    let store = HeadlessStore::for_repo(&repo).unwrap();
    let sibling = external.path().join("linked");
    common::git(
        root.path(),
        &[
            "worktree",
            "add",
            "--detach",
            sibling.to_str().unwrap(),
            "HEAD",
        ],
    );
    let linked = ahu::git::discover(&sibling).unwrap();
    assert_eq!(
        HeadlessStore::for_repo(&linked).unwrap().directory,
        store.directory
    );
    assert!(
        HeadlessStore::containing(&store.directory)
            .unwrap()
            .is_some()
    );
    let wrong = sibling
        .canonicalize()
        .unwrap()
        .join(".ahu/state/repos")
        .join(repo.identity())
        .join("headless");
    assert!(HeadlessStore::containing(&wrong).is_err());
}

#[test]
fn verified_primary_still_refuses_state_symlink_after_cache_hit() {
    use std::os::unix::fs::symlink;
    let root = common::TestRepo::new();
    let repo = ahu::git::discover(root.path()).unwrap();
    let store = HeadlessStore::for_repo(&repo).unwrap();
    assert!(
        HeadlessStore::containing(&store.directory)
            .unwrap()
            .is_some()
    );
    let external = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".ahu")).unwrap();
    symlink(external.path(), root.path().join(".ahu/state")).unwrap();
    assert!(HeadlessStore::containing(&store.directory).is_err());
}
