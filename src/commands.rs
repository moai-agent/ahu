//! Command implementations.

use std::path::{Path, PathBuf};

use crate::agent::{self, ResolvedAgent};
use crate::bail;
use crate::catalog;
use crate::cmux::Cmux;
use crate::config::{self, LoadedConfig};
use crate::drift;
use crate::git::{self, Repo};
use crate::harness;
use crate::hooks;
use crate::hygiene;
use crate::inventory;
use crate::launch;
use crate::launcher::{self, Console};
use crate::onboard;
use crate::selection::{self, ResolvedPair};
use crate::task;
use crate::util::{Result, display_path, display_safe, display_safe_block};

/// Locate the repository ahu was invoked from.
///
/// Commands take the resolved repository explicitly rather than reading the
/// process working directory themselves, so the whole flow is drivable from a
/// test without changing the current directory.
pub fn repo_from_cwd() -> Result<Repo> {
    let cwd = std::env::current_dir()?;
    git::discover(&cwd)
}

/// Load configuration, or run first-run setup, or explain why it cannot.
fn config_or_setup(repo: &Repo, console: &mut Console<'_>) -> Result<LoadedConfig> {
    if let Some(loaded) = config::load(&repo.root)? {
        return Ok(loaded);
    }
    let Some(new_config) = launcher::run_setup(console)? else {
        bail!("setup was cancelled; nothing was written.");
    };
    let path = config::write_new(&repo.root, &new_config)?;
    console.say(&format!("\nWrote {}\n", path.display()))?;
    console.say(
        "The configuration is in effect now; it does not need to be committed to be used.\n\
         Run `ahu onboard` to see native agent definitions you could register.\n\n",
    )?;
    config::load(&repo.root)?.ok_or_else(|| {
        crate::util::Error::new("configuration disappeared immediately after it was written")
    })
}

/// `ahu init`
pub fn init(console: &mut Console<'_>, repo: &Repo) -> Result<i32> {
    if let Some(loaded) = config::load(&repo.root)? {
        console.say(&format!(
            "ahu is already initialized in this repository.\n  {}\n  policy digest {}\n\
             \nPreferences are edited in that file, not through ahu.\n",
            loaded.path.display(),
            loaded.short_digest()
        ))?;
        return Ok(0);
    }
    let Some(new_config) = launcher::run_setup(console)? else {
        return Ok(1);
    };
    let path = config::write_new(&repo.root, &new_config)?;
    console.say(&format!("\nWrote {}\n", path.display()))?;
    console.say("Nothing else was created, registered, installed, staged, or committed.\n")?;
    Ok(0)
}

/// `ahu agents`
pub fn agents(console: &mut Console<'_>, repo: &Repo) -> Result<i32> {
    let agents = agent::load_all(&repo.root)?;
    if agents.is_empty() {
        console.say(
            "No ahu agents are registered.\n\
             Only .agents/ahu/agents/*.toml makes an agent launchable through ahu; native\n\
             definitions elsewhere are onboarding candidates. Run `ahu onboard` to see them.\n",
        )?;
        return Ok(0);
    }
    for agent in &agents {
        console.say(&format!(
            "@{} {}\n  harness  {}\n  model    {}\n  source   {} [{}]\n  identity {}\n",
            display_safe(&agent.manifest.name),
            display_safe(&agent.manifest.version),
            display_safe(&agent.manifest.harness),
            display_safe(&agent.manifest.model),
            display_safe(
                &agent
                    .source_path
                    .strip_prefix(&repo.root)
                    .unwrap_or(&agent.source_path)
                    .to_string_lossy()
            ),
            agent.manifest.source.format.as_str(),
            &agent.identity_digest()[..12],
        ))?;
        if !agent.manifest.description.is_empty() {
            console.say(&format!(
                "  {}\n",
                display_safe(&agent.manifest.description)
            ))?;
        }
        console.say("\n")?;
    }
    Ok(0)
}

