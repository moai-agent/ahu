//! The launch pipeline: validate, snapshot, worktree, cmux, record.
//!
//! Every step is ordered so that nothing irreversible happens before the launch
//! is known to be valid, and so that a failure part-way leaves either nothing
//! or something the user is told about.

use std::path::{Path, PathBuf};

use crate::agent::ResolvedAgent;
use crate::bail;
use crate::cmux::{self, Cmux};
use crate::config::LoadedConfig;
use crate::git::{self, Repo};
use crate::harness::{self, EnforcementReport, LaunchCommand, LaunchRequest, RELIABILITY_WARNING};
use crate::hooks::{self, HookInventory};
use crate::selection::ResolvedPair;
use crate::snapshot::{self, ConfigSnapshot};
use crate::state::{self, LaunchLock};
use crate::task::{self, LaunchIdentity, LaunchMode, TaskRecord, TaskState};
use crate::util::{Error, Result};

use serde::{Deserialize, Serialize};

/// Where a repository's cmux group lives. Stored by object id, not by title, so
/// renaming the group in cmux does not orphan the mapping.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupMapping {
    pub group_id: Option<String>,
    pub window_id: Option<String>,
    pub anchor_workspace_id: Option<String>,
}

fn mapping_path(repo_identity: &str) -> Result<PathBuf> {
    Ok(state::repo_dir(repo_identity)?.join("cmux.json"))
}

/// What a launch is going to do, shown before anything is created.
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    pub mode: LaunchMode,
    pub agent: Option<ResolvedAgent>,
    pub pair: ResolvedPair,
    pub enforcement: EnforcementReport,
    pub snapshot: ConfigSnapshot,
    /// Every hook ahu can see that will be in effect for this task.
    pub hooks: HookInventory,
    pub base_commit: Option<String>,
    pub parent_dirty: bool,
    pub task_id: String,
    pub branch: String,
    pub worktree: PathBuf,
    pub task_dir: PathBuf,
    pub title: String,
    pub command: LaunchCommand,
    /// Agent name requested from the harness by its own selection flag, if any.
    pub native_agent: Option<String>,
    /// Approval widening this launch was configured with.
    pub permissions: crate::agent::Permissions,
    /// Absolute path of the harness binary, resolved once at plan time.
    pub harness_executable: PathBuf,
}

impl LaunchPlan {
    pub fn agent_label(&self) -> String {
        match &self.agent {
            Some(agent) => agent.label(),
            None => "auto".to_string(),
        }
    }

    /// Hooks in effect that are not shared project policy.
    ///
    /// These raise the same consistency warning as an unenforceable model: they
    /// change behaviour and can differ for every teammate.
    pub fn non_project_hooks(&self) -> Vec<&crate::hooks::Hook> {
        self.hooks.outside_project_policy()
    }

    pub fn reliability_warning(&self) -> Option<&'static str> {
        if self.enforcement.needs_reliability_warning() {
            Some(RELIABILITY_WARNING)
        } else {
            None
        }
    }
}

