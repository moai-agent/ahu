//! The launch pipeline: validate, snapshot, worktree, cmux, record.
//!
//! Every step is ordered so that nothing irreversible happens before the launch
//! is known to be valid, and so that a failure part-way leaves either nothing
//! or something the user is told about.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// The launch preview discloses prompt delivery as an enforcement gap.
/// Instructions are delivered uniformly, without system-prompt or agent-selection
/// flags, and can be contradicted by the task prompt.
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
    /// Bounded native integration evidence shared by human and JSON previews.
    pub cmux_integration: crate::cmux::integration::Status,
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
        "cmux_integration": plan.cmux_integration,
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
    let task_id = task::new_task_id()?;
    let agent_segment = match &agent {
        Some(agent) => agent.manifest.name.clone(),
        None => "auto".to_string(),
    };
    let branch = format!("ahu/{agent_segment}/{task_id}");
    let repo_identity = repo.identity();
    let worktree = state::worktree_dir(&repo.root, &task_id)?;
    // The record and the prompt belong to the checkout this task will work in,
    // so they are placed inside it and removed with it. Nothing in the
    // environment selects this; it follows from the worktree ahu just chose.
    let task_dir = state::worktree_task_dir(&worktree, &repo_identity, &task_id);
    let title = crate::util::task_title_from_prompt(prompt);

    let permissions = agent
        .as_ref()
        .map(|a| a.manifest.permissions)
        .unwrap_or_default();
    // Adapters transport the fully composed text literally. All rendered facts
    // are frozen now, rather than reconstructed from the runtime environment.
    let metadata = crate::orchestration::Metadata {
        task_id: task_id.clone(),
        agent: agent
            .as_ref()
            .map(|a| a.label())
            .unwrap_or_else(|| "auto".into()),
        harness: pair.harness.clone(),
        model: pair.model.clone(),
        permissions,
    };
    let (delivered, delivery) = crate::orchestration::deliver_composed(
        agent.as_ref().map(|a| a.instructions.as_str()),
        prompt,
        crate::orchestration::Composition::interactive(Some(metadata)),
    )?;
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
        "the ahu delegation contract is delivered as prompt text using layout {}; both fence \
         tag names carry the delivery's nonce",
        delivery.layout_version
    ));
    enforcement
        .gaps
        .push(DELIVERY_IS_NOT_ENFORCEMENT.to_string());
    enforcement.gaps.push(
        "This task's record and prompt are kept inside its own worktree, so removing that \
         worktree removes them -- and the session working there can read and change them too. \
         ahu verifies both against the digests frozen at submission before it starts the \
         session; after that the record is a log the session itself could edit."
            .to_string(),
    );
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
    // A plugin entry names a module a harness installs and runs at startup. The
    // snapshot carries and digests the file that declares it, but the digest
    // describes the declaration, not the code it resolves to, and an npm
    // specifier is not a file the executable-bit scan can see. Counting the
    // names is what makes the trust decision visible on the launch preview.
    //
    // Gated through the feature matrix: `opencode.json` travels into every
    // task worktree, but only OpenCode reads it, and telling a launch of
    // another harness that it executes these modules would be false.
    // A harness with no catalog entry claims no feature, so the lookup and the
    // feature test are one step: an absent entry simply leaves the gap unsaid
    // rather than panicking the preview on a cross-module invariant.
    if let Some(entry) = crate::catalog::harness(&pair.harness)
        .filter(|entry| entry.supports(crate::catalog::Feature::StartupPluginInventory))
        && !found_hooks.declared_plugins.is_empty()
    {
        let modules: Vec<String> = found_hooks
            .declared_plugins
            .iter()
            .map(|plugin| crate::util::display_safe(&plugin.module))
            .collect();
        enforcement.gaps.push(format!(
            "{} plugin module(s) declared by this repository ({}) are installed and executed by \
             {} at startup; ahu carries the declaration into the task worktree but \
             neither resolves, pins, nor sandboxes what it fetches.",
            modules.len(),
            modules.join(", "),
            entry.display_name
        ));
    }
    // A wrapper between ahu and the harness can add flags ahu refuses to pass.
    if let Some(note) = harness::wrapper_interposed(&harness_executable) {
        enforcement.gaps.push(note);
    }

    let cmux_integration = crate::cmux::integration::inspect(&repo.root, &pair.harness);
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
        cmux_integration,
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