/// `ahu onboard [--register <name> [--model <id>] [--version <v>]] [--remove <name>]`
pub fn onboard_cmd(
    console: &mut Console<'_>,
    repo: &Repo,
    register: Option<&str>,
    remove: Option<&str>,
    model: Option<&str>,
    version: &str,
) -> Result<i32> {
    if let Some(name) = remove {
        let path = onboard::unregister(&repo.root, name)?;
        console.say(&format!(
            "Removed {}\nThe native definition, skills, sessions, and task worktrees are untouched.\n",
            path.display()
        ))?;
        return Ok(0);
    }
    let candidates = onboard::preview(&repo.root)?;
    let Some(name) = register else {
        console.say(&onboard::render(&candidates, &repo.root))?;
        return Ok(0);
    };
    let candidate = candidates.iter().find(|c| c.name == name).ok_or_else(|| {
        crate::util::Error::new(format!("no native definition named {name:?} was found."))
    })?;
    if candidate.already_registered {
        console.say(&format!(
            "{} is already registered; nothing was changed.\n",
            display_safe(name)
        ))?;
        return Ok(0);
    }
    // Refuse before printing anything else about it. `register` checks these
    // too, but it runs after the proposal has already been shown.
    if !candidate.blockers.is_empty() {
        bail!(
            "cannot register {:?}: {}",
            display_safe(name),
            display_safe(&candidate.blockers.join("; "))
        );
    }
    let model = match model.or(candidate.native_model.as_deref()) {
        Some(model) if model != "inherit" && !model.is_empty() => model.to_string(),
        _ => bail!(
            "{name} does not declare a usable model, so ahu needs an explicit one.\n\
             Re-run with --model <exact identifier>. Catalog {} lists: {}.",
            catalog::CATALOG_VERSION,
            catalog::models_for("claude-code")
                .iter()
                .map(|m| m.model)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    console.say("The following file will be created. Nothing else is touched:\n\n")?;
    console.say(&format!(
        ".agents/ahu/agents/{}.toml\n\n",
        display_safe(name)
    ))?;
    console.say(&onboard::proposed_manifest(candidate, &model, version))?;
    if !launcher::confirm(console, "\nCreate it? [y/N]: ")? {
        console.say("Cancelled. Nothing was written.\n")?;
        return Ok(1);
    }
    let path = onboard::register(&repo.root, candidate, &model, version)?;
    console.say(&format!(
        "\nWrote {}\nIt is usable now; ahu did not stage or commit it.\n",
        path.display()
    ))?;
    Ok(0)
}

/// `ahu doctor`
pub fn doctor(console: &mut Console<'_>, repo: &Result<Repo>) -> Result<i32> {
    let mut problems = 0;
    let mut warnings = 0;
    match repo {
        Ok(repo) => {
            console.say(&format!(
                "repository   {}\n  identity   {}\n  group name {}\n  HEAD       {}\n",
                display_path(&repo.root),
                repo.identity(),
                // Derived from a directory name, which ahu does not choose.
                display_safe(&repo.display_name()),
                display_safe(repo.head.as_deref().unwrap_or("(no commits)"))
            ))?;
        }
        Err(e) => {
            problems += 1;
            console.say(&format!(
                "repository   unavailable: {}\n",
                display_safe_block(&e.to_string())
            ))?;
        }
    }

    if let Ok(repo) = repo {
        match config::load(&repo.root) {
            Ok(Some(loaded)) => console.say(&format!(
                "config       {} ({})\n  catalog    {}\n  harnesses  {}\n",
                loaded.path.display(),
                loaded.short_digest(),
                loaded.config.catalog_version,
                loaded.config.harness_preferences.join(", ")
            ))?,
            Ok(None) => console.say("config       not initialized; run `ahu init`\n")?,
            Err(e) => {
                problems += 1;
                console.say(&format!(
                    "config       invalid: {}\n",
                    display_safe_block(&e.to_string())
                ))?;
            }
        }
    }

    if let Ok(repo) = repo {
        match hooks::collect(&repo.root, "claude-code") {
            Ok(found) => {
                console.say(&format!(
                    "hooks        {} visible in Claude Code's settings files, digest {}\n\
                     \x20            ahu reads no other harness's hook configuration\n",
                    found.hooks.len(),
                    found.short_digest()
                ))?;
                for hook in &found.hooks {
                    console.say(&format!(
                        "  {:<12} {} [{}]\n",
                        hook.scope.as_str(),
                        hook.label(),
                        hook.source_label()
                    ))?;
                }
                let outside = found.outside_project_policy();
                if !outside.is_empty() {
                    // A hook outside project policy is a warning, not a blocker:
                    // the launch will succeed, it just will not behave the same
                    // for every teammate.
                    warnings += 1;
                    console.say(&format!(
                        "  {} ({} of them)\n",
                        hooks::NON_PROJECT_HOOK_WARNING,
                        outside.len()
                    ))?;
                    for line in hooks::NON_PROJECT_HOOK_DETAIL {
                        console.say(&format!("  {line}\n"))?;
                    }
                    for hook in &outside {
                        console.say(&format!(
                            "    {} — {}\n",
                            hook.label(),
                            hook.scope.why_not_project_policy()
                        ))?;
                    }
                }
                if found.wrapper_injected {
                    console.say(
                        "  cmux injects its own Claude Code hooks; ahu cannot enumerate them\n",
                    )?;
                }
                for unreadable in &found.unreadable {
                    warnings += 1;
                    console.say(&format!("  unreadable {}\n", display_safe(unreadable)))?;
                }
            }
            Err(e) => {
                problems += 1;
                console.say(&format!(
                    "hooks        could not be read: {}\n",
                    display_safe_block(&e.to_string())
                ))?;
            }
        }
    }

    for harness in catalog::HARNESSES {
        let prerequisite = selection::check_prerequisite(harness.id);
        console.say(&format!(
            "harness      {:<14} adapter {:<13} {}\n",
            harness.id,
            if harness.adapter_available {
                "available"
            } else {
                "not in 0.1.1"
            },
            prerequisite
                .found_at
                .as_deref()
                // Both are outside ahu's control: the path comes from PATH and
                // the version is another program's stdout.
                .map(|p| {
                    format!(
                        "{} {}",
                        display_safe(p),
                        display_safe(prerequisite.version.as_deref().unwrap_or(""))
                    )
                })
                .unwrap_or_else(|| "not installed".to_string())
        ))?;
        for note in &prerequisite.notes {
            console.say(&format!("               note: {}\n", display_safe(note)))?;
        }
        if harness.adapter_available && !harness.enforces_model_for_session {
            console.say(&format!(
                "               {}\n",
                harness::RELIABILITY_WARNING
            ))?;
            for gap in harness.enforcement_gaps {
                console.say(&format!("               - {gap}\n"))?;
            }
        }
    }

    match Cmux::discover() {
        Ok(client) => {
            console.say("cmux         reachable\n")?;
            match client.check_capabilities() {
                Ok(_) => console.say("  groups     supported\n")?,
                Err(e) => {
                    problems += 1;
                    console.say(&format!(
                        "  groups     {}\n",
                        display_safe_block(&e.to_string())
                    ))?;
                }
            }
        }
        Err(e) => {
            problems += 1;
            console.say(&format!(
                "cmux         {}\n",
                display_safe_block(&e.to_string())
            ))?;
        }
    }

    console.say(&format!(
        "state        {}\n",
        crate::state::root()?.display()
    ))?;
    let summary = match (problems, warnings) {
        (0, 0) => "\nNo blocking problems found.\n".to_string(),
        (0, w) => format!(
            "\nNo blocking problems found. {w} warning(s) above affect behaviour but do not stop a launch.\n"
        ),
        (p, 0) => format!("\n{p} problem(s) would block a launch.\n"),
        (p, w) => format!(
            "\n{p} problem(s) would block a launch, and {w} warning(s) affect behaviour without stopping one.\n"
        ),
    };
    console.say(&summary)?;
    if problems == 0 { Ok(0) } else { Ok(1) }
}

/// The one sentence `ahu tasks` may print only when it really found nothing.
///
/// Named so a test can assert on its absence: for five refused records, five
/// worktrees and three live cmux sessions, this is not a reassurance, it is a
/// false statement — and the surface that should have told the user where their
/// leftover worktrees are is the one that denied they existed.
pub const NO_TASKS: &str = "No ahu tasks have been launched from this repository.";

/// `ahu tasks`
pub fn tasks(console: &mut Console<'_>, repo: &Repo) -> Result<i32> {
    let listing = launch::reconcile(&repo.identity())?;
    if listing.is_empty() {
        console.say(&format!("{NO_TASKS}\n"))?;
        return Ok(0);
    }
    for (dir, record) in &listing.records {
        // Every field here comes out of task.json, which was built from the
        // prompt and from repository configuration. `tasks` is as much a
        // disclosure surface as the launch preview, so it escapes the same way.
        console.say(&format!(
            "{} [{}] {}\n  agent     {}\n  harness   {} / {}\n  branch    {}\n  worktree  {}\n  record    {}\n",
            display_safe(&record.task_id),
            record.state.as_str(),
            display_safe(&record.title),
            display_safe(&record.agent_label()),
            display_safe(&record.identity.harness),
            display_safe(&record.identity.model),
            display_safe(&record.branch),
            display_path(&record.worktree),
            display_path(dir),
        ))?;
        if let Some(workspace) = &record.cmux_workspace_id {
            console.say(&format!("  cmux      {}\n", display_safe(workspace)))?;
        }
        if record.reliability_warning.is_some() {
            console.say(&format!("  warning   {}\n", harness::RELIABILITY_WARNING))?;
        }
        console.say("\n")?;
    }
    console.say(&render_unreadable_tasks(repo, &listing.unreadable))?;
    if !listing.records.is_empty() {
        // This footer explains the `exited` state in the listing above. With no
        // readable records there is no such listing for it to explain.
        console.say(
            "\n`exited` means the harness process ended. It is not a claim that the task \
             succeeded.\n",
        )?;
    }
    console.say("Worktrees and branches are kept until you remove them yourself.\n")?;
    Ok(0)
}

/// Report the task directories ahu could not read.
///
/// The user's actual problem with an unreadable record is a leftover worktree
/// and branch they can no longer enumerate through ahu, so this recovers what it
/// can — the task id is the directory name, the worktree path is derived from
/// it, and the branch is looked up in Git — and says plainly when it cannot.
///
/// Nothing is read out of the refused file. Reinterpreting a record whose schema
/// ahu does not understand is exactly what the schema check exists to prevent,
/// and it would be a strange fix for a disclosure bug to introduce one.
pub fn render_unreadable_tasks(repo: &Repo, unreadable: &[task::UnreadableTask]) -> String {
    if unreadable.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "\n!! {} task record(s) in this repository could not be read.\n\
         \x20  They are not listed above, and they are not gone: each one had a worktree and a\n\
         \x20  branch, and ahu does not delete either.\n",
        unreadable.len()
    ));

    // Grouped by reason. On a real machine every old record fails for the same
    // reason, and repeating a four-line explanation once per task buries the
    // task ids -- which are the part the reader needs -- in its own boilerplate.
    let mut groups: Vec<(String, Vec<&task::UnreadableTask>)> = Vec::new();
    for found in unreadable {
        let reason = strip_record_path(&found.reason, &found.dir);
        match groups.iter_mut().find(|(seen, _)| *seen == reason) {
            Some((_, members)) => members.push(found),
            None => groups.push((reason, vec![found])),
        }
    }

    let mut any_unrecovered = false;
    for (reason, members) in &groups {
        out.push_str(&format!(
            "\n   {}\n",
            display_safe_block(reason).replace('\n', "\n   ")
        ));
        for found in members {
            out.push_str(&format!(
                "\n     {} [unreadable]\n",
                display_safe(&found.task_id)
            ));
            out.push_str(&format!("       record    {}\n", display_path(&found.dir)));

            // The directory name is the task id ahu chose, so the worktree path
            // is recoverable without touching the record.
            match crate::state::worktree_dir(&repo.root, &found.task_id) {
                Ok(worktree) if worktree.is_dir() => {
                    out.push_str(&format!("       worktree  {}\n", display_path(&worktree)));
                }
                Ok(worktree) => {
                    out.push_str(&format!(
                        "       worktree  {} (no longer on disk)\n",
                        display_path(&worktree)
                    ));
                }
                Err(_) => {
                    any_unrecovered = true;
                    out.push_str("       worktree  could not be derived from the task id\n");
                }
            }
            match git::branches_matching(repo, &format!("ahu/*/{}", found.task_id)) {
                Ok(branches) if !branches.is_empty() => {
                    out.push_str(&format!(
                        "       branch    {}\n",
                        display_safe(&branches.join(", "))
                    ));
                }
                _ => {
                    any_unrecovered = true;
                    out.push_str("       branch    none found for this task id\n");
                }
            }
        }
    }

    out.push_str(
        "\n   ahu will not rewrite or migrate a record it cannot read: these files are the\n\
         \x20  audit trail of what actually ran. Re-submit the work as a new task rather than\n\
         \x20  trying to resume one of these.\n",
    );
    if any_unrecovered {
        out.push_str(
            "   For anything ahu could not recover above, `git worktree list` and the\n\
             \x20  .worktrees/ directory in this repository are the authoritative listing.\n",
        );
    }
    out
}

