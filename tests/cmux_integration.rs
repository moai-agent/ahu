//! Live cmux integration.
//!
//! These tests talk to a real cmux instance when one is reachable. Every test
//! creates its own group and workspaces and removes exactly what it created,
//! restoring the original focus; nothing pre-existing is touched. When cmux is
//! not reachable the tests skip with a message rather than failing, so the suite
//! still runs on a machine without it.

mod common;

use std::path::{Path, PathBuf};

use ahu::cmux::{self, Cmux};

/// A group plus everything this test created inside it, removed on drop.
struct TempGroup {
    client: Cmux,
    group_id: String,
    anchor: String,
    created: Vec<String>,
    original_focus: Option<String>,
}

impl TempGroup {
    fn new(client: Cmux, name: &str, cwd: &Path) -> Self {
        let original_focus = current_workspace(&client);
        let group = client.create_group(name, cwd).expect("group created");
        TempGroup {
            client,
            group_id: group.id.clone(),
            anchor: group.anchor_workspace_id.clone(),
            created: Vec::new(),
            original_focus,
        }
    }
}

impl Drop for TempGroup {
    fn drop(&mut self) {
        for workspace in &self.created {
            let _ = self.client.close_workspace(workspace);
        }
        // Closing the last member removes the group.
        let _ = self.client.close_workspace(&self.anchor);
        if let Some(original) = &self.original_focus {
            let _ = self.client.select_workspace(original);
        }
    }
}

fn current_workspace(client: &Cmux) -> Option<String> {
    client.current_window().ok().flatten()?;
    std::env::var("CMUX_WORKSPACE_ID").ok()
}

fn client_or_skip() -> Option<Cmux> {
    match Cmux::discover() {
        Ok(client) => match client.check_capabilities() {
            Ok(_) => Some(client),
            Err(e) => {
                eprintln!("skipping: cmux is reachable but lacks group support: {e}");
                None
            }
        },
        Err(e) => {
            eprintln!("skipping cmux integration test: {e}");
            None
        }
    }
}