pub(crate) fn prepared_record(
    repo: &Repo,
    loaded: &LoadedConfig,
    plan: &LaunchPlan,
    prompt: &str,
    materialize: crate::snapshot::MaterializeReport,
) -> TaskRecord {
    let repo_identity = repo.identity();
    TaskRecord {
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
    }
}

/// Undo the worktree a failed launch created, and say so when Git refuses.
///
/// `git worktree remove` will not force-delete a checkout that holds work, and
/// materialization can legitimately leave one dirty by copying uncommitted
/// agent configuration into it. When that happens the worktree and its branch
/// stay, so the failure that is reported has to name them: they are the user's
/// to inspect, and nothing else is going to mention them.
fn rollback_worktree(repo: &Repo, plan: &LaunchPlan, cause: Error) -> Error {
    // A dead pointer in the task index degrades into a lead at resolution
    // time, but removing it here keeps the index honest about live tasks.
    let _ = crate::task_index::remove_in(repo, &plan.task_id);
    // The record lives inside the worktree, so removing the worktree takes it.
    // If Git refuses, the record is removed on its own so no half-prepared
    // task is left claiming to be one.
    if let Err(refused) = git::remove_worktree(repo, &plan.worktree, &plan.branch) {
        discard_task_dir(&plan.task_dir);
        let kind = cause.kind();
        return Error::new(format!(
            "{cause}\n\n\
             The task checkout ahu created for this launch could not be removed, so it is still \
             here along with its branch:\n  \
             worktree {}\n  branch   {}\n\
             {refused}\n\
             ahu does not force-remove a checkout that holds work. Inspect it, then remove it \
             yourself once you are sure nothing in it is needed. `ahu tasks` lists it as a task \
             worktree with no record until then.",
            plan.worktree.display(),
            plan.branch
        ))
        .with_kind(kind);
    }
    discard_task_dir(&plan.task_dir);
    cause
}