/// Build a plan without creating anything.
pub fn plan(
    repo: &Repo,
    agent: Option<ResolvedAgent>,
    pair: ResolvedPair,
    prompt: &str,
) -> Result<LaunchPlan> {
    if prompt.trim().is_empty() {
        bail!("the task prompt is empty; nothing was launched.");
    }
    let adapter = harness::adapter_for(&pair.harness)?;
    let snapshot = snapshot::collect(&repo.root)?;
    let found_hooks = hooks::collect(&repo.root)?;
    let base_commit = repo.head.clone();
    if base_commit.is_none() {
        bail!(
            "this repository has no commits yet, so ahu cannot base a task worktree on its HEAD.\n\
             Make an initial commit first."
        );
    }
    let parent_dirty = git::is_dirty(repo)?;
    // Refuse early if `.worktrees` is a symlink, before anything is created.
    state::ensure_worktrees_root(&repo.root)?;
    let task_id = task::new_task_id();
    let agent_segment = match &agent {
        Some(agent) => agent.manifest.name.clone(),
        None => "auto".to_string(),
    };
    let branch = format!("ahu/{agent_segment}/{task_id}");
    let repo_identity = repo.identity();
    let worktree = state::worktree_dir(&repo.root, &task_id)?;
    let task_dir = state::task_dir(&repo_identity, &task_id)?;
    let title = crate::util::task_title_from_prompt(prompt);

    // Only formats the harness can select by name are requested by name.
    let native_agent = agent.as_ref().and_then(|a| {
        a.manifest
            .source
            .format
            .selects_native_agent()
            .then(|| a.manifest.name.clone())
    });
    let permissions = agent
        .as_ref()
        .map(|a| a.manifest.permissions)
        .unwrap_or_default();
    let command = adapter.launch_command(&LaunchRequest {
        model: &pair.model,
        native_agent: native_agent.as_deref(),
        prompt,
        cwd: &worktree,
        permissions,
    })?;
    let command = crate::orchestration::configure(command)?;
    let harness_executable = crate::selection::resolve_executable(&command.program)
        .map(PathBuf::from)
        .ok_or_else(|| {
            Error::new(format!(
                "{} is not installed on this machine ({} was not found on PATH).\n\
                 This is a diagnostic for your machine, not a reason to select a different \
                 harness: the project's policy is the same for everyone.",
                pair.harness, command.program
            ))
        })?;
    let mut enforcement = adapter.enforcement(&pair.model)?;
    enforcement.applied_controls.push(format!(
        "ahu delegation instructions v1 (digest {}) supplied to every task",
        crate::util::digest_bytes(crate::orchestration::INSTRUCTIONS.as_bytes())
    ));
    if pair.harness == "claude-code" {
        enforcement.applied_controls.push(
            "--disallowedTools Agent,Task,TeamCreate prevents native Claude delegation".to_string(),
        );
    }
    enforcement.gaps.push("Delegation guidance cannot prevent a harness from launching other processes through shell tools; non-Claude adapters have no native delegation-tool denial.".to_string());
    // A wrapper between ahu and the harness can add flags ahu refuses to pass.
    if let Some(note) = harness::wrapper_interposed(&harness_executable) {
        enforcement.gaps.push(note);
    }

    Ok(LaunchPlan {
        mode: if agent.is_some() {
            LaunchMode::Named
        } else {
            LaunchMode::Automatic
        },
        agent,
        pair,
        enforcement,
        snapshot,
        hooks: found_hooks,
        base_commit,
        parent_dirty,
        task_id,
        branch,
        worktree,
        task_dir,
        title,
        command,
        native_agent,
        permissions,
        harness_executable,
    })
}

/// Result of a successful launch.
#[derive(Debug, Clone)]
pub struct Launched {
    pub record: TaskRecord,
    pub task_dir: PathBuf,
    /// Non-fatal things the user should know, such as a reconciled anchor.
    pub notes: Vec<String>,
}

