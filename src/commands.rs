//! Command implementations.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::agent::{self, ResolvedAgent};
use crate::bail;
use crate::catalog;
use crate::cmux::{self, Cmux};
use crate::config::{self, LoadedConfig};
use crate::drift;
use crate::git::{self, Repo};
use crate::harness;
use crate::hooks;
use crate::hygiene;
use crate::inventory;
use crate::knowledge;
use crate::launch;
use crate::launcher::{self, Console};
use crate::onboard;
use crate::selection::{self, ResolvedPair};
use crate::style::{self, Role};
use crate::task;
use crate::util::{Error, Result, display_path, display_safe, display_safe_block};

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
/// registers executable exclusions before resolving the harness, just as for agents.
pub fn codex(repo: &Repo) -> Result<i32> {
    coordinating_session(
        repo,
        "codex",
        "Codex",
        &["--dangerously-bypass-approvals-and-sandbox"],
    )
}

/// Open Claude using its configured model with permission checks bypassed.
pub fn claude(repo: &Repo) -> Result<i32> {
    coordinating_session(
        repo,
        "claude",
        "Claude",
        &["--dangerously-skip-permissions"],
    )
}

/// Open OpenCode using its configured model and permission behavior.
///
/// This shortcut retains native permission settings and plugin loading.
pub fn opencode(repo: &Repo) -> Result<i32> {
    coordinating_session(repo, "opencode", "OpenCode", &[])
}

/// Open the Antigravity CLI in its unattended (YOLO) permission mode.
pub fn antigravity(repo: &Repo) -> Result<i32> {
    coordinating_session(
        repo,
        "agy",
        "Antigravity CLI",
        &["--dangerously-skip-permissions"],
    )
}

