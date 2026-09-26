//! What `ahu tasks` says about records it cannot read.
//!
//! `task::list` used to drop unreadable records, despite claiming to report them.
//! Unreadable records were initially the exception.
//!
//! The schema-2 bump made it the rule. Every record written by an earlier ahu is
//! refused, so a repository with five tasks, five worktrees, five branches and
//! three live cmux sessions had `ahu tasks` print "No ahu tasks have been
//! launched from this repository." A positive claim of absence, false, on the
//! one surface that could have told the user where their leftover worktrees
//! were.

mod common;

use std::path::{Path, PathBuf};

use common::TestRepo;

/// Write a task record directory by hand, at whichever schema version is asked
/// for, without going through `task::save`.
///
/// Schema 1 is written as raw JSON on purpose: this build cannot construct a
/// `TaskRecord` at that version, and a fixture that could would not be testing
/// the thing that actually happens on a real machine.
fn write_schema_1(tasks_dir: &Path, task_id: &str) -> PathBuf {
    let dir = tasks_dir.join(task_id);
    std::fs::create_dir_all(&dir).unwrap();
    let record = serde_json::json!({
        "schema_version": 1,
        "task_id": task_id,
        "title": "an earlier task",
        "created_at": "2026-09-01T00:00:00Z",
        "repo_identity": "r",
        "repo_root": "/nonexistent",
        "branch": format!("ahu/chris/{task_id}"),
        "worktree": "/nonexistent",
        "base_commit": null,
        "identity": {
            "mode": "named",
            "agent": "chris",
            "agent_version": "1.0.0",
            "permissions": "prompt",
            "harness": "claude-code",
            "model": "claude-opus-5",
            "instructions_source": ".claude/agents/chris.md",
            // Schema 1's meaning: the whole file's digest, under this name.
            "instructions_digest": "1".repeat(64),
            "identity_digest": null,
            "selection_basis": null,
        },
        "policy_digest": "0".repeat(64),
        "catalog_version": ahu::catalog::CATALOG_VERSION,
        "config_snapshot": {"entries": [], "skipped_directories": []},
        "config_snapshot_digest": "0".repeat(64),
        "materialize": {"written": [], "removed": [], "concurrently_modified": []},
        "launch_command": {"program": "claude", "args": []},
        "prompt_digest": "0".repeat(64),
        "enforcement": {
            "harness": "claude-code",
            "harness_version": null,
            "model_fixed_for_session": false,
            "gaps": [],
            "applied_controls": [],
        },
        "reliability_warning": null,
        "cmux_group_id": null,
        "cmux_workspace_id": null,
        "cmux_window_id": null,
        "state": "running",
    });
    std::fs::write(
        dir.join("task.json"),
        serde_json::to_string_pretty(&record).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("prompt.txt"), "earlier work").unwrap();
    dir
}

