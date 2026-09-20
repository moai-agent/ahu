//! Repository-scoped task pointers remain usable from sibling checkouts.
//! Explicit synthetic legacy roots are read in place; independent repositories
//! do not share an ambient index.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ahu::task_index::StoreKind;
use ahu::{git, state, task, task_index};

use common::TestRepo;

/// The task index is one shared surface: while one test holds this lock, no
/// other test touches it, in a child process or in this one.
static INDEX_LOCK: Mutex<()> = Mutex::new(());

/// A repository with a configuration and one registered agent.
fn fixture() -> TestRepo {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    repo
}

fn sibling(repo: &TestRepo) -> TestRepo {
    let dir = tempfile::tempdir().unwrap();
    common::git(
        repo.path(),
        &["worktree", "add", "--detach", dir.path().to_str().unwrap()],
    );
    TestRepo {
        dir,
        state: tempfile::tempdir().unwrap(),
    }
}

/// Deliberately stale pointers are fixtures, not registrations of live tasks.
fn pointer(repo: &TestRepo, id: &str, checkout: &Path) {
    let repo = git::discover(repo.path()).unwrap();
    state::write_json(
        &task_index::root_for(&repo)
            .unwrap()
            .join(format!("{id}.json")),
        &task_index::Entry {
            schema_version: 2,
            task_id: id.into(),
            repo_identity: repo.identity(),
            checkout: checkout.into(),
            store: StoreKind::Worktree,
        },
    )
    .unwrap();
}

/// The task index and the headless runtime one scenario runs against,
/// pointing every child process and every in-process call at the same
/// private state.
struct Scenario {
    _dir: tempfile::TempDir,
    index: PathBuf,
    runtime: PathBuf,
}

impl Scenario {
    fn path(&self) -> &Path {
        self._dir.path()
    }

    fn new() -> Scenario {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = private_dir(dir.path().join("index"));
        let runtime = private_dir(dir.path().join("runtime"));
        Scenario {
            _dir: dir,
            index,
            runtime,
        }
    }
}

/// A scenario directory that only its owner can read.
fn private_dir(path: PathBuf) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(&path).expect("scenario directory");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// Run the real binary from `dir`, against this scenario's task index.
fn ahu_at(scenario: &Scenario, dir: &Path, args: &[&str]) -> std::process::Output {
    common::ahu()
        .args(args)
        .current_dir(dir)
        .env_remove("AHU_STATE_DIR")
        .env("HOME", scenario.path())
        .env("AHU_TASK_INDEX_DIR", &scenario.index)
        .env("AHU_RUNTIME_DIR", &scenario.runtime)
        // Nothing here is about cmux; no reachable session may leak in.
        .env("AHU_CMUX_BIN", dir.join("no-such-cmux"))
        .output()
        .expect("ahu runs")
}

fn text_of(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The worktree ahu would create for `task_id` in this repository.
fn worktree_of(repo: &TestRepo, task_id: &str) -> PathBuf {
    repo.state.path().canonicalize().unwrap().join(task_id)
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

/// A finished task under `repo`, registered in the task index at its
/// worktree.
fn prepare_registered_task(repo: &TestRepo, task_id: &str) -> PathBuf {
    let discovered = git::discover(repo.path()).unwrap();
    let worktree = worktree_of(repo, task_id);
    git::add_worktree(&discovered, &worktree, &branch_of(task_id), "HEAD").unwrap();
    let identity = discovered.identity();
    let record_dir = state::worktree_task_dir(&worktree, &identity, task_id);
    task::save(
        &record_dir,
        &record_for(repo, task_id, &worktree),
        "current work",
    )
    .unwrap();
    task_index::register(
        &identity,
        task_id,
        &worktree_of(repo, task_id),
        StoreKind::Worktree,
    )
    .unwrap();
    record_dir
}

/// A registered task resolves from its own checkout, from a sibling,
/// and from a bare worktree of the repository, in every accepted form.
#[test]
fn a_registered_task_is_reachable_from_primary_and_sibling_checkouts() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let a = fixture();
    let c = sibling(&a);
    let id = "006aa900-1234-7e5f-8a9b-0c1d2e3f4a5b";
    prepare_registered_task(&a, id);

    let forms = [id.to_string(), format!("ahu:task:{id}"), id.to_uppercase()];
    for dir in [a.path(), c.path()] {
        for input in &forms {
            let out = ahu_at(&scenario, dir, &["task", input]);
            let text = text_of(&out);
            assert_eq!(out.status.code(), Some(0), "{input}: {text}");
            assert!(text.contains(id), "{input}: {text}");
        }
    }

    // A detached worktree holds no records of its own; only the index can
    // carry the id into it.
    let sibling = scenario.path().join("sibling");
    common::git(
        a.path(),
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            sibling.to_str().unwrap(),
        ],
    );
    let out = ahu_at(&scenario, &sibling, &["task", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains(id), "{text}");
}

/// A unique id prefix resolves through the task index as a full id does.
#[test]
fn unique_prefix_resolves_through_the_task_index() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let a = fixture();
    let c = sibling(&a);
    let id = "006aa911-2222-7e5f-8a9b-0c1d2e3f4a5c";
    prepare_registered_task(&a, id);

    let out = ahu_at(&scenario, c.path(), &["task", &id[..35]]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains(id), "{text}");
}