/// Remove a task directory a failed launch created, but never through a path
/// ahu would refuse to write to.
///
/// One of the ways a launch fails here is a repository that committed a link at
/// the worktree's `.ahu`. Deleting the task directory by name would follow that
/// same link and take the deletion outside the checkout, which would turn a
/// refusal into the damage the refusal exists to prevent. A path that cannot be
/// validated is left exactly as it is; the error already tells the user the
/// worktree was kept.
fn discard_task_dir(task_dir: &Path) {
    if state::confine_existing_dir(task_dir).is_ok() {
        let _ = std::fs::remove_dir_all(task_dir);
    }
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
        Err(e) => return Err(rollback_worktree(repo, plan, e)),
    };
    // The task's own state directory, created now that its worktree exists. A
    // repository that committed a link at `.ahu` or `.ahu/state` is refused
    // here, before a record or a prompt is written through it.
    if let Err(e) = state::ensure_checkout_state(&plan.worktree) {
        return Err(rollback_worktree(repo, plan, e));
    }
    if !materialize.concurrently_modified.is_empty() {
        notes.push(format!(
            "agent configuration changed while the task was being prepared: {}. \
             The worktree holds the version copied at submission.",
            materialize.concurrently_modified.join(", ")
        ));
    }

    let mut record = prepared_record(repo, loaded, plan, prompt, materialize);

    if let Err(e) = task::save(&plan.task_dir, &record, prompt) {
        return Err(rollback_worktree(repo, plan, e));
    }
    if let Err(e) = crate::task_index::register(
        &repo.identity(),
        &plan.task_id,
        &plan.worktree,
        crate::task_index::StoreKind::Worktree,
    ) {
        return Err(rollback_worktree(repo, plan, e));
    }

    let group = match ensure_group(&cmux_client, repo, &mut notes) {
        Ok(group) => group,
        Err(e) => return Err(rollback_worktree(repo, plan, e)),
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
        // No session exists, so nothing in the worktree came from a task.
        Err(e) => return Err(rollback_worktree(repo, plan, e)),
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
pub fn reconcile(repo: &Repo) -> Result<task::TaskListing> {
    let mut tasks = task::list(repo)?;
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
pub(crate) fn verify_task(
    task_dir: &Path,
    batch: Option<&crate::headless::Spec>,
) -> Result<(TaskRecord, LaunchCommand, PathBuf)> {
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
    let expected_composition = crate::orchestration::Composition {
        mode: match batch {
            Some(spec) => crate::orchestration::Mode::headless(&spec.options.native_helpers)?,
            None => crate::orchestration::Mode::Interactive,
        },
        metadata: Some(crate::orchestration::Metadata {
            task_id: record.task_id.clone(),
            agent: record.agent_label(),
            harness: record.identity.harness.clone(),
            model: record.identity.model.clone(),
            permissions: record.identity.permissions,
        }),
        state: batch.map(crate::headless::Spec::coordination_state),
    };
    record.delivery.verify_composition(&expected_composition)?;
    let delivered = if let Some(spec) = batch {
        crate::orchestration::redeliver_headless_policy(
            &record.delivery,
            &prompt,
            &spec.options.native_helpers,
        )?
    } else {
        crate::orchestration::redeliver(&record.delivery, &prompt)?
    };

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

    if batch.is_some_and(|spec| spec.schema_version == 2) {
        let expected = crate::headless::store(&discovered)?.join(&record.task_id);
        if task_dir != expected {
            bail!("headless task is not held by its primary-owned coordination store");
        }
        crate::headless::confined(task_dir, false)?;
    }

    // A record kept inside a task worktree must be that worktree's own. Records
    // written before task state moved into worktrees live in a checkout's store
    // rather than under `.worktrees/`, and are not subject to this.
    if let Some(owner) = state::enclosing_checkout(task_dir) {
        let worktrees = state::worktrees_root_at(&discovered.primary_root()?);
        let resolved = owner.canonicalize().ok();
        let inside = resolved
            .as_deref()
            .zip(worktrees.canonicalize().ok())
            .is_some_and(|(owner, worktrees)| owner.starts_with(&worktrees));
        if inside && resolved != record.worktree.canonicalize().ok() {
            bail!(
                "task {} is recorded in the worktree {}, but its record was read from {}.\n\
                 ahu will not start a session from a record held by a different task's checkout.",
                record.task_id,
                record.worktree.display(),
                owner.display()
            );
        }
    }

    let adapter = harness::adapter_for(&record.identity.harness)?;
    let request = LaunchRequest {
        model: &record.identity.model,
        prompt: &delivered,
        cwd: &record.worktree,
        permissions: record.identity.permissions,
    };
    let rebuilt = if let Some(spec) = batch {
        crate::headless::batch_command(&record.identity.harness, &request, spec)?
    } else {
        adapter.launch_command(&request)?
    };
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
    // The resolved executable must be absolute and outside the repository and
    // task worktree, including when reached through a symlink.
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
    if batch.is_none()
        && !record.harness_executable.as_os_str().is_empty()
        && record.harness_executable != executable
    {
        eprintln!(
            "ahu: this workspace resolves {} to {}, not the {} seen at submission. \
             That is normal under cmux, which installs a per-surface wrapper.",
            rebuilt.program,
            executable.display(),
            record.harness_executable.display()
        );
    }

    Ok((record, rebuilt, executable))
}

/// What happened to an interactive task's harness process, from the run-task
/// parent that spawned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessOutcome {
    /// The harness process ended on its own, successfully or not.
    Exited(std::process::ExitStatus),
    /// ahu terminated the harness process tree because a cancellation was
    /// requested.
    Cancelled,
}

/// Poll a spawned interactive harness, terminating its process tree when a
/// cancellation is requested in the task directory.
fn supervise_harness(child: &mut std::process::Child, task_dir: &Path) -> Result<HarnessOutcome> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(HarnessOutcome::Exited(status));
        }
        if task_dir.join("cancel.json").exists() {
            terminate_group(child)?;
            return Ok(HarnessOutcome::Cancelled);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Terminal ownership borrowed by a harness. Drop also restores it on errors.
struct Foreground {
    fd: Option<(i32, libc::pid_t)>,
}

impl Foreground {
    /// Piped stdin needs no transfer; a terminal transfer must succeed before
    /// supervision starts. `pgid` is the unreaped harness child's PID in its
    /// fresh process group. Never change process-wide job-control dispositions.
    fn hand_to(fd: i32, pgid: libc::pid_t) -> Result<Option<Self>> {
        if unsafe { libc::isatty(fd) } != 1 {
            return Ok(None);
        }
        let previous = unsafe { libc::tcgetpgrp(fd) };
        if previous < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let foreground = Self {
            fd: Some((fd, previous)),
        };
        Self::set(fd, pgid)?;
        // Recover a stdin read that raced the transfer and stopped the group.
        if let Err(error) = signal_owned_group(pgid as u32, libc::SIGCONT)
            && !crate::headless::child_exited(pgid as u32)?
        {
            return Err(error);
        }
        Ok(Some(foreground))
    }

    fn set(fd: i32, pgid: libc::pid_t) -> Result<()> {
        // POSIX permits tcsetpgrp from a background group when SIGTTOU is
        // blocked. Scope the mask to this thread and this syscall, including
        // failure paths, rather than leaving SIGTTIN/SIGTTOU ignored globally.
        let mut block: libc::sigset_t = unsafe { std::mem::zeroed() };
        let mut previous: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut block);
            libc::sigaddset(&mut block, libc::SIGTTOU);
        }
        let error = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &block, &mut previous) };
        if error != 0 {
            return Err(std::io::Error::from_raw_os_error(error).into());
        }
        let result = loop {
            if unsafe { libc::tcsetpgrp(fd, pgid) } == 0 {
                break Ok(());
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                break Err(error);
            }
        };
        let restored =
            unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut()) };
        result.map_err(|error| {
            Error::new(format!(
                "cannot set terminal foreground to process group {pgid}: {error}"
            ))
        })?;
        if restored != 0 {
            return Err(std::io::Error::from_raw_os_error(restored).into());
        }
        Ok(())
    }

    fn take_back(mut self) -> Result<()> {
        self.restore()
    }

    fn restore(&mut self) -> Result<()> {
        if let Some((fd, previous)) = self.fd {
            Self::set(fd, previous)?;
            self.fd = None;
        }
        Ok(())
    }
}