/// A valid current-schema record, written the way a launch writes one.
fn write_current(tasks_dir: &Path, repo: &TestRepo, task_id: &str) -> PathBuf {
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let (delivered, delivery) =
        ahu::orchestration::deliver(Some("You are chris."), "current work").unwrap();
    let command = adapter
        .launch_command(&ahu::harness::LaunchRequest {
            model: "claude-opus-5",
            prompt: &delivered,
            cwd: &discovered.root,
            permissions: Default::default(),
        })
        .unwrap();
    let record = ahu::task::TaskRecord {
        schema_version: ahu::task::TASK_SCHEMA_VERSION,
        task_id: task_id.to_string(),
        title: "a current task".to_string(),
        summary: String::new(),
        created_at: "2026-09-12T00:00:00Z".to_string(),
        repo_identity: discovered.identity(),
        repo_root: discovered.root.clone(),
        branch: format!("ahu/chris/{task_id}"),
        worktree: discovered.root.join(format!(".worktrees/{task_id}")),
        base_commit: discovered.head.clone(),
        identity: ahu::task::LaunchIdentity {
            mode: ahu::task::LaunchMode::Named,
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
        state: ahu::task::TaskState::Exited,
    };
    let dir = tasks_dir.join(task_id);
    ahu::task::save(&dir, &record, "current work").unwrap();
    dir
}

/// Run a command against that state, capturing what it printed.
fn scripted(
    _repo: &TestRepo,
    f: impl FnOnce(&mut ahu::launcher::Console<'_>) -> ahu::util::Result<i32>,
) -> (i32, String) {
    let mut input = std::io::Cursor::new(Vec::new());
    let mut output: Vec<u8> = Vec::new();
    let code = {
        let mut console = ahu::launcher::Console {
            input: &mut input,
            output: &mut output,
            interactive: false,
        };
        f(&mut console)
    };
    let text = String::from_utf8_lossy(&output).to_string();
    match code {
        Ok(code) => (code, text),
        Err(e) => (1, format!("{text}\nERROR: {e}")),
    }
}

/// The tasks directory for a repository, created.
fn tasks_dir(repo: &TestRepo) -> PathBuf {
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let dir = ahu::storage::CheckoutStorage::new(repo.path())
        .tasks_dir(&discovered.identity())
        .unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One readable record and one refused one: both are accounted for.
#[test]
fn tasks_lists_the_readable_record_and_reports_the_unreadable_one() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    write_schema_1(&dir, "006aa50000000000a1");
    write_current(&dir, &repo, "006aa50000000000b2");

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let (code, text) = scripted(&repo, |console| ahu::commands::tasks(console, &discovered));
    assert_eq!(code, 0, "{text}");

    // The readable one is listed as usual. Its title is a column the layout
    // gives up first, so it is asserted at a width that has room for it.
    assert!(text.contains("006aa50000000000b2"), "{text}");
    let (_, wide) = scripted(&repo, |console| {
        ahu::commands::tasks_at(console, &discovered, 240)
    });
    assert!(wide.contains("a current task"), "{wide}");

    // And the refused one is reported, with its id and the reason.
    assert!(text.contains("006aa50000000000a1"), "{text}");
    assert!(text.contains("[unreadable]"), "{text}");
    assert!(
        text.contains("1 task(s) in this repository could not be read or are incomplete"),
        "{text}"
    );
    assert!(
        text.contains("schema version (1)") && text.contains("reads schema 3"),
        "the reason must say why, not just that: {text}"
    );
    assert!(
        text.contains("they are not gone"),
        "the user's problem is the leftover worktree: {text}"
    );

    // Nothing was rewritten. The refused file is audit trail.
    let raw = std::fs::read_to_string(dir.join("006aa50000000000a1/task.json")).unwrap();
    assert!(raw.contains("\"schema_version\": 1"), "{raw}");
    assert!(
        raw.contains(&"1".repeat(64)),
        "schema 1's instructions_digest must be left exactly as it was"
    );

    // And `task_dirs` accounts for both, so a helper cannot reintroduce the bug.
    let dirs = ahu::commands::task_dirs(&discovered).unwrap();
    assert_eq!(dirs.len(), 2, "{dirs:?}");
}

#[test]
fn agent_listing_uses_a_table_and_marks_configuration_drift() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    write_current(&dir, &repo, &ahu::task::new_task_id().unwrap());

    let (_, text) = scripted(&repo, |console| {
        let discovered = ahu::git::discover(repo.path()).unwrap();
        ahu::commands::agents(console, &discovered)
    });
    assert!(text.contains("AGENT"), "{text}");
    assert!(text.contains("@chris 1.0.0 [drifted]"), "{text}");
    assert!(text.contains(".claude/agents/chris.md"), "{text}");
}

#[test]
fn launch_displays_a_prominent_drift_warning() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    write_current(&dir, &repo, &ahu::task::new_task_id().unwrap());
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harness(scratch.path(), &scratch.path().join("argv"));

    let output = common::ahu()
        .args(["launch", "@chris", "--prompt", "review", "--dry-run"])
        .current_dir(repo.path())
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_CMUX_BIN", scratch.path().join("missing-cmux"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("!! CONFIGURATION DRIFT"), "{text}");
    assert!(
        text.contains("Drift since the last chris@1.0.0 launch"),
        "{text}"
    );
}