/// An ambiguous prefix names every match it stands for, records first.
#[test]
fn ambiguous_prefix_names_every_match() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let c = fixture();
    let base = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5";
    for suffix in ["b", "c"] {
        let checkout = scenario.path().join(format!("no-such-checkout-{suffix}"));
        pointer(&c, &format!("{base}{suffix}"), &checkout);
    }

    let out = ahu_at(&scenario, c.path(), &["task", base]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    assert!(text.contains("it matches 2 tasks"), "{text}");
    assert_eq!(text.matches("  task index entry at ").count(), 2, "{text}");
}

/// A prefix shared by a local record and an index entry stays ambiguous,
/// while each task alone resolves by a longer prefix.
#[test]
fn local_records_and_index_entries_compete_for_a_prefix() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let a = fixture();
    let c = sibling(&a);
    let id = "006aa900-1234-7e5f-8a9b-0c1d2e3f4a5b";
    prepare_registered_task(&a, id);
    let local = "006aa50000000000ab";
    prepare_task_in_state(&c, local, task::TaskState::Exited);

    let out = ahu_at(&scenario, c.path(), &["task", "006aa"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    assert!(text.contains("it matches 2 tasks"), "{text}");
    assert!(text.contains("  record at "), "{text}");
    assert!(text.contains("  task index entry at "), "{text}");

    let out = ahu_at(&scenario, c.path(), &["task", "006aa9"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains(id), "{text}");

    let out = ahu_at(&scenario, c.path(), &["task", "006aa5"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains(local), "{text}");
}

/// An entry whose checkout vanished is reported as stale, not as a match.
#[test]
fn stale_checkout_entry_reports_the_missing_checkout() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let a = fixture();
    let c = sibling(&a);
    let id = "006aa550-1234-7e5f-8a9b-0c1d2e3f4a5d";
    prepare_registered_task(&a, id);
    std::fs::remove_dir_all(worktree_of(&a, id)).unwrap();

    let out = ahu_at(&scenario, c.path(), &["task", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(
        text.contains(
            "that checkout no longer exists; the entry is stale and the task cannot be reached from here."
        ),
        "{text}"
    );
}

/// An entry pointing at a checkout whose task directory is missing is
/// reported as stale, not as a match.
#[test]
fn missing_task_directory_reports_the_stale_entry() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let a = fixture();
    let c = sibling(&a);
    let id = "006aa560-1234-7e5f-8a9b-0c1d2e3f4a5e";
    let checkout = private_dir(scenario.path().join("checkout"));
    pointer(&a, id, &checkout);

    let out = ahu_at(&scenario, c.path(), &["task", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(text.contains("its task directory"), "{text}");
    assert!(text.contains("the entry is stale"), "{text}");
}

/// An unreadable record still names the checkout the index recorded for it.
#[test]
fn unreadable_record_reports_the_entrys_checkout() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let a = fixture();
    let c = sibling(&a);
    let id = "006aa570-1234-7e5f-8a9b-0c1d2e3f4a5f";
    let record_dir = prepare_registered_task(&a, id);
    std::fs::write(record_dir.join("task.json"), "not json").unwrap();

    let out = ahu_at(&scenario, c.path(), &["task", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(text.contains("has an unreadable record at"), "{text}");
    assert!(
        text.contains("The task index records its checkout as"),
        "{text}"
    );
}

/// A record that describes a different task than the one resolved is
/// refused, with nothing changed.
#[test]
fn a_record_describing_another_task_is_refused() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let a = fixture();
    let c = sibling(&a);
    let stored = "006aa800-1234-7e5f-8a9b-0c1d2e3f4a5b";
    let claimed = "006aa811-1234-7e5f-8a9b-0c1d2e3f4a5c";
    let discovered = git::discover(a.path()).unwrap();
    let worktree = scenario.path().join("worktree-b");
    git::add_worktree(&discovered, &worktree, &branch_of(stored), "HEAD").unwrap();
    state::ensure_checkout_state(&worktree).unwrap();
    let identity = discovered.identity();
    let dir = state::worktree_task_dir(&worktree, &identity, stored);
    task::save(&dir, &record_for(&a, claimed, &worktree), "current work").unwrap();
    task_index::register(&identity, stored, &worktree, StoreKind::Worktree).unwrap();

    let out = ahu_at(&scenario, c.path(), &["task", stored]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(text.contains("describes a different task"), "{text}");
    assert!(text.contains("Nothing was done"), "{text}");
}

/// `ahu focus` resolves a registered task from a sibling checkout.
#[test]
fn focus_resolves_through_the_task_index() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let a = fixture();
    let c = sibling(&a);
    let id = "006aa590-1234-7e5f-8a9b-0c1d2e3f4a51";
    prepare_registered_task(&a, id);

    let out = ahu_at(&scenario, c.path(), &["focus", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(text.contains("has no recorded cmux session"), "{text}");
}

/// `ahu diff` resolves a registered task from a sibling checkout and
/// writes the bare patch to stdout.
#[test]
fn diff_resolves_through_the_task_index() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let a = fixture();
    let c = sibling(&a);
    let id = "006aa5a0-1234-7e5f-8a9b-0c1d2e3f4a52";
    prepare_registered_task(&a, id);

    let out = ahu_at(&scenario, c.path(), &["diff", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(out.stdout.is_empty(), "{text}");
}

/// `ahu remove` resolves a registered task from a sibling checkout and
/// clears the record, the worktree, the branch and the index entry.
#[test]
fn remove_resolves_through_the_task_index_and_cleans_up() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let a = fixture();
    let c = sibling(&a);
    let id = "006aa5b0-1234-7e5f-8a9b-0c1d2e3f4a53";
    let record_dir = prepare_registered_task(&a, id);
    let worktree = worktree_of(&a, id);
    let branch = branch_of(id);

    let out = ahu_at(&scenario, c.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains(&format!("removed task {id}")), "{text}");
    assert!(text.contains("  worktree  removed\n"), "{text}");
    assert!(text.contains("  branch    deleted\n"), "{text}");
    assert!(
        text.contains("  record    removed with its worktree\n"),
        "{text}"
    );
    assert!(!worktree.exists(), "the worktree outlived removal");
    assert!(!record_dir.exists(), "the record outlived removal");
    assert_eq!(
        common::git(a.path(), &["branch", "--list", &branch]),
        "",
        "the branch outlived removal"
    );
    assert!(
        task_index::lookup_in(&git::discover(a.path()).unwrap(), id)
            .unwrap()
            .is_none(),
        "the index entry outlived removal"
    );
}

/// A headless entry resolves from a sibling checkout through the
/// runtime directory it names.
#[test]
fn legacy_headless_entry_resolves_from_a_sibling_checkout() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let a = fixture();
    let c = sibling(&a);
    let id = "006aa700-1234-7e5f-8a9b-0c1d2e3f4a54";
    let identity = git::discover(a.path()).unwrap().identity();
    state::write_json(&a.path().join(".ahu/state/legacy-lookup.json"), &serde_json::json!({"schema_version":1,"runtime_roots":[scenario.runtime.canonicalize().unwrap()]})).unwrap();
    let dir = scenario.runtime.join(&identity).join(id);
    task::save(&dir, &record_for(&a, id, a.path()), "current work").unwrap();
    task_index::register(&identity, id, a.path(), StoreKind::Headless).unwrap();

    let out = ahu_at(&scenario, c.path(), &["task", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains(id), "{text}");
    assert!(text.contains("  liveness  unknown"), "{text}");
}

/// Registration itself refuses non-canonical ids and relative checkouts.
#[test]
fn register_rejects_unusable_ids_and_checkouts() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _scenario = Scenario::new();
    let a = fixture();
    let identity = git::discover(a.path()).unwrap().identity();

    let err = task_index::register(&identity, "not-a-uuid", a.path(), StoreKind::Worktree)
        .expect_err("non-canonical ids must be refused");
    assert!(
        err.to_string()
            .contains("task index accepts canonical task ids only"),
        "{err}"
    );

    let err = task_index::register(
        &identity,
        "006aa600-0000-7000-8000-000000000000",
        Path::new("relative"),
        StoreKind::Worktree,
    )
    .expect_err("relative checkout paths must be refused");
    assert!(
        err.to_string()
            .contains("task index accepts absolute checkout paths only"),
        "{err}"
    );
}

/// Forms the grammar does not recognize are usage errors, not resolutions.
#[test]
fn unrecognized_forms_are_usage_errors() {
    let _lock = INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scenario = Scenario::new();
    let a = fixture();
    let c = sibling(&a);
    let id = "006aa900-1234-7e5f-8a9b-0c1d2e3f4a5b";
    prepare_registered_task(&a, id);

    let out = ahu_at(&scenario, c.path(), &["task", &format!("urn:task:{id}")]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    assert!(text.contains("no task matching"), "{text}");

    let out = ahu_at(&scenario, c.path(), &["task", "ahu:task:"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    assert!(text.contains("a task id must not be empty."), "{text}");
}
