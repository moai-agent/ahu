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
use crate::harness::{self, EnforcementReport, LaunchCommand, LaunchRequest};
use crate::hooks::{self, HookInventory};
use crate::selection::ResolvedPair;
use crate::snapshot::{self, ConfigSnapshot};
use crate::state::{self, LaunchLock};
use crate::task::{self, LaunchIdentity, LaunchMode, TaskRecord, TaskState};
use crate::util::{Error, Result};

use serde::{Deserialize, Serialize};

/// The one thing ahu most needs a reader of a launch preview to understand.
///
/// ahu used to spend one channel per harness on this — `--agent` and
/// `--append-system-prompt` for Claude Code, `--agent` for the Antigravity CLI,
/// a bare prefix on the prompt for Codex — and could enforce none of them. It
/// now delivers the same text the same way everywhere, which is weaker than a
/// system prompt and much easier to describe truthfully. This is the description,
/// and it is a **gap**, never an applied control.
pub const DELIVERY_IS_NOT_ENFORCEMENT: &str = "ahu delivers the agent's instructions and its delegation contract as prompt text on every \
     harness. This is not an enforced system prompt: the task prompt that follows can contradict \
     it, and the model may follow the task prompt instead. ahu does not use harness \
     agent-selection or system-prompt flags, so no harness enforces this agent's identity.";

/// Where a repository's cmux group lives. Stored by object id, not by title, so
/// renaming the group in cmux does not orphan the mapping.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupMapping {
    pub group_id: Option<String>,
    pub window_id: Option<String>,
    pub anchor_workspace_id: Option<String>,
}

fn mapping_path(repo: &Repo) -> Result<PathBuf> {
    Ok(state::coordination_dir(repo)?.join("cmux.json"))
}

/// Optional sidebar text, independent of the assignment delivered to the harness.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisplayMetadata {
    pub title: Option<String>,
    pub summary: Option<String>,
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
    pub summary: String,
    pub command: LaunchCommand,
    /// What ahu put in the harness's prompt slot, frozen so `run_task` can
    /// rebuild it byte for byte and refuse anything else.
    pub delivery: crate::orchestration::Delivery,
    /// Approval widening this launch was configured with.
    pub permissions: crate::agent::Permissions,
    /// Absolute path of the harness binary, resolved once at plan time.
    pub harness_executable: PathBuf,
}

impl LaunchPlan {
    pub fn apply_display(&mut self, display: &DisplayMetadata) -> Result<()> {
        for (flag, value) in [("--title", &display.title), ("--summary", &display.summary)] {
            if let Some(value) = value
                && crate::util::sidebar_text(value, 160).is_empty()
            {
                bail!(kind: crate::util::ErrorKind::Usage, "{flag} needs visible display text.");
            }
        }
        if let Some(title) = &display.title {
            self.title = crate::util::task_title_from_prompt(title);
            // A supplied title also prevents the fallback description from
            // exposing the beginning of a long operational assignment.
            self.summary = crate::util::sidebar_text(title, 160);
        }
        if let Some(summary) = &display.summary {
            self.summary = crate::util::sidebar_text(summary, 160);
        }
        Ok(())
    }

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
}

