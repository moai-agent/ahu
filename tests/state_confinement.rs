//! ahu state stays inside the store it was given.
//!
//! `.ahu/` sits inside the checkout, so a repository can track a symlink at
//! `.ahu/state/repos` — or deeper, at the repository-identity or `tasks`
//! directory — and Git will check it out. Following one would let the
//! repository choose where ahu creates directories, writes records, and
//! changes permissions. Every component below the state root is refused
//! instead.

#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::TestRepo;
use tempfile::TempDir;

/// A directory outside any checkout, with one file whose bytes and mode the
/// tests assert never move.
struct External {
    dir: TempDir,
}

impl External {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp dir");
        std::fs::write(dir.path().join("keep.txt"), "untouched\n").expect("seed file");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
            .expect("seed mode");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn mode(&self) -> u32 {
        std::fs::symlink_metadata(self.dir.path())
            .expect("external directory")
            .permissions()
            .mode()
            & 0o777
    }

    /// Nothing created, nothing rewritten, nothing chmod-ed.
    fn assert_untouched(&self) {
        assert_eq!(self.mode(), 0o755, "external directory was chmod-ed");
        let entries: Vec<String> = std::fs::read_dir(self.dir.path())
            .expect("read external")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(
            entries,
            vec!["keep.txt".to_string()],
            "external gained files"
        );
        assert_eq!(
            std::fs::read_to_string(self.dir.path().join("keep.txt")).unwrap(),
            "untouched\n"
        );
    }
}

/// A repository with a valid configuration, an agent, and a state directory
/// whose `.ahu` and `.ahu/state` are ordinary directories — exactly the shape
/// the descendant-link escape needs.
fn repo_with_state() -> TestRepo {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.write(
        ".ahu/.gitignore",
        "# Local ahu session state. Never commit.\n*\n",
    );
    std::fs::create_dir_all(repo.path().join(".ahu/state")).expect("state directory");
    repo
}

fn ahu(repo: &TestRepo, args: &[&str]) -> std::process::Output {
    common::ahu()
        .args(args)
        .current_dir(repo.path())
        .env_remove("AHU_STATE_DIR")
        .env("XDG_STATE_HOME", repo.state_path())
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("ahu runs")
}

fn identity(repo: &TestRepo) -> String {
    ahu::git::discover(repo.path()).unwrap().identity()
}

fn link(target: &Path, at: PathBuf) {
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent).expect("link parent");
    }
    std::os::unix::fs::symlink(target, &at).expect("create link");
}

/// The same escape one and two levels deeper, where the caller joins the
/// repository identity and then `tasks`.
#[test]
fn deeper_identity_and_tasks_links_are_refused_for_every_state_command() {
    // Each link is paired with the commands whose state paths actually run
    // through it: hygiene writes `repos/<identity>/task.json`, and `ahu
    // tasks` reads `repos/<identity>/tasks/`.
    for (relative, commands) in [
        (".ahu/state/repos", &["tasks"][..]),
        (".ahu/state/repos/IDENTITY", &["tasks"][..]),
        (".ahu/state/repos/IDENTITY/tasks", &["tasks"][..]),
    ] {
        let repo = repo_with_state();
        let external = External::new();
        let at = relative.replace("IDENTITY", &identity(&repo));
        link(external.path(), repo.path().join(&at));
        repo.commit("fixture with a tracked state link");

        for command in commands {
            let args = vec![*command];
            let output = ahu(&repo, &args);
            let message = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                !output.status.success(),
                "{args:?} must refuse {at}, got: {message}"
            );
            assert!(
                message.contains("refusing ahu state path"),
                "{args:?} on {at}: {message}"
            );
        }
        external.assert_untouched();
    }
}

/// Private exclusive creation refuses a symlink and can replace a stale file.
#[test]
fn private_file_creation_refuses_a_link() {
    let repo = repo_with_state();
    let external = External::new();
    let state = repo.path().join(".ahu/state");
    let record = state.join("task.json");
    let temp = record.with_extension(format!("tmp{}", std::process::id()));
    link(&external.path().join("keep.txt"), temp.clone());

    let error = ahu::state::create_new_private_file(&temp)
        .expect_err("a link at a private file name must not be written through")
        .to_string();
    assert!(error.contains("refusing ahu state path"), "{error}");
    external.assert_untouched();
    assert!(!record.exists(), "no record should have been produced");

    // A plain file left by a process that died mid-write is not repository
    // interference, so it is cleared and the write completes.
    std::fs::remove_file(&temp).unwrap();
    std::fs::write(&temp, b"leftover").unwrap();
    ahu::state::create_new_private_file(&temp).unwrap();
    assert!(temp.is_file());
    external.assert_untouched();
}