/// Drop the leading `<record path>: ` or `<record path> ` a loader message
/// starts with, since the path is printed on its own line beside it.
fn strip_record_path(reason: &str, dir: &Path) -> String {
    let record = dir.join("task.json");
    let prefix = record.to_string_lossy().to_string();
    reason
        .strip_prefix(&prefix)
        .map(|rest| rest.trim_start_matches([':', ' ']).to_string())
        .unwrap_or_else(|| reason.to_string())
}

/// `ahu focus <task-id>`
pub fn focus(console: &mut Console<'_>, repo: &Repo, task_id: &str) -> Result<i32> {
    let listing = task::list(&repo.identity())?;
    let found = listing
        .records
        .iter()
        .find(|(_, r)| r.task_id == task_id || r.task_id.starts_with(task_id));
    let Some((_, record)) = found else {
        // "no task matching" would be a claim that nothing here is that task.
        // If a directory with that id exists and ahu simply could not read its
        // record, saying so is the difference between a user looking for a
        // typo and a user looking at a leftover worktree.
        let unreadable: Vec<&task::UnreadableTask> = listing
            .unreadable
            .iter()
            .filter(|u| u.task_id == task_id || u.task_id.starts_with(task_id))
            .collect();
        if let Some(blocked) = unreadable.first() {
            bail!(
                "task {} exists but ahu cannot read its record, so it cannot find its cmux \
                 session.\n{}\nThe record is at {}. Run `ahu tasks` for its worktree and branch.",
                display_safe(&blocked.task_id),
                display_safe_block(&blocked.reason),
                display_path(&blocked.dir)
            );
        }
        if listing.unreadable.is_empty() {
            bail!("no task matching {task_id:?}.");
        }
        bail!(
            "no readable task matching {task_id:?}. {} other task record(s) in this repository \
             could not be read either; run `ahu tasks` to see them.",
            listing.unreadable.len()
        );
    };
    let Some(workspace) = record.cmux_workspace_id.as_deref() else {
        bail!("task {} has no recorded cmux session.", record.task_id);
    };
    let client = Cmux::discover()?;
    client.select_workspace(workspace)?;
    console.say(&format!(
        "Focused {} — {}\n  worktree {}\n",
        display_safe(&record.agent_label()),
        display_safe(&record.title),
        display_path(&record.worktree)
    ))?;
    Ok(0)
}