/// A listing with one long-titled task, its handle reserved.
fn listed(repo: &TestRepo, name: &str, title: &str) -> String {
    let dir = tasks_dir(repo);
    let id = ahu::task::new_task_id().unwrap();
    let path = write_current(&dir, repo, &id);
    let record = path.join("task.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    value["title"] = title.into();
    std::fs::write(&record, serde_json::to_vec(&value).unwrap()).unwrap();
    let discovered = ahu::git::discover(repo.path()).unwrap();
    ahu::task_handles::reserve(&discovered, &id, Some(name), title).unwrap();
    format!("ahu/chris/{id}")
}

/// The branch as a listing row shows it: the prefix and the first eight
/// characters of the task id.
fn short_branch(branch: &str) -> String {
    let (prefix, id) = branch.rsplit_once('/').unwrap();
    format!("{prefix}/{}", &id[..8])
}

/// A listing of short-titled tasks, the shape a working repository has: every
/// handle was generated from its own title, so the titles repeat the handles.
fn listed_short(repo: &TestRepo) -> Vec<String> {
    let dir = tasks_dir(repo);
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let mut branches = Vec::new();
    for (title, state) in [
        ("Fix flaky test", "running"),
        ("Trim the cache", "exited"),
        ("Ship the notes", "failed"),
    ] {
        let id = ahu::task::new_task_id().unwrap();
        let path = write_current(&dir, repo, &id);
        let record = path.join("task.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
        value["title"] = title.into();
        value["state"] = state.into();
        std::fs::write(&record, serde_json::to_vec(&value).unwrap()).unwrap();
        ahu::task_handles::reserve(&discovered, &id, None, title).unwrap();
        branches.push(format!("ahu/chris/{id}"));
    }
    branches
}

/// Terminal columns a rendered line occupies, budgeted the way the layout
/// budgets them: a non-ASCII glyph may be double width, bar the ellipsis.
fn line_width(line: &str) -> usize {
    line.chars()
        .map(|c| match c {
            '…' => 1,
            c if c.is_ascii() => 1,
            _ => 2,
        })
        .sum()
}

#[test]
fn task_listing_fits_the_terminal_and_keeps_handles_whole() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let branch = listed(
        &repo,
        "storage-cleanup",
        "Remove the redundant inspection command and fit the task table to the terminal",
    );
    let discovered = ahu::git::discover(repo.path()).unwrap();

    for width in [80usize, 88, 120] {
        let (code, text) = scripted(&repo, |console| {
            ahu::commands::tasks_at(console, &discovered, width)
        });
        assert_eq!(code, 0, "{text}");
        for line in text.lines() {
            assert!(
                line_width(line) <= width,
                "{width}: {line:?} is {} wide\n{text}",
                line_width(line)
            );
        }
        // The handle is what the reader pastes into the next command, so it
        // survives every width; so does what the row cannot say twice.
        assert!(text.contains("@storage-cleanup"), "{width}: {text}");
        for whole in ["exited", "chris@1.0.0", "claude-code / claude-opus-5"] {
            assert!(text.contains(whole), "{width}: {whole} is cut:\n{text}");
        }
        // Wherever the branch appears it is the whole compact one -- the prefix
        // and enough of the id to recognise, never the whole UUID and never cut
        // shorter than that, because a cut branch name matches nothing. This
        // handle is wide enough that 80 columns cannot hold the column at all.
        assert!(!text.contains(&branch), "{width}: {text}");
        if text.contains("BRANCH") {
            assert!(text.contains(&short_branch(&branch)), "{width}: {text}");
        } else {
            assert!(!text.contains("ahu/chris/"), "{width}: {text}");
        }
        assert!(text.contains("Run `ahu task <handle>`"), "{text}");
    }

    // A title the handle does not carry earns a column once there is room for
    // it, and gives up width before anything else does.
    let (_, narrow) = scripted(&repo, |console| {
        ahu::commands::tasks_at(console, &discovered, 88)
    });
    assert!(!narrow.contains("TITLE"), "{narrow}");
    // The title leaves before the branch does, and the branch it leaves room
    // for is the whole compact one.
    assert!(narrow.contains(&short_branch(&branch)), "{narrow}");
    let (_, roomy) = scripted(&repo, |console| {
        ahu::commands::tasks_at(console, &discovered, 120)
    });
    assert!(roomy.contains("TITLE"), "{roomy}");
    assert!(roomy.contains("Remove the redun"), "{roomy}");
    assert!(
        !roomy.contains("fit the task table to the terminal"),
        "{roomy}"
    );
    assert!(roomy.contains(&short_branch(&branch)), "{roomy}");

    // Wide enough for everything: nothing is dropped and nothing is cut, and
    // the branch is still the compact one -- `ahu task` prints it whole.
    let (_, wide) = scripted(&repo, |console| {
        ahu::commands::tasks_at(console, &discovered, 240)
    });
    assert!(!wide.contains('…'), "{wide}");
    for header in [
        "HANDLE", "TITLE", "STATE", "AGENT", "MODE", "LIVE", "RUNTIME", "BRANCH",
    ] {
        assert!(wide.contains(header), "{header}: {wide}");
    }
    assert!(wide.contains(&short_branch(&branch)), "{wide}");
    assert!(!wide.contains(&branch), "{wide}");
    assert!(
        wide.contains(
            "Remove the redundant inspection command and fit the task table to the terminal"
        ),
        "{wide}"
    );
    assert!(wide.contains("claude-code / claude-opus-5"), "{wide}");
}

/// Handles are generated from titles, so a short title comes through its handle
/// whole. Spending the widest column in the table on the first column's words
/// is what pushed the runtime out of an ordinary terminal.
#[test]
fn a_title_its_handle_already_carries_does_not_take_a_column() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let branches = listed_short(&repo);
    let discovered = ahu::git::discover(repo.path()).unwrap();

    for width in [80usize, 88, 120, 240] {
        let (code, text) = scripted(&repo, |console| {
            ahu::commands::tasks_at(console, &discovered, width)
        });
        assert_eq!(code, 0, "{text}");
        for line in text.lines() {
            assert!(line_width(line) <= width, "{width}: {line:?}\n{text}");
        }
        assert!(!text.contains("TITLE"), "{width}: {text}");
        assert!(!text.contains("Fix flaky test"), "{width}: {text}");
        // What the handles do not say is what the row is for.
        for whole in [
            "@fix-flaky-test",
            "running",
            "chris@1.0.0",
            "claude-code / claude-opus-5",
        ] {
            assert!(text.contains(whole), "{width}: {whole} is missing:\n{text}");
        }
    }

    // From the width where the branch fits at all, it is the compact one.
    let (_, text) = scripted(&repo, |console| {
        ahu::commands::tasks_at(console, &discovered, 120)
    });
    for branch in &branches {
        assert!(text.contains(&short_branch(branch)), "{branch}: {text}");
        assert!(!text.contains(branch), "{branch}: {text}");
    }
}