/// Wait for a child with a deadline, killing it rather than blocking forever.
///
/// A regression in the leaf check would leave `open` waiting on a named pipe
/// that nothing ever writes to, which would hang the whole suite. The bound
/// turns that into a failure with evidence.
fn wait_bounded(mut child: std::process::Child, seconds: u64) -> std::process::Output {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    loop {
        match child.try_wait().expect("poll child") {
            Some(_) => return child.wait_with_output().expect("collect output"),
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("ahu blocked for more than {seconds}s on a state path");
            }
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
}

/// A named pipe where a state file should be is refused before it is opened.
#[test]
fn a_named_pipe_in_place_of_a_task_record_is_refused_without_blocking() {
    let repo = repo_with_state();
    let identity = identity(&repo);
    let record = repo
        .path()
        .join(".ahu/state/repos")
        .join(&identity)
        .join("tasks")
        .join("01a0e2ee-7475-7ed7-835b-4bc000000001")
        .join("task.json");
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(&record)
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "the fixture needs a named pipe");

    let child = common::ahu()
        .args(["tasks"])
        .current_dir(repo.path())
        .env_remove("AHU_STATE_DIR")
        .env("AHU_STATE_DIR", repo.path().join(".ahu/state"))
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("ahu starts");
    let output = wait_bounded(child, 20);
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{message}");
    assert!(message.contains("refusing ahu state path"), "{message}");
    assert!(message.contains("named pipe"), "{message}");
    // Still a pipe: nothing replaced it, and nothing was written through it.
    let kind = std::fs::symlink_metadata(&record).unwrap().file_type();
    assert!(std::os::unix::fs::FileTypeExt::is_fifo(&kind));
}

/// A reader that is handed a record path directly still checks the two
/// components that make up a checkout's store, rather than trusting that some
/// earlier call did.
#[test]
fn a_direct_record_read_validates_the_checkout_store_itself() {
    let repo = repo_with_state();
    let external = External::new();
    let identity = identity(&repo);
    let task_dir = repo
        .path()
        .join(".ahu/state/repos")
        .join(&identity)
        .join("tasks/006aa50000000000c1");

    // The store is replaced wholesale: `.ahu/state` becomes a link, which is
    // the shape `root()` would have refused — but this caller never went
    // through `root()`.
    std::fs::remove_dir_all(repo.path().join(".ahu/state")).unwrap();
    std::os::unix::fs::symlink(external.path(), repo.path().join(".ahu/state")).unwrap();

    let error = ahu::task::load(&task_dir).unwrap_err().to_string();
    assert!(error.contains("refusing ahu state path"), "{error}");
    external.assert_untouched();
}

/// A state file is owner-only from the moment it exists, not from a later
/// chmod: nothing else on the machine gets a window in which to open it.
#[test]
fn a_state_file_is_created_owner_only() {
    let repo = repo_with_state();
    let record = repo.path().join(".ahu/state/repos/created/task.json");
    ahu::state::write_json(&record, &serde_json::json!({"written": true})).unwrap();
    assert_eq!(
        std::fs::symlink_metadata(&record)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::symlink_metadata(record.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

/// A nested marker cannot reset the checkout confinement boundary.
#[test]
fn a_nested_state_marker_does_not_move_the_confinement_boundary() {
    let repo = repo_with_state();
    refuse_through_a_nested_marker(&repo.path().join(".ahu/state"));
}

/// A link directly below the store is refused, and so is the same link with an
/// ordinary `.ahu/state` pair sitting under it. Nothing reaches the target.
fn refuse_through_a_nested_marker(store: &Path) {
    let external = External::new();
    // The target is a complete, ordinary store of its own, so nothing below the
    // marker is refusable on its own merits.
    std::fs::create_dir_all(external.path().join(".ahu/state")).unwrap();
    link(external.path(), store.join("link"));

    for relative in ["link/value.json", "link/.ahu/state/value.json"] {
        let error =
            ahu::state::write_json(&store.join(relative), &serde_json::json!({"written": true}))
                .expect_err("a link below the store must not be written through")
                .to_string();
        assert!(
            error.contains("refusing ahu state path"),
            "{relative}: {error}"
        );
    }

    assert!(!external.path().join("value.json").exists());
    assert!(!external.path().join(".ahu/state/value.json").exists());
    assert_eq!(external.mode(), 0o755);
    assert_eq!(
        std::fs::read_to_string(external.path().join("keep.txt")).unwrap(),
        "untouched\n"
    );
}