/// Execute a plan: create the worktree, the task record, and the cmux session.
pub fn execute(
    repo: &Repo,
    loaded: &LoadedConfig,
    plan: &LaunchPlan,
    prompt: &str,
    focus: bool,
) -> Result<Launched> {
    let repo_identity = repo.identity();
    let _lock = LaunchLock::acquire(&repo_identity)?;
    let mut notes = Vec::new();

    // cmux must be reachable before a worktree is created, so an unavailable
    // cmux never leaves a worktree behind.
    let cmux_client = Cmux::discover()?;
    cmux_client.check_capabilities()?;

    if git::branch_exists(repo, &plan.branch)? {
        bail!(
            "branch {} already exists; ahu will not reuse or move it.",
            plan.branch
        );
    }
    let base = plan
        .base_commit
        .as_deref()
        .ok_or_else(|| Error::new("the launch plan has no base commit"))?;
    // `.worktrees/` ignores itself, so task checkouts never show up in
    // `git status` and cannot be committed by accident.
    state::ensure_worktrees_root(&repo.root)?;
    git::add_worktree(repo, &plan.worktree, &plan.branch, base)?;

    // From here on, a failure must clean up the worktree it just made, but only
    // while no session could have started in it.
    let materialize = match snapshot::materialize(&repo.root, &plan.snapshot, &plan.worktree) {
        Ok(report) => report,
        Err(e) => {
            let _ = git::remove_worktree(repo, &plan.worktree, &plan.branch);
            return Err(e);
        }
    };
    if !materialize.concurrently_modified.is_empty() {
        notes.push(format!(
            "agent configuration changed while the task was being prepared: {}. \
             The worktree holds the version copied at submission.",
            materialize.concurrently_modified.join(", ")
        ));
    }

    let mut record = TaskRecord {
        schema_version: task::TASK_SCHEMA_VERSION,
        task_id: plan.task_id.clone(),
        title: plan.title.clone(),
        created_at: task::now_rfc3339(),
        repo_identity: repo_identity.clone(),
        repo_root: repo.root.clone(),
        branch: plan.branch.clone(),
        worktree: plan.worktree.clone(),
        base_commit: plan.base_commit.clone(),
        identity: LaunchIdentity {
            mode: plan.mode.clone(),
            agent: plan
                .agent
                .as_ref()
                .map(|a| a.manifest.name.clone())
                .unwrap_or_else(|| "auto".to_string()),
            agent_version: plan.agent.as_ref().map(|a| a.manifest.version.clone()),
            native_agent: plan.native_agent.clone(),
            permissions: plan.permissions,
            harness: plan.pair.harness.clone(),
            model: plan.pair.model.clone(),
            instructions_source: plan.agent.as_ref().map(|a| {
                a.source_path
                    .strip_prefix(&repo.root)
                    .unwrap_or(&a.source_path)
                    .to_string_lossy()
                    .to_string()
            }),
            instructions_digest: plan.agent.as_ref().map(|a| a.source_digest.clone()),
            identity_digest: plan.agent.as_ref().map(|a| a.identity_digest()),
            selection_basis: if plan.mode == LaunchMode::Automatic {
                Some(plan.pair.basis.clone())
            } else {
                None
            },
        },
        policy_digest: loaded.digest.clone(),
        catalog_version: plan.pair.catalog_version.clone(),
        config_snapshot_digest: plan.snapshot.digest(),
        config_snapshot: plan.snapshot.clone(),
        hooks: plan.hooks.clone(),
        hooks_digest: plan.hooks.digest(),
        materialize,
        // The prompt lives only in prompt.txt, which is owner-only.
        launch_command: plan.command.redacted(),
        prompt_digest: crate::util::digest_bytes(prompt.as_bytes()),
        harness_executable: plan.harness_executable.clone(),
        enforcement: plan.enforcement.clone(),
        reliability_warning: plan.reliability_warning().map(str::to_string),
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: TaskState::Starting,
    };

    if let Err(e) = task::save(&plan.task_dir, &record, prompt) {
        let _ = git::remove_worktree(repo, &plan.worktree, &plan.branch);
        return Err(e);
    }

    let group = match ensure_group(&cmux_client, repo, &repo_identity, &mut notes) {
        Ok(group) => group,
        Err(e) => {
            let _ = git::remove_worktree(repo, &plan.worktree, &plan.branch);
            let _ = std::fs::remove_dir_all(&plan.task_dir);
            return Err(e);
        }
    };
    record.cmux_group_id = Some(group.id.clone());

    let executable = std::env::current_exe()
        .map_err(|e| Error::new(format!("cannot locate the ahu executable: {e}")))?;
    let startup = cmux::startup_command(&executable, &plan.task_dir);
    let title = cmux::workspace_title(&plan.agent_label(), &plan.title);

    let window = cmux_client.current_window().ok().flatten();
    let created = match cmux_client.create_task_workspace(
        &group.id,
        window.as_deref(),
        &title,
        &plan.worktree,
        &startup,
        focus,
    ) {
        Ok(created) => created,
        Err(e) => {
            // No session exists, so the worktree cannot hold work yet.
            let _ = git::remove_worktree(repo, &plan.worktree, &plan.branch);
            let _ = std::fs::remove_dir_all(&plan.task_dir);
            return Err(e);
        }
    };

    record.cmux_workspace_id = Some(created.workspace_id.clone());
    record.cmux_window_id = Some(created.window_id.clone());
    // Past this point the workspace exists and its shell may already be running
    // the harness, so nothing is torn down automatically; problems are reported.
    if let Err(e) = task::save(&plan.task_dir, &record, prompt) {
        notes.push(format!(
            "the cmux session started but its task record could not be updated: {e}"
        ));
    }
    if let Err(e) = cmux_client.set_status(&created.workspace_id, TaskState::Starting.as_str()) {
        notes.push(format!("could not set the sidebar status: {e}"));
    }
    if let Err(e) = cmux_client.expand_group(&group.id) {
        notes.push(format!("could not expand the repository group: {e}"));
    }

    Ok(Launched {
        record,
        task_dir: plan.task_dir.clone(),
        notes,
    })
}

