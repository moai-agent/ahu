mod common;

use common::TestRepo;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;

#[test]
fn antigravity_probe_disables_color_without_weakening_account_checks() {
    let repo = TestRepo::new();
    let bin = tempfile::tempdir().unwrap();
    let script = bin.path().join("agy");
    std::fs::write(
        &script,
        r#"#!/bin/sh
if [ "$AHU_TEST_AGY_IDENTITY" = missing ]; then exit 1; fi
if [ "$AHU_TEST_AGY_KIND" = key ]; then printf 'Gemini API key\n'; fi
if [ "$NO_COLOR" = 1 ]; then
    printf 'Signed in as %s\n' "$AHU_TEST_AGY_IDENTITY"
else
    printf '\033[32m%s\033[0m\n' "$AHU_TEST_AGY_IDENTITY"
fi
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = std::env::join_paths(std::iter::once(bin.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let run = |args: &[&str], identity: &str, kind: &str, inherited_color: Option<&str>| {
        let mut command = common::ahu();
        command
            .current_dir(repo.path())
            .args(args)
            .env("PATH", &path)
            // The regression must not depend on the test runner already
            // suppressing color, as an unattended coordinator often does.
            .env_remove("NO_COLOR")
            .env("AHU_TEST_AGY_IDENTITY", identity)
            .env("AHU_TEST_AGY_KIND", kind);
        if let Some(value) = inherited_color {
            command.env("NO_COLOR", value);
        }
        command.output().unwrap()
    };
    let bound = run(
        &["auth", "bind", "--harness", "antigravity"],
        "first@example.invalid",
        "account",
        None,
    );
    assert!(
        bound.status.success(),
        "{}",
        String::from_utf8_lossy(&bound.stderr)
    );
    let args = [
        "auth",
        "readiness",
        "--harness",
        "antigravity",
        "--output",
        "json",
    ];
    for inherited_color in [None, Some(""), Some("0")] {
        assert!(
            run(&args, "first@example.invalid", "account", inherited_color)
                .status
                .success()
        );
    }
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let binding = repo
        .path()
        .join(".ahu/state/repos")
        .join(discovered.identity())
        .join("auth-bindings.json");
    let before = std::fs::read(&binding).unwrap();
    for (identity, kind, expected, ready) in [
        ("first@example.invalid", "account", "matched", true),
        ("second@example.invalid", "account", "mismatch", false),
        ("first@example.invalid", "key", "unavailable", false),
        ("missing", "account", "unavailable", false),
    ] {
        let output = run(&args, identity, kind, None);
        assert_eq!(output.status.success(), ready);
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["binding"], expected);
        assert_eq!(value["ready"], ready);
        assert_eq!(std::fs::read(&binding).unwrap(), before);
        for bytes in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(bytes).contains("example.invalid"));
        }
    }
}

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

#[test]
fn doctor_reports_account_changes_without_disclosing_identity_or_mutating_bindings() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("one", "1.0.0", "claude-opus-5");
    repo.add_agent("two", "1.0.0", "claude-opus-5");
    repo.commit("fixture agents");
    let scratch = tempfile::tempdir().unwrap();
    let script = scratch.path().join("claude");
    std::fs::write(&script, "#!/bin/sh\nif [ \"$1\" = --version ]; then printf '2.1.288\\n'; else printf '%s\\n' \"$AHU_TEST_IDENTITY\"; fi\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = std::env::join_paths(std::iter::once(scratch.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let run = |args: &[&str], email: &str| {
        common::ahu()
            .current_dir(repo.path())
            .args(args)
            .env("PATH", &path)
            .env("AHU_CMUX_BIN", scratch.path().join("absent-cmux"))
            .env(
                "AHU_TEST_IDENTITY",
                serde_json::json!({
                    "loggedIn": true, "email": email, "orgId": "private-org-marker",
                    "authMethod": "oauth", "apiProvider": "first-party"
                })
                .to_string(),
            )
            .output()
            .unwrap()
    };
    assert!(
        run(
            &["auth", "bind", "--harness", "claude-code"],
            "first@example.invalid"
        )
        .status
        .success()
    );
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let binding = repo
        .path()
        .join(".ahu/state/repos")
        .join(discovered.identity())
        .join("auth-bindings.json");
    let before = std::fs::read(&binding).unwrap();
    for (email, expected) in [
        ("first@example.invalid", "account binding matches"),
        (
            "second@example.invalid",
            "blocked: account differs from project binding",
        ),
    ] {
        let out = run(&["doctor"], email);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains(expected), "{text}");
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("auth "))
                .count(),
            1,
            "{text}"
        );
        for secret in [email, "private-org-marker"] {
            assert!(!text.contains(secret), "{text}");
            assert!(!String::from_utf8_lossy(&out.stderr).contains(secret));
        }
        assert_eq!(std::fs::read(&binding).unwrap(), before);
    }
}
