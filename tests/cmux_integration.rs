//! Live cmux integration.
//!
//! **These tests are opt-in.** They are skipped unless `AHU_TEST_CMUX=1` is set.
//!
//! They drive a *real* cmux instance: each one creates a group and several
//! workspaces, then removes exactly what it created. On a developer's machine
//! cmux is essentially always reachable, so gating on reachability alone meant
//! an ordinary `cargo test` opened and closed roughly eighteen workspaces in
//! whoever's cmux window happened to be in front — churning the sidebar and
//! taking the focus away from the person running the tests. Reachability is a
//! statement about the machine; it is not consent to redecorate the user's
//! session. So the gate is an explicit variable.
//!
//! Run them deliberately, and preferably one at a time so the workspace churn
//! is ordered rather than interleaved:
//!
//! ```text
//! AHU_TEST_CMUX=1 cargo test --test cmux_integration -- --test-threads=1
//! ```
//!
//! Nothing here spends model tokens. `launch::execute` builds a workspace's
//! startup command from `std::env::current_exe()`, which inside a test binary
//! is that test binary, so the workspace runs a harmless no-op instead of a
//! coding agent. The only harness subprocesses are `--version` probes.

mod common;

use std::path::{Path, PathBuf};

use ahu::cmux::{self, Cmux};

/// A group plus everything this test created inside it, removed on drop.
struct TempGroup {
    client: Cmux,
    group_id: String,
    anchor: String,
    created: Vec<String>,
}

