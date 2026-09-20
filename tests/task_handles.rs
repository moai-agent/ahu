mod common;

use ahu::{git, task, task_handles as handles, task_ref};

#[test]
fn handles_are_exact_case_insensitive_and_share_only_the_repository_scope() {
    let fixture = common::TestRepo::new();
    let repo = git::discover(fixture.path()).unwrap();
    let other = common::TestRepo::new();
    let other_repo = git::discover(other.path()).unwrap();
    let linked = fixture.state_path().join("linked");
    common::git(
        fixture.path(),
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    let sibling = git::discover(&linked).unwrap();
    let id = task::new_task_id().unwrap();
    assert_eq!(
        handles::reserve(&repo, &id, Some("@Storage-Cleanup"), "unused").unwrap(),
        "@storage-cleanup"
    );
    assert_eq!(task_ref::resolve(&sibling, "@STORAGE-CLEANUP").unwrap(), id);
    assert!(task_ref::resolve(&repo, "@storage").is_err());
    assert!(task_ref::resolve(&other_repo, "@storage-cleanup").is_err());
    assert_eq!(
        task_ref::resolve(&repo, &format!("ahu:task:{id}")).unwrap(),
        id
    );
    assert_eq!(task_ref::resolve(&repo, &id[..12]).unwrap(), id[..12]);
    assert!(task_ref::resolve(&repo, "ahu:task:@storage-cleanup").is_err());
    assert!(task_ref::resolve(&repo, "ahu:agent:storage-cleanup").is_err());
    assert!(handles::reserve(&repo, &id, Some("renamed"), "unused").is_err());
    let second = task::new_task_id().unwrap();
    assert!(handles::reserve(&repo, &second, Some("storage-cleanup"), "unused").is_err());
    assert_eq!(
        handles::reserve(&other_repo, &second, Some("storage-cleanup"), "unused").unwrap(),
        "@storage-cleanup"
    );
    assert_eq!(common::git(fixture.path(), &["status", "--porcelain"]), "");
}

#[test]
fn concurrent_allocations_never_reassign_a_name_or_collide_across_backends() {
    let fixture = common::TestRepo::new();
    let repo = git::discover(fixture.path()).unwrap();
    let ids: Vec<_> = (0..12).map(|_| task::new_task_id().unwrap()).collect();
    let winners = std::thread::scope(|scope| {
        let threads: Vec<_> = ids
            .iter()
            .map(|id| {
                let repo = &repo;
                scope.spawn(move || (id, handles::reserve(repo, id, Some("shared-name"), "")))
            })
            .collect();
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|(_, r)| r.is_ok())
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(winners.len(), 1);
    assert_eq!(handles::resolve(&repo, "@shared-name").unwrap(), winners[0]);
    let automatic = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..12)
            .map(|_| {
                let repo = &repo;
                scope.spawn(move || {
                    let id = task::new_task_id().unwrap();
                    let name = handles::reserve(repo, &id, None, "Fix the parser").unwrap();
                    assert_eq!(handles::resolve(repo, &name).unwrap(), id);
                    name
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
    });
    assert_eq!(automatic.len(), 12);
    assert!(automatic.contains("@fix-the-parser"));
}

#[test]
fn names_are_bounded_and_cannot_be_paths_or_options() {
    for invalid in [
        "",
        "@",
        "../escape",
        "foo/bar",
        "-flag",
        "a--b",
        "a-",
        "a\nb",
        "a b",
        "@@name",
        "123",
        "é",
        &"a".repeat(49),
    ] {
        assert!(handles::name(invalid).is_err(), "{invalid:?}");
    }
    for title in [
        "",
        "纯中文",
        "123 starts numerically",
        "**Fix** parser $(date)",
        &"x".repeat(500),
    ] {
        assert!(handles::name(&handles::generated_name(title)).is_ok());
    }
    for args in [
        vec!["launch", "@worker", "--name", "@short", "--prompt", "work"],
        vec![
            "launch",
            "@worker",
            "--headless",
            "--name",
            "short",
            "--prompt",
            "work",
        ],
    ] {
        assert!(ahu::cli::parse(args).is_ok());
    }
    assert!(
        ahu::cli::parse([
            "launch", "@worker", "--name", "a", "--name", "b", "--prompt", "work"
        ])
        .is_err()
    );
}

#[test]
#[cfg(unix)]
fn malformed_redirected_or_partial_bindings_never_resolve_or_get_reused() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for case in [
        "symlink",
        "hardlink",
        "mode",
        "oversized",
        "partial",
        "mismatch",
        "foreign",
    ] {
        let fixture = common::TestRepo::new();
        let repo = git::discover(fixture.path()).unwrap();
        let id = task::new_task_id().unwrap();
        handles::reserve(&repo, &id, Some("kept"), "").unwrap();
        let root = ahu::state::coordination_dir(&repo)
            .unwrap()
            .join("task-handles");
        let path = root.join("names/kept.json");
        let original = std::fs::read(&path).unwrap();
        let outside = fixture.state_path().join("binding");
        std::fs::write(&outside, &original).unwrap();
        match case {
            "symlink" => {
                std::fs::remove_file(&path).unwrap();
                symlink(&outside, &path).unwrap();
            }
            "hardlink" => {
                std::fs::hard_link(&path, fixture.state_path().join("second-link")).unwrap();
            }
            "mode" => {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap()
            }
            "oversized" => std::fs::write(&path, vec![b' '; 1025]).unwrap(),
            "partial" => std::fs::remove_file(root.join(format!("ids/{id}.json"))).unwrap(),
            "mismatch" | "foreign" => {
                let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
                value[if case == "foreign" {
                    "repo_identity"
                } else {
                    "task_id"
                }] = "wrong".into();
                std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(handles::resolve(&repo, "@kept").is_err(), "{case}");
        assert!(
            handles::reserve(&repo, &task::new_task_id().unwrap(), Some("kept"), "").is_err(),
            "{case}"
        );
        assert_eq!(std::fs::read(&outside).unwrap(), original);
    }
    let fixture = common::TestRepo::new();
    let repo = git::discover(fixture.path()).unwrap();
    let root = ahu::state::coordination_dir(&repo).unwrap();
    ahu::state::create_private_dir_all(&root).unwrap();
    symlink(fixture.state_path(), root.join("task-handles")).unwrap();
    assert!(handles::reserve(&repo, &task::new_task_id().unwrap(), Some("escape"), "").is_err());
    assert!(!fixture.state_path().join("names").exists());
}