/// One task whose title says more than its handle and one whose title does not:
/// the column is worth keeping, and the row that has nothing to add is blank.
#[test]
fn only_the_rows_whose_titles_add_something_fill_the_title_column() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    listed_short(&repo);
    listed(
        &repo,
        "storage-cleanup",
        "Remove the redundant inspection command and fit the task table to the terminal",
    );
    let discovered = ahu::git::discover(repo.path()).unwrap();

    let (_, text) = scripted(&repo, |console| {
        ahu::commands::tasks_at(console, &discovered, 240)
    });
    assert!(text.contains("TITLE"), "{text}");
    assert!(
        text.contains(
            "Remove the redundant inspection command and fit the task table to the terminal"
        ),
        "{text}"
    );
    assert!(!text.contains("Fix flaky test"), "{text}");
    let row = text
        .lines()
        .find(|line| line.contains("@fix-flaky-test"))
        .unwrap();
    // The blank title cell leaves the handle and the state adjacent, separated
    // by the column's own width and nothing else.
    assert!(row.contains("@fix-flaky-test  "), "{row:?}");
    assert!(row.contains("running"), "{row:?}");
}

/// The table is a human surface. Nothing about how it is laid out or painted
/// may reach the machine-readable one, at any width.
#[test]
fn task_listing_json_is_unaffected_by_width_and_color() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    listed(
        &repo,
        "storage-cleanup",
        "Remove the redundant inspection command and fit the task table to the terminal",
    );

    let json = |columns: &str, color: &str| {
        let output = common::ahu()
            .arg(color)
            .args(["tasks", "--output", "json"])
            .current_dir(repo.path())
            .env("COLUMNS", columns)
            .env_remove("NO_COLOR")
            .env("AHU_CMUX_BIN", repo.state_path().join("absent-cmux"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };

    let baseline = json("80", "--color=never");
    for (columns, color) in [("40", "--color=never"), ("400", "--color=always")] {
        assert_eq!(json(columns, color), baseline, "{columns} {color}");
    }
    assert!(!baseline.contains(&0x1b));
    let value: serde_json::Value = serde_json::from_slice(&baseline).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["tasks"][0]["task_handle"], "@storage-cleanup");
    assert_eq!(
        value["tasks"][0]["title"],
        serde_json::Value::Null,
        "titles stay out of JSON: {value}"
    );
}

