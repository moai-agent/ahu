//! `ahu message` delivers operator text to a task's inbox, and the listing
//! surfaces the question a task leaves behind.
//!
//! Delivery is the operator's exclusive right: a worker session inherits the
//! marker and is refused. The inbox is size-bounded and confined to the task
//! directory, and ahu fails closed around anything in it it does not
//! recognize. `result.md` and `question.md` display under the same bound.

mod common;

use std::path::{Path, PathBuf};

use ahu::{git, state, task};
use common::TestRepo;

/// A repository with a configuration and one registered agent.
fn fixture() -> TestRepo {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    repo
}

/// Run the real binary with no state override at all, from `dir`.
///
/// `env_remove` rather than a pointed-at temporary directory: these tests are
/// about ordinary operation, where nothing sets that variable.
fn ahu_in(dir: &Path, args: &[&str]) -> std::process::Output {
    common::ahu()
        .args(args)
        .current_dir(dir)
        .env_remove("AHU_STATE_DIR")
        // These commands reconcile against cmux when they can reach one. These
        // tests are about what is on disk, so they are pointed at a cmux that
        // is not there: neither a developer's real session nor another test's
        // stub can change what they see.
        .env("AHU_CMUX_BIN", dir.join("no-such-cmux"))
        .output()
        .expect("ahu runs")
}

/// Run the real binary as a worker session would, from `dir`.
///
/// `ahu message` refuses a session that carries the worker marker, so the
/// marker is set on top of the cleared environment.
fn worker_in(dir: &Path, args: &[&str]) -> std::process::Output {
    let mut command = common::ahu();
    command
        .args(args)
        .current_dir(dir)
        .env_remove("AHU_STATE_DIR")
        .env("AHU_CMUX_BIN", dir.join("no-such-cmux"))
        .env("AHU_WORKER_SESSION", "cmux");
    command.output().expect("ahu runs")
}