#[test]
fn a_repository_group_holds_one_child_workspace_per_task() {
    let Some(client) = client_or_skip() else {
        return;
    };
    let temp = tempfile::TempDir::new().unwrap();
    let mut group = TempGroup::new(client, "ahu-test-tasks", temp.path());

    for (agent, title) in [
        ("chris@1.0.0", "First task"),
        ("chris@1.0.0", "Second task"),
        ("auto", "Third task"),
    ] {
        let created = group
            .client
            .create_task_workspace(
                &group.group_id,
                None,
                &cmux::workspace_title(agent, title),
                temp.path(),
                "true",
                false,
            )
            .expect("task workspace created");
        assert_eq!(created.group_id.as_deref(), Some(group.group_id.as_str()));
        group.created.push(created.workspace_id);
    }

    let found = group
        .client
        .find_group(&group.group_id, None)
        .unwrap()
        .expect("group still exists");
    // Anchor plus one visible row per task.
    assert_eq!(found.member_workspace_ids.len(), 4);
    assert_eq!(found.anchor_workspace_id, group.anchor);
    for workspace in &group.created {
        assert!(
            found.member_workspace_ids.contains(workspace),
            "task workspace {workspace} is not in the repository group"
        );
        assert_ne!(
            &found.anchor_workspace_id, workspace,
            "a task must never become the group header, or its row disappears"
        );
    }

    // Task state is published as a namespaced sidebar status.
    group
        .client
        .set_status(&group.created[0], "running")
        .expect("status set");

    // Every task row names its agent, version, and task so the expanded group
    // identifies the session at a glance.
    let listed = group.client.workspaces().unwrap();
    let titles: Vec<String> = group
        .created
        .iter()
        .map(|id| {
            listed
                .get(id)
                .and_then(|w| w.title.clone())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        titles,
        vec![
            "chris@1.0.0 — First task".to_string(),
            "chris@1.0.0 — Second task".to_string(),
            "auto — Third task".to_string(),
        ],
        "task rows must identify the agent and task"
    );

    group.client.expand_group(&group.group_id).expect("expand");
    let expanded = group
        .client
        .find_group(&group.group_id, None)
        .unwrap()
        .unwrap();
    assert!(!expanded.is_collapsed);
}

#[test]
fn a_closed_anchor_is_replaced_so_no_task_is_hidden_under_the_header() {
    let Some(client) = client_or_skip() else {
        return;
    };
    let temp = tempfile::TempDir::new().unwrap();
    let mut group = TempGroup::new(client, "ahu-test-anchor", temp.path());

    let task = group
        .client
        .create_task_workspace(
            &group.group_id,
            None,
            &cmux::workspace_title("chris@1.0.0", "Only task"),
            temp.path(),
            "true",
            false,
        )
        .unwrap();
    group.created.push(task.workspace_id.clone());

    // Closing the anchor makes cmux promote the task into the header row.
    group.client.close_workspace(&group.anchor).unwrap();
    let promoted = group
        .client
        .find_group(&group.group_id, None)
        .unwrap()
        .expect("group survives");
    assert_eq!(promoted.anchor_workspace_id, task.workspace_id);

    // ahu restores a dedicated anchor, giving the task its own row back.
    let replacement = group
        .client
        .create_anchor_workspace(&group.group_id, "ahu-test-anchor", temp.path())
        .unwrap();
    group
        .client
        .set_anchor(&group.group_id, &replacement)
        .unwrap();
    group.anchor = replacement.clone();

    let restored = group
        .client
        .find_group(&group.group_id, None)
        .unwrap()
        .unwrap();
    assert_eq!(restored.anchor_workspace_id, replacement);
    assert!(restored.member_workspace_ids.contains(&task.workspace_id));
    assert_ne!(restored.anchor_workspace_id, task.workspace_id);
}

/// The startup command cmux types into a shell must reach `ahu run-task` with
/// its task directory intact, even when that path contains shell syntax.
#[test]
fn the_startup_command_reaches_ahu_intact_through_a_real_cmux_shell() {
    let Some(client) = client_or_skip() else {
        return;
    };
    let temp = tempfile::TempDir::new().unwrap();
    let task_dir = temp.path().join("task dir with 'quote' and $VAR");
    std::fs::create_dir_all(&task_dir).unwrap();
    let recorder = temp.path().join("argv.txt");
    let recorder_script = write_argv_recorder(temp.path(), &recorder);

    let mut group = TempGroup::new(client, "ahu-test-transport", temp.path());
    let startup = cmux::startup_command(&recorder_script, &task_dir);
    let created = group
        .client
        .create_task_workspace(
            &group.group_id,
            None,
            &cmux::workspace_title("chris@1.0.0", "Transport check"),
            temp.path(),
            &startup,
            false,
        )
        .unwrap();
    group.created.push(created.workspace_id.clone());

    // The workspace's shell has to start before the command runs.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline && !recorder.exists() {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let recorded = std::fs::read_to_string(&recorder)
        .unwrap_or_else(|e| panic!("the startup command never reached the recorder: {e}"));
    let args: Vec<&str> = recorded.lines().collect();
    assert_eq!(
        args,
        vec![
            "run-task",
            "--task-dir",
            task_dir.to_string_lossy().as_ref(),
        ],
        "cmux's shell altered ahu's arguments"
    );
}

fn write_argv_recorder(dir: &Path, record: &Path) -> PathBuf {
    let script = dir.join("record-argv");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             : > '{record}'\n\
             for arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{record}'; done\n",
            record = record.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script
}

#[test]
fn the_socket_path_is_discovered_rather_than_hard_coded() {
    // The installed build's socket is not the legacy /tmp/cmux.sock, so ahu must
    // never assume that path.
    let discovered = std::env::var("CMUX_SOCKET_PATH").ok();
    if let Some(path) = discovered {
        assert_ne!(path, "/tmp/cmux.sock");
        assert!(
            !ahu::cmux::startup_command(Path::new("/bin/true"), Path::new("/tmp"))
                .contains("cmux.sock")
        );
    }
}

/// The whole launch, against a real repository and a real cmux instance.
///
/// `launch::execute` builds the startup command from `std::env::current_exe()`,
/// which inside an integration test is this test binary, so the workspace's
/// shell runs a harmless no-op instead of a coding agent. No Claude Code session
/// is started here — see the README for what that leaves unverified.
#[test]
fn a_full_launch_creates_one_worktree_and_one_child_workspace() {
    let Some(client) = client_or_skip() else {
        return;
    };
    let repo = common::TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.write("CLAUDE.md", "committed guidance\n");
    repo.commit("fixture");
    // An uncommitted, ignored configuration file must still be inherited.
    repo.write(".claude/settings.local.json", "{\"local\":true}\n");

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let loaded = ahu::config::load(repo.path()).unwrap().unwrap();
    let agent = ahu::agent::find(repo.path(), "chris").unwrap();
    let pair = ahu::selection::ResolvedPair {
        harness: agent.manifest.harness.clone(),
        model: agent.manifest.model.clone(),
        basis: "named agent".to_string(),
        policy_digest: loaded.digest.clone(),
        catalog_version: loaded.config.catalog_version.clone(),
    };
    let prompt = "Implement settings validation\nwith $(a hostile) second line";

    // The state directory must be set before planning: the plan is where the
    // worktree and task-record paths are chosen.
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: the guard makes this the only test mutating the variable.
    unsafe { std::env::set_var("AHU_STATE_DIR", repo.state_path()) };
    let planned = ahu::launch::plan(&discovered, Some(agent), pair, prompt);
    let launched = planned
        .as_ref()
        .map_err(|e| e.to_string())
        .and_then(|plan| {
            ahu::launch::execute(&discovered, &loaded, plan, prompt, false)
                .map_err(|e| e.to_string())
        });
    drop(guard);
    let plan = planned.expect("plan builds");
    let launched = launched.expect("launch succeeds");
    assert!(
        plan.worktree.starts_with(repo.state_path()),
        "the test must never write into the real ahu state directory"
    );

    let workspace = launched
        .record
        .cmux_workspace_id
        .clone()
        .expect("a cmux session was recorded");
    let group_id = launched
        .record
        .cmux_group_id
        .clone()
        .expect("a group was recorded");

    // Clean up whatever we created, whatever the assertions do next.
    let cleanup = || {
        let _ = client.close_workspace(&workspace);
        if let Ok(Some(group)) = client.find_group(&group_id, None) {
            let _ = client.close_workspace(&group.anchor_workspace_id);
        }
        let _ = ahu::git::remove_worktree(&discovered, &plan.worktree, &plan.branch);
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Exactly one worktree, on its own branch, holding the parent's config.
        assert!(plan.worktree.join("CLAUDE.md").is_file());
        assert_eq!(
            std::fs::read_to_string(plan.worktree.join(".claude/settings.local.json")).unwrap(),
            "{\"local\":true}\n",
            "ignored native configuration must reach the task worktree"
        );
        assert!(
            plan.worktree
                .join(".agents/ahu/agents/chris.toml")
                .is_file()
        );

        // The task record froze the identity that was launched.
        assert_eq!(launched.record.identity.model, "claude-opus-5");
        assert_eq!(launched.record.agent_label(), "chris@1.0.0");
        assert!(launched.record.reliability_warning.is_some());

        // The prompt is on disk as data, never on a command line.
        let stored = std::fs::read_to_string(launched.task_dir.join("prompt.txt")).unwrap();
        assert_eq!(stored, prompt);

        // One child row under the repository group, titled by agent and task.
        let group = client
            .find_group(&group_id, None)
            .unwrap()
            .expect("repository group exists");
        assert!(group.member_workspace_ids.contains(&workspace));
        assert_ne!(group.anchor_workspace_id, workspace);
        let listed = client.workspaces().unwrap();
        let info = listed.get(&workspace).expect("workspace is listed");
        assert_eq!(
            info.title.as_deref(),
            Some("chris@1.0.0 — Implement settings validation")
        );
        assert_eq!(Path::new(&info.directory), plan.worktree.as_path());
    }));
    cleanup();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