#[test]
fn task_listing_paints_its_roles_only_when_color_is_in_effect() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let branch = listed(&repo, "storage-cleanup", "a current task");

    let run = |color: &str, no_color: Option<&str>| {
        let mut command = common::ahu();
        command
            .arg(color)
            .arg("tasks")
            .current_dir(repo.path())
            .env("COLUMNS", "240")
            .env("AHU_CMUX_BIN", repo.state_path().join("absent-cmux"));
        match no_color {
            Some(value) => command.env("NO_COLOR", value),
            None => command.env_remove("NO_COLOR"),
        };
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    let painted = run("--color=always", None);
    // The handle and the agent are the same role; the state is coloured by
    // what it means, the runtime by being a runtime, and the branch is dim
    // because it is where to go, not what to read.
    assert!(
        painted.contains("\x1b[1;36m@storage-cleanup\x1b[0m"),
        "{painted:?}"
    );
    assert!(
        painted.contains("\x1b[1;36mchris@1.0.0\x1b[0m"),
        "{painted:?}"
    );
    assert!(painted.contains("\x1b[2mexited\x1b[0m"), "{painted:?}");
    assert!(
        painted.contains("\x1b[36mclaude-code / claude-opus-5\x1b[0m"),
        "{painted:?}"
    );
    assert!(
        painted.contains(&format!("\x1b[2m{}\x1b[0m", short_branch(&branch))),
        "{painted:?}"
    );
    // Liveness ahu could not read is dimmed rather than asserted.
    assert!(painted.contains("\x1b[2munknown\x1b[0m"), "{painted:?}");
    assert!(painted.contains("\x1b[1mHANDLE\x1b[0m"), "{painted:?}");
    // Padding is never inside a styled span, so a colour never bleeds across
    // a column boundary.
    assert!(!painted.contains("\x1b[0m \x1b[0m"), "{painted:?}");

    for text in [
        run("--color=auto", Some("1")),
        run("--color=never", None),
        // Redirected stdout is not a terminal, so auto stays plain.
        run("--color=auto", None),
    ] {
        assert!(!text.contains('\x1b'), "{text:?}");
        assert!(text.contains("@storage-cleanup"), "{text}");
    }
}

/// Every record unreadable: the absence claim must not be printed.
///
/// This is the assertion the whole finding reduces to.
#[test]
fn tasks_never_claims_no_tasks_exist_when_records_were_refused() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    for id in [
        "006aa50000000000c1",
        "006aa50000000000c2",
        "006aa50000000000c3",
        "006aa50000000000c4",
        "006aa50000000000c5",
    ] {
        write_schema_1(&dir, id);
    }

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let (code, text) = scripted(&repo, |console| ahu::commands::tasks(console, &discovered));
    assert_eq!(code, 0, "{text}");

    assert!(
        !text.contains(ahu::commands::NO_TASKS),
        "ahu claimed no tasks exist while holding five refused records:\n{text}"
    );
    assert!(
        text.contains("5 task(s) in this repository could not be read or are incomplete"),
        "{text}"
    );
    for id in [
        "006aa50000000000c1",
        "006aa50000000000c2",
        "006aa50000000000c3",
        "006aa50000000000c4",
        "006aa50000000000c5",
    ] {
        assert!(text.contains(id), "{id} is missing from:\n{text}");
    }
}

