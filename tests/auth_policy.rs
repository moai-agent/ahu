mod common;

use common::TestRepo;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;

#[test]
fn auth_mutations_require_operator_before_probing_or_changing_state() {
    let repo = TestRepo::new();
    let bin = tempfile::tempdir().unwrap();
    let sentinel = bin.path().join("probed");
    let script = bin.path().join("claude");
    std::fs::write(&script, "#!/bin/sh\nprintf called > \"$AHU_TEST_AUTH_SENTINEL\"\nprintf '%s\\n' '{\"loggedIn\":true,\"email\":\"fixture@example.invalid\",\"orgId\":\"fixture-org\",\"authMethod\":\"oauth\",\"apiProvider\":\"first-party\"}'\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = std::env::join_paths(std::iter::once(bin.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let run = |args: &[&str], context: Option<(&str, &OsString)>| {
        let mut c = common::ahu();
        c.current_dir(repo.path())
            .args(args)
            .env("PATH", &path)
            .env("AHU_TEST_AUTH_SENTINEL", &sentinel);
        if let Some((key, value)) = context {
            c.env(key, value);
        }
        c.output().unwrap()
    };
    let host = run(&["auth", "bind", "--harness", "claude-code"], None);
    assert!(
        host.status.success(),
        "{}",
        String::from_utf8_lossy(&host.stderr)
    );
    assert!(sentinel.exists());
    std::fs::remove_file(&sentinel).unwrap();
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let binding = repo
        .path()
        .join(".ahu/state/repos")
        .join(discovered.identity())
        .join("auth-bindings.json");
    let before = std::fs::read(&binding).unwrap();
    let mut contexts = Vec::new();
    for marker in [
        "AHU_WORKER_SESSION",
        "AHU_TASK_ID",
        "AHU_TASK_DIR",
        "AHU_PARENT_TASK",
        "AHU_BROKER_TOKEN",
    ] {
        for value in ["", "headless", "cmux"] {
            contexts.push((marker, OsString::from(value)));
        }
    }
    use std::os::unix::ffi::OsStringExt;
    contexts.push(("AHU_TASK_ID", OsString::from_vec(vec![0xff])));
    for args in [
        vec!["auth", "bind", "--harness", "claude-code"],
        vec!["auth", "bind", "--harness", "claude-code", "--replace"],
        vec![
            "auth",
            "bind",
            "--harness",
            "claude-code",
            "--profile",
            "new",
        ],
        vec!["auth", "select", "--profile", "default"],
    ] {
        for (marker, value) in &contexts {
            let out = run(&args, Some((marker, value)));
            assert_eq!(out.status.code(), Some(5));
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("require a host operator context")
            );
            assert_eq!(std::fs::read(&binding).unwrap(), before);
            assert!(!sentinel.exists());
        }
    }
    let fresh = TestRepo::new();
    let mut combined = common::ahu();
    combined
        .current_dir(fresh.path())
        .args([
            "auth",
            "bind",
            "--harness",
            "claude-code",
            "--profile",
            "new",
        ])
        .env("PATH", &path)
        .env("AHU_TEST_AUTH_SENTINEL", &sentinel);
    for marker in [
        "AHU_WORKER_SESSION",
        "AHU_TASK_ID",
        "AHU_TASK_DIR",
        "AHU_PARENT_TASK",
        "AHU_BROKER_TOKEN",
    ] {
        combined.env(marker, "");
    }
    let out = combined.output().unwrap();
    assert_eq!(out.status.code(), Some(5));
    assert!(!sentinel.exists());
    assert!(!fresh.path().join(".ahu").exists());
    let worker = OsString::from("headless");
    let read = run(
        &[
            "auth",
            "readiness",
            "--harness",
            "claude-code",
            "--output",
            "json",
        ],
        Some(("AHU_WORKER_SESSION", &worker)),
    );
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert_eq!(std::fs::read(&binding).unwrap(), before);
    for args in [
        vec!["auth", "bind", "--harness", "claude-code", "--replace"],
        vec!["auth", "select", "--profile", "default"],
    ] {
        assert!(run(&args, None).status.success());
    }
}