/// JSON schema 1: additive fields are compatible; changing/removing fields or
/// their meaning requires a schema_version bump. Values bypass display_safe:
/// JSON escaping preserves exact metadata and digest inputs without terminal controls.
pub fn render_json(plan: &LaunchPlan, prompt: &str) -> Result<String> {
    let agent = plan.agent.as_ref().map(|agent| {
        serde_json::json!({
            "name": agent.manifest.name,
            "version": agent.manifest.version,
            "description": agent.manifest.description,
            "source_path": agent.source_path,
            "identity_digest": agent.identity_digest(),
        })
    });
    let mut argv = vec![plan.command.program.clone()];
    argv.extend(plan.command.redacted().args);
    let mut warnings = Vec::new();
    if !plan.non_project_hooks().is_empty() {
        warnings.push(crate::hooks::NON_PROJECT_HOOK_WARNING.to_string());
    }
    if plan.permissions.widens_defaults() {
        warnings.push(plan.permissions.disclosure().to_string());
    }
    if plan.parent_dirty {
        warnings.push("This checkout has uncommitted changes; unrelated source changes are not copied into the task worktree.".to_string());
    }
    warnings.extend(
        plan.hooks
            .unreadable
            .iter()
            .map(|path| format!("Hook configuration could not be read: {path}")),
    );
    if !plan.snapshot.skipped_directories.is_empty() {
        warnings.push(format!(
            "Directories not scanned or inventoried: {}",
            plan.snapshot.skipped_directories.join(", ")
        ));
    }
    if !plan.snapshot.unscanned_config.is_empty() {
        warnings.push(format!(
            "Configuration carried by the checkout but not inventoried: {}",
            plan.snapshot.unscanned_config.join(", ")
        ));
    }
    if !plan.snapshot.symlinks.is_empty() {
        warnings.push(format!(
            "Configuration symlinks not followed or inherited: {}",
            plan.snapshot.symlinks.join(", ")
        ));
    }
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 1,
        "agent": agent,
        "title": plan.title,
        "summary": plan.summary,
        "harness": plan.pair.harness,
        "model": plan.pair.model,
        "selection_basis": plan.pair.basis,
        "policy_digest": plan.pair.policy_digest,
        "catalog_version": plan.pair.catalog_version,
        "permissions": plan.permissions.as_str(),
        "argv": argv,
        "prompt_digest": crate::util::digest_bytes(prompt.as_bytes()),
        "prompt_bytes": prompt.len(),
        "enforcement": {
            "model_fixed_for_session": plan.enforcement.model_fixed_for_session,
            "gaps": plan.enforcement.gaps,
            "applied_controls": plan.enforcement.applied_controls,
        },
        "warnings": warnings,
        "executed": false,
    }))?)
}