/// `ahu inventory [@agent]`
pub fn inventory_cmd(
    console: &mut Console<'_>,
    repo: &Repo,
    agent_name: Option<&str>,
) -> Result<i32> {
    let Some(loaded) = config::load(&repo.root)? else {
        bail!("this repository is not initialized. Run `ahu init` first.");
    };
    let snapshot = crate::snapshot::collect(&repo.root)?;
    let (resolved, pair) = resolve_identity(repo, &loaded, agent_name)?;
    let adapter = harness::adapter_for(&pair.harness)?;
    // Same permissions the launch would use, so this report describes the same
    // flags a launch of this identity would actually pass.
    let permissions = resolved
        .as_ref()
        .map(|a| a.manifest.permissions)
        .unwrap_or_default();
    let enforcement = adapter.enforcement(&pair.model, permissions)?;
    let found_hooks = hooks::collect(&repo.root, &pair.harness)?;
    let built = inventory::build(&inventory::Subject {
        repo_root: &repo.root,
        loaded_config: &loaded,
        snapshot: &snapshot,
        agent: resolved.as_ref(),
        harness: &pair.harness,
        model: &pair.model,
        enforcement: &enforcement,
        hooks: &found_hooks,
        prompt: None,
    })?;
    console.say(&inventory::render(&built))?;
    Ok(0)
}

