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
use crate::util::{Result, display_safe};

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
            "{name} is already registered; nothing was changed.\n"
        ))?;
        return Ok(0);
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
    console.say(&format!(".agents/ahu/agents/{name}.toml\n\n"))?;
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
                repo.root.display(),
                repo.identity(),
                repo.display_name(),
                repo.head.as_deref().unwrap_or("(no commits)")
            ))?;
        }
        Err(e) => {
            problems += 1;
            console.say(&format!("repository   unavailable: {e}\n"))?;
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
                console.say(&format!("config       invalid: {e}\n"))?;
            }
        }
    }

    if let Ok(repo) = repo {
        match hooks::collect(&repo.root) {
            Ok(found) => {
                console.say(&format!(
                    "hooks        {} visible, digest {}\n",
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
                console.say(&format!("hooks        could not be read: {e}\n"))?;
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
                .map(|p| format!("{p} {}", prerequisite.version.as_deref().unwrap_or("")))
                .unwrap_or_else(|| "not installed".to_string())
        ))?;
        for note in &prerequisite.notes {
            console.say(&format!("               note: {note}\n"))?;
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
                    console.say(&format!("  groups     {e}\n"))?;
                }
            }
        }
        Err(e) => {
            problems += 1;
            console.say(&format!("cmux         {e}\n"))?;
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

/// `ahu tasks`
pub fn tasks(console: &mut Console<'_>, repo: &Repo) -> Result<i32> {
    let records = launch::reconcile(&repo.identity())?;
    if records.is_empty() {
        console.say("No ahu tasks have been launched from this repository.\n")?;
        return Ok(0);
    }
    for (dir, record) in &records {
        console.say(&format!(
            "{} [{}] {}\n  agent     {}\n  harness   {} / {}\n  branch    {}\n  worktree  {}\n  record    {}\n",
            record.task_id,
            record.state.as_str(),
            record.title,
            record.agent_label(),
            record.identity.harness,
            record.identity.model,
            record.branch,
            record.worktree.display(),
            dir.display(),
        ))?;
        if let Some(workspace) = &record.cmux_workspace_id {
            console.say(&format!("  cmux      {workspace}\n"))?;
        }
        if record.reliability_warning.is_some() {
            console.say(&format!("  warning   {}\n", harness::RELIABILITY_WARNING))?;
        }
        console.say("\n")?;
    }
    console.say(
        "`exited` means the harness process ended. It is not a claim that the task succeeded.\n\
         Worktrees and branches are kept until you remove them yourself.\n",
    )?;
    Ok(0)
}

/// `ahu focus <task-id>`
pub fn focus(console: &mut Console<'_>, repo: &Repo, task_id: &str) -> Result<i32> {
    let records = task::list(&repo.identity())?;
    let (_, record) = records
        .iter()
        .find(|(_, r)| r.task_id == task_id || r.task_id.starts_with(task_id))
        .ok_or_else(|| crate::util::Error::new(format!("no task matching {task_id:?}.")))?;
    let Some(workspace) = record.cmux_workspace_id.as_deref() else {
        bail!("task {} has no recorded cmux session.", record.task_id);
    };
    let client = Cmux::discover()?;
    client.select_workspace(workspace)?;
    console.say(&format!(
        "Focused {} — {}\n  worktree {}\n",
        record.agent_label(),
        record.title,
        record.worktree.display()
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
    let enforcement = adapter.enforcement(&pair.model);
    let found_hooks = hooks::collect(&repo.root)?;
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
    let enforcement = adapter.enforcement(&pair.model);
    let found_hooks = hooks::collect(&repo.root)?;
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
        repo.root.display(),
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

    let plan = launch::plan(repo, resolved.clone(), pair.clone(), &prompt)?;

    // First-load and overdue context hygiene review, before submission.
    let identity = repo.identity();
    let key = plan.agent_label();
    let review_state = hygiene::load_state(&identity)?;
    let trigger = hygiene::due(&loaded, &review_state, &key);
    if trigger != hygiene::Trigger::NotDue {
        let built = inventory::build(&inventory::Subject {
            repo_root: &repo.root,
            loaded_config: &loaded,
            snapshot: &plan.snapshot,
            agent: plan.agent.as_ref(),
            harness: &plan.pair.harness,
            model: &plan.pair.model,
            enforcement: &plan.enforcement,
            hooks: &plan.hooks,
            prompt: Some(&prompt),
        })?;
        let review = hygiene::review(&key, &built, &plan.enforcement, &loaded, &review_state);
        console.say("\n")?;
        console.say(&hygiene::render(&review, trigger, &loaded))?;
        hygiene::record_review(&identity, &key)?;
    }

    // Drift against the last launch of this same agent at this same version.
    let previous = task::list(&identity)?;
    if let Some(found) = drift::detect(
        &key,
        plan.agent.as_ref().map(|a| a.identity_digest()).as_deref(),
        &plan.snapshot.digest(),
        &loaded.digest,
        &plan.hooks.digest(),
        &previous,
    ) {
        console.say("\n")?;
        console.say(&drift::render(&found))?;
    }

    console.say(&render_preview(repo, &plan, &prompt))?;

    if !launcher::confirm_submit(console)? {
        console.say("Cancelled. No worktree, branch, or session was created.\n")?;
        return Ok(1);
    }

    let launched = launch::execute(repo, &loaded, &plan, &prompt, focus_new)?;
    console.say(&format!(
        "\nLaunched {} — {}\n  task     {}\n  branch   {}\n  worktree {}\n  record   {}\n",
        launched.record.agent_label(),
        launched.record.title,
        launched.record.task_id,
        launched.record.branch,
        launched.record.worktree.display(),
        launched.task_dir.display(),
    ))?;
    if let Some(workspace) = &launched.record.cmux_workspace_id {
        console.say(&format!("  cmux     {workspace}\n"))?;
    }
    for note in &launched.notes {
        console.say(&format!("  note     {note}\n"))?;
    }
    Ok(0)
}

/// The submission preview: identity, Git effects, and every warning.
pub fn render_preview(repo: &Repo, plan: &launch::LaunchPlan, prompt: &str) -> String {
    let mut out = String::new();
    out.push_str("\nAbout to submit\n===============\n");
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
        out.push_str(&format!(
            "  prompt src {} ({})\n",
            display_safe(
                &agent
                    .source_path
                    .strip_prefix(&repo.root)
                    .unwrap_or(&agent.source_path)
                    .to_string_lossy()
            ),
            &agent.source_digest[..12]
        ));
    }
    out.push_str(&format!("  title      {}\n", display_safe(&plan.title)));
    out.push_str(&format!(
        "  prompt     {} line(s), {} character(s)\n",
        prompt.lines().count(),
        prompt.chars().count()
    ));
    out.push_str(&format!("  branch     {}\n", display_safe(&plan.branch)));
    out.push_str(&format!("  worktree   {}\n", plan.worktree.display()));
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
        out.push_str(&format!(
            "Not scanned, so not inherited: {}\n",
            display_safe(&plan.snapshot.skipped_directories.join(", "))
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
            "\nApprovals\n  {} \n",
            plan.permissions.disclosure()
        ));
    }

    out.push_str("\nEnforcement\n");
    for control in &plan.enforcement.applied_controls {
        out.push_str(&format!("  + {control}\n"));
    }
    if let Some(warning) = plan.reliability_warning() {
        out.push_str(&format!("\n  !! {warning}\n"));
        for gap in &plan.enforcement.gaps {
            out.push_str(&format!("     - {gap}\n"));
        }
        out.push_str(
            "     ahu still requests the configured identity and never substitutes another.\n\
             \x20    This limitation is recorded in the task metadata and the context inventory.\n",
        );
    }
    out.push_str(&format!(
        "\nCommand to be run in the worktree (the prompt is one argument, never shell input):\n  {} {}\n",
        plan.harness_executable.display(),
        plan.command
            .redacted()
            .args
            .iter()
            .map(|a| display_safe(a))
            .collect::<Vec<_>>()
            .join(" ")
    ));
    out.push_str(
        "The harness is resolved from PATH by name in the task workspace, and its name is checked\n\
         against the adapter's. The path above is what PATH resolves to here; under cmux the\n\
         workspace may resolve a different per-surface wrapper of the same harness.\n",
    );
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

/// Paths used by `ahu tasks` output, exposed for tests.
pub fn task_dirs(repo_identity: &str) -> Result<Vec<PathBuf>> {
    Ok(task::list(repo_identity)?
        .into_iter()
        .map(|(dir, _)| dir)
        .collect())
}