/// Build a plan without creating anything.
pub fn plan(
    repo: &Repo,
    agent: Option<ResolvedAgent>,
    pair: ResolvedPair,
    prompt: &str,
) -> Result<LaunchPlan> {
    if prompt.trim().is_empty() {
        bail!(kind: crate::util::ErrorKind::Usage, "the task prompt is empty; nothing was launched.");
    }
    let adapter = harness::adapter_for(&pair.harness)?;
    let snapshot = snapshot::collect(&repo.root)?;
    // Scanning for hooks is harness-specific: `hooks::collect` knows Claude
    // Code's settings files and nothing else, so it is told which harness this
    // launch is for and reports a coverage gap rather than "none found" when it
    // has no implementation for it.
    let found_hooks = hooks::collect(&repo.root, &pair.harness)?;
    let base_commit = repo.head.clone();
    if base_commit.is_none() {
        bail!(kind: crate::util::ErrorKind::Prerequisite,
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

    let permissions = agent
        .as_ref()
        .map(|a| a.manifest.permissions)
        .unwrap_or_default();
    // Everything ahu supplies is composed here, once, for every harness: the
    // fenced delegation contract, then the resolved agent's fenced instructions,
    // then the task prompt. Adapters receive the finished text and have no say
    // in its construction, so a per-harness difference cannot reappear.
    let (delivered, delivery) =
        crate::orchestration::deliver(agent.as_ref().map(|a| a.instructions.as_str()), prompt)?;
    let command = adapter.launch_command(&LaunchRequest {
        model: &pair.model,
        prompt: &delivered,
        cwd: &worktree,
        permissions,
    })?;
    let harness_executable = crate::selection::resolve_executable(&command.program)
        .map(PathBuf::from)
        .ok_or_else(|| {
            Error::new(format!(
                "{} is not installed on this machine ({} was not found on PATH).\n\
                 This is a diagnostic for your machine, not a reason to select a different \
                 harness: the project's policy is the same for everyone.",
                pair.harness, command.program
            ))
            .with_kind(crate::util::ErrorKind::Prerequisite)
        })?;
    let mut enforcement = adapter.enforcement(&pair.model, permissions)?;
    // An *applied control* is something ahu did, stated without implying more.
    // Delivering text is something ahu did; the model heeding it is not, and the
    // gap below says so in the same block.
    enforcement.applied_controls.push(format!(
        "the ahu delegation contract v1 (digest {}) is delivered as prompt text, fenced with this \
         launch's nonce {}",
        &crate::util::digest_bytes(crate::orchestration::INSTRUCTIONS.as_bytes())[..12],
        delivery.nonce
    ));
    enforcement
        .gaps
        .push(DELIVERY_IS_NOT_ENFORCEMENT.to_string());
    enforcement.gaps.push(
        "Delegation guidance cannot prevent a harness from launching other processes through \
         shell tools, and no adapter denies a harness's own delegation tools."
            .to_string(),
    );
    if let Some(harness) = &found_hooks.unscanned_harness {
        enforcement.gaps.push(format!(
            "ahu does not read {harness}'s hook or lifecycle configuration, so hooks for this \
             launch are unknown rather than absent."
        ));
    }
    for facts in found_hooks.widening_settings() {
        enforcement.gaps.push(format!(
            "{} declares approval settings ahu does not set and cannot override; the effective \
             boundary of this session is whatever the harness reads from it, not the flags ahu \
             passes.",
            crate::util::display_safe(&facts.source)
        ));
    }
    if !found_hooks.mcp_servers.is_empty() {
        enforcement.gaps.push(format!(
            "{} MCP server(s) declared by this repository are processes the harness may start; \
             ahu neither launches nor sandboxes them.",
            found_hooks.mcp_servers.len()
        ));
    }
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
        summary: crate::util::sidebar_text(prompt, 160),
        title,
        command,
        delivery,
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
    let _lock = LaunchLock::acquire_at(state::coordination_dir(repo)?.join("launch.lock"))?;
    let mut notes = Vec::new();

    // cmux must be reachable before a worktree is created, so an unavailable
    // cmux never leaves a worktree behind.
    let cmux_client =
        Cmux::discover().map_err(|e| e.with_kind(crate::util::ErrorKind::Prerequisite))?;
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
        summary: plan.summary.clone(),
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
            source_digest: plan.agent.as_ref().map(|a| a.source_digest.clone()),
            instructions_digest: plan.agent.as_ref().map(|a| a.instructions_digest.clone()),
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
        delivery: plan.delivery.clone(),
        prompt_digest: crate::util::digest_bytes(prompt.as_bytes()),
        harness_executable: plan.harness_executable.clone(),
        enforcement: plan.enforcement.clone(),
        // Retain the field for compatibility with older records.
        reliability_warning: None,
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: TaskState::Starting,
    };

    if let Err(e) = task::save(&plan.task_dir, &record, prompt) {
        let _ = git::remove_worktree(repo, &plan.worktree, &plan.branch);
        return Err(e);
    }

    let group = match ensure_group(&cmux_client, repo, &mut notes) {
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
        &plan.summary,
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
    if let Err(e) = cmux_client.set_status(
        &created.workspace_id,
        &cmux::workspace_identity(&record.identity.agent, &record.identity.model),
    ) {
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
fn ensure_group(client: &Cmux, repo: &Repo, notes: &mut Vec<String>) -> Result<cmux::Group> {
    let path = mapping_path(repo)?;
    let mut mapping: GroupMapping = state::read_json(&path)?;
    let current_window = client.current_window().ok().flatten();

    // A user may already be working in a repository group before ahu has any
    // saved mapping. Prefer that group to creating a duplicate, even when an
    // earlier ahu invocation saved a different group after losing its state.
    let groups = client.list_groups(current_window.as_deref())?;
    let current_workspace = client.current_workspace()?;
    let workspaces = client.workspaces()?;
    let candidates = repository_group_candidates(&groups, &repo.display_name(), |id| {
        workspaces.get(id).is_some_and(|workspace| {
            git::discover(Path::new(&workspace.directory))
                .is_ok_and(|found| found.identity() == repo.identity())
        })
    });
    let current_group = if candidates.iter().any(|group| {
        current_workspace
            .as_ref()
            .is_some_and(|id| group.member_workspace_ids.contains(id))
    }) {
        recover_group(&candidates, current_workspace.as_deref())?
    } else {
        None
    };
    if let Some(group) = current_group
        && mapping.group_id.as_deref() != Some(group.id.as_str())
    {
        mapping = GroupMapping {
            group_id: Some(group.id.clone()),
            window_id: current_window.clone(),
            anchor_workspace_id: Some(group.anchor_workspace_id.clone()),
        };
        state::write_json(&path, &mapping)?;
        return Ok(group.clone());
    }

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
             ahu will find an existing group or create one."
        ));
    }

    let group = match recover_group(&candidates, None)? {
        Some(group) => group.clone(),
        None => client.create_group(&repo.display_name(), &repo.root)?,
    };
    mapping = GroupMapping {
        group_id: Some(group.id.clone()),
        window_id: current_window,
        anchor_workspace_id: Some(group.anchor_workspace_id.clone()),
    };
    state::write_json(&path, &mapping)?;
    Ok(group)
}

fn repository_group_candidates<'a>(
    groups: &'a [cmux::Group],
    name: &str,
    belongs: impl Fn(&str) -> bool,
) -> Vec<&'a cmux::Group> {
    groups
        .iter()
        .filter(|group| {
            group.name == name && group.member_workspace_ids.iter().any(|id| belongs(id))
        })
        .collect()
}