/// `ahu hygiene [@agent]`
pub fn hygiene_cmd(
    console: &mut Console<'_>,
    repo: &Repo,
    agent_name: Option<&str>,
) -> Result<i32> {
    let Some(loaded) = config::load(&repo.root)? else {
        bail!("this repository is not initialized. Run `ahu init` first.");
    };
    let snapshot = crate::snapshot::collect(&repo.root)?;
    let (resolved, pair) = resolve_identity(repo, &loaded, agent_name)?;
    let adapter = harness::adapter_for(&pair.harness)?;
    // Same permissions the launch would use, so this report describes the same
    // flags a launch of this identity would actually pass.
    let permissions = resolved
        .as_ref()
        .map(|a| a.manifest.permissions)
        .unwrap_or_default();
    let enforcement = adapter.enforcement(&pair.model, permissions)?;
    let found_hooks = hooks::collect(&repo.root, &pair.harness)?;
    let built = inventory::build(&inventory::Subject {
        repo_root: &repo.root,
        loaded_config: &loaded,
        snapshot: &snapshot,
        agent: resolved.as_ref(),
        harness: &pair.harness,
        model: &pair.model,
        enforcement: &enforcement,
        hooks: &found_hooks,
        prompt: None,
    })?;
    let key = resolved
        .as_ref()
        .map(|a| a.label())
        .unwrap_or_else(|| "auto".to_string());
    let identity = repo.identity();
    let review_state = hygiene::load_state(&identity)?;
    let review = hygiene::review(&key, &built, &enforcement, &loaded, &review_state);
    console.say(&hygiene::render(
        &review,
        hygiene::Trigger::Requested,
        &loaded,
    ))?;
    hygiene::record_review(&identity, &key)?;
    Ok(0)
}

/// `ahu run-task --task-dir <dir>` — the fixed entrypoint cmux starts.
pub fn run_task(task_dir: &Path) -> Result<i32> {
    let status = launch::run_task(task_dir)?;
    Ok(status.code().unwrap_or(1))
}

/// Resolve a launch identity from an optional `@name`.
fn resolve_identity(
    repo: &Repo,
    loaded: &LoadedConfig,
    agent_name: Option<&str>,
) -> Result<(Option<ResolvedAgent>, ResolvedPair)> {
    match agent_name {
        Some(name) => {
            let resolved = agent::find(&repo.root, name)?;
            let pair = ResolvedPair {
                harness: resolved.manifest.harness.clone(),
                model: resolved.manifest.model.clone(),
                basis: format!(
                    "named agent @{} {} pins this harness and model",
                    resolved.manifest.name, resolved.manifest.version
                ),
                policy_digest: loaded.digest.clone(),
                catalog_version: loaded.config.catalog_version.clone(),
            };
            Ok((Some(resolved), pair))
        }
        None => Ok((None, selection::resolve_automatic(loaded)?)),
    }
}

/// The default `ahu` flow: setup if needed, then select, compose, preview, submit.
pub fn interactive(console: &mut Console<'_>, repo: &Repo, focus_new: bool) -> Result<i32> {
    let loaded = config_or_setup(repo, console)?;

    console.say(&format!(
        "ahu {} — {}\n  policy {} · catalog {}\n\n",
        env!("CARGO_PKG_VERSION"),
        display_path(&repo.root),
        loaded.short_digest(),
        loaded.config.catalog_version
    ))?;

    let registered = agent::load_all(&repo.root)?;
    let selector = launcher::read_selector(console, &registered)?;
    let (resolved, pair) = resolve_identity(repo, &loaded, selector.as_deref())?;

    // The resolved pair is shown before the prompt is entered, and again in the
    // submission preview, so the provider is never a surprise.
    console.say(&format!(
        "\nResolved for this task:\n  agent   {}\n  harness {}\n  model   {}\n  because {}\n",
        display_safe(
            &resolved
                .as_ref()
                .map(|a| a.label())
                .unwrap_or_else(|| "auto (no named agent)".to_string())
        ),
        display_safe(&pair.harness),
        display_safe(&pair.model),
        display_safe(&pair.basis)
    ))?;
    let prerequisite = selection::check_prerequisite(&pair.harness);
    if !prerequisite.satisfied() {
        bail!(
            "{} is not installed on this machine ({} not found on PATH).\n\
             This is a diagnostic for your machine, not a reason to select a different \
             harness: the project's policy is the same for everyone.",
            pair.harness,
            prerequisite.executable
        );
    }
    for note in &prerequisite.notes {
        console.say(&format!("  note    {note}\n"))?;
    }

    let Some(prompt) = launcher::read_prompt(console)? else {
        console.say("Cancelled. Nothing was created.\n")?;
        return Ok(1);
    };

    submit(
        console, repo, &loaded, resolved, pair, &prompt, true, false, focus_new,
    )
}