fn text_of(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The branch ahu would create for `task_id`.
fn branch_of(task_id: &str) -> String {
    format!("ahu/chris/{task_id}")
}

fn record_for(repo: &TestRepo, task_id: &str, worktree: &Path) -> task::TaskRecord {
    let discovered = git::discover(repo.path()).unwrap();
    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let (delivered, delivery) =
        ahu::orchestration::deliver(Some("You are chris."), "current work").unwrap();
    let command = adapter
        .launch_command(&ahu::harness::LaunchRequest {
            model: "claude-opus-5",
            prompt: &delivered,
            cwd: worktree,
            permissions: Default::default(),
        })
        .unwrap();
    task::TaskRecord {
        schema_version: task::TASK_SCHEMA_VERSION,
        task_id: task_id.to_string(),
        title: format!("task {task_id}"),
        summary: String::new(),
        created_at: format!("2026-09-12T00:00:{:02}Z", task_id.len()),
        repo_identity: discovered.identity(),
        repo_root: discovered.root.clone(),
        branch: format!("ahu/chris/{task_id}"),
        worktree: worktree.to_path_buf(),
        base_commit: discovered.head.clone(),
        identity: task::LaunchIdentity {
            mode: task::LaunchMode::Named,
            agent: "chris".to_string(),
            agent_version: Some("1.0.0".to_string()),
            permissions: Default::default(),
            harness: "claude-code".to_string(),
            model: "claude-opus-5".to_string(),
            instructions_source: Some(".claude/agents/chris.md".to_string()),
            source_digest: Some("2".repeat(64)),
            instructions_digest: Some("3".repeat(64)),
            identity_digest: Some("4".repeat(64)),
            selection_basis: None,
        },
        policy_digest: "0".repeat(64),
        catalog_version: ahu::catalog::CATALOG_VERSION.to_string(),
        config_snapshot: Default::default(),
        config_snapshot_digest: "0".repeat(64),
        hooks: Default::default(),
        hooks_digest: String::new(),
        materialize: Default::default(),
        launch_command: command.redacted(),
        delivery,
        prompt_digest: ahu::util::digest_bytes(b"current work"),
        harness_executable: PathBuf::from("/usr/local/bin/claude"),
        reliability_warning: None,
        enforcement: adapter
            .enforcement("claude-opus-5", Default::default())
            .unwrap(),
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: task::TaskState::Exited,
    }
}

/// Create a task worktree and the record that belongs to it, the way `execute`
/// does: worktree first, then its own state directory, then the record. The
/// record's state is the caller's to choose.
fn prepare_task_in_state(repo: &TestRepo, task_id: &str, task_state: task::TaskState) -> PathBuf {
    let discovered = git::discover(repo.path()).unwrap();
    state::ensure_worktrees_root(&discovered.root).unwrap();
    let worktree = state::worktree_dir(&discovered.root, task_id).unwrap();
    let branch = branch_of(task_id);
    git::add_worktree(&discovered, &worktree, &branch, "HEAD").unwrap();
    state::ensure_checkout_state(&worktree).unwrap();

    let identity = discovered.identity();
    let dir = state::worktree_task_dir(&worktree, &identity, task_id);
    let mut record = record_for(repo, task_id, &worktree);
    record.state = task_state;
    task::save(&dir, &record, "current work").unwrap();
    dir
}

/// A finished task, ready to receive a message.
fn prepare_task(repo: &TestRepo, task_id: &str) -> PathBuf {
    prepare_task_in_state(repo, task_id, task::TaskState::Exited)
}

/// Messages are numbered in arrival order and kept verbatim, with the
/// surrounding whitespace trimmed and no newline of ahu's own added.
#[test]
fn delivered_messages_are_numbered_and_kept_verbatim() {
    let repo = fixture();
    let id = "006aa5c000000000c1";
    let dir = prepare_task(&repo, id);
    let inbox = dir.join("inbox");

    let out = ahu_in(repo.path(), &["message", id, "  first words  "]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains(&format!("delivered inbox message 0001 to task {id}.")),
        "{text}"
    );
    assert_eq!(
        std::fs::read_to_string(inbox.join("0001.md")).unwrap(),
        "first words"
    );

    let out = ahu_in(repo.path(), &["message", id, "second", "message"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains(&format!("delivered inbox message 0002 to task {id}.")),
        "{text}"
    );
    assert_eq!(
        std::fs::read_to_string(inbox.join("0002.md")).unwrap(),
        "second message"
    );
}

/// One hundred entries fill the inbox; the hundred-and-first is refused and
/// nothing is written.
#[test]
fn the_inbox_refuses_the_hundred_and_first_entry() {
    let repo = fixture();
    let id = "006aa5c000000000c2";
    let dir = prepare_task(&repo, id);
    let inbox = dir.join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    for entry in 1..=99 {
        std::fs::write(inbox.join(format!("{entry:04}.md")), "x").unwrap();
    }

    let out = ahu_in(repo.path(), &["message", id, "number one hundred"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains(&format!("delivered inbox message 0100 to task {id}.")),
        "{text}"
    );

    let out = ahu_in(repo.path(), &["message", id, "one too many"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(
        text.contains("is full (100 entries); no message was written."),
        "{text}"
    );
    assert!(!inbox.join("0101.md").exists());
}

/// An inbox already holding a full mebibyte refuses further text and nothing
/// is written.
#[test]
fn the_inbox_refuses_messages_beyond_its_byte_budget() {
    let repo = fixture();
    let id = "006aa5c000000000c3";
    let dir = prepare_task(&repo, id);
    let inbox = dir.join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::write(inbox.join("0001.md"), "a".repeat(1024 * 1024)).unwrap();

    let out = ahu_in(repo.path(), &["message", id, "over budget"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(
        text.contains("holds more than 1048576 bytes already; no message was written."),
        "{text}"
    );
    assert!(!inbox.join("0002.md").exists());
}

/// A worker session cannot deliver messages, and the refusal happens before
/// the task's inbox is so much as created.
#[test]
fn a_worker_session_cannot_deliver_messages() {
    let repo = fixture();
    let id = "006aa5c000000000c4";
    let dir = prepare_task(&repo, id);

    let out = worker_in(repo.path(), &["message", id, "from inside"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(
        text.contains("ahu message is an operator command."),
        "{text}"
    );
    assert!(
        text.contains("delivery belongs to the operator or to a broker-bound child."),
        "{text}"
    );
    assert!(!dir.join("inbox").exists());
}

/// A message for an unknown task is refused, and no inbox is created for it.
#[test]
fn a_message_for_an_unknown_task_is_refused() {
    let repo = fixture();
    let id = "006aa5c000000000c5";
    let dir = prepare_task(&repo, id);

    let out = ahu_in(repo.path(), &["message", "006aa5c00000000q5", "anyone"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    assert!(text.contains("no task matching"), "{text}");
    assert!(!dir.join("inbox").exists());
}

/// A message with no words is refused however it arrives: a bare id, blank
/// text, and a missing id altogether.
#[test]
fn a_message_without_words_is_refused() {
    let repo = fixture();
    let id = "006aa5c000000000c6";
    let dir = prepare_task(&repo, id);
    let inbox = dir.join("inbox");

    let out = ahu_in(repo.path(), &["message", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    assert!(
        text.contains("`ahu message` needs a task id and a message text."),
        "{text}"
    );

    let out = ahu_in(repo.path(), &["message", id, "   "]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    assert!(
        text.contains("`ahu message` needs a task id and a message text."),
        "{text}"
    );

    let out = ahu_in(repo.path(), &["message"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    assert!(
        text.contains("`ahu message` needs a task id and a message text."),
        "{text}"
    );

    assert!(std::fs::read_dir(&inbox).is_err());
}

/// `ahu task` prints result.md under the display bound and refuses to print
/// it beyond it.
#[test]
fn task_shows_result_under_the_display_bound_and_refuses_beyond_it() {
    let repo = fixture();
    let id = "006aa5c000000000c7";
    let dir = prepare_task(&repo, id);

    std::fs::write(dir.join("result.md"), "all done\n").unwrap();
    let out = ahu_in(repo.path(), &["task", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("\nresult:\nall done\n"), "{text}");

    std::fs::write(dir.join("result.md"), "a".repeat(1024 * 1024 + 1)).unwrap();
    let out = ahu_in(repo.path(), &["task", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("result.md is larger than the 1 MiB display bound; ahu will not print it."),
        "{text}"
    );
    assert!(!text.contains("\nresult:\n"), "{text}");
}

/// A question surfaces in the listing and in `ahu task` as it was written.
#[test]
fn a_question_surfaces_in_the_listing_and_in_task() {
    let repo = fixture();
    let id = "006aa5c000000000c8";
    let dir = prepare_task(&repo, id);
    std::fs::write(dir.join("question.md"), "should I proceed with plan B?\n").unwrap();

    let out = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("  question  should I proceed with plan B?\n"),
        "{text}"
    );

    let out = ahu_in(repo.path(), &["task", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("\nquestion:\nshould I proceed with plan B?\n"),
        "{text}"
    );
}

/// A long question is cut to sixty characters in the listing.
#[test]
fn a_long_question_is_cut_to_sixty_characters_in_the_listing() {
    let repo = fixture();
    let id = "006aa5c000000000c9";
    let dir = prepare_task(&repo, id);
    std::fs::write(dir.join("question.md"), "w".repeat(80)).unwrap();

    let out = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains(&format!("  question  {}…\n", "w".repeat(60))),
        "{text}"
    );
}

/// An oversized question is named in both views, never printed.
#[test]
fn an_oversized_question_is_named_not_printed() {
    let repo = fixture();
    let id = "006aa5c000000000d1";
    let dir = prepare_task(&repo, id);
    std::fs::write(dir.join("question.md"), "w".repeat(1024 * 1024 + 1)).unwrap();

    let out = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("  question  present, larger than the display bound\n"),
        "{text}"
    );

    let out = ahu_in(repo.path(), &["task", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("question.md is larger than the 1 MiB display bound; ahu will not print it."),
        "{text}"
    );
}

/// An inbox entry ahu does not recognize stops delivery instead of writing
/// next to it.
#[test]
fn an_unrecognized_inbox_entry_stops_delivery() {
    let repo = fixture();
    let id = "006aa5c000000000d2";
    let dir = prepare_task(&repo, id);
    let inbox = dir.join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::write(inbox.join("readme.txt"), "not mine").unwrap();

    let out = ahu_in(repo.path(), &["message", id, "operator here"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(text.contains("holds an unrecognized entry"), "{text}");
    assert!(text.contains("\"readme.txt\""), "{text}");
    assert!(text.contains("ahu will not write next to it."), "{text}");
    assert!(!inbox.join("0001.md").exists());
}