fn recover_group<'a>(
    groups: &[&'a cmux::Group],
    current: Option<&str>,
) -> Result<Option<&'a cmux::Group>> {
    if let Some(group) = groups.iter().find(|group| {
        current.is_some_and(|id| group.member_workspace_ids.iter().any(|member| member == id))
    }) {
        return Ok(Some(group));
    }
    match groups {
        [group] => Ok(Some(group)),
        [] => Ok(None),
        _ => bail!(
            "Multiple cmux groups match this repository. Run ahu from the group you want to use."
        ),
    }
}

fn restore_anchor(client: &Cmux, repo: &Repo, group: &cmux::Group) -> Result<String> {
    let anchor = client.create_anchor_workspace(&group.id, &repo.display_name(), &repo.root)?;
    client.set_anchor(&group.id, &anchor)?;
    Ok(anchor)
}

/// Reconcile recorded tasks against cmux, so sessions closed outside ahu do not
/// linger as "running" forever.
///
/// Returns the whole listing, unreadable directories included. Reconciliation
/// can only touch records it can read, and a caller that is about to tell the
/// user what exists needs to know about the ones it could not.
pub fn reconcile(repo_identity: &str) -> Result<task::TaskListing> {
    let mut tasks = task::list(repo_identity)?;
    let Ok(client) = Cmux::discover() else {
        return Ok(tasks);
    };
    let live = client.workspaces()?;
    for (dir, record) in tasks.records.iter_mut() {
        let Some(workspace_id) = record.cmux_workspace_id.as_deref() else {
            continue;
        };
        if !live.contains_key(workspace_id)
            && matches!(record.state, TaskState::Starting | TaskState::Running)
        {
            record.state = TaskState::Exited;
            // Best effort, and deliberately not fatal: reconciliation is a
            // status refresh, and a state directory that has gone read-only
            // must not stop `ahu tasks` listing what exists. The value returned
            // to the caller is the true one either way — cmux is the authority
            // on whether the workspace is still there, not this file.
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
    // A refused record is read here inside a live cmux pane, where the user has
    // no other context, so the error says what to do next rather than only what
    // went wrong.
    let record = task::load(task_dir).map_err(|e| {
        Error::new(format!(
            "{e}\n\
             This task's worktree and branch are untouched; ahu never deletes either. \
             Run `ahu tasks` in the repository to see them, and re-submit the work as a new \
             task rather than trying to resume this one."
        ))
    })?;
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
    // An absent integrity value is a refusal, not a skip. `prompt_digest` used
    // to be checked only `if !record.prompt_digest.is_empty()`, so a `task.json`
    // that simply omitted the key turned the check off — defeating it by
    // deleting a field rather than by forging a digest.
    if record.prompt_digest.is_empty() {
        bail!(
            "task {} has no recorded prompt digest, so ahu cannot vouch for its prompt file. \
             Re-submit the task.",
            record.task_id
        );
    }
    let prompt_digest = crate::util::digest_bytes(prompt.as_bytes());
    if prompt_digest != record.prompt_digest {
        bail!(
            "the prompt file for task {} does not match the digest recorded at submission. \
             ahu will not start a session with a prompt it cannot vouch for.",
            record.task_id
        );
    }
    // The redacted-command comparison below cannot see any of this: the
    // contract, the agent's instructions and the prompt all live in the one argv
    // element that redaction replaces. `redeliver` rebuilds that element from
    // the frozen delivery and refuses if its digest has moved.
    let delivered = crate::orchestration::redeliver(&record.delivery, &prompt)?;

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
        prompt: &delivered,
        cwd: &record.worktree,
        permissions: record.identity.permissions,
    })?;
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
            .with_kind(crate::util::ErrorKind::Prerequisite)
        })?;
    // `which` builds this path as `<PATH entry>/<program>`, so comparing the
    // file name to the program name can never fail — it was a tautology, not a
    // check. What actually has to hold is that the binary is not something the
    // repository put there: an absolute path, and outside both the repository
    // and the task worktree. A harness resolved from inside the tree the agent
    // is about to edit is exactly the case worth refusing.
    if !executable.is_absolute() {
        bail!(
            "PATH resolved {} to the relative path {}, which would be taken from the task \
             worktree. ahu will not run it.",
            rebuilt.program,
            executable.display()
        );
    }
    if let Ok(resolved) = executable.canonicalize() {
        for enclosing in [discovered.root.as_path(), record.worktree.as_path()] {
            if let Ok(enclosing) = enclosing.canonicalize()
                && resolved.starts_with(&enclosing)
            {
                bail!(
                    "PATH resolved {} to {}, which is inside {}. ahu will not run a harness \
                     binary that comes from the repository it is about to work on.",
                    rebuilt.program,
                    resolved.display(),
                    enclosing.display()
                );
            }
        }
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

    // A session's nested ahu commands belong to the checkout it edits, even
    // when the launcher inherited an explicit state override from its caller.
    let session_state = state::ensure_checkout_state(&record.worktree)?;
    let _ = task::set_state(task_dir, TaskState::Running);
    if let (Ok(client), Some(workspace)) = (Cmux::discover(), record.cmux_workspace_id.as_deref()) {
        let _ = client.set_status(
            workspace,
            &cmux::workspace_identity(&record.identity.agent, &record.identity.model),
        );
    }

    let status = std::process::Command::new(&executable)
        .args(&rebuilt.args)
        .env("AHU_BIN", std::env::current_exe()?)
        .env("AHU_STATE_DIR", &session_state)
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
    eprintln!(
        "\nahu: the harness exited ({}). The worktree {} and its branch {} are kept.\n\
         Exiting does not mean the task succeeded, and ahu does not delete either for you.",
        final_state.as_str(),
        record.worktree.display(),
        record.branch
    );
    Ok(status)
}

#[cfg(test)]
mod group_recovery_tests {
    use super::*;
    fn group(id: &str, member: &str) -> cmux::Group {
        cmux::Group {
            id: id.into(),
            name: "ahu".into(),
            anchor_workspace_id: member.into(),
            member_workspace_ids: vec![member.into()],
            is_collapsed: false,
        }
    }
    #[test]
    fn reuse_current_group_and_refuse_ambiguous_recovery() {
        let groups = vec![
            group("new", "task"),
            group("original", "coordinator"),
            group("unrelated", "other-repo"),
        ];
        let matches = repository_group_candidates(&groups, "ahu", |id| id != "other-repo");
        assert_eq!(matches.len(), 2);
        assert_eq!(
            recover_group(&matches, Some("coordinator"))
                .unwrap()
                .unwrap()
                .id,
            "original"
        );
        assert!(recover_group(&matches, None).is_err());
        assert_eq!(
            recover_group(&matches[..1], None).unwrap().unwrap().id,
            "new"
        );
        assert!(recover_group(&[], None).unwrap().is_none());
        assert!(repository_group_candidates(&groups, "different", |_| true).is_empty());
    }
}