/// Assign work without an interactive composer; the command itself requests launch.
///
/// This path has no human at a terminal — the preview goes to a pipe read by
/// another agent — so the interactive confirmation cannot be the gate on
/// approval widening. `--allow-widened-approvals` is that gate instead: it makes
/// the widening appear verbatim in the command line the delegating harness shows
/// its own user before running it, and it is recorded on the task.
pub fn launch_cmd(
    console: &mut Console<'_>,
    repo: &Repo,
    agent: &str,
    prompt_file: &Path,
    dry_run: bool,
    allow_widened_approvals: bool,
) -> Result<i32> {
    let loaded = config::load(&repo.root)?.ok_or_else(|| {
        crate::util::Error::new(
            "project configuration is missing; run ahu init before assigning work.",
        )
    })?;
    let (resolved, pair) = resolve_identity(repo, &loaded, Some(agent))?;
    let permissions = resolved
        .as_ref()
        .map(|a| a.manifest.permissions)
        .unwrap_or_default();
    if permissions.widens_defaults() && !allow_widened_approvals {
        bail!(
            "@{} runs with permissions = {}, which widens the harness's own approval boundary:\n  \
             {}\n\
             `ahu launch` starts a session with no interactive confirmation, so it will not widen \
             approvals on your behalf.\n\
             Re-run with --allow-widened-approvals if that is what you intend. Passing it puts \
             the widening in the command line your own harness shows you before it runs, and \
             records it on the task.",
            display_safe(agent),
            permissions.as_str(),
            permissions.disclosure()
        );
    }
    let prompt = std::fs::read_to_string(prompt_file).map_err(|e| {
        crate::util::Error::new(format!(
            "cannot read prompt file {}: {e}",
            prompt_file.display()
        ))
    })?;
    submit(
        console, repo, &loaded, resolved, pair, &prompt, false, dry_run, false,
    )
}

#[allow(clippy::too_many_arguments)]
fn submit(
    console: &mut Console<'_>,
    repo: &Repo,
    loaded: &LoadedConfig,
    resolved: Option<ResolvedAgent>,
    pair: ResolvedPair,
    prompt: &str,
    confirm: bool,
    dry_run: bool,
    focus_new: bool,
) -> Result<i32> {
    let plan = launch::plan(repo, resolved.clone(), pair.clone(), prompt)?;

    // First-load and overdue context hygiene review, before submission.
    let identity = repo.identity();
    let key = plan.agent_label();
    let review_state = hygiene::load_state(&identity)?;
    let trigger = hygiene::due(loaded, &review_state, &key);
    if trigger != hygiene::Trigger::NotDue {
        let built = inventory::build(&inventory::Subject {
            repo_root: &repo.root,
            loaded_config: loaded,
            snapshot: &plan.snapshot,
            agent: plan.agent.as_ref(),
            harness: &plan.pair.harness,
            model: &plan.pair.model,
            enforcement: &plan.enforcement,
            hooks: &plan.hooks,
            prompt: Some(prompt),
        })?;
        let review = hygiene::review(&key, &built, &plan.enforcement, loaded, &review_state);
        console.say("\n")?;
        console.say(&hygiene::render(&review, trigger, loaded))?;
        if !dry_run {
            hygiene::record_review(&identity, &key)?;
        }
    }

    // Drift against the last launch of this same agent at this same version.
    let previous = task::list(&identity)?;
    // Drift can only compare against records it can read. Saying nothing when
    // some are unreadable would make "no drift reported" mean two different
    // things — nothing changed, or ahu could not look — and only one of those
    // is a reason to go ahead.
    if !previous.unreadable.is_empty() {
        console.say(&format!(
            "\n!! {} earlier task record(s) for this repository could not be read, so drift was \n\
             \x20  not compared against them. `ahu tasks` lists them.\n",
            previous.unreadable.len()
        ))?;
    }
    let agent_identity = plan.agent.as_ref().map(|a| a.identity_digest());
    if let Some(found) = drift::detect(
        &key,
        plan.agent
            .as_ref()
            .zip(agent_identity.as_deref())
            .map(|(a, identity)| drift::AgentDigests {
                identity,
                source: &a.source_digest,
                instructions: &a.instructions_digest,
            }),
        &plan.snapshot.digest(),
        &loaded.digest,
        &plan.hooks.digest(),
        &previous.records,
    ) {
        console.say("\n")?;
        console.say(&drift::render(&found))?;
    }

    // Generated here, after the prompt has been read and after the plan is
    // built, so nothing in the prompt can have contained it.
    let code = confirmation_code();
    console.say(&render_preview(
        repo,
        &plan,
        prompt,
        confirm.then_some(code.as_str()),
    ))?;

    if dry_run {
        console.say("Dry run. No task or session was created.\n")?;
        return Ok(0);
    }
    if confirm && !launcher::confirm_submit(console, &code)? {
        console.say("Cancelled. No worktree, branch, or session was created.\n")?;
        return Ok(1);
    }

    let launched = launch::execute(repo, loaded, &plan, prompt, focus_new)?;
    console.say(&format!(
        "\nLaunched {} — {}\n  task     {}\n  branch   {}\n  worktree {}\n  record   {}\n",
        display_safe(&launched.record.agent_label()),
        display_safe(&launched.record.title),
        display_safe(&launched.record.task_id),
        display_safe(&launched.record.branch),
        display_path(&launched.record.worktree),
        display_path(&launched.task_dir),
    ))?;
    if let Some(workspace) = &launched.record.cmux_workspace_id {
        console.say(&format!("  cmux     {workspace}\n"))?;
    }
    console.say(&render_launch_notes(&launched.notes))?;
    Ok(0)
}