impl Drop for Foreground {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!("ahu: could not restore terminal foreground: {error}");
        }
    }
}

/// Only for a fresh group whose unreaped child this supervisor owns.
fn signal_owned_group(pid: u32, signal: i32) -> Result<()> {
    let pid = i32::try_from(pid).map_err(|_| Error::new("invalid owned process group"))?;
    if pid <= 0 {
        bail!("invalid owned process group");
    }
    if unsafe { libc::kill(-pid, signal) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error.into());
        }
    }
    Ok(())
}

/// Resume and terminate the owned group, then reap its leader. Keep the leader
/// unreaped until the final group signal so its PID cannot be reused first.
fn terminate_group(child: &mut std::process::Child) -> Result<()> {
    let pid = child.id();
    crate::headless::child_exited(pid)?;
    let graceful = signal_owned_group(pid, libc::SIGCONT)
        .and_then(|()| signal_owned_group(pid, libc::SIGTERM));
    if let Err(error) = graceful
        && !crate::headless::child_exited(pid)?
    {
        return Err(error);
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if crate::headless::child_exited(pid)? {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Even a cooperative leader can leave TERM-ignoring descendants behind.
    let killed = signal_owned_group(pid, libc::SIGKILL);
    // Do not wait indefinitely after a denied signal to a still-live leader.
    if killed.is_err() && !crate::headless::child_exited(pid)? {
        return killed;
    }
    child.wait()?;
    if let Err(error) = killed {
        // Darwin can return EPERM for a group containing only zombies. Reap
        // our leader, then accept that error only if the group is now absent.
        // This is a read-only probe: never send another signal after reaping,
        // when the identifier could refer to an unrelated new process group.
        let absent = unsafe { libc::kill(-(pid as i32), 0) } < 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if !absent {
            return Err(error);
        }
    }
    Ok(())
}

pub fn run_task(task_dir: &Path) -> Result<HarnessOutcome> {
    let (record, rebuilt, executable) = verify_task(task_dir, None)?;
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

    // Nested commands discover the checkout the session edits.
    state::ensure_checkout_state(&record.worktree)?;
    let _ = task::set_state(task_dir, TaskState::Running);
    if let (Ok(client), Some(workspace)) = (Cmux::discover(), record.cmux_workspace_id.as_deref()) {
        let _ = client.set_status(
            workspace,
            &cmux::workspace_identity(&record.identity.agent, &record.identity.model),
        );
    }

    // A cancellation that arrived before the harness started must not be
    // lost to the spawn that follows.
    if task_dir.join("cancel.json").exists() {
        let _ = task::set_state(task_dir, TaskState::Cancelled);
        eprintln!(
            "ahu: the task was cancelled before the harness started. The worktree {} and its \
             branch {} are kept.",
            record.worktree.display(),
            record.branch
        );
        return Ok(HarnessOutcome::Cancelled);
    }

    let mut child = {
        use std::os::unix::process::CommandExt;
        std::process::Command::new(&executable)
            .args(&rebuilt.args)
            .env("AHU_BIN", std::env::current_exe()?)
            .env("AHU_WORKER_SESSION", "cmux")
            .env("AHU_TASK_ID", &record.task_id)
            .env("AHU_TASK_DIR", task_dir)
            .current_dir(&record.worktree)
            // The run-task parent owns the harness's fresh process group, so
            // cancellation can terminate the whole tree without signalling
            // this parent or the pane it lives in.
            .process_group(0)
            .spawn()
            .map_err(|e| {
                Error::new(format!(
                    "cannot start {}: {e}\nThe worktree and task record are preserved at {} and {}.",
                    executable.display(),
                    record.worktree.display(),
                    task_dir.display()
                ))
            })?
    };

    // The TUI harness needs the terminal's foreground or its first stdin read
    // stops it with SIGTTIN; hand it over now and take it back when the
    // harness is done.
    let foreground = match Foreground::hand_to(libc::STDIN_FILENO, child.id() as libc::pid_t) {
        Ok(foreground) => foreground,
        Err(error) => {
            terminate_group(&mut child)?;
            task::set_state(task_dir, TaskState::Failed)?;
            return Err(error);
        }
    };

    let outcome = supervise_harness(&mut child, task_dir)?;
    let restoration = foreground.map_or(Ok(()), Foreground::take_back);
    let final_state = record_harness_outcome(outcome, restoration, |state| {
        task::set_state(task_dir, state)
    })?;
    match outcome {
        HarnessOutcome::Exited(status) => {
            eprintln!(
                "\nahu: the harness exited ({}). The worktree {} and its branch {} are kept.\n\
                 Exiting does not mean the task succeeded, and ahu does not delete either for you.",
                final_state.as_str(),
                record.worktree.display(),
                record.branch
            );
            Ok(HarnessOutcome::Exited(status))
        }
        HarnessOutcome::Cancelled => {
            eprintln!(
                "\nahu: cancellation of the owned harness process group completed.\n\
                 The worktree {} and its branch {} are kept; cancelling does not delete either \
                 for you.",
                record.worktree.display(),
                record.branch
            );
            Ok(HarnessOutcome::Cancelled)
        }
    }
}

/// Persist an observed process outcome independently of terminal restoration.
fn record_harness_outcome(
    outcome: HarnessOutcome,
    restoration: Result<()>,
    persist: impl FnOnce(TaskState) -> Result<()>,
) -> Result<TaskState> {
    // Exited records process exit, not successful completion of the task.
    let state = match outcome {
        HarnessOutcome::Exited(status) if status.success() => TaskState::Exited,
        HarnessOutcome::Exited(_) => TaskState::Failed,
        HarnessOutcome::Cancelled => TaskState::Cancelled,
    };
    persist(state)?;
    // A terminal restore error must not erase an already observed outcome.
    restoration?;
    Ok(state)
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

    fn spawn_sleep(seconds: &str) -> std::process::Child {
        use std::os::unix::process::CommandExt;
        std::process::Command::new("sleep")
            .arg(seconds)
            .process_group(0)
            .spawn()
            .expect("POSIX sleep is available in the test environment")
    }

    #[test]
    fn supervise_returns_exit_when_child_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = spawn_sleep("0.1");
        match supervise_harness(&mut child, dir.path()).unwrap() {
            HarnessOutcome::Exited(status) => assert!(status.success()),
            HarnessOutcome::Cancelled => panic!("no cancellation was requested"),
        }
    }

    #[test]
    fn supervise_cancels_child_on_cancel_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = spawn_sleep("30");
        let _ = child.try_wait().expect("child has not exited");
        std::fs::write(dir.path().join("cancel.json"), b"").unwrap();
        match supervise_harness(&mut child, dir.path()).unwrap() {
            HarnessOutcome::Cancelled => {}
            HarnessOutcome::Exited(_) => panic!("cancellation was requested"),
        }
        assert!(
            child.try_wait().expect("child is reaped").is_some(),
            "the cancelled child must be reaped"
        );
    }

    #[test]
    fn terminal_outcome_is_persisted_when_foreground_restoration_fails() {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::ExitStatusExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let file = std::fs::File::open("/dev/null").unwrap();
        let mut lost = Vec::new();
        for (outcome, expected) in [
            (
                HarnessOutcome::Exited(std::process::ExitStatus::from_raw(0)),
                TaskState::Exited,
            ),
            (
                HarnessOutcome::Exited(std::process::ExitStatus::from_raw(1 << 8)),
                TaskState::Failed,
            ),
            (HarnessOutcome::Cancelled, TaskState::Cancelled),
        ] {
            state::write_json(&path, &TaskState::Running).unwrap();
            // Inject a real tcsetpgrp failure without changing this runner's
            // terminal: the captured descriptor no longer names a terminal.
            let foreground = Foreground {
                fd: Some((file.as_raw_fd(), unsafe { libc::getpgrp() })),
            };
            let restoration = foreground.take_back();
            let restoration_error = restoration.as_ref().unwrap_err().to_string();
            let result = record_harness_outcome(outcome, restoration, |state| {
                state::write_json(&path, &state)
            });
            assert_eq!(result.unwrap_err().to_string(), restoration_error);
            let saved: TaskState = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            if saved != expected {
                lost.push((expected, saved));
            }
        }
        assert!(
            lost.is_empty(),
            "terminal states lost on restore failure: {lost:?}"
        );
    }

    #[test]
    fn foreground_handover_is_a_noop_without_a_terminal() {
        use std::os::fd::AsRawFd;
        // Cargo runs tests with piped stdio, so /dev/null stands in for any
        // non-terminal fd: the handover must leave it alone.
        let file = std::fs::File::open("/dev/null").unwrap();
        assert!(
            Foreground::hand_to(file.as_raw_fd(), 12345)
                .unwrap()
                .is_none()
        );
    }
}

