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
use crate::style::{self, Role};
use crate::task;
use crate::util::{Result, display_path, display_safe, display_safe_block};

/// Locate the repository ahu was invoked from.
///
/// Commands take the resolved repository explicitly rather than reading the
/// process working directory themselves, so the whole flow is drivable from a
/// test without changing the current directory.
pub fn repo_from_cwd() -> Result<Repo> {
    let cwd = std::env::current_dir()?;
    git::discover(&cwd).map_err(|e| e.with_kind(crate::util::ErrorKind::Prerequisite))
}

/// Open a coordinating session in the invoking terminal. Repository discovery
/// registers executable exclusions before resolving Codex, just as for agents.
pub fn codex(repo: &Repo) -> Result<i32> {
    let executable = selection::resolve_executable("codex").ok_or_else(|| {
        crate::util::Error::new(
            "Codex is not installed or is not available on PATH outside the repository.",
        )
        .with_kind(crate::util::ErrorKind::Prerequisite)
    })?;
    let state = crate::state::ensure_checkout_state(&repo.root)?;
    let mut command = std::process::Command::new(executable);
    command
        .args([
            "--sandbox",
            "workspace-write",
            "--ask-for-approval",
            "on-request",
        ])
        .env("AHU_BIN", std::env::current_exe()?)
        .env("AHU_STATE_DIR", state);
    // Inherit the terminal and cwd. Replacing ahu gives Codex terminal signals
    // directly and preserves its exit status, including signal termination.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(crate::util::Error::new(format!(
            "cannot start Codex: {}",
            command.exec()
        )))
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .map_err(|e| crate::util::Error::new(format!("cannot start Codex: {e}")))?;
        Ok(status.code().unwrap_or(5))
    }
}