/// The launch summary's note lines.
///
/// Notes carry repository-relative configuration paths verbatim — a
/// concurrently modified file is named in one — so they are escaped like every
/// other repository-derived string ahu prints, not trusted because they were
/// assembled by ahu's own code.
pub fn render_launch_notes(notes: &[String]) -> String {
    let mut out = String::new();
    for note in notes {
        out.push_str(&format!("  note     {}\n", display_safe(note)));
    }
    out
}

/// A short confirmation code for one submission.
///
/// Six hex characters: enough that a prompt written before the launch cannot
/// contain it, short enough to retype.
fn confirmation_code() -> String {
    crate::orchestration::new_nonce()[..6].to_string()
}

/// The submission preview: identity, Git effects, and every warning.
///
/// `confirmation_code` is `Some` only when a confirmation will actually be asked
/// for, so the preview never shows a code nothing will read.
pub fn render_preview(
    repo: &Repo,
    plan: &launch::LaunchPlan,
    prompt: &str,
    confirmation_code: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("\nAbout to submit\n===============\n");
    out.push_str(
        "  delegation All assigned agents must launch through ahu in separate cmux sessions.\n",
    );
    out.push_str(&format!(
        "  guidance   ahu supplies its delegation contract and this agent's instructions as {}\n",
        // Not "Claude Agent/Task/TeamCreate tools are denied": that was printed
        // unconditionally, on Codex and Antigravity launches where no such flag
        // was passed and no Claude was involved, twenty-five lines above the
        // gaps list that contradicted it. ahu now denies no tool on any harness,
        // and this line says what it does instead.
        "prompt text. No harness denies its own delegation tools for this launch."
    ));
    out.push_str(&format!(
        "  agent      {}\n",
        display_safe(&plan.agent_label())
    ));
    out.push_str(&format!("  harness    {}\n", plan.pair.harness));
    out.push_str(&format!("  model      {}\n", plan.pair.model));
    out.push_str(&format!(
        "  because    {}\n",
        display_safe(&plan.pair.basis)
    ));
    out.push_str(&format!(
        "  policy     {} · catalog {}\n",
        &plan.pair.policy_digest[..12],
        plan.pair.catalog_version
    ));
    if let Some(agent) = &plan.agent {
        // This attribution is now checkable rather than asserted: ahu delivers
        // the instruction text it parsed out of exactly this file, so there is
        // no name for a harness to resolve somewhere else.
        //
        // Two digests, each labelled. One value could only ever be right about
        // one of the two questions a reader has — "is this the file I reviewed?"
        // and "is that what the model was given?" — and a reader must never have
        // to work out which one a bare digest answers.
        out.push_str(&format!(
            "  agent src  {}\n",
            display_safe(
                &agent
                    .source_path
                    .strip_prefix(&repo.root)
                    .unwrap_or(&agent.source_path)
                    .to_string_lossy()
            ),
        ));
        out.push_str(&format!(
            "             file digest         {} (the whole file as it is on disk)\n",
            &agent.source_digest[..12]
        ));
        out.push_str(&format!(
            "             instructions digest {} ({})\n",
            &agent.instructions_digest[..12],
            if agent.manifest.source.format.has_frontmatter() {
                "the body ahu delivers, YAML frontmatter read as metadata and not delivered"
            } else {
                "the text ahu delivers; this format has no frontmatter, so it is the whole file"
            }
        ));
    }
    out.push_str(&format!("  title      {}\n", display_safe(&plan.title)));
    out.push_str(&format!(
        "  prompt     {} line(s), {} character(s)\n",
        prompt.lines().count(),
        prompt.chars().count()
    ));
    out.push_str(&format!("  branch     {}\n", display_safe(&plan.branch)));
    out.push_str(&format!("  worktree   {}\n", display_path(&plan.worktree)));
    out.push_str(&format!(
        "  base       {}\n",
        plan.base_commit.as_deref().unwrap_or("(none)")
    ));
    out.push_str(&format!(
        "  config     {} file(s), snapshot {}\n",
        plan.snapshot.entries.len(),
        plan.snapshot.short_digest()
    ));
    out.push_str(&format!(
        "  hooks      {} visible, digest {}\n",
        plan.hooks.hooks.len(),
        plan.hooks.short_digest()
    ));
    out.push_str(
        "\nThe task worktree starts at the base commit above and then receives this checkout's\n\
         complete agent configuration as it stands right now, including uncommitted and ignored\n\
         files, at their native paths.\n",
    );
    if plan.parent_dirty {
        out.push_str(
            "This checkout has uncommitted changes. Unrelated source changes stay here; they are\n\
             not copied into the task worktree. Nothing is staged, committed, or stashed.\n",
        );
    }
    if !plan.snapshot.skipped_directories.is_empty() {
        // Not "not inherited": the worktree is a checkout of the base commit,
        // so committed files under these paths are in it either way. What the
        // scan skipped is the inventory, not the inheritance.
        out.push_str(&format!(
            "Not scanned, so not inventoried; committed files under these paths are still present\n\
             in the task worktree: {}\n",
            display_safe(&plan.snapshot.skipped_directories.join(", "))
        ));
    }
    if !plan.snapshot.unscanned_config.is_empty() {
        out.push_str(&format!(
            "Agent configuration found directly inside them, carried in by the checkout and not\n\
             inventoried: {}\n",
            display_safe(&plan.snapshot.unscanned_config.join(", "))
        ));
    }
    if !plan.snapshot.symlinks.is_empty() {
        // `collect` never follows one and `materialize` deletes any the base
        // commit put in the worktree, so this configuration is neither
        // inherited nor counted in `config N file(s)` above. Saying nothing
        // left the reader to infer it from a number that silently excluded it.
        out.push_str(&format!(
            "Configuration symlinks, not followed and not inherited; ahu also removes them from\n\
             the task worktree: {}\n",
            display_safe(&plan.snapshot.symlinks.join(", "))
        ));
    }
    out.push_str(&hooks::render_for_preview(
        &plan.hooks,
        plan.snapshot.executable_count(),
    ));

    // A widened approval boundary is the most consequential thing in a launch,
    // so it is stated before the enforcement list, not buried in it.
    if plan.permissions.widens_defaults() {
        out.push_str(&format!(
            "\nApprovals\n  !! This agent runs with permissions = {}.\n     {}\n     \
             ahu passes the harness's own flag for this because the agent's manifest asks for it. \
             It is a committed, reviewable field, not an ahu default.\n",
            plan.permissions.as_str(),
            plan.permissions.disclosure()
        ));
    } else {
        out.push_str(&format!(
            "\nApprovals\n  {}\n",
            display_safe(plan.permissions.disclosure())
        ));
    }
    // The flags ahu passes are not the approval boundary. The harness's own
    // settings files are, and this repository carries some of them into the task
    // worktree, so they are printed here rather than left to the file count.
    out.push_str(&hooks::render_settings_for_preview(&plan.hooks));

    out.push_str("\nEnforcement\n");
    for control in &plan.enforcement.applied_controls {
        out.push_str(&format!("  + {}\n", display_safe(control)));
    }
    // Gaps print unconditionally. They used to appear only under the reliability
    // warning, which made the single most important sentence about a launch --
    // that nothing ahu supplies is enforced -- conditional on an unrelated flag.
    for gap in &plan.enforcement.gaps {
        out.push_str(&format!("  - {}\n", display_safe(gap)));
    }
    if let Some(warning) = plan.reliability_warning() {
        out.push_str(&format!("\n  !! {warning}\n"));
        out.push_str(
            "     ahu still pins the configured harness and model and never substitutes another.\n\
             \x20    This limitation is recorded in the task metadata and the context inventory.\n",
        );
    }
    out.push_str(&format!(
        "\nCommand to be run in the worktree (the prompt is one argument, never shell input):\n  {} {}\n",
        display_path(&plan.harness_executable),
        plan.command
            .redacted()
            .args
            .iter()
            .map(|a| display_safe(a))
            .collect::<Vec<_>>()
            .join(" ")
    ));
    out.push_str(&format!(
        "The redacted slot above holds {}.\n",
        crate::orchestration::delivery_summary(
            &plan.delivery.nonce,
            plan.delivery.agent_instructions.is_some()
        )
    ));
    out.push_str(
        "The harness is resolved from PATH by name in the task workspace, and only from an\n\
         absolute PATH entry outside this repository. The path above is what PATH resolves to\n\
         here; under cmux the workspace may resolve a different per-surface wrapper of the same\n\
         harness.\n",
    );
    if let Some(code) = confirmation_code {
        out.push_str(&format!(
            "\nConfirmation code for this submission: {code}\n\
             It was generated after your prompt was read, so no pasted text can have supplied it.\n",
        ));
    }
    out
}

/// Wire the standard streams into a console.
pub fn with_stdio<T>(f: impl FnOnce(&mut Console<'_>) -> Result<T>) -> Result<T> {
    let stdin = std::io::stdin();
    let mut locked = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut console = launcher::stdio_console(&mut locked, &mut out);
    f(&mut console)
}

/// Every task directory `ahu tasks` accounts for, exposed for tests.
///
/// Includes the ones whose records could not be read: they are still tasks that
/// were launched, and a helper that silently omitted them would reproduce the
/// bug this listing exists to fix.
pub fn task_dirs(repo_identity: &str) -> Result<Vec<PathBuf>> {
    Ok(task::list(repo_identity)?.dirs())
}