impl TempGroup {
    fn new(client: Cmux, name: &str, cwd: &Path) -> Self {
        let group = client.create_group(name, cwd).expect("group created");
        TempGroup {
            client,
            group_id: group.id.clone(),
            anchor: group.anchor_workspace_id.clone(),
            created: Vec::new(),
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
        // Deliberately no `select_workspace` here. The old teardown restored
        // the focus it had captured on entry, which is itself a focus change:
        // a test that never should have taken the focus "restored" it, and a
        // suite of them produced a visible flicker per test. Creating every
        // workspace unfocused and never selecting one leaves the focus where
        // the user put it, which is the only correct amount of focus handling
        // for a test.
    }
}

/// The opt-in gate.
///
/// Returns `None`, and says why, unless `AHU_TEST_CMUX=1` is set *and* a cmux
/// with group support is reachable. Reachability alone is not enough: see the
/// module comment.
fn client_or_skip() -> Option<Cmux> {
    match std::env::var("AHU_TEST_CMUX").as_deref() {
        Ok("1") => {}
        _ => {
            eprintln!(
                "skipping live cmux test: set AHU_TEST_CMUX=1 to run it. \
                 It creates and closes real cmux workspaces in your session."
            );
            return None;
        }
    }
    // Past this point the run has opted in, so an unreachable cmux is a
    // failure rather than a skip. A skipped test still reports `ok`; if opting
    // in could also silently skip, a broken integration would stay green in
    // exactly the run that was meant to check it.
    match Cmux::discover() {
        Ok(client) => match client.check_capabilities() {
            Ok(_) => Some(client),
            Err(e) => panic!(
                "AHU_TEST_CMUX=1 was set, so this test must run, but the reachable cmux \
                 lacks group support: {e}"
            ),
        },
        Err(e) => {
            panic!("AHU_TEST_CMUX=1 was set, so this test must run, but cmux is not reachable: {e}")
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
        plan.worktree
            .starts_with(discovered.root.join(".worktrees")),
        "task worktrees live under .worktrees/ in the repository"
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
                plan.worktree.starts_with(source.root.join(".worktrees")),
                "task worktrees live under .worktrees/ in the repository"
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

/// Two agents, two harnesses, one repository.
///
/// The launcher's whole promise is that a named agent runs under *its own*
/// configured harness and model in *its own* cmux workspace. Nothing above
/// proves that: every earlier launch in this file uses one agent on one
/// harness, so a coordinator that quietly ran both assignments under whichever
/// binary PATH resolved first would pass the suite unchanged.
///
/// This test launches two differently-configured agents, then runs each
/// prepared task against a fake executable per harness. Each fake records its
/// own argv to its own file, so the recorded argv is direct evidence of which
/// binary actually ran, with which model and which agent name.
#[test]
fn two_agents_keep_their_own_harness_model_and_workspace_in_one_group() {
    let Some(client) = client_or_skip() else {
        return;
    };
    let repo = common::TestRepo::new();
    repo.write(
        ".agents/ahu/config.toml",
        &format!(
            "schema_version = 1\n\
             harness_preferences = [\"claude-code\", \"codex\"]\n\
             model_selection = \"project-ranked\"\n\
             catalog_version = \"{}\"\n\
             \n[model_rankings]\n\
             \"claude-code\" = [\"claude-opus-5\"]\n\
             \"codex\" = [\"gpt-6-astra\"]\n\
             \n[context_hygiene]\n\
             review_on_first_load = false\n\
             review_interval_days = 7\n",
            ahu::catalog::CATALOG_VERSION
        ),
    );
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.add_agent_on("dana", "2.0.0", "codex", "gpt-6-astra");
    repo.commit("fixture");

    let records = repo.state_path().join("argv");
    std::fs::create_dir_all(&records).unwrap();
    let bin = common::fake_harnesses(repo.state_path(), &["claude", "codex"], |program| {
        records.join(program)
    });

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let loaded = ahu::config::load(repo.path()).unwrap().unwrap();

    let mut launched = Vec::new();
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let original_path = std::env::var("PATH").unwrap();
    // SAFETY: the guard makes this the only test mutating these variables.
    unsafe {
        std::env::set_var("AHU_STATE_DIR", repo.state_path());
        std::env::set_var("PATH", format!("{}:{original_path}", bin.display()));
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for (name, prompt) in [
            ("chris", "Review the launcher"),
            ("dana", "Review the launcher independently"),
        ] {
            let agent = ahu::agent::find(&discovered.root, name).unwrap();
            let pair = ahu::selection::ResolvedPair {
                harness: agent.manifest.harness.clone(),
                model: agent.manifest.model.clone(),
                basis: "named agent".to_string(),
                policy_digest: loaded.digest.clone(),
                catalog_version: loaded.config.catalog_version.clone(),
            };
            let plan = ahu::launch::plan(&discovered, Some(agent), pair, prompt).unwrap();
            let result = ahu::launch::execute(&discovered, &loaded, &plan, prompt, false)
                .expect("launch succeeds");
            // Run the prepared task so a real executable, resolved by name from
            // PATH, records what it was actually given.
            ahu::launch::run_task(&result.task_dir).expect("prepared task runs");
            launched.push((plan, result));
        }
    }));
    // SAFETY: still under the guard.
    unsafe { std::env::set_var("PATH", &original_path) };
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
    };

    let assertions = outcome.and_then(|()| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_eq!(launched.len(), 2);
            let (chris_plan, chris) = &launched[0];
            let (dana_plan, dana) = &launched[1];

            // Each task record froze its own agent's configured identity.
            assert_eq!(chris.record.identity.harness, "claude-code");
            assert_eq!(chris.record.identity.model, "claude-opus-5");
            assert_eq!(chris.record.agent_label(), "chris@1.0.0");
            assert_eq!(dana.record.identity.harness, "codex");
            assert_eq!(dana.record.identity.model, "gpt-6-astra");
            assert_eq!(dana.record.agent_label(), "dana@2.0.0");

            // Separate worktrees, branches, and cmux workspaces.
            assert_ne!(chris_plan.worktree, dana_plan.worktree);
            assert_ne!(chris_plan.branch, dana_plan.branch);
            let chris_ws = chris.record.cmux_workspace_id.clone().unwrap();
            let dana_ws = dana.record.cmux_workspace_id.clone().unwrap();
            assert_ne!(
                chris_ws, dana_ws,
                "two assignments must not share one cmux workspace"
            );

            // One repository group holding both, each rooted in its own worktree.
            let group_id = chris.record.cmux_group_id.clone().unwrap();
            assert_eq!(group_id, dana.record.cmux_group_id.clone().unwrap());
            let group = client
                .find_group(&group_id, None)
                .unwrap()
                .expect("group exists");
            for workspace in [&chris_ws, &dana_ws] {
                assert!(group.member_workspace_ids.contains(workspace));
                assert_ne!(&group.anchor_workspace_id, workspace);
            }
            let listed = client.workspaces().unwrap();
            assert_eq!(
                Path::new(&listed.get(&chris_ws).expect("listed").directory),
                chris_plan.worktree.as_path()
            );
            assert_eq!(
                Path::new(&listed.get(&dana_ws).expect("listed").directory),
                dana_plan.worktree.as_path()
            );

            // Each harness binary ran, and each received its own agent's model.
            // Separate record files, so neither can stand in for the other.
            // The fakes record one argument per line, so a multi-line argument
            // spans several lines: match the contract against the whole dump.
            let claude_raw = std::fs::read_to_string(records.join("claude"))
                .expect("the claude-code agent ran the claude binary");
            let codex_raw = std::fs::read_to_string(records.join("codex"))
                .expect("the codex agent ran the codex binary");
            let claude: Vec<String> = claude_raw.lines().map(str::to_string).collect();
            let codex: Vec<String> = codex_raw.lines().map(str::to_string).collect();

            // The real point of this test: each harness binary got its own
            // agent's model, and neither got the other's.
            assert!(
                claude.windows(2).any(|p| p == ["--model", "claude-opus-5"]),
                "{claude:?}"
            );
            assert!(
                !claude.iter().any(|a| a == "gpt-6-astra"),
                "the codex model must never reach the claude binary: {claude:?}"
            );
            assert!(
                codex.windows(2).any(|p| p == ["-m", "gpt-6-astra"]),
                "{codex:?}"
            );
            assert!(
                !codex.iter().any(|a| a == "claude-opus-5"),
                "the claude model must never reach the codex binary: {codex:?}"
            );

            // Delivery is now identical on both: no agent-selection or
            // system-prompt flag anywhere, and the contract, the agent's
            // instructions and the task prompt all arrive as fenced prompt text.
            for (label, argv, raw, plan, instructions) in [
                ("claude", &claude, &claude_raw, chris_plan, "You are chris."),
                (
                    "codex",
                    &codex,
                    &codex_raw,
                    dana_plan,
                    "You are dana. Fixture instructions.",
                ),
            ] {
                for forbidden in ["--agent", "--append-system-prompt", "--disallowedTools"] {
                    assert!(
                        !argv.iter().any(|a| a == forbidden),
                        "{label} must not receive {forbidden}: {argv:?}"
                    );
                }
                assert!(
                    raw.contains(ahu::orchestration::INSTRUCTIONS.trim()),
                    "the delegation contract must reach the {label} session: {raw}"
                );
                assert!(
                    raw.contains(instructions),
                    "the agent's own instructions must reach the {label} session: {raw}"
                );
                let nonce = &plan.delivery.nonce;
                let agent_close = ahu::orchestration::close_tag("agent", nonce);
                assert!(
                    raw.contains(&ahu::orchestration::open_tag("contract", nonce))
                        && raw.contains(&agent_close),
                    "{label} must receive ahu's sections inside this launch's fence: {raw}"
                );
                // The task prompt sits outside the fence, after it.
                let prompt_at = raw.find("Review the launcher").expect("the prompt arrived");
                assert!(
                    raw.find(&agent_close).unwrap() < prompt_at,
                    "{label}: ahu's fence must close before the task prompt begins"
                );
            }
            // And the two launches did not share a fence tag.
            assert_ne!(chris_plan.delivery.nonce, dana_plan.delivery.nonce);
        }))
    });
    cleanup();
    if let Err(panic) = assertions {
        std::panic::resume_unwind(panic);
    }
}