/// Guards the one test that points ahu's state directory somewhere else.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Repeated launches, including from a second worktree of the same repository,
/// must land in one group as distinct tasks.
#[test]
fn repeated_launches_reuse_one_repository_group_with_distinct_tasks() {
    let Some(client) = client_or_skip() else {
        return;
    };
    let repo = common::TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let loaded = ahu::config::load(repo.path()).unwrap().unwrap();

    // A second worktree of the same repository, standing in for a teammate
    // launching from somewhere else in the same checkout.
    let sibling_path = repo.state_path().join("sibling");
    ahu::git::add_worktree(
        &discovered,
        &sibling_path,
        "sibling",
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();
    let sibling = ahu::git::discover(&sibling_path).unwrap();
    assert_eq!(sibling.identity(), discovered.identity());

    let mut launched = Vec::new();
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: the guard makes this the only test mutating the variable.
    unsafe { std::env::set_var("AHU_STATE_DIR", repo.state_path()) };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for (source, prompt) in [
            (&discovered, "First task from the main checkout"),
            (&discovered, "Second task from the main checkout"),
            (&sibling, "Third task from a sibling worktree"),
        ] {
            let agent = ahu::agent::find(&source.root, "chris").unwrap();
            let pair = ahu::selection::ResolvedPair {
                harness: agent.manifest.harness.clone(),
                model: agent.manifest.model.clone(),
                basis: "named agent".to_string(),
                policy_digest: loaded.digest.clone(),
                catalog_version: loaded.config.catalog_version.clone(),
            };
            let plan = ahu::launch::plan(source, Some(agent), pair, prompt).unwrap();
            assert!(
                plan.worktree.starts_with(repo.state_path()),
                "the test must never write into the real ahu state directory"
            );
            let result = ahu::launch::execute(source, &loaded, &plan, prompt, false)
                .expect("launch succeeds");
            launched.push((plan, result));
        }
    }));
    drop(guard);

    let cleanup = || {
        for (plan, result) in &launched {
            if let Some(workspace) = &result.record.cmux_workspace_id {
                let _ = client.close_workspace(workspace);
            }
            let _ = ahu::git::remove_worktree(&discovered, &plan.worktree, &plan.branch);
        }
        if let Some((_, first)) = launched.first()
            && let Some(group_id) = &first.record.cmux_group_id
            && let Ok(Some(group)) = client.find_group(group_id, None)
        {
            let _ = client.close_workspace(&group.anchor_workspace_id);
        }
        let _ = ahu::git::remove_worktree(&discovered, &sibling_path, "sibling");
    };

    let assertions = outcome.and_then(|()| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let groups: std::collections::BTreeSet<String> = launched
                .iter()
                .map(|(_, r)| r.record.cmux_group_id.clone().unwrap())
                .collect();
            assert_eq!(
                groups.len(),
                1,
                "every task must share one repository group"
            );

            let tasks: std::collections::BTreeSet<String> = launched
                .iter()
                .map(|(_, r)| r.record.task_id.clone())
                .collect();
            assert_eq!(tasks.len(), 3, "repeated launches must be distinct tasks");
            let branches: std::collections::BTreeSet<String> =
                launched.iter().map(|(p, _)| p.branch.clone()).collect();
            assert_eq!(
                branches.len(),
                3,
                "repeated launches must not share a branch"
            );
            let worktrees: std::collections::BTreeSet<std::path::PathBuf> =
                launched.iter().map(|(p, _)| p.worktree.clone()).collect();
            assert_eq!(worktrees.len(), 3, "each task gets its own worktree");

            let group_id = groups.into_iter().next().unwrap();
            let group = client
                .find_group(&group_id, None)
                .unwrap()
                .expect("group exists");
            for (_, result) in &launched {
                let workspace = result.record.cmux_workspace_id.clone().unwrap();
                assert!(group.member_workspace_ids.contains(&workspace));
                assert_ne!(group.anchor_workspace_id, workspace);
            }
            assert_eq!(
                group.member_workspace_ids.len(),
                4,
                "anchor plus three tasks"
            );
        }))
    });
    cleanup();
    if let Err(panic) = assertions {
        std::panic::resume_unwind(panic);
    }
}