/// Load configuration, or run first-run setup, or explain why it cannot.
fn config_or_setup(repo: &Repo, console: &mut Console<'_>) -> Result<Option<LoadedConfig>> {
    if let Some(loaded) = config::load(&repo.root)? {
        return Ok(Some(loaded));
    }
    let Some(new_config) = launcher::run_setup(console)? else {
        console.say("Cancelled. Nothing was written.\n")?;
        return Ok(None);
    };
    let path = config::write_new(&repo.root, &new_config)?;
    console.say(&format!("\nWrote {}\n", path.display()))?;
    console.say(
        "The configuration is in effect now; it does not need to be committed to be used.\n\
         Run `ahu onboard` to see native agent definitions you could register.\n\n",
    )?;
    config::load(&repo.root)?.map(Some).ok_or_else(|| {
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
    let style = crate::style::stdout();
    let agents = agent::load_all(&repo.root)?;
    if agents.is_empty() {
        console.say(&style.paint(
            Role::Hint,
            "No ahu agents are registered.\n\
             Only .agents/ahu/agents/*.toml makes an agent launchable through ahu; native\n\
             definitions elsewhere are onboarding candidates. Run `ahu onboard` to see them.\n",
        ))?;
        return Ok(0);
    }
    for agent in &agents {
        console.say(&format!(
            "@{} {}\n  harness  {}\n  model    {}\n  source   {} [{}]\n  identity {}\n",
            style.paint(Role::Agent, &display_safe(&agent.manifest.name)),
            style.paint(Role::Hint, &display_safe(&agent.manifest.version)),
            style.paint(Role::Runtime, &display_safe(&agent.manifest.harness)),
            style.paint(Role::Runtime, &display_safe(&agent.manifest.model)),
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
                style.paint(Role::Hint, &display_safe(&agent.manifest.description))
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
            .with_kind(crate::util::ErrorKind::UnknownAgent)
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
        _ => bail!(kind: crate::util::ErrorKind::Usage,
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
    let mut project_harnesses = std::collections::BTreeSet::new();
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
            Ok(Some(loaded)) => {
                project_harnesses.extend(loaded.config.harness_preferences.iter().cloned());
                console.say(&format!(
                    "config       {} ({})\n  catalog    {}\n  harnesses  {}\n",
                    display_path(&loaded.path),
                    loaded.short_digest(),
                    display_safe(&loaded.config.catalog_version),
                    display_safe(&loaded.config.harness_preferences.join(", "))
                ))?;
            }
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
        match agent::load_all(&repo.root) {
            Ok(agents) => {
                project_harnesses.extend(agents.iter().map(|agent| agent.manifest.harness.clone()));
            }
            Err(e) => {
                problems += 1;
                console.say(&format!(
                    "agents       invalid: {}\n",
                    display_safe_block(&e.to_string())
                ))?;
            }
        }
    }

    if let Ok(repo) = repo
        && project_harnesses.contains("claude-code")
    {
        match hooks::collect(&repo.root, "claude-code") {
            Ok(found) => {
                console.say(&format!(
                    "hooks        Claude Code: {} configured\n",
                    found.hooks.len()
                ))?;
                for hook in &found.hooks {
                    console.say(&format!("  {:<12} {}\n", hook.scope.as_str(), hook.label()))?;
                }
                for unreadable in &found.unreadable {
                    warnings += 1;
                    console.say(&format!(
                        "  {} {}\n",
                        style::stdout().paint(Role::Warning, "unreadable"),
                        display_safe(unreadable)
                    ))?;
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

    for harness in &project_harnesses {
        let prerequisite = selection::check_prerequisite(harness);
        let status = if !catalog::harness(harness).is_some_and(|entry| entry.adapter_available) {
            problems += 1;
            "unsupported by this ahu version".to_string()
        } else if !prerequisite.satisfied() {
            problems += 1;
            "not installed".to_string()
        } else if let Some(version) = &prerequisite.version {
            let version = version.strip_prefix("codex-cli ").unwrap_or(version);
            format!("{} — ready", display_safe(version))
        } else {
            "installed (version unavailable)".to_string()
        };
        console.say(&format!(
            "harness      {} {status}\n",
            display_safe(harness)
        ))?;
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

    let state_root = crate::state::root()?;
    let state_display = repo
        .as_ref()
        .ok()
        .and_then(|repo| state_root.strip_prefix(&repo.root).ok())
        .unwrap_or(&state_root);
    console.say(&format!("state        {}\n", display_path(state_display)))?;
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
    console.say(&style::stdout().paint(
        if problems > 0 {
            Role::Error
        } else if warnings > 0 {
            Role::Warning
        } else {
            Role::Success
        },
        &summary,
    ))?;
    if problems == 0 {
        Ok(0)
    } else {
        Err(
            crate::util::Error::new("doctor found blocking problems; see the diagnostic report.")
                .with_kind(crate::util::ErrorKind::Prerequisite),
        )
    }
}

/// The one sentence `ahu tasks` may print only when it really found nothing.
///
/// Unreadable records still count as task directories; they must not produce
/// a false claim that no tasks exist.
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
            "{} [session {}] {}\n  agent     {}\n  harness   {} / {}\n  branch    {}\n  worktree  {}\n  record    {}\n",
            display_safe(&record.task_id),
            record.state.as_str(),
            display_safe(&record.title),
            style::stdout().paint(Role::Agent, &display_safe(&record.agent_label())),
            style::stdout().paint(Role::Runtime, &display_safe(&record.identity.harness)),
            style::stdout().paint(Role::Runtime, &display_safe(&record.identity.model)),
            display_safe(&record.branch),
            display_path(&record.worktree),
            display_path(dir),
        ))?;
        if let Some(workspace) = &record.cmux_workspace_id {
            console.say(&format!("  cmux      {}\n", display_safe(workspace)))?;
        }
        console.say("\n")?;
    }
    console.say(&style::stdout().paint(
        Role::Warning,
        &render_unreadable_tasks(repo, &listing.unreadable),
    ))?;
    if !listing.records.is_empty() {
        // This footer explains the `exited` state in the listing above. With no
        // readable records there is no such listing for it to explain.
        console.say(
            "\nSession state does not indicate whether the agent is working or awaiting input.\n\
             `exited` means the harness process ended. It is not a claim that the task \
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

/// Resolve exact IDs before unique prefixes, including unreadable candidates.
fn inspect_task(repo: &Repo, id: &str) -> Result<(PathBuf, task::TaskRecord)> {
    if id.is_empty() {
        bail!(kind: crate::util::ErrorKind::Usage, "a task id must not be empty.");
    }
    // Inspection needs no live cmux connection and does not rewrite records.
    let listing = task::list(&repo.identity())?;
    let exact = listing.records.iter().any(|(_, r)| r.task_id == id)
        || listing.unreadable.iter().any(|r| r.task_id == id);
    let matches = |candidate: &str| candidate == id || (!exact && candidate.starts_with(id));
    let records: Vec<_> = listing
        .records
        .into_iter()
        .filter(|(_, r)| matches(&r.task_id))
        .collect();
    let unreadable: Vec<_> = listing
        .unreadable
        .into_iter()
        .filter(|r| matches(&r.task_id))
        .collect();
    if records.len() + unreadable.len() > 1 {
        bail!(kind: crate::util::ErrorKind::Usage, "ambiguous task prefix {id:?}; use a full task id from `ahu tasks`.");
    }
    if let Some(record) = unreadable.first() {
        bail!(
            "task {} has an unreadable record at {}: {}",
            record.task_id,
            record.dir.display(),
            record.reason
        );
    }
    records.into_iter().next().ok_or_else(|| {
        crate::util::Error::new(format!("no task matching {id:?}."))
            .with_kind(crate::util::ErrorKind::Usage)
    })
}

/// A small, versioned inspection contract; never expose the full launch record.
pub fn task_cmd(console: &mut Console<'_>, repo: &Repo, id: &str, json: bool) -> Result<i32> {
    let (dir, record) = inspect_task(repo, id)?;
    if json {
        let value = serde_json::json!({
            "schema_version": 1,
            "task_id": record.task_id,
            "agent": record.agent_label(),
            "harness": record.identity.harness,
            "model": record.identity.model,
            "branch": record.branch,
            "base_commit": record.base_commit,
            "worktree": record.worktree,
            "worktree_exists": record.worktree.is_dir(),
            "record_path": dir.join("task.json"),
            "cmux_workspace_id": record.cmux_workspace_id,
            "cmux_window_id": record.cmux_window_id,
            "session_state": record.state.as_str(),
            "state_source": "record",
            "completion_verified": false,
        });
        console.say(&format!("{}\n", serde_json::to_string(&value)?))?;
    } else {
        console.say(&format!(
            "{} [session {}]\n  agent     {}\n  harness   {} / {}\n  branch    {}\n  base      {}\n  worktree  {}\n  exists    {}\n  record    {}\n  cmux      {}\n\nState is recorded, not a live activity check. Task completion is not verified.\n",
            display_safe(&record.task_id), record.state.as_str(), display_safe(&record.agent_label()),
            display_safe(&record.identity.harness), display_safe(&record.identity.model),
            display_safe(&record.branch), display_safe(record.base_commit.as_deref().unwrap_or("unknown")),
            display_path(&record.worktree), record.worktree.is_dir(), display_path(&dir.join("task.json")),
            display_safe(record.cmux_workspace_id.as_deref().unwrap_or("none")),
        ))?;
    }
    Ok(0)
}

/// Compare the task checkout to its launch base without staging or running diff helpers.
pub fn diff_cmd(console: &mut Console<'_>, repo: &Repo, id: &str) -> Result<i32> {
    use std::io::IsTerminal;
    let (_, record) = inspect_task(repo, id)?;
    let task_repo = git::discover(&record.worktree)?;
    if task_repo.identity() != repo.identity()
        || task_repo.root.canonicalize()? != record.worktree.canonicalize()?
    {
        bail!("task worktree does not belong to this repository or is not a checkout root.");
    }
    let base = record
        .base_commit
        .as_deref()
        .filter(|base| matches!(base.len(), 40 | 64) && base.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| crate::util::Error::new("task has no valid launch base commit."))?;
    let run = |args: &[&str]| -> Result<Vec<u8>> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&record.worktree)
            .output()?;
        if !output.status.success() {
            bail!(
                "cannot inspect task diff: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(output.stdout)
    };
    let patch = run(&[
        "--no-pager",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--binary",
        base,
        "--",
    ])?;
    let untracked = run(&["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        eprintln!(
            "Untracked (not included in diff): {}",
            display_safe(&String::from_utf8_lossy(path))
        );
    }
    if std::io::stdout().is_terminal() {
        console.say(&display_safe_block(&String::from_utf8_lossy(&patch)))?;
    } else {
        // Redirected output stays a byte-exact patch, including non-UTF-8 data.
        console.output.write_all(&patch)?;
    }
    Ok(0)
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
            bail!(kind: crate::util::ErrorKind::Usage, "no task matching {task_id:?}.");
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
        style::stdout().paint(Role::Agent, &display_safe(&record.agent_label())),
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
        bail!(kind: crate::util::ErrorKind::Prerequisite, "this repository is not initialized. Run `ahu init` first.");
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
        bail!(kind: crate::util::ErrorKind::Prerequisite, "this repository is not initialized. Run `ahu init` first.");
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
    if status.success() {
        Ok(0)
    } else {
        Err(crate::util::Error::new(format!(
            "the harness exited unsuccessfully ({status})."
        )))
    }
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
    let Some(loaded) = config_or_setup(repo, console)? else {
        return Ok(1);
    };

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
        style::stdout().paint(
            Role::Agent,
            &display_safe(
                &resolved
                    .as_ref()
                    .map(|a| a.label())
                    .unwrap_or_else(|| "auto (no named agent)".to_string())
            )
        ),
        style::stdout().paint(Role::Runtime, &display_safe(&pair.harness)),
        style::stdout().paint(Role::Runtime, &display_safe(&pair.model)),
        display_safe(&pair.basis)
    ))?;
    let prerequisite = selection::check_prerequisite(&pair.harness);
    if !prerequisite.satisfied() {
        bail!(kind: crate::util::ErrorKind::Prerequisite,
            "{} is not installed on this machine ({} not found on PATH).\n\
             This is a diagnostic for your machine, not a reason to select a different \
             harness: the project's policy is the same for everyone.",
            pair.harness,
            prerequisite.executable
        );
    }
    for note in &prerequisite.notes {
        console.say(&format!(
            "  {}    {}\n",
            style::stdout().paint(Role::Hint, "note"),
            display_safe(note)
        ))?;
    }

    let Some(prompt) = launcher::read_prompt(console)? else {
        console.say("Cancelled. Nothing was created.\n")?;
        return Ok(1);
    };

    submit(
        console,
        repo,
        &loaded,
        resolved,
        pair,
        &prompt,
        true,
        false,
        focus_new,
        false,
        &launch::DisplayMetadata::default(),
    )
}

/// Assign work without an interactive composer; the command itself requests launch.
///
/// This path has no human at a terminal — the preview goes to a pipe read by
/// another agent — so the interactive confirmation cannot be the gate on
/// approval widening. `--allow-widened-approvals` is that gate instead: it makes
/// the widening appear verbatim in the command line the delegating harness shows
/// its own user before running it, and it is recorded on the task.
#[allow(clippy::too_many_arguments)]
pub fn launch_cmd(
    console: &mut Console<'_>,
    repo: &Repo,
    agent: &str,
    prompt: &str,
    output_json: bool,
    dry_run: bool,
    allow_widened_approvals: bool,
    display: &launch::DisplayMetadata,
) -> Result<i32> {
    let loaded = config::load(&repo.root)?.ok_or_else(|| {
        crate::util::Error::new(
            "project configuration is missing; run ahu init before assigning work.",
        )
        .with_kind(crate::util::ErrorKind::Prerequisite)
    })?;
    let (resolved, pair) = resolve_identity(repo, &loaded, Some(agent))?;
    let permissions = resolved
        .as_ref()
        .map(|a| a.manifest.permissions)
        .unwrap_or_default();
    if permissions.widens_defaults() && !allow_widened_approvals {
        bail!(kind: crate::util::ErrorKind::Usage,
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
    submit(
        console,
        repo,
        &loaded,
        resolved,
        pair,
        prompt,
        false,
        dry_run,
        false,
        output_json,
        display,
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
    output_json: bool,
    display: &launch::DisplayMetadata,
) -> Result<i32> {
    let mut plan = launch::plan(repo, resolved.clone(), pair.clone(), prompt)?;
    plan.apply_display(display)?;

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
        console.say(&style::stdout().paint(Role::Drift, &drift::render(&found)))?;
    }

    // Generated here, after the prompt has been read and after the plan is
    // built, so nothing in the prompt can have contained it.
    let code = confirmation_code();
    if dry_run {
        console.say(&render_preview(
            repo,
            &plan,
            prompt,
            confirm.then_some(code.as_str()),
        ))?;
    } else {
        console.say(&render_launch_preview(
            repo,
            &plan,
            prompt,
            confirm.then_some(code.as_str()),
        ))?;
    }

    if dry_run {
        console.say("Dry run. No task or session was created.\n")?;
        if output_json {
            println!("{}", launch::render_json(&plan, prompt)?);
        }
        return Ok(0);
    }
    if confirm && !launcher::confirm_submit(console, &code)? {
        console.say("Cancelled. No worktree, branch, or session was created.\n")?;
        return Ok(1);
    }

    let launched = launch::execute(repo, loaded, &plan, prompt, focus_new)?;
    console.say(&format!(
        "\nStarted @{} in cmux.\n  task       {}\n  worktree   {}\n\nOpen session: ahu focus {}\nList tasks:   ahu tasks\n",
        display_safe(&launched.record.identity.agent),
        display_safe(&launched.record.task_id),
        display_path(launched.record.worktree.strip_prefix(&repo.root).unwrap_or(&launched.record.worktree)),
        display_safe(&launched.record.task_id),
    ))?;
    console.say(&style::stdout().paint(Role::Warning, &render_launch_notes(&launched.notes)))?;
    Ok(0)
}

/// The normal launch view contains decisions and next actions. Full audit
/// details remain available through dry-run previews, JSON, and inventory.
pub fn render_launch_preview(
    repo: &Repo,
    plan: &launch::LaunchPlan,
    prompt: &str,
    code: Option<&str>,
) -> String {
    let style = style::stdout();
    let mut out = format!(
        "\nLaunch @{}\n  runtime    {} / {}\n  task       {}\n  worktree   {}\n",
        style.paint(Role::Agent, &display_safe(&plan.agent_label())),
        style.paint(Role::Runtime, &display_safe(&plan.pair.harness)),
        style.paint(Role::Runtime, &display_safe(&plan.pair.model)),
        display_safe(&plan.title),
        display_path(
            plan.worktree
                .strip_prefix(&repo.root)
                .unwrap_or(&plan.worktree)
        ),
    );
    out.push_str(&format!(
        "  prompt     {} line(s)\n",
        prompt.lines().count()
    ));
    out.push_str(&format!(
        "  approvals {}\n",
        style.paint(
            if plan.permissions.widens_defaults() {
                Role::Warning
            } else {
                Role::Heading
            },
            match plan.permissions {
                crate::agent::Permissions::Auto => "automatic tool approval (permissions = auto)",
                crate::agent::Permissions::AcceptEdits =>
                    "file edits approved automatically (permissions = accept-edits)",
                _ => "harness defaults",
            }
        )
    ));
    if plan.parent_dirty {
        out.push_str(&style.paint(Role::Drift,
            "\nCheckout changes\nUncommitted source changes are not included. Agent configuration is copied as-is.\n",
        ));
    }
    for unreadable in &plan.hooks.unreadable {
        out.push_str(&style.paint(
            Role::Warning,
            &format!(
                "\n!! Could not read settings: {}\n",
                display_safe(unreadable)
            ),
        ));
    }
    if !plan.hooks.hooks.is_empty() {
        out.push_str(&format!(
            "  hooks      {} configured; details: ahu inventory\n",
            plan.hooks.hooks.len()
        ));
    }
    if !plan.enforcement.gaps.is_empty() {
        out.push_str(&style.paint(
            Role::Gap,
            &format!(
                "  gaps       {} capability limit(s); details: ahu inventory\n",
                plan.enforcement.gaps.len()
            ),
        ));
    }
    if !plan.non_project_hooks().is_empty() {
        out.push_str(&style.paint(
            Role::Warning,
            &format!("\n!! {}\n", hooks::NON_PROJECT_HOOK_WARNING),
        ));
    }
    if let Some(code) = code {
        out.push_str(&style.paint(Role::Heading, "\nAbout to submit\n"));
        out.push_str(&style.paint(
            Role::Heading,
            &format!(
                "\nConfirmation code for this submission: {}\n",
                display_safe(code)
            ),
        ));
    }
    out
}

fn render_enforcement_gaps(plan: &launch::LaunchPlan) -> String {
    let style = style::stdout();
    let mut out = String::new();
    if !plan.enforcement.gaps.is_empty() {
        out.push_str(&style.paint(Role::Gap, "\nEnforcement gaps\n"));
        out.push_str(&style.paint(
            Role::Gap,
            &format!(
                "  ! {} capability limit(s); details: ahu inventory\n",
                plan.enforcement.gaps.len()
            ),
        ));
    }
    out
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
    let style = style::stdout();
    let mut out = String::new();
    out.push_str(&style.paint(Role::Heading, "\nAbout to submit\n===============\n"));
    out.push_str(&style.paint(Role::Heading, "\nIdentity and runtime\n"));
    out.push_str(
        "  delegation All assigned agents must launch through ahu in separate cmux sessions.\n",
    );
    out.push_str("  guidance   ahu supplies its delegation contract and this agent's instructions as prompt text.\n");
    out.push_str(&format!(
        "  agent      {}\n",
        style.paint(Role::Agent, &display_safe(&plan.agent_label()))
    ));
    out.push_str(&format!(
        "  harness    {}\n",
        style.paint(Role::Runtime, &display_safe(&plan.pair.harness))
    ));
    out.push_str(&format!(
        "  model      {}\n",
        style.paint(Role::Runtime, &display_safe(&plan.pair.model))
    ));
    out.push_str(&format!(
        "  because    {}\n",
        display_safe(&plan.pair.basis)
    ));
    out.push_str(&format!(
        "  policy     {} · catalog {}\n",
        &plan.pair.policy_digest[..12],
        display_safe(&plan.pair.catalog_version)
    ));
    if let Some(agent) = &plan.agent {
        // Attribution covers the delivered bytes: ahu delivers
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
    out.push_str(&style.paint(Role::Heading, "\nTask and Git effects\n"));
    out.push_str(&format!("  title      {}\n", display_safe(&plan.title)));
    out.push_str(&format!(
        "  prompt     {} line(s), {} character(s)\n",
        prompt.lines().count(),
        prompt.chars().count()
    ));
    out.push_str(&format!(
        "  prompt bytes {}\n  prompt sha256 {}\n",
        prompt.len(),
        crate::util::digest_bytes(prompt.as_bytes())
    ));
    out.push_str(&format!("  branch     {}\n", display_safe(&plan.branch)));
    out.push_str(&format!("  worktree   {}\n", display_path(&plan.worktree)));
    out.push_str(&format!(
        "  base       {}\n",
        display_safe(plan.base_commit.as_deref().unwrap_or("(none)"))
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
        out.push_str(&style.paint(Role::Drift, "\nCheckout changes\n"));
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
        out.push_str(&style.paint(
            Role::Warning,
            &format!(
                "\nApprovals\n  !! This agent runs with permissions = {}.\n     {}\n     \
             ahu passes the harness's own flag for this because the agent's manifest asks for it. \
             It is a committed, reviewable field, not an ahu default.\n",
                plan.permissions.as_str(),
                display_safe(plan.permissions.disclosure())
            ),
        ));
    } else {
        out.push_str(&format!(
            "\n{}\n  {}\n",
            style.paint(Role::Heading, "Approvals"),
            display_safe(plan.permissions.disclosure())
        ));
    }
    // The flags ahu passes are not the approval boundary. The harness's own
    // settings files are, and this repository carries some of them into the task
    // worktree, so they are printed here rather than left to the file count.
    out.push_str(&hooks::render_settings_for_preview(&plan.hooks));

    out.push_str(&style.paint(Role::Heading, "\nEnforcement\n"));
    for control in &plan.enforcement.applied_controls {
        out.push_str(&format!(
            "  + {}\n",
            style.paint(Role::Success, &display_safe(control))
        ));
    }
    out.push_str(&render_enforcement_gaps(plan));
    out.push_str("  Detailed runtime capabilities: ahu inventory\n");
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
        out.push_str(&style.paint(
            Role::Heading,
            &format!(
                "\nConfirmation code for this submission: {}\n",
                display_safe(code)
            ),
        ));
        out.push_str("It was generated after your prompt was read, so no pasted text can have supplied it.\n");
    }
    out
}

/// Wire the standard streams into a console.
pub fn with_stdio<T>(f: impl FnOnce(&mut Console<'_>) -> Result<T>) -> Result<T> {
    with_stdio_output(false, f)
}

pub fn with_stdio_output<T>(
    diagnostics_to_stderr: bool,
    f: impl FnOnce(&mut Console<'_>) -> Result<T>,
) -> Result<T> {
    let stdin = std::io::stdin();
    let mut locked = stdin.lock();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();
    let writer: &mut dyn std::io::Write = if diagnostics_to_stderr {
        &mut err
    } else {
        &mut out
    };
    let mut console = launcher::stdio_console(&mut locked, writer);
    f(&mut console)
}

/// Every task directory `ahu tasks` accounts for, exposed for tests.
///
/// Includes unreadable records so callers can account for retained work even
/// when a record cannot be trusted or interpreted.
pub fn task_dirs(repo_identity: &str) -> Result<Vec<PathBuf>> {
    Ok(task::list(repo_identity)?.dirs())
}