/// Find the repository's cmux group, creating it when it is absent.
///
/// The mapping is stored by object id. A stored group that no longer exists is
/// replaced; a group whose anchor was closed (cmux promotes a child, which then
/// loses its own sidebar row) gets a fresh dedicated anchor so every task keeps
/// a visible row.
fn ensure_group(
    client: &Cmux,
    repo: &Repo,
    repo_identity: &str,
    notes: &mut Vec<String>,
) -> Result<cmux::Group> {
    let path = mapping_path(repo_identity)?;
    let mut mapping: GroupMapping = state::read_json(&path)?;
    let current_window = client.current_window().ok().flatten();

    if let Some(group_id) = mapping.group_id.clone() {
        let window: Option<String> = mapping.window_id.clone().or_else(|| current_window.clone());
        if let Some(group) = client.find_group(&group_id, window.as_deref())? {
            if mapping.anchor_workspace_id.as_deref() != Some(group.anchor_workspace_id.as_str()) {
                match restore_anchor(client, repo, &group) {
                    Ok(anchor) => {
                        notes.push(
                            "the repository group's anchor workspace had been closed, so cmux had \
                             promoted a task into the header row. ahu created a new anchor so that \
                             task is visible again."
                                .to_string(),
                        );
                        mapping.anchor_workspace_id = Some(anchor);
                    }
                    Err(e) => notes.push(format!(
                        "the repository group's anchor changed and ahu could not restore a \
                         dedicated one ({e}); one task may be hidden under the group header."
                    )),
                }
                mapping.window_id = current_window.clone().or(mapping.window_id.clone());
                state::write_json(&path, &mapping)?;
                if let Some(refreshed) = client.find_group(&group_id, window.as_deref())? {
                    return Ok(refreshed);
                }
            }
            return Ok(group);
        }
        notes.push(format!(
            "the cmux group recorded for this repository ({group_id}) no longer exists; \
             ahu created a new one."
        ));
    }

    let group = client.create_group(&repo.display_name(), &repo.root)?;
    mapping = GroupMapping {
        group_id: Some(group.id.clone()),
        window_id: current_window,
        anchor_workspace_id: Some(group.anchor_workspace_id.clone()),
    };
    state::write_json(&path, &mapping)?;
    Ok(group)
}

fn restore_anchor(client: &Cmux, repo: &Repo, group: &cmux::Group) -> Result<String> {
    let anchor = client.create_anchor_workspace(&group.id, &repo.display_name(), &repo.root)?;
    client.set_anchor(&group.id, &anchor)?;
    Ok(anchor)
}

/// Reconcile recorded tasks against cmux, so sessions closed outside ahu do not
/// linger as "running" forever.
pub fn reconcile(repo_identity: &str) -> Result<Vec<(PathBuf, TaskRecord)>> {
    let mut tasks = task::list(repo_identity)?;
    let Ok(client) = Cmux::discover() else {
        return Ok(tasks);
    };
    let live = client.workspaces()?;
    for (dir, record) in tasks.iter_mut() {
        let Some(workspace_id) = record.cmux_workspace_id.as_deref() else {
            continue;
        };
        if !live.contains_key(workspace_id)
            && matches!(record.state, TaskState::Starting | TaskState::Running)
        {
            record.state = TaskState::Exited;
            let _ = state::write_json(&dir.join("task.json"), record);
        }
    }
    Ok(tasks)
}