/// A repository that genuinely has no tasks still says so.
///
/// The fix must not turn an honest statement into a hedge.
#[test]
fn tasks_still_says_so_when_there_really_are_none() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    tasks_dir(&repo);

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let (code, text) = scripted(&repo, |console| ahu::commands::tasks(console, &discovered));
    assert_eq!(code, 0, "{text}");
    assert!(text.contains(ahu::commands::NO_TASKS), "{text}");
}

/// The leftover worktree and branch are what the user actually needs, and the
/// task id in the directory name is enough to recover both.
#[test]
fn an_unreadable_record_still_names_its_worktree_and_branch() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    let task_id = "006aa50000000000d1";
    write_schema_1(&dir, task_id);

    // Create the worktree and branch the task would have left behind.
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let worktree = ahu::state::worktree_dir(&discovered.root, task_id).unwrap();
    ahu::git::add_worktree(
        &discovered,
        &worktree,
        &format!("ahu/chris/{task_id}"),
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    let (_, text) = scripted(&repo, |console| ahu::commands::tasks(console, &discovered));

    assert!(
        text.contains(&format!(".worktrees/{task_id}")),
        "the worktree must be named: {text}"
    );
    assert!(
        text.contains(&format!("ahu/chris/{task_id}")),
        "the branch must be recovered from git: {text}"
    );
    assert!(
        !text.contains("no longer on disk"),
        "the worktree is right there: {text}"
    );
}

/// When the worktree is gone the output says that rather than implying it is
/// still there, and points at the authoritative listing.
#[test]
fn an_unreadable_record_whose_worktree_is_gone_says_so() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    write_schema_1(&dir, "006aa50000000000e1");

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let (_, text) = scripted(&repo, |console| ahu::commands::tasks(console, &discovered));

    assert!(text.contains("no longer on disk"), "{text}");
    assert!(
        text.contains("branch    none found for this task id"),
        "{text}"
    );
    assert!(
        text.contains("git worktree list") && text.contains(".worktrees/"),
        "the user must be pointed at what ahu could not recover: {text}"
    );
}

/// `ahu focus` must not report a task as nonexistent when its record is merely
/// unreadable.
#[test]
fn focus_distinguishes_an_unreadable_task_from_a_missing_one() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    write_schema_1(&dir, "006aa50000000000f1");

    let discovered = ahu::git::discover(repo.path()).unwrap();

    // The task exists; ahu just cannot read its record.
    let (_, text) = scripted(&repo, |console| {
        ahu::commands::focus(console, &discovered, "006aa50000000000f1")
    });
    assert!(
        text.contains("exists but ahu cannot read its record"),
        "{text}"
    );
    assert!(text.contains("schema version (1)"), "{text}");
    assert!(text.contains("Run `ahu tasks`"), "{text}");
    assert!(
        !text.contains("no task matching"),
        "an unreadable task is not a missing one: {text}"
    );

    // A genuinely absent id still says so — and mentions that other records
    // could not be read, so "not found" is not mistaken for "nothing here".
    let (_, text) = scripted(&repo, |console| {
        ahu::commands::focus(console, &discovered, "006aa5000000000099")
    });
    assert!(text.contains("no readable task matching"), "{text}");
    assert!(text.contains("could not be read either"), "{text}");
}

/// With no unreadable records at all, `focus` keeps its original message.
#[test]
fn focus_on_a_clean_repository_still_reports_a_missing_task_plainly() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    tasks_dir(&repo);

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let (_, text) = scripted(&repo, |console| {
        ahu::commands::focus(console, &discovered, "006aa5000000000099")
    });
    assert!(text.contains("no task matching"), "{text}");
    assert!(!text.contains("could not be read"), "{text}");
}

/// `task::list` is the layer the bug lived in: assert its contract directly.
#[test]
fn list_carries_what_it_could_not_read() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    write_schema_1(&dir, "006aa50000000000g1");
    write_current(&dir, &repo, "006aa50000000000g2");

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let listing = ahu::task::list(&discovered).unwrap();

    assert_eq!(listing.records.len(), 1);
    assert_eq!(listing.unreadable.len(), 1);
    assert!(!listing.is_empty());

    let found = &listing.unreadable[0];
    assert_eq!(found.task_id, "006aa50000000000g1");
    assert!(found.dir.ends_with("006aa50000000000g1"));
    assert!(
        found.reason.contains("schema version (1)"),
        "{}",
        found.reason
    );
    assert_eq!(listing.dirs().len(), 2);

    // Nothing was read out of the refused file: the id comes from the directory
    // name, which ahu chose, not from a schema it does not understand.
    assert!(
        !found.reason.contains("an earlier task"),
        "the refused record's fields must not be surfaced: {}",
        found.reason
    );
}