#[cfg(test)]
mod pty_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};

    // Each case runs in a separate session with a disposable controlling PTY.
    // Neither job-control signals nor terminal settings touch the test runner.
    fn pty_case(case: &str) {
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let mut master = unsafe { std::fs::File::from_raw_fd(master) };
        let slave = unsafe { std::fs::File::from_raw_fd(slave) };
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
        }
        let output = tempfile::tempfile().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "launch::pty_tests::pty_child_entry",
                "--nocapture",
            ])
            .env("AHU_TEST_PTY_CASE", case)
            .stdin(slave)
            .stdout(output.try_clone().unwrap())
            .stderr(output.try_clone().unwrap());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        master.write_all(b"input\n").unwrap();
        unsafe {
            libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            // Drain terminal echo: Darwin can wait for pending PTY output when
            // the controlling session exits, even with stdout redirected.
            let mut echo = [0; 256];
            let _ = master.read(&mut echo);
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                eprintln!("PTY {case}: timeout");
                use std::os::unix::fs::FileExt;
                let mut debug = vec![0; output.metadata().unwrap().len() as usize];
                output.read_at(&mut debug, 0).unwrap();
                eprintln!("{}", String::from_utf8_lossy(&debug));
                // Closing the master also releases a session stuck in tty exit.
                drop(master);
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("PTY case {case} timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        use std::os::unix::fs::FileExt;
        let mut bytes = vec![0; output.metadata().unwrap().len() as usize];
        output.read_at(&mut bytes, 0).unwrap();
        assert!(
            status.success(),
            "PTY case {case}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }

    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if matches!(self.0.try_wait(), Ok(None)) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    fn shell(script: &str) -> OwnedChild {
        OwnedChild(
            Command::new("/bin/sh")
                .args(["-c", script])
                .process_group(0)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }

    fn stopped(child: &Child, signal: i32) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut status = 0;
            let result = unsafe {
                libc::waitpid(
                    child.id() as _,
                    &mut status,
                    libc::WUNTRACED | libc::WNOHANG,
                )
            };
            assert!(result >= 0);
            if result > 0 {
                assert!(
                    libc::WIFSTOPPED(status),
                    "expected a stopped child: {status}"
                );
                assert_eq!(libc::WSTOPSIG(status), signal);
                return;
            }
            assert!(Instant::now() < deadline, "child did not stop");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn pty_stdin_recovers_from_sigttin_and_restores_on_exit() {
        pty_case("read");
    }
    #[test]
    fn pty_foreground_restores_on_drop() {
        pty_case("drop");
    }
    extern "C" fn job_control_handler(_: libc::c_int) {}

    #[test]
    fn pty_handover_preserves_signal_dispositions() {
        pty_case("signals");
    }
    #[test]
    fn pty_cancellation_resumes_stopped_group_and_cleans_descendants() {
        pty_case("cancel");
    }

    #[test]
    fn pty_fast_exit_during_handover_is_not_a_failure() {
        pty_case("fast-exit");
    }
    #[test]
    fn pty_handover_failure_restores_owner_and_signal_mask() {
        pty_case("failure");
    }
    #[test]
    fn pty_foreground_restores_during_unwind() {
        pty_case("unwind");
    }
    #[test]
    fn pty_cancellation_escalates_for_stopped_term_ignoring_leader() {
        pty_case("kill");
    }

    #[test]
    fn pty_child_entry() {
        let Ok(case) = std::env::var("AHU_TEST_PTY_CASE") else {
            return;
        };
        let parent = unsafe { libc::tcgetpgrp(0) };
        assert_eq!(parent, unsafe { libc::getpgrp() });
        if case == "fast-exit" {
            let mut child = shell("exit 0");
            let deadline = Instant::now() + Duration::from_secs(3);
            while !crate::headless::child_exited(child.0.id()).unwrap() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            let foreground = Foreground::hand_to(0, child.0.id() as _).unwrap().unwrap();
            assert!(child.0.wait().unwrap().success());
            foreground.take_back().unwrap();
            assert_eq!(unsafe { libc::tcgetpgrp(0) }, parent);
            return;
        }
        if case == "failure" {
            let mut before: libc::sigset_t = unsafe { std::mem::zeroed() };
            let mut after: libc::sigset_t = unsafe { std::mem::zeroed() };
            assert_eq!(
                unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut before) },
                0
            );
            assert!(Foreground::hand_to(0, -1).is_err());
            assert_eq!(unsafe { libc::tcgetpgrp(0) }, parent);
            assert_eq!(
                unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut after) },
                0
            );
            for signal in [libc::SIGTTIN, libc::SIGTTOU] {
                assert_eq!(unsafe { libc::sigismember(&before, signal) }, unsafe {
                    libc::sigismember(&after, signal)
                });
            }
            return;
        }
        if case == "cancel" || case == "kill" {
            // The leader cooperates with TERM; its descendant deliberately does
            // not. An inherited pipe proves the whole group stopped, not just
            // the leader. No persisted PID is used as signal authority.
            let mut unrelated = shell("exec sleep 30");
            let script = if case == "kill" {
                "trap '' TERM; echo ready; exec sleep 30"
            } else {
                "trap 'exit 0' TERM; /bin/sh -c 'trap \"\" TERM; echo ready; for i in 1 2 3 4 5 6; do sleep 1; done' & wait"
            };
            let mut child = shell(script);
            let mut pipe = child.0.stdout.take().unwrap();
            let mut ready = [0; 6];
            pipe.read_exact(&mut ready).unwrap();
            assert_eq!(&ready, b"ready\n");
            let foreground = Foreground::hand_to(0, child.0.id() as _).unwrap().unwrap();
            crate::headless::signal_group(child.0.id(), libc::SIGSTOP);
            stopped(&child.0, libc::SIGSTOP);
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("cancel.json"), b"{}").unwrap();
            assert_eq!(
                supervise_harness(&mut child.0, dir.path()).unwrap(),
                HarnessOutcome::Cancelled
            );
            let status = child.0.try_wait().unwrap().unwrap();
            if case == "cancel" {
                assert!(status.success(), "stopped leader did not handle TERM");
            } else {
                use std::os::unix::process::ExitStatusExt;
                assert_eq!(status.signal(), Some(libc::SIGKILL));
            }
            assert!(unrelated.0.try_wait().unwrap().is_none());
            foreground.take_back().unwrap();
            assert_eq!(unsafe { libc::tcgetpgrp(0) }, parent);
            let mut poll = libc::pollfd {
                fd: pipe.as_raw_fd(),
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            };
            assert_eq!(
                unsafe { libc::poll(&mut poll, 1, 1500) },
                1,
                "descendant still holds its output pipe"
            );
            assert_eq!(pipe.read(&mut [0; 1]).unwrap(), 0);
            return;
        }
        let mut child = shell("read line; test \"$line\" = input");
        // Force the real read-before-handoff race rather than relying on timing.
        stopped(&child.0, libc::SIGTTIN);
        if case == "signals" {
            // Preserve caller-installed handlers as well as ignored signals.
            // Install after the child stops so its read still tests SIGTTIN.
            unsafe {
                assert_ne!(
                    libc::signal(libc::SIGTTIN, job_control_handler as *const () as usize),
                    libc::SIG_ERR
                );
                assert_ne!(libc::signal(libc::SIGTTOU, libc::SIG_IGN), libc::SIG_ERR);
            }
        }
        let foreground = Foreground::hand_to(0, child.0.id() as _).unwrap().unwrap();
        assert_eq!(unsafe { libc::tcgetpgrp(0) }, child.0.id() as i32);
        if case == "signals" {
            for (signal, expected) in [
                (libc::SIGTTIN, job_control_handler as *const () as usize),
                (libc::SIGTTOU, libc::SIG_IGN),
            ] {
                let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
                assert_eq!(
                    unsafe { libc::sigaction(signal, std::ptr::null(), &mut action) },
                    0
                );
                assert_eq!(action.sa_sigaction, expected, "signal disposition leaked");
            }
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(
            matches!(supervise_harness(&mut child.0, dir.path()).unwrap(), HarnessOutcome::Exited(status) if status.success())
        );
        if case == "drop" {
            drop(foreground);
        } else if case == "unwind" {
            assert!(
                std::panic::catch_unwind(move || {
                    let _foreground = foreground;
                    panic!("synthetic unwind");
                })
                .is_err()
            );
        } else {
            foreground.take_back().unwrap();
        }
        assert_eq!(unsafe { libc::tcgetpgrp(0) }, parent);
    }
}