/// Run a task inside its cmux workspace.
///
/// This is the fixed entrypoint that cmux's shell-interpreted startup command
/// invokes. It receives only a directory path; the prompt is read from a file
/// and handed to the harness as one argument, so shell syntax in the prompt is
/// never interpreted.
pub fn run_task(task_dir: &Path) -> Result<std::process::ExitStatus> {
    let record = task::load(task_dir)?;
    let prompt = task::load_prompt(task_dir)?;

    if !record.worktree.is_dir() {
        bail!(
            "the task worktree {} is missing. The task record is still at {}.",
            record.worktree.display(),
            task_dir.display()
        );
    }

    // Verify the prompt file against the digest frozen at submission, then
    // rebuild the argument vector from the record's identity and compare the
    // redacted form. Together these cover the whole command without the record
    // holding a second copy of the prompt.
    let prompt_digest = crate::util::digest_bytes(prompt.as_bytes());
    if !record.prompt_digest.is_empty() && prompt_digest != record.prompt_digest {
        bail!(
            "the prompt file for task {} does not match the digest recorded at submission. \
             ahu will not start a session with a prompt it cannot vouch for.",
            record.task_id
        );
    }

    // The working directory is part of the launch identity: the harness
    // discovers instructions, skills, hooks, and MCP configuration from it.
    //
    // Re-deriving it from `record.repo_root` alone would be circular, since that
    // is a field of the same record. So the repository is opened and its identity
    // recomputed from the Git common directory, and the record's own
    // `repo_identity` has to match before its `repo_root` is used for anything.
    let discovered = git::discover(&record.repo_root)?;
    if discovered.identity() != record.repo_identity {
        bail!(
            "task {} records repository identity {} but {} is repository {}. \
             ahu will not start a session against a different repository than the one it prepared.",
            record.task_id,
            record.repo_identity,
            record.repo_root.display(),
            discovered.identity()
        );
    }
    let expected_worktree = state::worktree_dir(&discovered.root, &record.task_id)?;
    // `.worktrees` could have been replaced by a symlink between submission and
    // this shell starting.
    state::verify_worktree_inside_repo(&discovered.root, &record.worktree)?;
    if record.worktree != expected_worktree {
        bail!(
            "task {} records a working directory that is not the one ahu would create for it.\n\
             expected {}\n\
             found    {}\n\
             ahu will not start a session in a directory it did not prepare.",
            record.task_id,
            expected_worktree.display(),
            record.worktree.display()
        );
    }

    let adapter = harness::adapter_for(&record.identity.harness)?;
    let rebuilt = adapter.launch_command(&LaunchRequest {
        model: &record.identity.model,
        native_agent: record.identity.native_agent.as_deref(),
        prompt: &prompt,
        cwd: &record.worktree,
        permissions: record.identity.permissions,
    })?;
    let rebuilt = crate::orchestration::configure(rebuilt)?;
    if rebuilt.redacted() != record.launch_command {
        bail!(
            "the recorded launch command for task {} does not match what its configuration \
             produces now. ahu will not start a session under a changed identity.",
            record.task_id
        );
    }

    // Resolve the harness here rather than executing a path taken from the
    // record. A record is an ordinary file; trusting a binary path out of it
    // would let anyone who can write ahu's state directory choose what runs.
    // Resolving by name and checking the name is what the adapter asked for
    // removes that surface instead of trying to validate it.
    //
    // The path is deliberately not pinned across sessions: cmux installs a
    // per-surface shim directory, so the absolute path of `claude` differs
    // between the shell that submitted the task and the workspace shell that
    // runs it. The plan-time resolution is recorded for disclosure, and the
    // preview says the workspace may resolve a different wrapper.
    let executable = crate::selection::resolve_executable(&rebuilt.program)
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            Error::new(format!(
                "{} is not on PATH in this workspace, so task {} cannot start.\n\
                 ahu will not fall back to a different harness.",
                rebuilt.program, record.task_id
            ))
        })?;
    if executable.file_name().and_then(|n| n.to_str()) != Some(rebuilt.program.as_str()) {
        bail!(
            "PATH resolved {} to {}, which is not {}. ahu will not run it.",
            rebuilt.program,
            executable.display(),
            rebuilt.program
        );
    }
    if !record.harness_executable.as_os_str().is_empty() && record.harness_executable != executable
    {
        eprintln!(
            "ahu: this workspace resolves {} to {}, not the {} seen at submission. \
             That is normal under cmux, which installs a per-surface wrapper.",
            rebuilt.program,
            executable.display(),
            record.harness_executable.display()
        );
    }

    if let Some(warning) = &record.reliability_warning {
        eprintln!("\n!! {warning}");
        for gap in &record.enforcement.gaps {
            eprintln!("   - {gap}");
        }
        eprintln!();
    }
    eprintln!(
        "ahu task {} — {} on {} / {}",
        record.task_id,
        record.agent_label(),
        record.identity.harness,
        record.identity.model
    );
    eprintln!("worktree {}", record.worktree.display());
    eprintln!("branch   {}", record.branch);
    eprintln!();

    let _ = task::set_state(task_dir, TaskState::Running);
    if let (Ok(client), Some(workspace)) = (Cmux::discover(), record.cmux_workspace_id.as_deref()) {
        let _ = client.set_status(workspace, TaskState::Running.as_str());
    }

    let status = std::process::Command::new(&executable)
        .args(&rebuilt.args)
        .env("AHU_BIN", std::env::current_exe()?)
        .current_dir(&record.worktree)
        .status()
        .map_err(|e| {
            Error::new(format!(
                "cannot start {}: {e}\nThe worktree and task record are preserved at {} and {}.",
                executable.display(),
                record.worktree.display(),
                task_dir.display()
            ))
        })?;

    // A process exit is not evidence the task succeeded, so the state says only
    // that the harness stopped.
    let final_state = if status.success() {
        TaskState::Exited
    } else {
        TaskState::Failed
    };
    let _ = task::set_state(task_dir, final_state);
    if let (Ok(client), Some(workspace)) = (Cmux::discover(), record.cmux_workspace_id.as_deref()) {
        let _ = client.set_status(workspace, final_state.as_str());
    }
    eprintln!(
        "\nahu: the harness exited ({}). The worktree {} and its branch {} are kept.\n\
         Exiting does not mean the task succeeded, and ahu does not delete either for you.",
        final_state.as_str(),
        record.worktree.display(),
        record.branch
    );
    Ok(status)
}