/// A directory that is not a schema problem at all — truncated JSON — is
/// reported the same way, so the fix is about unreadability, not about schema 1.
#[test]
fn a_corrupt_record_is_reported_rather_than_dropped() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    let corrupt = dir.join("006aa50000000000h1");
    std::fs::create_dir_all(&corrupt).unwrap();
    std::fs::write(corrupt.join("task.json"), "{ this is not json").unwrap();

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let (code, text) = scripted(&repo, |console| ahu::commands::tasks(console, &discovered));
    assert_eq!(code, 0, "{text}");
    assert!(!text.contains(ahu::commands::NO_TASKS), "{text}");
    assert!(text.contains("006aa50000000000h1"), "{text}");
    assert!(text.contains("not a valid ahu task record"), "{text}");
}

/// A launch whose drift comparison could not see earlier records says so, rather
/// than letting "no drift" mean both "nothing changed" and "ahu could not look".
#[test]
fn a_launch_says_when_drift_could_not_read_earlier_records() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.write("assignment.txt", "do the thing\n");
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    write_schema_1(&dir, "006aa50000000000i1");

    let bin = common::fake_harness(repo.state_path(), &repo.state_path().join("argv"));
    let output = common::ahu()
        .current_dir(repo.path())
        .args(["launch", "@chris", "--prompt-file"])
        .arg(repo.path().join("assignment.txt"))
        .arg("--dry-run")
        .env("AHU_CMUX_BIN", repo.state_path().join("missing-cmux"))
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("1 earlier task record(s) for this repository could not be read"),
        "{text}"
    );
    assert!(text.contains("drift was"), "{text}");
}

/// Spare width belongs to the compact branch before it belongs to the two
/// columns a reader can infer from the rest of the row.
///
/// At 96 to 100 columns the layout used to keep MODE and LIVE and pay for them
/// out of the branch, printing `ahu/chris/01a0df…`: a name that matches nothing
/// the reader can look up. The branch column is worth its full compact width or
/// nothing at all.
#[test]
fn the_compact_branch_is_whole_before_mode_and_liveness_are_shown() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let branches = listed_short(&repo);
    let discovered = ahu::git::discover(repo.path()).unwrap();

    // 100 is one column short of the whole table, so MODE leaves rather than the
    // branch giving up the digits that identify it; 104 has room for both.
    for width in [100usize, 104] {
        let (code, text) = scripted(&repo, |console| {
            ahu::commands::tasks_at(console, &discovered, width)
        });
        assert_eq!(code, 0, "{text}");
        for line in text.lines() {
            assert!(line_width(line) <= width, "{width}: {line:?}\n{text}");
        }
        assert!(text.contains("BRANCH"), "{width}: {text}");
        for branch in &branches {
            assert!(
                text.contains(&short_branch(branch)),
                "{width}: the compact {branch} is cut:\n{text}"
            );
            assert!(!text.contains(branch), "{width}: {branch}\n{text}");
        }
        // Nothing the reader cannot get elsewhere was traded for it either.
        for whole in ["@fix-flaky-test", "running", "chris@1.0.0"] {
            assert!(text.contains(whole), "{width}: {whole}\n{text}");
        }
        assert!(
            text.contains("claude-code / claude-opus-5"),
            "{width}: {text}"
        );
    }

    let (_, narrow) = scripted(&repo, |console| {
        ahu::commands::tasks_at(console, &discovered, 100)
    });
    assert!(!narrow.contains("MODE"), "{narrow}");
    assert!(narrow.contains("LIVE"), "{narrow}");
    let (_, roomy) = scripted(&repo, |console| {
        ahu::commands::tasks_at(console, &discovered, 104)
    });
    assert!(roomy.contains("MODE"), "{roomy}");
    assert!(roomy.contains("LIVE"), "{roomy}");
}