fn coordinating_session(repo: &Repo, program: &str, label: &str, args: &[&str]) -> Result<i32> {
    let harness = match program {
        "claude" => "claude-code",
        "agy" => "antigravity",
        other => other,
    };
    let loaded = config::load(&repo.root)?;
    let model = loaded
        .as_ref()
        .and_then(|loaded| selection::ranked_models(loaded, harness).into_iter().next())
        .unwrap_or_else(|| "unconfigured".to_string());
    if let Some(loaded) = &loaded {
        crate::telemetry::initialize(&loaded.config.telemetry)?;
    }
    let executable = selection::resolve_executable(program).ok_or_else(|| {
        crate::util::Error::new(format!(
            "{label} is not installed or is not available on PATH outside the repository."
        ))
        .with_kind(crate::util::ErrorKind::Prerequisite)
    })?;
    crate::state::ensure_checkout_state(&repo.root)?;
    let placement = launch::group_coordinator(repo, &executable, label, harness, &model, args)?;
    for note in placement.notes {
        eprintln!("ahu: {}", display_safe(&note));
    }
    if placement.opened_workspace {
        return Ok(0);
    }
    if !args.is_empty() {
        eprintln!("{label} coordinator: {}", args.join(" "));
    }
    let mut command = std::process::Command::new(executable);
    command.args(args).env("AHU_BIN", std::env::current_exe()?);
    if let Some(loaded) = &loaded {
        crate::telemetry::configure_child(
            &mut command,
            &loaded.config.telemetry,
            "director",
            harness,
            &model,
            None,
            selection::installed_version(program).as_deref(),
            None,
        );
    }
    // Inherit the terminal and cwd. Replacing ahu gives the harness terminal signals
    // directly and preserves its exit status, including signal termination.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(crate::util::Error::new(format!(
            "cannot start {label}: {}",
            command.exec()
        )))
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .map_err(|e| crate::util::Error::new(format!("cannot start {label}: {e}")))?;
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
              Only .agents/ahu/agents/*.md makes an agent launchable through ahu; native\n\
              definitions elsewhere are onboarding candidates. Run `ahu onboard` to see them.\n",
        ))?;
        return Ok(0);
    }
    let drifted = match agent_drift(repo, &agents) {
        Ok(names) => names,
        Err(error) => {
            console.say(&style.paint(
                Role::Warning,
                &format!(
                    "!! drift could not be checked: {}\n",
                    display_safe_block(&error.to_string())
                ),
            ))?;
            std::collections::BTreeSet::new()
        }
    };
    console.say("AGENT                 HARNESS       MODEL                         STATUS\n")?;
    for agent in &agents {
        let label = if drifted.contains(&agent.manifest.name) {
            format!(
                "@{} {} [drifted]",
                agent.manifest.name, agent.manifest.version
            )
        } else {
            format!("@{} {}", agent.manifest.name, agent.manifest.version)
        };
        console.say(&format!(
            "{:<22} {:<13} {:<29} {}\n",
            style.paint(Role::Agent, &display_safe(&label)),
            style.paint(Role::Runtime, &display_safe(&agent.manifest.harness)),
            style.paint(Role::Runtime, &display_safe(&agent.manifest.model)),
            display_safe(
                &agent
                    .source_path
                    .strip_prefix(&repo.root)
                    .unwrap_or(&agent.source_path)
                    .to_string_lossy()
            ),
        ))?;
    }
    Ok(0)
}

fn agent_drift(
    repo: &Repo,
    agents: &[ResolvedAgent],
) -> Result<std::collections::BTreeSet<String>> {
    let Some(loaded) = config::load(&repo.root)? else {
        return Ok(std::collections::BTreeSet::new());
    };
    let snapshot = crate::snapshot::collect(&repo.root)?;
    let previous = task::list(repo)?;
    if !previous.unreadable.is_empty() {
        return Err(Error::new(format!(
            "{} earlier task record(s) could not be read",
            previous.unreadable.len()
        )));
    }
    let mut drifted = std::collections::BTreeSet::new();
    for agent in agents {
        let found_hooks = hooks::collect(&repo.root, &agent.manifest.harness)?;
        let identity = agent.identity_digest();
        if drift::detect(
            &agent.label(),
            Some(drift::AgentDigests {
                identity: &identity,
                source: &agent.source_digest,
                instructions: &agent.instructions_digest,
            }),
            &snapshot.digest(),
            &loaded.digest,
            &found_hooks.digest(),
            &previous.records,
        )
        .is_some()
        {
            drifted.insert(agent.manifest.name.clone());
        }
    }
    Ok(drifted)
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
    let harness = candidate.format.native_harness().unwrap_or("claude-code");
    let model = match model.or(candidate.native_model.as_deref()) {
        Some(model) if model != "inherit" && !model.is_empty() => model.to_string(),
        // Listed for the harness this definition actually belongs to. A Claude
        // Code list in front of someone registering an OpenCode agent names
        // models their manifest would be refused for.
        _ => bail!(kind: crate::util::ErrorKind::Usage,
            "{name} does not declare a usable model, so ahu needs an explicit one.\n\
             Re-run with --model <exact identifier>. Catalog {} lists for {}: {}.",
            catalog::CATALOG_VERSION,
            harness,
            catalog::models_for(harness)
                .iter()
                .map(|m| m.model)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    console.say("The following file will be created. Nothing else is touched:\n\n")?;
    console.say(&format!(".agents/ahu/agents/{}.md\n\n", display_safe(name)))?;
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
    let mut loaded_config: Option<LoadedConfig> = None;
    let mut registered_agents = Vec::new();
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
                loaded_config = Some(loaded.clone());
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
                registered_agents = agents;
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

    if let Ok(repo) = repo {
        for harness in &project_harnesses {
            if hooks::hook_surface_is_implemented(harness) {
                match hooks::collect(&repo.root, harness) {
                    Ok(found) => {
                        let display_name = catalog::harness(harness)
                            .map(|h| h.display_name)
                            .unwrap_or(harness);
                        let count = found.hooks.len() + found.declared_plugins.len();
                        console.say(&format!(
                            "hooks        {display_name}: {count} configured\n",
                        ))?;
                        for hook in &found.hooks {
                            console.say(&format!(
                                "  {:<12} {}\n",
                                hook.scope.as_str(),
                                hook.label()
                            ))?;
                        }
                        for plugin in &found.declared_plugins {
                            let scope = if plugin.source.starts_with('/')
                                || plugin.source.starts_with('~')
                            {
                                "user"
                            } else {
                                "project"
                            };
                            console.say(&format!(
                                "  {:<12} plugin → {}\n",
                                scope,
                                display_safe(&plugin.module)
                            ))?;
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
                            "hooks        could not be read for {harness}: {}\n",
                            display_safe_block(&e.to_string())
                        ))?;
                    }
                }
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
            format!("{} — executable ready", display_safe(version))
        } else {
            "installed (version unavailable)".to_string()
        };
        console.say(&format!(
            "harness      {} {status}\n",
            display_safe(harness)
        ))?;
    }

    if let Ok(repo) = repo
        && let Some(loaded) = &loaded_config
    {
        let (label, collector_warning) = telemetry_collector_status(&loaded.config.telemetry);
        if collector_warning {
            warnings += 1;
        }
        console.say(&format!("telemetry    {label}\n"))?;

        let review_state = match hygiene::load_state(repo) {
            Ok(state) => state,
            Err(error) => {
                warnings += 1;
                console.say(&format!(
                    "hygiene      review cadence unavailable: {}\n",
                    display_safe_block(&error.to_string())
                ))?;
                hygiene::ReviewState::default()
            }
        };
        let mut cadence_due = 0;
        let cadence_agents: Vec<_> = if registered_agents.is_empty() {
            vec![None]
        } else {
            registered_agents.iter().map(Some).collect()
        };
        for agent in cadence_agents {
            let key = agent
                .map(ResolvedAgent::label)
                .unwrap_or_else(|| "auto".into());
            let state = hygiene::due(loaded, &review_state, &key);
            let status = match state {
                hygiene::Trigger::FirstLoad => {
                    cadence_due += 1;
                    "due (first load)"
                }
                hygiene::Trigger::Overdue => {
                    cadence_due += 1;
                    "due (interval elapsed)"
                }
                hygiene::Trigger::NotDue => "current",
                hygiene::Trigger::Requested => "current",
            };
            console.say(&format!("hygiene      {key}: {status}\n"))?;
        }
        if cadence_due > 0 {
            warnings += 1;
            console
                .say("  Run `ahu hygiene` or launch the affected agent to review its context.\n")?;
        }

        match crate::mcp::verify_bundled_skills(&repo.root) {
            Ok((verified, missing, changed)) => {
                console.say(&format!(
                        "skills       {verified}/{} bundled skills verified; {missing} missing, {changed} changed\n",
                        crate::mcp::BUNDLED_SKILLS.len()
                    ))?;
                if missing + changed > 0 {
                    warnings += 1;
                    console.say(
                        "  Review with `ahu mcp setup`; changed skills are left untouched.\n",
                    )?;
                }
            }
            Err(error) => {
                warnings += 1;
                console.say(&format!(
                    "skills       verification unavailable: {}\n",
                    display_safe_block(&error.to_string())
                ))?;
            }
        }

        match agent_drift(repo, &registered_agents) {
            Ok(names) if names.is_empty() => {
                console.say("drift        no registered agents are drifted\n")?;
            }
            Ok(names) => {
                warnings += 1;
                console.say(&format!(
                    "drift        {}\n",
                    display_safe(
                        &names
                            .into_iter()
                            .map(|name| format!("@{name}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                ))?;
            }
            Err(error) => {
                warnings += 1;
                console.say(&format!(
                    "drift        could not be checked: {}\n",
                    display_safe_block(&error.to_string())
                ))?;
            }
        }
    }

    let integration_root = repo
        .as_ref()
        .map(|r| r.root.clone())
        .unwrap_or(std::env::current_dir()?);
    let native_cli = cmux::integration::NativeCli::discover();
    console.say(&format!(
        "cmux CLI     {}\n",
        display_safe(
            native_cli
                .version
                .as_deref()
                .unwrap_or("unavailable or version unknown")
        )
    ))?;
    for harness in ["claude-code", "codex", "opencode", "antigravity"] {
        let status = cmux::integration::inspect(&integration_root, harness);
        if status
            .components
            .iter()
            .any(|c| c.registration != cmux::integration::Registration::Installed)
        {
            warnings += 1;
        }
        console.say(&format!("cmux integration {}:\n", display_safe(harness)))?;
        console.say(&cmux::integration::render_summary(&status))?;
        if let Some(reason) = status.headless.reasons.first() {
            let safe = display_safe(reason);
            let mut shown: String = safe.chars().take(240).collect();
            if safe.chars().count() > 240 {
                shown.push('…');
            }
            console.say(&format!("  headless   {shown}\n"))?;
        }
        let installer = cmux::integration::installation_plan(harness, &native_cli)?;
        console.say(&format!(
            "  installer  {}; inspect: ahu cmux install --harness {} --dry-run\n",
            if installer.available {
                "available"
            } else {
                "unavailable/unknown"
            },
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

    let state_root = match repo {
        Ok(repo) => crate::storage::CheckoutStorage::new(&repo.root).state_root()?,
        Err(_) => crate::state::root()?,
    };
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

fn telemetry_collector_status(config: &crate::config::TelemetryConfig) -> (String, bool) {
    if !config.enabled {
        return ("off (local telemetry is opt-in)".to_string(), false);
    }
    let endpoint = match config.endpoint.parse::<url::Url>() {
        Ok(endpoint) => endpoint,
        Err(_) => return ("enabled; endpoint is invalid".to_string(), true),
    };
    let port = endpoint.port_or_known_default().unwrap_or(4318);
    let address = match endpoint.host_str() {
        Some("localhost" | "127.0.0.1") => SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        Some("::1" | "[::1]") => SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
        _ => return ("enabled; endpoint is not local".to_string(), true),
    };
    if local_collector_reachable(address) {
        (
            format!(
                "enabled; TCP listener reachable at {} (OTLP not verified)",
                address
            ),
            false,
        )
    } else {
        (format!("enabled; no local listener at {}", address), true)
    }
}

fn local_collector_reachable(address: SocketAddr) -> bool {
    TcpStream::connect_timeout(&address, Duration::from_millis(150)).is_ok()
}

/// The one sentence `ahu tasks` may print only when it really found nothing.
///
/// Unreadable records still count as task directories; they must not produce
/// a false claim that no tasks exist.
pub const NO_TASKS: &str = "No ahu tasks have been launched from this repository.";

/// Whether this task's liveness would be read from the cmux surface.
///
/// Headless tasks carry their signal in `owner.lock` inside the task
/// directory; everything else is interactive and, when a workspace id was
/// recorded, is read from the cmux workspace list.
fn cmux_liveness_needed(dir: &Path, record: &task::TaskRecord) -> bool {
    !dir.join("headless.json").exists() && record.cmux_workspace_id.is_some()
}

/// The cmux workspace list, if any record being shown needs it.
///
/// Each window is fetched once per listing instead of once per row, and not at
/// all when no row can use it. An unreachable cmux is `None`, so liveness degrades to
/// `unknown` rather than failing the listing.
fn cmux_workspaces() -> Option<BTreeMap<String, cmux::WorkspaceInfo>> {
    Cmux::discover().ok()?.workspaces().ok()
}

/// Read the cmux workspace list once, if the records being listed need it.
pub fn liveness_workspaces(
    records: &[(PathBuf, task::TaskRecord)],
) -> Option<BTreeMap<String, cmux::WorkspaceInfo>> {
    if records
        .iter()
        .any(|(dir, record)| cmux_liveness_needed(dir, record))
    {
        cmux_workspaces()
    } else {
        None
    }
}

/// Whether the task's session signal is held, if ahu could read one.
///
/// The answer is an observation for display, never a recorded state, and
/// this function writes nothing: neither the lock file it probes nor the
/// cmux surface it lists is modified.
fn session_owner(
    dir: &Path,
    record: &task::TaskRecord,
    workspaces: Option<&BTreeMap<String, cmux::WorkspaceInfo>>,
) -> Option<bool> {
    if dir.join("headless.json").exists() {
        return crate::headless::supervisor_owns_attempt(dir).ok();
    }
    let id = record.cmux_workspace_id.as_deref()?;
    workspaces.map(|list| list.contains_key(id))
}

/// `ahu tasks`
pub fn tasks(console: &mut Console<'_>, repo: &Repo) -> Result<i32> {
    let listing = if std::env::var("AHU_EXECUTION_BACKEND").ok().as_deref() == Some("headless")
        || !crate::headless::discover(repo)?.is_empty()
    {
        task::list(repo)?
    } else {
        launch::reconcile(repo)?
    };
    // Printed before anything returns: a store can hold something that is not a
    // task and nothing that is, and that is exactly when saying so matters.
    for note in &listing.notes {
        console.say(&style::stdout().paint(
            Role::Warning,
            &format!("!! {}\n\n", display_safe_block(note)),
        ))?;
    }
    if listing.is_empty() {
        console.say(&format!("{NO_TASKS}\n"))?;
        return Ok(0);
    }
    let workspaces = liveness_workspaces(&listing.records);
    console.say(
        "TASK HANDLE             TITLE                        STATE     AGENT                  MODE     LIVE    RUNTIME                         WORKTREE\n",
    )?;
    for (dir, record) in &listing.records {
        let review = if crate::headless::review::is_headless(dir) {
            Some(crate::headless::inspection(dir)?)
        } else {
            None
        };
        let mode = if review.is_some() { "headless" } else { "cmux" };
        // Every field here comes out of task.json, which was built from the
        // prompt and from repository configuration. `tasks` is as much a
        // disclosure surface as the launch preview, so it escapes the same way.
        let live = task::observed_liveness(session_owner(dir, record, workspaces.as_ref()));
        let runtime = format!("{} / {}", record.identity.harness, record.identity.model);
        let worktree = repo_relative_path(repo, &record.worktree);
        console.say(&format!(
            "{} {} {} {} {} {} {} {}\n",
            table_cell(
                &display_safe(&crate::task_handles::label(repo, &record.task_id)),
                22
            ),
            table_cell(&display_safe(&record.title), 28),
            table_cell(record.state.as_str(), 9),
            table_cell(&display_safe(&record.agent_label()), 22),
            table_cell(mode, 8),
            table_cell(live.as_str(), 7),
            table_cell(&display_safe(&runtime), 30),
            display_safe(&worktree),
        ))?;
        if let Some(review) = &review {
            console.say(&crate::headless::review::render(review, true))?;
        }
        if let Some(question) = question_excerpt(dir) {
            console.say(&format!("  question  {question}\n"))?;
        }
    }
    console.say(&style::stdout().paint(
        Role::Warning,
        &render_unreadable_tasks(repo, &listing.unreadable),
    ))?;
    console.say("Run `ahu task <handle>` for task details.\n")?;
    Ok(0)
}

fn table_cell(value: &str, width: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    let cell = if chars.len() <= width {
        value.to_string()
    } else {
        chars
            .into_iter()
            .take(width.saturating_sub(1))
            .collect::<String>()
            + "…"
    };
    format!("{cell:<width$}")
}

fn repo_relative_path(repo: &Repo, path: &Path) -> String {
    path.strip_prefix(&repo.root)
        .map(|relative| relative.to_string_lossy().into_owned())
        .unwrap_or_else(|_| display_path(path))
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
        "\n!! {} task(s) in this repository could not be read or are incomplete.\n\
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
                display_safe(&crate::task_ref::display(&found.task_id))
            ));
            out.push_str(&format!("       record    {}\n", display_path(&found.dir)));

            // The directory name is the task id ahu chose, so the worktree path
            // is recoverable without touching the record.
            match crate::state::worktree_dir(&repo.root, &found.task_id) {
                Ok(worktree) if worktree.is_dir() => {
                    out.push_str(&format!(
                        "       worktree  {}\n",
                        repo_relative_path(repo, &worktree)
                    ));
                }
                Ok(worktree) => {
                    out.push_str(&format!(
                        "       worktree  {} (no longer on disk)\n",
                        repo_relative_path(repo, &worktree)
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

/// The shared wording for a task whose record exists but cannot be read.
fn unreadable_record(blocked: &task::UnreadableTask) -> Error {
    Error::new(format!(
        "task {} has an unreadable record at {}: {}",
        blocked.task_id,
        blocked.dir.display(),
        blocked.reason
    ))
}

/// Where a task id resolved from. The listing and pointer forms both carry a
/// readable record; the pointer records which checkout advertised it.
enum Located {
    Listing(PathBuf, task::TaskRecord),
    Pointer(crate::task_index::Entry, PathBuf, task::TaskRecord),
    Unreadable(task::UnreadableTask),
    NoMatch { unreadable_in_repo: usize },
}

/// Load the record behind a task index entry, refusing stale or hostile
/// entries instead of guessing.
fn load_indexed_task(entry: &crate::task_index::Entry) -> Result<(PathBuf, task::TaskRecord)> {
    if !entry.checkout.is_dir() {
        bail!(
            "the task index records task {} at {}, but that checkout no longer exists; the entry \
             is stale and the task cannot be reached from here.",
            display_safe(&crate::task_ref::display(&entry.task_id)),
            display_path(&entry.checkout)
        );
    }
    let dir = match entry.store {
        crate::task_index::StoreKind::Worktree => crate::state::checkout_root(&entry.checkout)?
            .join("repos")
            .join(&entry.repo_identity)
            .join("tasks")
            .join(&entry.task_id),
        crate::task_index::StoreKind::Headless => {
            let repo = crate::git::discover(&entry.checkout)?;
            if repo.identity() != entry.repo_identity {
                bail!("task index repository identity mismatch");
            }
            crate::headless::lookup(&repo, &entry.task_id)?
        }
    };
    if !dir.exists() {
        bail!(
            "the task index records task {} at {}, but its task directory {} no longer exists; the \
             entry is stale.",
            display_safe(&crate::task_ref::display(&entry.task_id)),
            display_path(&entry.checkout),
            display_path(&dir)
        );
    }
    let record = task::load(&dir).map_err(|e| {
        Error::new(format!(
            "task {} has an unreadable record at {}: {}\nThe task index records its checkout as \
             {}.",
            display_safe(&crate::task_ref::display(&entry.task_id)),
            display_path(&dir),
            strip_record_path(&e.to_string(), &dir),
            display_path(&entry.checkout)
        ))
    })?;
    let hostile = match entry.store {
        crate::task_index::StoreKind::Worktree => {
            record.task_id != entry.task_id
                || record.repo_identity != entry.repo_identity
                || record.worktree.canonicalize().ok() != entry.checkout.canonicalize().ok()
        }
        crate::task_index::StoreKind::Headless => {
            record.task_id != entry.task_id || record.repo_identity != entry.repo_identity
        }
    };
    if hostile {
        bail!(
            "the task index records task {} at checkout {}, but the record ahu found there \
             describes a different task. Nothing was done; re-check the task id.",
            display_safe(&crate::task_ref::display(&entry.task_id)),
            display_path(&entry.checkout)
        );
    }
    Ok((dir, record))
}

/// Resolve exact IDs before unique prefixes, including unreadable candidates.
/// The task index is consulted for ids that are not local records, so a task is
/// reachable from any checkout of the repository that launched it.
fn resolve_task(repo: &Repo, input: &str) -> Result<Located> {
    let id = crate::task_ref::resolve(repo, input)?;
    // Inspection needs no live cmux connection and does not rewrite records.
    let listing = task::list(repo)?;
    if let Some((dir, record)) = listing.records.iter().find(|(_, r)| r.task_id == id) {
        return Ok(Located::Listing(dir.clone(), record.clone()));
    }
    if let Some(blocked) = listing.unreadable.iter().find(|u| u.task_id == id) {
        return Ok(Located::Unreadable(blocked.clone()));
    }
    let unreadable_in_repo = listing.unreadable.len();
    let records: Vec<_> = listing
        .records
        .into_iter()
        .filter(|(_, r)| r.task_id.starts_with(&id))
        .collect();
    let unreadable: Vec<task::UnreadableTask> = listing
        .unreadable
        .into_iter()
        .filter(|u| u.task_id.starts_with(&id))
        .collect();
    let entries = if task::is_canonical_task_uuid(&id) {
        match crate::task_index::lookup_in(repo, &id)? {
            Some(entry) => {
                let (dir, record) = load_indexed_task(&entry)?;
                return Ok(Located::Pointer(entry, dir, record));
            }
            None => Vec::new(),
        }
    } else {
        let mut entries = crate::task_index::lookup_prefix_in(repo, &id)?;
        entries.retain(|e| {
            !records.iter().any(|(_, r)| r.task_id == e.task_id)
                && !unreadable.iter().any(|u| u.task_id == e.task_id)
        });
        entries
    };
    let total = records.len() + unreadable.len() + entries.len();
    if total > 1 {
        let mut message = format!(
            "ambiguous task id {input:?}: it matches {total} tasks; use a full task id from \
             `ahu tasks`.\n"
        );
        for (dir, _) in &records {
            message.push_str(&format!("  record at {}\n", display_path(dir)));
        }
        for blocked in &unreadable {
            message.push_str(&format!(
                "  unreadable record at {}\n",
                display_path(&blocked.dir)
            ));
        }
        for entry in &entries {
            message.push_str(&format!(
                "  task index entry at {}\n",
                display_path(&entry.checkout)
            ));
        }
        return Err(Error::new(message).with_kind(crate::util::ErrorKind::Usage));
    }
    if let Some((dir, record)) = records.first() {
        return Ok(Located::Listing(dir.clone(), record.clone()));
    }
    if let Some(blocked) = unreadable.first() {
        return Ok(Located::Unreadable(blocked.clone()));
    }
    if let Some(entry) = entries.first() {
        let (dir, record) = load_indexed_task(entry)?;
        return Ok(Located::Pointer(entry.clone(), dir, record));
    }
    Ok(Located::NoMatch { unreadable_in_repo })
}

/// Resolve exact IDs before unique prefixes, including unreadable candidates.
pub(crate) fn inspect_task(repo: &Repo, id: &str) -> Result<(PathBuf, task::TaskRecord)> {
    let normalized = crate::task_ref::resolve(repo, id)?;
    let id: &str = &normalized;
    // Inspection needs no live cmux connection and does not rewrite records.
    let listing = task::list(repo)?;
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

pub fn task_summary(
    dir: &Path,
    record: &task::TaskRecord,
    workspaces: Option<&BTreeMap<String, cmux::WorkspaceInfo>>,
) -> Result<serde_json::Value> {
    let mut value = serde_json::json!({
        "schema_version": 1,
        "task_id": record.task_id,
        "task_ref": crate::task_ref::display(&record.task_id),
        "task_handle": crate::task_handles::at(dir, &record.task_id),
        "repo_identity": record.repo_identity,
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
        "liveness": task::observed_liveness(session_owner(dir, record, workspaces)).as_str(),
        "completion_verified": false,
        "harness_executable": record.harness_executable,
    });
    value["execution_backend"] = if crate::headless::review::is_headless(dir) {
        "headless".into()
    } else {
        "cmux".into()
    };
    if crate::headless::review::is_headless(dir) {
        value["attempt"] = crate::headless::inspection(dir)?;
        value["liveness"] = value["attempt"]["liveness"].clone();
    }
    Ok(value)
}

/// A small, versioned inspection contract; never expose the full launch record.
pub fn task_cmd(console: &mut Console<'_>, repo: &Repo, id: &str, json: bool) -> Result<i32> {
    let (dir, record) = match resolve_task(repo, id)? {
        Located::Listing(dir, record) | Located::Pointer(_, dir, record) => (dir, record),
        Located::Unreadable(blocked) => return Err(unreadable_record(&blocked)),
        Located::NoMatch { .. } => {
            bail!(kind: crate::util::ErrorKind::Usage, "no task matching {id:?}.")
        }
    };
    let workspaces = if cmux_liveness_needed(&dir, &record) {
        cmux_workspaces()
    } else {
        None
    };
    let value = task_summary(&dir, &record, workspaces.as_ref())?;
    if json {
        console.say(&format!("{}\n", serde_json::to_string(&value)?))?;
    } else {
        console.say(&format!(
            "{} [session {}]\n  backend   {}\n  agent     {}\n  harness   {} / {}\n  branch    {}\n  base      {}\n  worktree  {}\n  exists    {}\n  record    {}\n  liveness  {}\n\nState is recorded, not a live activity check. Task completion is not verified. Liveness is observed at this moment, not a verdict; `unknown` means ahu could not read the signal.\n",
            display_safe(&crate::task_handles::label(repo, &record.task_id)), record.state.as_str(), value["execution_backend"].as_str().unwrap_or("unknown"), display_safe(&record.agent_label()),
            display_safe(&record.identity.harness), display_safe(&record.identity.model),
            display_safe(&record.branch), display_safe(record.base_commit.as_deref().unwrap_or("unknown")),
            display_path(&record.worktree), record.worktree.is_dir(), display_path(&dir.join("task.json")),
            value["liveness"].as_str().unwrap_or("unknown"),
        ))?;
        if value["execution_backend"] == "headless" {
            console.say(&crate::headless::review::render(&value["attempt"], false))?;
        } else {
            console.say(&format!(
                "  cmux      {}\n",
                display_safe(record.cmux_workspace_id.as_deref().unwrap_or("none"))
            ))?;
        }
        if let Some(body) = read_artifact(&dir, "result.md") {
            match body {
                ArtifactBody::Content(body) => {
                    console.say(&format!("\nresult:\n{body}\n"))?;
                }
                ArtifactBody::Oversized => {
                    console.say(&style::stdout().paint(
                        Role::Warning,
                        &format!(
                            "\n!! result.md is larger than the 1 MiB display bound; ahu will not \
                             print it. Read it at {}.\n",
                            display_path(&dir.join("result.md"))
                        ),
                    ))?;
                }
                ArtifactBody::Unreadable => {
                    console.say(&style::stdout().paint(
                        Role::Warning,
                        &format!(
                            "\n!! ahu cannot safely read result.md; inspect it at {}.\n",
                            display_path(&dir.join("result.md"))
                        ),
                    ))?;
                }
            }
        }
        if let Some(body) = read_artifact(&dir, "question.md") {
            match body {
                ArtifactBody::Content(body) => {
                    console.say(&format!("\nquestion:\n{body}\n"))?;
                }
                ArtifactBody::Oversized => {
                    console.say(&style::stdout().paint(
                        Role::Warning,
                        &format!(
                            "\n!! question.md is larger than the 1 MiB display bound; ahu will \
                             not print it. Read it at {}.\n",
                            display_path(&dir.join("question.md"))
                        ),
                    ))?;
                }
                ArtifactBody::Unreadable => {
                    console.say(&style::stdout().paint(
                        Role::Warning,
                        &format!(
                            "\n!! ahu cannot safely read question.md; inspect it at {}.\n",
                            display_path(&dir.join("question.md"))
                        ),
                    ))?;
                }
            }
        }
    }
    Ok(0)
}

/// The most inbox entries a task directory accepts.
const INBOX_MAX_ENTRIES: usize = 100;

/// The byte budget all inbox entries in one task directory share.
const INBOX_MAX_TOTAL_BYTES: u64 = 1024 * 1024;

/// The largest task artifact ahu will read back for display.
const TASK_ARTIFACT_LIMIT: u64 = 1024 * 1024;

/// `ahu message <task-id> <text>`
///
/// Appends an operator message to the task's inbox. Delivery is the
/// operator's exclusive right: a worker session inherits the environment
/// marker below and is refused, so a task cannot message itself or another
/// task. The inbox is size-bounded and confined to the task directory.
pub fn message_cmd(
    console: &mut Console<'_>,
    repo: &Repo,
    task_id: &str,
    text: &str,
) -> Result<i32> {
    if std::env::var_os("AHU_WORKER_SESSION").is_some() {
        bail!(
            "ahu message is an operator command. Working agents cannot deliver inbox messages; \
             delivery belongs to the operator or to a broker-bound child."
        );
    }
    let text = text.trim();
    if text.is_empty() {
        bail!(
            kind: crate::util::ErrorKind::Usage,
            "`ahu message` needs a task id and a message text."
        );
    }
    let (dir, record) = match resolve_task(repo, task_id)? {
        Located::Listing(dir, record) | Located::Pointer(_, dir, record) => (dir, record),
        Located::Unreadable(blocked) => return Err(unreadable_record(&blocked)),
        Located::NoMatch { .. } => {
            bail!(kind: crate::util::ErrorKind::Usage, "no task matching {task_id:?}.")
        }
    };
    let entry = deliver_inbox_message(&dir, text)?;
    console.say(&format!(
        "delivered inbox message {entry:04} to task {}.\n",
        display_safe(&crate::task_handles::label(repo, &record.task_id))
    ))?;
    Ok(0)
}

/// The number of a numbered inbox entry file, when its name is one ahu wrote.
fn parse_inbox_entry(name: &str) -> Option<u64> {
    let stem = name.strip_suffix(".md")?;
    let digits = stem.strip_prefix('0')?;
    let digits = digits.strip_prefix('0').unwrap_or(digits);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: u64 = digits.parse().ok()?;
    if number < 1 {
        return None;
    }
    Some(number)
}

/// Append one operator message to the task's inbox, enforcing its bounds.
///
/// The scan fails closed: an inbox entry ahu does not recognize, or one it
/// cannot inspect safely, stops delivery instead of writing next to it.
fn deliver_inbox_message(dir: &Path, text: &str) -> Result<usize> {
    let inbox = dir.join("inbox");
    crate::state::create_private_dir_all(&inbox)?;
    let mut highest = 0u64;
    let mut total = 0u64;
    for entry in std::fs::read_dir(&inbox).map_err(|e| {
        Error::new(format!(
            "cannot scan the task inbox at {}: {e}",
            display_path(&inbox)
        ))
    })? {
        let entry = entry.map_err(|e| {
            Error::new(format!(
                "cannot scan the task inbox at {}: {e}",
                display_path(&inbox)
            ))
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(number) = parse_inbox_entry(&name) else {
            bail!(
                "the task inbox at {} holds an unrecognized entry {name:?}; ahu will not write \
                 next to it.",
                display_path(&inbox)
            );
        };
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(|e| {
            Error::new(format!(
                "cannot inspect the task inbox entry {} at {}: {e}",
                display_path(&entry.path()),
                display_path(&inbox)
            ))
        })?;
        if !metadata.is_file() {
            bail!(
                "the task inbox at {} holds an entry {name:?} that is not a regular file; ahu \
                 will not write next to it.",
                display_path(&inbox)
            );
        }
        total = total.saturating_add(metadata.len());
        highest = highest.max(number);
    }
    let next = highest
        .checked_add(1)
        .ok_or_else(|| Error::new("the task inbox is full."))?;
    if highest >= INBOX_MAX_ENTRIES as u64 {
        bail!(
            "the task inbox at {} is full ({INBOX_MAX_ENTRIES} entries); no message was written.",
            display_path(&inbox)
        );
    }
    let bytes = text.len() as u64;
    if total.saturating_add(bytes) > INBOX_MAX_TOTAL_BYTES {
        bail!(
            "the task inbox at {} holds more than {} bytes already; no message was written.",
            display_path(&inbox),
            INBOX_MAX_TOTAL_BYTES
        );
    }
    crate::state::write_private_file(&inbox.join(format!("{next:04}.md")), text.as_bytes())?;
    Ok(next as usize)
}

/// `ahu tasks`: the question.md line for one task, when one is on record.
fn question_excerpt(dir: &Path) -> Option<String> {
    let path = dir.join("question.md");
    if !path.exists() {
        return None;
    }
    let body = match std::fs::read(&path) {
        Ok(body) => body,
        Err(_) => return Some("present, unreadable".to_string()),
    };
    if body.len() as u64 > TASK_ARTIFACT_LIMIT {
        return Some("present, larger than the display bound".to_string());
    }
    let text = String::from_utf8_lossy(&body);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some("present, empty".to_string());
    }
    let first = trimmed.lines().next().unwrap_or("");
    let mut line: String = first.chars().take(60).collect();
    if first.chars().count() > 60 {
        line.push('…');
    }
    Some(display_safe(&line))
}

/// A task artifact read back for display, with the bounds that allow it.
enum ArtifactBody {
    Content(String),
    Oversized,
    Unreadable,
}

/// Read result.md or question.md under the display bound, never trusting it.
fn read_artifact(dir: &Path, file: &str) -> Option<ArtifactBody> {
    let path = dir.join(file);
    if !path.exists() {
        return None;
    }
    let body = match std::fs::read(&path) {
        Ok(body) => body,
        Err(_) => return Some(ArtifactBody::Unreadable),
    };
    if body.len() as u64 > TASK_ARTIFACT_LIMIT {
        return Some(ArtifactBody::Oversized);
    }
    Some(ArtifactBody::Content(display_safe_block(
        &String::from_utf8_lossy(&body),
    )))
}

/// Compare the task checkout to its launch base without staging or running diff helpers.
pub fn diff_cmd(console: &mut Console<'_>, repo: &Repo, id: &str) -> Result<i32> {
    use std::io::IsTerminal;
    let (dir, record, owner_identity, via_index) = match resolve_task(repo, id)? {
        Located::Listing(dir, record) => (dir, record, repo.identity(), false),
        Located::Pointer(entry, dir, record) => (dir, record, entry.repo_identity.clone(), true),
        Located::Unreadable(blocked) => return Err(unreadable_record(&blocked)),
        Located::NoMatch { .. } => {
            bail!(kind: crate::util::ErrorKind::Usage, "no task matching {id:?}.")
        }
    };
    let task_repo = git::discover(&record.worktree)?;
    if task_repo.identity() != owner_identity
        || task_repo.root.canonicalize()? != record.worktree.canonicalize()?
    {
        if via_index {
            bail!(
                "task worktree does not belong to the repository that launched it or is not a checkout root."
            );
        }
        bail!("task worktree does not belong to this repository or is not a checkout root.");
    }
    let base = record
        .base_commit
        .as_deref()
        .filter(|base| matches!(base.len(), 40 | 64) && base.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| crate::util::Error::new("task has no valid launch base commit."))?;
    let run = |args: &[&str]| -> Result<Vec<u8>> {
        let output = git::run(&record.worktree, args)?;
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
    let untracked: Vec<String> = untracked
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| display_safe(&String::from_utf8_lossy(p)))
        .collect();
    for path in &untracked {
        eprintln!("Untracked (not included in diff): {path}");
    }
    if patch.is_empty()
        && untracked.is_empty()
        && let Some(outside) = crate::headless::recorded_writes_outside_worktree(&dir)
    {
        eprintln!(
            "No changes in the task worktree, but write tool calls in the recorded event stream targeted paths outside it:"
        );
        for path in outside.iter().take(3) {
            eprintln!("  {}", display_safe(path));
        }
        if outside.len() > 3 {
            eprintln!("... and {} more", outside.len() - 3);
        }
        eprintln!(
            "Run `ahu result {}` for the full recorded list.",
            display_safe(&crate::task_handles::reference(repo, &record.task_id))
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
    let record = match resolve_task(repo, task_id)? {
        Located::Listing(_, record) | Located::Pointer(_, _, record) => record,
        Located::Unreadable(blocked) => {
            // "no task matching" would be a claim that nothing here is that task.
            // If a directory with that id exists and ahu simply could not read its
            // record, saying so is the difference between a user looking for a
            // typo and a user looking at a leftover worktree.
            bail!(
                "task {} exists but ahu cannot read its record, so it cannot find its cmux \
                 session.\n{}\nThe record is at {}. Run `ahu tasks` for its worktree and branch.",
                display_safe(&crate::task_ref::display(&blocked.task_id)),
                display_safe_block(&blocked.reason),
                display_path(&blocked.dir)
            );
        }
        Located::NoMatch {
            unreadable_in_repo: 0,
        } => {
            bail!(kind: crate::util::ErrorKind::Usage, "no task matching {task_id:?}.")
        }
        Located::NoMatch { unreadable_in_repo } => {
            bail!(
                "no readable task matching {task_id:?}. {unreadable_in_repo} other task record(s) \
                 in this repository could not be read either; run `ahu tasks` to see them."
            );
        }
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

/// `ahu remove <task-id>`
///
/// Removes a terminal task's record, worktree, and branch in one explicit
/// action. Every gate runs before anything is removed: a live task is a
/// cancellation, not a removal, and a dirty worktree or a branch holding
/// unmerged commits keeps its work for review.
pub fn remove_cmd(console: &mut Console<'_>, repo: &Repo, task_id: &str) -> Result<i32> {
    let (record_dir, record, owner_identity, via_index, gate_discovered) =
        match resolve_task(repo, task_id)? {
            Located::Listing(dir, record) => (dir, record, repo.identity(), false, None),
            Located::Pointer(entry, dir, record) => {
                let gate = git::discover(&entry.checkout).map_err(|e| {
                    Error::new(format!(
                        "the task index records task {} at {}, but that checkout cannot be \
                     inspected: {e}\nNothing was removed.",
                        display_safe(&crate::task_ref::display(&entry.task_id)),
                        display_path(&entry.checkout)
                    ))
                })?;
                (dir, record, entry.repo_identity.clone(), true, Some(gate))
            }
            Located::Unreadable(blocked) => return Err(unreadable_record(&blocked)),
            Located::NoMatch { .. } => {
                bail!(kind: crate::util::ErrorKind::Usage, "no task matching {task_id:?}.")
            }
        };
    let gate: &Repo = gate_discovered.as_ref().unwrap_or(repo);
    if record.state.is_live() {
        bail!(
            "task {} is {} — not a terminal state; nothing was removed.\n\
             Stop it with `ahu cancel {}` first; removal only takes tasks that have finished.",
            display_safe(task_id),
            record.state.as_str(),
            display_safe(task_id)
        );
    }
    let worktree_present = record.worktree.is_dir();
    let branch_present = git::branch_exists(gate, &record.branch)?;
    if worktree_present {
        let discovered = git::discover(&record.worktree).map_err(|e| {
            Error::new(format!(
                "the task worktree {} cannot be inspected: {e}\n\
                 Nothing was removed.",
                display_path(&record.worktree)
            ))
        })?;
        if discovered.identity() != owner_identity
            || discovered.root.canonicalize()? != record.worktree.canonicalize()?
        {
            if via_index {
                bail!(
                    "task worktree does not belong to the repository that launched it or is not a checkout root."
                );
            }
            bail!("task worktree does not belong to this repository or is not a checkout root.");
        }
        if discovered.root.canonicalize()? == repo.root.canonicalize()? {
            bail!(
                "this command is running inside the worktree being removed. Run `ahu remove {}` \
                 from another checkout of the repository.",
                display_safe(task_id)
            );
        }
        if let Err(e) = git::is_dirty(&discovered) {
            bail!(
                "cannot tell whether the worktree for task {} is clean: {e}\n\
                 Nothing was removed.",
                display_safe(task_id)
            );
        }
        if git::is_dirty(&discovered)? {
            bail!(
                "the worktree for task {} has uncommitted changes; nothing was removed.\n\
                 Uncommitted changes are reviewable work. Inspect {}, commit or discard what you \
                 find there, then run `ahu remove {}` again.",
                display_safe(task_id),
                display_path(&record.worktree),
                display_safe(task_id)
            );
        }
    }
    if branch_present && !git::branch_merged_into_primary_head(gate, &record.branch)? {
        bail!(
            "branch {} has commits that are not in the primary checkout's current branch; nothing \
             was removed.\n\
             Removal never deletes work that exists only on that branch. Merge or apply it first, \
             then run `ahu remove {}` again.",
            display_safe(&record.branch),
            display_safe(task_id)
        );
    }
    // The branch deletion runs from the primary checkout, which must be
    // resolved while the task's own worktree still exists: `primary_root`
    // asks Git to run from `gate`'s root, and below this point that root may
    // be the worktree being removed.
    let branch_checkout = if branch_present {
        Some(gate.primary_root()?)
    } else {
        None
    };
    let mut completed: Vec<(&str, String)> = Vec::new();
    let mut not_removed: Vec<(&str, String)> = Vec::new();
    let mut worktree_removed = false;
    if worktree_present {
        match git::remove_task_worktree(gate, &record.worktree) {
            Ok(()) => {
                completed.push(("worktree", "removed".to_string()));
                worktree_removed = true;
            }
            Err(e) => not_removed.push(("worktree", format!("{e}"))),
        }
    } else {
        completed.push(("worktree", "already absent".to_string()));
    }
    if record_dir.exists() {
        let record_result = crate::state::confine_existing_dir(&record_dir)
            .and_then(|()| std::fs::remove_dir_all(&record_dir).map_err(Error::from));
        match record_result {
            Ok(()) => completed.push(("record", "removed".to_string())),
            Err(e) => not_removed.push(("record", format!("{e}"))),
        }
    } else if worktree_removed {
        completed.push(("record", "removed with its worktree".to_string()));
    } else {
        completed.push(("record", "already absent".to_string()));
    }
    if let Some(primary) = &branch_checkout {
        match git::delete_task_branch(primary, &record.branch) {
            Ok(()) => completed.push(("branch", "deleted".to_string())),
            Err(e) => not_removed.push(("branch", format!("{e}"))),
        }
    } else {
        completed.push(("branch", "already absent".to_string()));
    }
    if !not_removed.is_empty() {
        let mut message = format!(
            "task {} was only partly removed.\n\
             Completed:\n",
            display_safe(task_id)
        );
        for (label, status) in &completed {
            message.push_str(&format!("  {label:<8}  {status}\n"));
        }
        message.push_str("Not removed:\n");
        for (label, status) in &not_removed {
            message.push_str(&format!("  {label:<8}  {status}\n"));
        }
        if record_dir.exists() {
            message.push_str(&format!(
                "Fix the problem, then run `ahu remove {}` again.",
                display_safe(task_id)
            ));
        } else {
            message.push_str(
                "Delete the branch yourself once you are sure its commits are not needed.",
            );
        }
        bail!("{message}");
    }
    if let Err(e) = crate::task_index::remove_in(repo, &record.task_id) {
        eprintln!(
            "warning: could not remove the task index entry for {}: {e}",
            display_safe(&crate::task_handles::label(repo, &record.task_id))
        );
    }
    let mut said = format!("removed task {}\n", display_safe(task_id));
    said.push_str(&format!(
        "  worktree  {}\n  branch    {}\n  record    {}\n",
        completed[0].1, completed[2].1, completed[1].1
    ));
    console.say(&said)?;
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
    let review_state = hygiene::load_state(repo)?;
    let review = hygiene::review(&key, &built, &enforcement, &loaded, &review_state);
    console.say(&hygiene::render(
        &review,
        hygiene::Trigger::Requested,
        &loaded,
    ))?;
    hygiene::record_review(repo, &key)?;
    Ok(0)
}

/// `ahu knowledge lint [--output json]`
///
/// A check, so its exit status is the result: 0 when the bundles pass under the
/// project's policy, 5 when they do not. Nothing in a bundle is written to,
/// and no cmux session or harness is involved.
pub fn knowledge_lint(console: &mut Console<'_>, repo: &Repo, json: bool) -> Result<i32> {
    let loaded = config::load(&repo.root)?.ok_or_else(|| {
        crate::util::Error::new(
            "project configuration is missing; run `ahu init` before checking knowledge bundles.",
        )
        .with_kind(crate::util::ErrorKind::Prerequisite)
    })?;
    let report = knowledge::lint(&repo.root, &loaded)?;
    if json {
        println!("{}", knowledge::render_json(&report)?);
    }
    console.say(&knowledge::render(&report))?;
    if report.passed() {
        return Ok(0);
    }
    Err(crate::util::Error::new(
        "knowledge lint found problems; see the findings above.",
    ))
}

/// `ahu cancel <task-id>` — request cancellation of a running task.
///
/// A headless task is delegated to the headless supervisor's cancel flow
/// unchanged. An interactive (cmux) task is stopped by its run-task parent,
/// which owns the harness process tree. Confirmed cancellation closes the
/// recorded cmux workspace; an unconfirmed request leaves it open. The worktree,
/// branch and record are never deleted.
pub fn cancel_cmd(repo: &Repo, id: &str, json_output: bool) -> Result<i32> {
    let (dir, record) = inspect_task(repo, id)?;
    if dir.join("headless.json").exists() {
        return crate::headless::control(repo, "cancel", &record.task_id, None, json_output);
    }
    if !record.state.is_live() {
        eprintln!(
            "ahu: task {} is recorded as terminal ({}); no new cancellation was requested. The worktree, \
             branch and record are kept.",
            record.task_id,
            record.state.as_str()
        );
        return crate::headless::emit(
            &serde_json::json!({
                    "schema_version": 1,
                    "task_id": record.task_id,
            "task_ref": crate::task_ref::display(&record.task_id),
                    "cancellation": "already-terminal",
                    "state": record.state.as_str(),
                    "workspace": "left open",
                    "retention": "the worktree, branch and record are kept",
                }),
            json_output,
        )
        .map(|()| 0);
    }

    // The run-task parent supervises this directory and does the terminating;
    // this request is the only signal it needs.
    let request = serde_json::json!({
        "requested_at": task::now_rfc3339(),
        "reason": "cancelled",
    });
    crate::state::write_private_file(
        &dir.join("cancel.json"),
        serde_json::to_string(&request)
            .unwrap_or_default()
            .as_bytes(),
    )?;

    let mut cancellation = "requested-unconfirmed";
    let mut state = record.state;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match task::load(&dir) {
            Ok(current) if !current.state.is_live() => {
                state = current.state;
                if state == task::TaskState::Cancelled {
                    cancellation = "confirmed";
                } else {
                    cancellation = "finished-on-its-own";
                }
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Only the spawning supervisor can confirm termination. A timeout or a
    // read failure preserves the pane; closing it is not process supervision.
    let workspace = match record.cmux_workspace_id.as_deref() {
        None => "absent",
        Some(workspace_id) if cancellation == "confirmed" => {
            Cmux::discover()
                .and_then(|client| client.close_workspace(workspace_id))
                .map_err(|error| {
                    crate::util::Error::new(format!(
                        "cancellation confirmed, but cmux workspace close failed: {error}"
                    ))
                })?;
            "closed"
        }
        Some(_) => "left open",
    };

    crate::headless::emit(
        &serde_json::json!({
            "schema_version": 1,
            "task_id": record.task_id,
            "task_ref": crate::task_ref::display(&record.task_id),
            "cancellation": cancellation,
            "state": state.as_str(),
            "workspace": workspace,
            "retention": "the worktree, branch and record are kept",
        }),
        json_output,
    )?;
    Ok(0)
}

/// `ahu run-task --task-dir <dir>` — the fixed entrypoint cmux starts.
pub fn run_task(task_dir: &Path) -> Result<i32> {
    let record = crate::task::load(task_dir)?;
    if let Some(loaded) = crate::config::load(&record.worktree)? {
        crate::telemetry::initialize(&loaded.config.telemetry)?;
    }
    match launch::run_task(task_dir)? {
        launch::HarnessOutcome::Exited(status) if status.success() => Ok(0),
        launch::HarnessOutcome::Exited(status) => Err(crate::util::Error::new(format!(
            "the harness exited unsuccessfully ({status})."
        ))),
        // The harness was terminated on request; the pane reports the
        // cancellation itself rather than an unsuccessful exit.
        launch::HarnessOutcome::Cancelled => Ok(1),
    }
}

/// Resolve a launch identity from an optional `@name`.
pub(crate) fn resolve_identity(
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
pub fn interactive(
    console: &mut Console<'_>,
    repo: &Repo,
    focus_new: bool,
    preselected_agent: Option<&str>,
) -> Result<i32> {
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
    let selector = match preselected_agent {
        Some(name) => {
            console.say(&format!("Preselected agent: @{name}\n"))?;
            Some(name.to_string())
        }
        None => launcher::read_selector(console, &registered)?,
    };
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
    crate::telemetry::initialize(&loaded.config.telemetry)?;
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
    crate::telemetry::initialize(&loaded.config.telemetry)?;
    let mut plan = launch::plan(repo, resolved.clone(), pair.clone(), prompt)?;
    let mut _span = crate::telemetry::span(
        "ahu.launch",
        [
            (
                "ahu.agent.name",
                resolved
                    .as_ref()
                    .map_or_else(|| "auto".to_string(), |agent| agent.label()),
            ),
            ("ahu.harness", pair.harness.clone()),
            ("ahu.model.requested", pair.model.clone()),
            ("ahu.version", env!("CARGO_PKG_VERSION").to_string()),
        ],
    );
    if let Some(version) = plan
        .agent
        .as_ref()
        .map(|agent| agent.manifest.version.as_str())
    {
        _span.set_string("ahu.agent.version", version);
    }
    if let Some(version) = plan.enforcement.harness_version.as_deref() {
        _span.set_string("ahu.harness.version", version);
    }
    plan.apply_display(display)?;

    preflight(console, repo, loaded, &plan, prompt, dry_run)?;

    // Generated here, after the prompt has been read and after the plan is
    // built, so nothing in the prompt can have contained it.
    let code = confirmation_code()?;
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
        _span.set_string("ahu.status", "planned");
        if output_json {
            println!("{}", launch::render_json(&plan, prompt)?);
        }
        return Ok(0);
    }
    if confirm && !launcher::confirm_submit(console, &code)? {
        console.say("Cancelled. No worktree, branch, or session was created.\n")?;
        _span.set_string("ahu.status", "cancelled");
        return Ok(1);
    }

    let launched = launch::execute(repo, loaded, &plan, prompt, focus_new)?;
    console.say(&format!(
        "\nStarted @{} in cmux.\n  task       {}\n  worktree   {}\n\nOpen session: ahu focus {}\nList tasks:   ahu tasks\n",
        display_safe(&launched.record.identity.agent),
        display_safe(&crate::task_handles::label(repo, &launched.record.task_id)),
        display_path(launched.record.worktree.strip_prefix(&repo.root).unwrap_or(&launched.record.worktree)),
        display_safe(&crate::task_handles::reference(repo, &launched.record.task_id)),
    ))?;
    console.say(&style::stdout().paint(Role::Warning, &render_launch_notes(&launched.notes)))?;
    _span.set_string("ahu.status", "submitted");
    Ok(0)
}

pub(crate) fn preflight(
    console: &mut Console<'_>,
    repo: &Repo,
    loaded: &LoadedConfig,
    plan: &launch::LaunchPlan,
    prompt: &str,
    dry_run: bool,
) -> Result<()> {
    // First-load and overdue context hygiene review, before submission.
    let key = plan.agent_label();
    let review_state = hygiene::load_state(repo)?;
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
            hygiene::record_review(repo, &key)?;
        }
    }

    // Drift against the last launch of this same agent at this same version.
    let previous = task::list(repo)?;
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
        console.say(&style::stdout().paint(
            Role::Drift,
            "!! CONFIGURATION DRIFT: the selected agent's effective inputs changed.\n",
        ))?;
        console.say(&style::stdout().paint(Role::Drift, &drift::render(&found)))?;
    }

    Ok(())
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
    out.push_str(&cmux::integration::render_summary(&plan.cmux_integration));
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
fn confirmation_code() -> Result<String> {
    Ok(crate::orchestration::new_nonce()?[..6].to_string())
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
            if agent
                .manifest
                .source
                .as_ref()
                .map(|s| s.format.has_frontmatter())
                .unwrap_or(true)
            {
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
    out.push_str(&cmux::integration::render(&plan.cmux_integration));
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
pub fn task_dirs(repo: &Repo) -> Result<Vec<PathBuf>> {
    Ok(task::list(repo)?.dirs())
}

#[cfg(test)]
mod doctor_tests {
    use super::{local_collector_reachable, telemetry_collector_status};
    use crate::config::TelemetryConfig;
    use std::net::TcpListener;

    #[test]
    fn telemetry_is_off_by_default_and_describes_listener_evidence_precisely() {
        let config = TelemetryConfig::default();
        assert_eq!(
            telemetry_collector_status(&config),
            ("off (local telemetry is opt-in)".into(), false)
        );
    }

    #[test]
    fn local_collector_probe_only_checks_tcp_reachability() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(local_collector_reachable(listener.local_addr().unwrap()));
        let address = listener.local_addr().unwrap();
        drop(listener);
        assert!(!local_collector_reachable(address));
    }
}
