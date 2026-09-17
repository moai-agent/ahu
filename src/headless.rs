//! Unattended attempts with external artifacts and an ahu-owned supervisor.
//! Process and harness outcomes are evidence, never work acceptance.
use crate::harness::{LaunchCommand, LaunchRequest};
use crate::util::{Error, Result, digest_bytes};
use crate::{bail, state, task};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Options {
    pub background: bool,
    pub timeout_seconds: u64,
    pub native_helpers: String,
    #[serde(default)]
    pub native_helpers_explicit: bool,
    #[serde(default)]
    pub child_agents: Vec<String>,
    #[serde(default)]
    pub child_widened: Vec<String>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            background: false,
            timeout_seconds: 1800,
            native_helpers: "disabled".into(),
            native_helpers_explicit: false,
            child_agents: Vec::new(),
            child_widened: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    pub schema_version: u32,
    pub options: Options,
    pub harness_version: String,
    pub executable_digest: String,
    pub parent_task: Option<String>,
    #[serde(default)]
    pub parent_attempt: Option<u32>,
    #[serde(default)]
    pub root_task: Option<String>,
    #[serde(default)]
    pub broker_request: Option<String>,
    #[serde(default)]
    pub child_grants: Vec<crate::broker::ChildGrant>,
    pub depth: u32,
    pub attempt: u32,
    pub session: Option<String>,
    #[serde(default)]
    pub broker_dir: Option<PathBuf>,
    #[serde(default)]
    pub native_profile: Option<crate::native::Profile>,
    pub native_controls: Vec<String>,
    pub gaps: Vec<String>,
}

/// Batch arguments are built separately: interactive and resume parsers differ.
pub fn batch_command(
    harness: &str,
    request: &LaunchRequest<'_>,
    spec: &Spec,
) -> Result<LaunchCommand> {
    use crate::agent::Permissions;
    if request.model.is_empty() || request.model.starts_with('-') {
        bail!("an exact model is required");
    }
    if spec.options.native_helpers != "disabled" && spec.native_profile.is_none() {
        bail!(
            "bounded native helpers are unavailable: headless join, model/tool limits, and provider-owned cancellation have not been validated for this harness version. Use disabled or validate the adapter; ahu will not substitute a helper backend."
        );
    }
    let native_profile = if let Some(frozen) = &spec.native_profile {
        let rebuilt = build_native_profile(harness, request.model, spec)?;
        if &rebuilt != frozen {
            bail!("native policy differs from the frozen profile");
        }
        Some(rebuilt)
    } else {
        None
    };
    let mut args: Vec<String> = Vec::new();
    let mut add = |values: &[&str]| args.extend(values.iter().map(|v| v.to_string()));
    let program = match harness {
        "codex" => {
            add(&["exec"]);
            if spec.session.is_some() {
                add(&["resume"]);
            }
            add(&[
                "--model",
                request.model,
                "--json",
                "-c",
                "agents.enabled=false",
                "-c",
                "approval_policy=\"never\"",
            ]);
            match request.permissions {
                Permissions::Prompt => add(&["-c", "sandbox_mode=\"read-only\""]),
                Permissions::Auto => add(&["-c", "sandbox_mode=\"workspace-write\""]),
                Permissions::AcceptEdits if spec.session.is_none() => add(&["--approve-for-me"]),
                Permissions::AcceptEdits => bail!(
                    "Codex exec resume has no validated accept-edits mapping; submit a new assignment instead"
                ),
            }
            if spec.session.is_none() {
                add(&["--color", "never"]);
            }
            if let Some(dir) = &spec.broker_dir {
                let paths = toml::Value::Array(vec![toml::Value::String(
                    dir.to_string_lossy().into_owned(),
                )]);
                add(&[
                    "-c",
                    &format!("sandbox_workspace_write.writable_roots={paths}"),
                ]);
            }
            add(&["--"]);
            if let Some(session) = &spec.session {
                add(&[session]);
            }
            "codex"
        }
        "claude-code" => {
            add(&[
                "--print",
                "--model",
                request.model,
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-prompts",
                "none",
            ]);
            if let Some(profile) = &native_profile {
                add(&profile.args.iter().map(String::as_str).collect::<Vec<_>>());
            } else {
                add(&["--disallowedTools", "Agent,Task,TeamCreate,TeamDelete"]);
            }
            match request.permissions {
                Permissions::Prompt => (),
                Permissions::Auto => add(&["--permission-mode", "auto"]),
                Permissions::AcceptEdits => add(&["--permission-mode", "acceptEdits"]),
            }
            if let Some(session) = &spec.session {
                add(&["--resume", session]);
            }
            add(&["--"]);
            "claude"
        }
        "antigravity" => {
            add(&[
                "--model",
                request.model,
                "--output-format",
                "stream-json",
                "--print-timeout",
                &format!("{}s", spec.options.timeout_seconds),
            ]);
            match request.permissions {
                Permissions::Prompt => (),
                Permissions::AcceptEdits => add(&["--mode", "accept-edits"]),
                Permissions::Auto => add(&["--dangerously-skip-permissions"]),
            }
            if let Some(session) = &spec.session {
                add(&["--conversation", session]);
            }
            add(&["--print"]);
            "agy"
        }
        // Verified against OpenCode 1.18.30 on 2026-09-14 by running
        // `opencode run --format json` against a local Ollama-served model.
        // `run` is the non-interactive form: it takes the message as a
        // positional argument, streams one JSON event per line on stdout, and
        // exits without a listening port. `-i/--interactive` is the flag that
        // would make it a session, and this path never passes it.
        "opencode" => {
            add(&["run", "--format", "json", "--model", request.model]);
            match request.permissions {
                // As in the interactive adapter: OpenCode's permission actions
                // are static configuration, its only permission flag widens,
                // and there is no accept-edits equivalent to map onto.
                Permissions::Prompt => (),
                Permissions::Auto => add(&["--auto"]),
                Permissions::AcceptEdits => bail!(
                    "OpenCode has no accept-edits mode, so the headless path refuses it for the \
                     same reason the interactive adapter does: --auto would widen the request to \
                     every permission OpenCode does not explicitly deny, and passing nothing \
                     while reporting accept-edits would misdescribe the launch. Declare \
                     permissions = \"prompt\" or permissions = \"auto\"."
                ),
            }
            // `--session <id>` continues a recorded session; observed to keep
            // the same `sessionID` and the prior turns' context. `--fork` would
            // branch it instead, which would break the identity ahu recorded.
            if let Some(session) = &spec.session {
                add(&["--session", session]);
            }
            // Verified: `opencode run --format json -m <model> -- --version`
            // treated `--version` as the message rather than printing the
            // version, so `--` ends the option list here as it does elsewhere.
            add(&["--"]);
            "opencode"
        }
        _ => bail!("no headless adapter for {harness}; no fallback selected"),
    };
    let prompt_arg = Some(args.len());
    args.push(request.prompt.to_string());
    Ok(LaunchCommand {
        program: program.into(),
        args,
        prompt_arg,
    })
}

fn build_native_profile(harness: &str, model: &str, spec: &Spec) -> Result<crate::native::Profile> {
    let version = spec
        .harness_version
        .split_whitespace()
        .find(|v| crate::util::is_semver(v))
        .unwrap_or(&spec.harness_version);
    crate::native::profile(&crate::native::Request {
        harness,
        harness_version: version,
        policy: &spec.options.native_helpers,
        session_model: model,
        helper_model: None,
        helper_role: "ahu-reader",
        max_concurrent: 1,
        max_depth: 1,
        budget_usd: Some(5.0),
        assignment_writes: false,
    })
}

/// Only CLI versions whose argument surface was inspected are admitted.
///
/// The validated sets live in the catalog's `headless_verified_versions`, one
/// table instead of a second opinion in this module.
fn check_version(harness: &str, version: &str) -> Result<()> {
    let supported = crate::catalog::harness(harness)
        .map(|h| h.headless_verified_versions)
        .unwrap_or(&[]);
    // Probe output is `VERSION` plus optional decoration after whitespace; the
    // token is what got inspected, so the whole output must start with it.
    let token = version.split_whitespace().next().unwrap_or("");
    if !supported.contains(&token) {
        bail!(
            "unvalidated headless {harness} version {version:?}; supported CLI profiles: {supported:?}. Update the compatibility validation before launching; no fallback was selected."
        );
    }
    Ok(())
}

fn validate_executable(path: &Path, repo: &crate::git::Repo) -> Result<PathBuf> {
    if let Some(note) = crate::harness::wrapper_interposed(path) {
        bail!("headless launch refuses cmux wrappers: {note}");
    }
    let real = path.canonicalize()?;
    if real.starts_with(repo.primary_root()?.canonicalize()?) {
        bail!("headless executable must be outside the repository");
    }
    // Recognise script shims as well as the known cmux path conventions.
    let mut prefix = vec![0; 16384];
    let n = std::fs::File::open(&real)?.read(&mut prefix)?;
    if prefix[..n].starts_with(b"#!") && String::from_utf8_lossy(&prefix[..n]).contains("cmux") {
        bail!("headless launch refuses a script wrapper referencing cmux");
    }
    Ok(real)
}

/// Resolve existing ancestors first, then refuse redirected runtime components.
/// The caller-selected root may be outside the default home, but never in Git.
pub fn runtime_root() -> Result<PathBuf> {
    let raw = std::env::var_os("AHU_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state/ahu/runtime"))
        })
        .ok_or_else(|| {
            Error::new("set AHU_RUNTIME_DIR to a private directory outside repositories")
        })?;
    if !raw.is_absolute()
        || raw
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        bail!("runtime root must be an absolute path without '..'");
    }
    let mut existing = raw.as_path();
    let mut tail = Vec::new();
    while !existing.exists() {
        if std::fs::symlink_metadata(existing).is_ok() {
            bail!("runtime root contains a dangling symlink");
        }
        tail.push(
            existing
                .file_name()
                .ok_or_else(|| Error::new("invalid runtime root"))?
                .to_os_string(),
        );
        existing = existing
            .parent()
            .ok_or_else(|| Error::new("invalid runtime root"))?;
    }
    let mut root = existing.canonicalize()?;
    for component in tail.into_iter().rev() {
        root.push(component);
    }
    if root.ancestors().any(|p| p.join(".git").exists()) {
        bail!("runtime and artifacts must be outside every repository checkout");
    }
    Ok(root)
}

pub fn store(repo: &crate::git::Repo) -> Result<PathBuf> {
    Ok(runtime_root()?.join(repo.identity()))
}

pub(crate) fn confined(path: &Path, create: bool) -> Result<()> {
    let root = runtime_root()?;
    if !path.starts_with(&root) {
        bail!("task path is outside the external runtime root");
    }
    let mut cursor = root.clone();
    if let Ok(meta) = std::fs::symlink_metadata(&root) {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            bail!(
                "existing runtime root must be an owner-only directory owned by the current user"
            );
        }
    }
    if create && !cursor.exists() {
        state::create_private_dir_all(&cursor)?;
    }
    for part in path
        .strip_prefix(&root)
        .map_err(|e| Error::new(e.to_string()))?
        .components()
    {
        if !matches!(part, std::path::Component::Normal(_)) {
            bail!("invalid runtime component");
        }
        cursor.push(part);
        match std::fs::symlink_metadata(&cursor) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => bail!(
                "runtime directory is redirected or not a directory: {}",
                cursor.display()
            ),
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                state::create_private_dir_all(&cursor)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// A runtime directory held open, so removals below it resolve against the
/// inode that was validated rather than against its name.
///
/// `confined` validates a path, and a path is a name another process can
/// re-point. A same-user process that renames a validated attempt directory
/// away and drops a symlink in its place turns a later `remove_file` on
/// `<attempt>/<file>` into a deletion somewhere else entirely. Opening the
/// directory `O_NOFOLLOW | O_DIRECTORY` pins what was checked: every removal
/// here goes through this descriptor, so the swap has nothing left to redirect.
///
/// It does not make cleanup atomic. What happens *inside* the pinned directory
/// between the stat and the unlink is still a race -- but both halves run
/// against the same descriptor, and `unlinkat` without `AT_REMOVEDIR` removes
/// the entry itself, never what a symlink at that name points to.
struct ConfinedDir {
    path: PathBuf,
    handle: std::fs::File,
}

impl ConfinedDir {
    fn open(path: &Path) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        confined(path, false)?;
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)
            .map_err(|e| {
                Error::new(format!(
                    "refusing runtime directory {}: {e}",
                    path.display()
                ))
            })?;
        Ok(Self {
            path: path.to_path_buf(),
            handle,
        })
    }

    /// The kind of `name` in this directory, without following a symlink at it.
    /// `None` when nothing is there.
    fn kind(&self, name: &std::ffi::CStr) -> Result<Option<libc::mode_t>> {
        use std::os::unix::io::AsRawFd;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the descriptor is owned and open, the name is NUL-terminated,
        // and the buffer is sized by its own type.
        let probed = unsafe {
            libc::fstatat(
                self.handle.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if probed != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(self.io_error("inspect", name, error));
        }
        // SAFETY: `fstatat` returned success, so the buffer is initialised.
        Ok(Some(unsafe { stat.assume_init() }.st_mode & libc::S_IFMT))
    }

    /// Remove one entry by name. Reports whether anything was there.
    fn unlink(&self, name: &std::ffi::CStr) -> Result<bool> {
        use std::os::unix::io::AsRawFd;
        // SAFETY: as in `kind`. No `AT_REMOVEDIR`, so this removes the entry
        // itself rather than a directory or a symlink's target.
        if unsafe { libc::unlinkat(self.handle.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(false);
            }
            return Err(self.io_error("remove", name, error));
        }
        Ok(true)
    }

    /// Remove an artifact ahu wrote. Anything that is not a regular file at
    /// that name did not come from ahu, and is refused rather than deleted.
    fn remove_artifact(&self, name: &str) -> Result<bool> {
        let c_name = Self::entry_name(name)?;
        match self.kind(&c_name)? {
            None => Ok(false),
            Some(libc::S_IFREG) => self.unlink(&c_name),
            Some(_) => bail!(
                "refusing runtime artifact {}: expected a regular file.",
                self.path.join(name).display()
            ),
        }
    }

    /// Remove one mailbox entry. Directories are left alone; everything else,
    /// including a symlink left at the name, is unlinked where it stands.
    fn remove_mailbox_entry(&self, name: &std::ffi::OsStr) -> Result<bool> {
        let c_name = Self::entry_name(&name.to_string_lossy())?;
        match self.kind(&c_name)? {
            None | Some(libc::S_IFDIR) => Ok(false),
            Some(_) => self.unlink(&c_name),
        }
    }

    fn entry_name(name: &str) -> Result<std::ffi::CString> {
        std::ffi::CString::new(name)
            .map_err(|_| Error::new("runtime entry name contains an interior NUL"))
    }

    fn io_error(&self, action: &str, name: &std::ffi::CStr, error: std::io::Error) -> Error {
        Error::new(format!(
            "cannot {action} runtime entry {}: {error}",
            self.path.join(name.to_string_lossy().as_ref()).display()
        ))
    }
}

pub fn discover(repo: &crate::git::Repo) -> Result<Vec<PathBuf>> {
    let dir = store(repo)?;
    confined(&dir, false)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
        {
            confined(&entry.path(), false)?;
            if entry.path().join("task.json").exists() {
                out.push(entry.path());
            }
        }
    }
    Ok(out)
}

pub(crate) fn durable_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("runtime file needs a parent"))?;
    confined(parent, true)?;
    state::confine_file(path)?;
    let temp = parent.join(format!(".write-{}", crate::orchestration::new_nonce()));
    let mut file = state::create_new_private_file(&temp)?;
    let result = (|| -> Result<()> {
        serde_json::to_writer(&mut file, value).map_err(|e| Error::new(e.to_string()))?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    confined(
        path.parent().ok_or_else(|| Error::new("missing parent"))?,
        false,
    )?;
    serde_json::from_slice(&state::read_private_file(path)?)
        .map_err(|e| Error::new(format!("invalid {}: {e}", path.display())))
}
fn attempt_dir(dir: &Path, spec: &Spec) -> PathBuf {
    dir.join(format!("attempt-{}", spec.attempt))
}
fn frozen_digest(dir: &Path) -> Result<String> {
    let mut bytes = state::read_private_file(&dir.join("task.json"))?;
    bytes.extend(state::read_private_file(&dir.join("headless.json"))?);
    Ok(digest_bytes(&bytes))
}

#[allow(clippy::too_many_arguments)]
pub fn launch(
    console: &mut crate::launcher::Console<'_>,
    repo: &crate::git::Repo,
    agent: &str,
    prompt: &str,
    display: &crate::launch::DisplayMetadata,
    json_output: bool,
    dry_run: bool,
    allow: bool,
    mut options: Options,
) -> Result<i32> {
    if !dry_run
        && std::env::var_os("AHU_PARENT_TASK").is_some()
        && std::env::var_os("AHU_BROKER_DISPATCH").is_none()
    {
        return crate::broker::request(repo, agent, prompt, display, &options, allow, json_output);
    }
    let loaded = crate::config::load(&repo.root)?
        .ok_or_else(|| Error::new("project configuration is missing; run ahu init"))?;
    let (agent, pair) = crate::commands::resolve_identity(repo, &loaded, Some(agent))?;
    if !options.native_helpers_explicit {
        let project: toml::Value = toml::from_str(&std::fs::read_to_string(&loaded.path)?)
            .map_err(|e| Error::new(e.to_string()))?;
        let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(
            &agent
                .as_ref()
                .ok_or_else(|| Error::new("named agent required"))?
                .manifest_path,
        )?)
        .map_err(|e| Error::new(e.to_string()))?;
        if let Some(value) = manifest.get("native_helpers").or_else(|| {
            project
                .get("execution")
                .and_then(|v| v.get("native_helpers"))
        }) {
            options.native_helpers = value
                .as_str()
                .ok_or_else(|| Error::new("native_helpers must be disabled or bounded"))?
                .into();
        }
    }
    let permissions = agent
        .as_ref()
        .map(|a| a.manifest.permissions)
        .unwrap_or_default();
    if permissions.widens_defaults() && !allow {
        bail!(
            "manifest approval widening requires --allow-widened-approvals for headless launches too"
        );
    }
    // The catalog is the one table of which harnesses have a validated batch
    // profile and which executable to probe for them. A harness with an
    // interactive adapter still needs its batch argument surface and event
    // stream validated separately before it can run here, so the refusal below
    // is named: the caller's next question is always "which one?", and the only
    // correct answer to it is to report the refusal.
    let entry = crate::catalog::harness(&pair.harness)
        .filter(|h| h.supports(crate::catalog::Feature::HeadlessLaunch))
        .ok_or_else(|| {
            crate::util::Error::new(format!(
                "unsupported headless harness {:?}; ahu has no validated batch profile for it and selected no fallback.",
                pair.harness
            ))
        })?;
    let program = entry.executable;
    let candidate = crate::selection::resolve_executable(program)
        .ok_or_else(|| Error::new("harness executable unavailable"))?;
    validate_executable(Path::new(&candidate), repo)?;
    let mut plan = crate::launch::plan(repo, agent, pair, prompt)?;
    plan.apply_display(display)?;
    // cmux integration environment is removed before supervisor and worker execution.
    plan.hooks.wrapper_injected = false;
    let executable = validate_executable(&plan.harness_executable, repo)?;
    let version = plan
        .enforcement
        .harness_version
        .clone()
        .ok_or_else(|| Error::new("cannot determine installed harness version"))?;
    check_version(&plan.pair.harness, &version)?;
    let parent_task = std::env::var("AHU_PARENT_TASK").ok();
    let depth = if let Some(parent) = &parent_task {
        let parent_dir = lookup(repo, parent)?;
        let parent_spec: Spec = read_json(&parent_dir.join("headless.json"))?;
        let parent_record = task::load(&parent_dir)?;
        validate_frozen_configuration(&parent_record)?;
        if parent_record.worktree.canonicalize()? != repo.root.canonicalize()? {
            bail!("child must launch from its owning task worktree");
        }
        if parent_dir.join("cancel.json").exists() {
            bail!("parent task is cancelling; child admission refused");
        }
        parent_spec.depth + 1
    } else {
        0
    };
    if depth > 8 {
        bail!("ahu child depth limit (8) reached");
    }
    let child_grants = if parent_task.is_some() {
        if !options.child_agents.is_empty() || !options.child_widened.is_empty() {
            bail!("a child cannot expand the host delegation grant");
        }
        serde_json::from_str(
            &std::env::var("AHU_FROZEN_CHILD_GRANTS").unwrap_or_else(|_| "[]".into()),
        )
        .map_err(|e| Error::new(e.to_string()))?
    } else {
        crate::broker::freeze_grants(repo, &options.child_agents, &options.child_widened)?
    };
    if let Ok(expected) = std::env::var("AHU_EXPECTED_CHILD_IDENTITY")
        && plan.agent.as_ref().map(|a| a.identity_digest()).as_deref() != Some(expected.as_str())
    {
        bail!("registered child identity changed from the host-approved grant");
    }
    if let Ok(expected) = std::env::var("AHU_EXPECTED_CHILD_SNAPSHOT")
        && plan.snapshot.digest() != expected
    {
        bail!("child configuration changed from the host-approved snapshot");
    }
    if let Ok(expected) = std::env::var("AHU_EXPECTED_CHILD_HOOKS")
        && plan.hooks.digest() != expected
    {
        bail!("child hooks changed from the host-approved inventory");
    }
    let root_task = if let Some(parent) = &parent_task {
        let parent_spec: Spec = read_json(&lookup(repo, parent)?.join("headless.json"))?;
        parent_spec.root_task.unwrap_or_else(|| parent.clone())
    } else {
        plan.task_id.clone()
    };
    let mut spec = Spec { schema_version: 1, options, harness_version: version, executable_digest: digest_bytes(&std::fs::read(&executable)?),
        root_task: Some(root_task),
        parent_attempt: std::env::var("AHU_PARENT_ATTEMPT").ok().and_then(|v| v.parse().ok()),
        broker_request: std::env::var("AHU_BROKER_DISPATCH").ok(),
        child_grants,
        parent_task, depth, attempt: 1, session: None,
        broker_dir: Some(store(repo)?.join(&plan.task_id).join("requests")),
        native_profile: None,
        native_controls: match plan.pair.harness.as_str() {
            "codex" => vec!["agents.enabled=false".into()],
            "claude-code" => vec!["deny Agent, Task, TeamCreate, TeamDelete".into()],
            _ => vec!["disabled by prompt guidance only; no validated native tool switch".into()],
        },
        gaps: vec!["CLI argument profiles are inspected; authenticated end-to-end and provider-managed child lifecycle probes remain opt-in.".into(),
            "Native tools cannot prevent delegation through arbitrary shell tools, plugins or external services; no global token/concurrency cap is claimed.".into(),
            "Agent reports and same-user editable runtime records are untrusted evidence. Orchestrator acceptance is separate.".into(),
            "Native session stores use existing external harness homes and credentials; their retention and disk limits are not owned by ahu.".into()],
    };
    spec.gaps.extend([
        "Stderr classification recognizes a bounded set of permission/authentication/quota signatures conservatively; other stderr is preserved with explicit diagnostic uncertainty. Authenticated agy denial coverage remains unvalidated.".into(),
        "Automatic timeout/capture failure and explicit cancellation stop registered descendants of the current attempt; results record bounded reconciliation and unknown cleanup. Supervisor crash and provider-managed work remain outside guaranteed cleanup.".into(),
        "Broker scans at most 32 entries per tick, retains at most 256 consumed requests per attempt, and stops admission above 4096 entries per scan. Cleanup removes at most 4097 inbox entries per call while retaining private claims/responses; arbitrary worker filesystem writes require external disk quotas.".into(),
        "Child resume and worker-originated resume are unsupported: submit a new registered assignment through the broker or a new root from the host with explicit grants.".into(),
    ]);
    let profile = build_native_profile(&plan.pair.harness, &plan.pair.model, &spec)?;
    spec.native_controls = profile.control_ids();
    spec.gaps.extend(profile.gaps.clone());
    spec.gaps.push("Integration inventory is incomplete for Codex and agy. ahu itself never invokes cmux in this backend; arbitrary native configuration/hooks and shell commands require a vetted cmux-free environment. Same-UID code is not isolated from supervisor state.".into());
    if !spec.child_grants.is_empty() {
        spec.gaps.push("Broker grants approve only frozen registered identities and profiles. Codex workspace-write receives the task's requests directory as a narrow additional write root; read-only Codex transport is refused. Limits: 128 tasks per root grant, depth 8, 16 active/interrupted tasks per repository, no hidden queue or global token cap.".into());
    }

    if spec.options.native_helpers == "bounded" {
        spec.gaps.push("The ENTIRE assignment has a read-only MODEL TOOL ceiling: parent and helpers have no model tools for editing, building, shell commands or shell-launching registered children. Settings-defined hooks are outside that tool ceiling and their side effects are not proven read-only. MCP tools and slash commands are disabled; repository settings remain discoverable. Roles are requested/observed, not an allowlist; total helper count is not capped. Budget is 5 USD per attempt, concurrency 1, depth 1, helper model equals manifest model.".into());
    }
    spec.native_profile = Some(profile);
    validate_environment(repo, &plan.hooks)?;
    let (delivered, delivery) = crate::orchestration::deliver_headless_policy(
        plan.agent.as_ref().map(|a| a.instructions.as_str()),
        prompt,
        &spec.options.native_helpers,
    )?;
    plan.delivery = delivery;
    plan.command = batch_command(
        &plan.pair.harness,
        &LaunchRequest {
            model: &plan.pair.model,
            prompt: &delivered,
            cwd: &plan.worktree,
            permissions,
        },
        &spec,
    )?;
    plan.harness_executable = executable;
    plan.task_dir = store(repo)?.join(&plan.task_id);
    confined(&plan.task_dir, false)?;
    crate::commands::preflight(console, repo, &loaded, &plan, prompt, dry_run)?;
    let preview = json!({"schema_version":1,"backend":"headless","task_id":plan.task_id,"worktree":plan.worktree,
        "branch":plan.branch,"command":plan.command.redacted(),"executable":plan.harness_executable,"capabilities":spec,
        "runtime":plan.task_dir,"identity": {"agent":plan.agent_label(),"harness":plan.pair.harness,"model":plan.pair.model},
        "acceptance":"not assessed"});
    if dry_run {
        emit(&preview, json_output)?;
        return Ok(0);
    }
    console.say(&format!(
        "Headless {} on {} / {}; output {}\n",
        plan.agent_label(),
        plan.pair.harness,
        plan.pair.model,
        plan.task_dir.display()
    ))?;
    let _lock = Lock::acquire(&{
        let dir = store(repo)?;
        confined(&dir, true)?;
        dir.join("launch.lock")
    })?;
    if let Some(parent) = &spec.parent_task
        && lookup(repo, parent)?.join("cancel.json").exists()
    {
        bail!("parent is cancelling; child admission refused");
    }
    validate_parent_attempt(repo, &spec)?;
    let previous = discover(repo)?;
    let mut tree_count = 0;
    for dir in &previous {
        let recorded: Spec = read_json(&dir.join("headless.json"))?;
        if recorded.root_task == spec.root_task {
            tree_count += 1;
        }
    }
    if tree_count >= 128 {
        bail!(
            "tree_capacity_exceeded: root delegation grant is limited to 128 assignments; no task was queued"
        );
    }
    let active = previous
        .iter()
        .filter(|dir| {
            let Ok(spec) = read_json::<Spec>(&dir.join("headless.json")) else {
                return true;
            };
            !attempt_dir(dir, &spec).join("result.json").exists()
        })
        .count();
    if active >= 16 {
        bail!(
            "capacity_exceeded: 16 active or interrupted assignments; inspect existing tasks before retrying. No work was queued"
        );
    }
    if plan.task_dir.exists() || crate::git::branch_exists(repo, &plan.branch)? {
        bail!("task already exists; refusing duplicate submission");
    }
    let base = plan
        .base_commit
        .as_deref()
        .ok_or_else(|| Error::new("missing base commit"))?;
    crate::git::add_worktree(repo, &plan.worktree, &plan.branch, base)?;
    // Preserve preparation failures for review; never delete reviewable work.
    let materialize = crate::snapshot::materialize(&repo.root, &plan.snapshot, &plan.worktree)?;
    if !materialize.concurrently_modified.is_empty() {
        bail!(
            "configuration changed during preparation; worktree preserved at {}",
            plan.worktree.display()
        );
    }
    confined(&plan.task_dir, true)?;
    let record = crate::launch::prepared_record(repo, &loaded, &plan, prompt, materialize);
    task::save(&plan.task_dir, &record, prompt)?;
    durable_json(&plan.task_dir.join("headless.json"), &spec)?;
    drop(_lock);
    if spec.options.background {
        start(&plan.task_dir, &spec)?;
        emit(&preview, json_output)?;
        Ok(0)
    } else {
        confined(&attempt_dir(&plan.task_dir, &spec), true)?;
        let outcome = supervise_authorized(&plan.task_dir, &frozen_digest(&plan.task_dir)?);
        if let Err(error) = outcome {
            console.say(&format!("Headless execution failed: {error}\n"))?;
        }
        wait(&plan.task_dir, json_output)
    }
}

pub(crate) fn validate_frozen_configuration(record: &task::TaskRecord) -> Result<()> {
    let loaded = crate::config::load(&record.worktree)?
        .ok_or_else(|| Error::new("task configuration is missing"))?;
    let snapshot = crate::snapshot::collect(&record.worktree)?;
    let mut hooks = crate::hooks::collect(&record.worktree, &record.identity.harness)?;
    let mut expected_hooks = record.hooks.clone();
    if expected_hooks.digest() != record.hooks_digest {
        bail!("recorded hook inventory integrity mismatch");
    }
    // Submission may come from a cmux pane; neither worker backend nor hook content changes.
    hooks.wrapper_injected = false;
    expected_hooks.wrapper_injected = false;
    if loaded.digest != record.policy_digest
        || snapshot.digest() != record.config_snapshot_digest
        || hooks.digest() != expected_hooks.digest()
    {
        bail!(
            "live task configuration, snapshot, or hooks changed since submission; execution refused (policy={}, snapshot={}, hooks={})",
            loaded.digest != record.policy_digest,
            snapshot.digest() != record.config_snapshot_digest,
            hooks.digest() != expected_hooks.digest()
        );
    }
    Ok(())
}

fn resolve_missing_path(raw: &Path) -> Result<PathBuf> {
    if !raw.is_absolute()
        || raw
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        bail!("external state path must be absolute without '..'");
    }
    let mut existing = raw;
    let mut tail = Vec::new();
    while !existing.exists() {
        if std::fs::symlink_metadata(existing).is_ok() {
            bail!("external state path contains a dangling symlink");
        }
        tail.push(
            existing
                .file_name()
                .ok_or_else(|| Error::new("invalid external path"))?
                .to_os_string(),
        );
        existing = existing
            .parent()
            .ok_or_else(|| Error::new("invalid external path"))?;
    }
    let mut resolved = existing.canonicalize()?;
    for part in tail.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn validate_environment(
    repo: &crate::git::Repo,
    hooks: &crate::hooks::HookInventory,
) -> Result<()> {
    for variable in [
        "HOME",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "XDG_STATE_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "TMPDIR",
        "TMP",
        "TEMP",
        "CLAUDE_CODE_TMPDIR",
    ] {
        if let Some(path) = std::env::var_os(variable) {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                bail!("{variable} must point outside checkouts for headless execution");
            }
            let path = resolve_missing_path(&path)?;
            if path.starts_with(repo.primary_root()?.canonicalize()?)
                || path.ancestors().any(|p| p.join(".git").exists())
            {
                bail!(
                    "{variable} would place native state inside a checkout; choose an external harness home"
                );
            }
        }
    }
    for hook in &hooks.hooks {
        if hook.command.as_deref().is_some_and(|s| s.contains("cmux")) {
            bail!(
                "a configured lifecycle hook references cmux; remove that dependency before using headless execution"
            );
        }
    }
    // Snapshot covers repository-local configuration; inspect only integration files.
    for file in [
        ".mcp.json",
        ".claude/settings.json",
        ".claude/settings.local.json",
        ".codex/config.toml",
        ".agents/settings.json",
    ] {
        if let Ok(body) = std::fs::read_to_string(repo.root.join(file))
            && body.contains("cmux")
        {
            bail!("{file} references cmux; headless execution requires cmux-free integrations");
        }
    }
    Ok(())
}

pub(crate) fn emit(value: &Value, json_output: bool) -> Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(|e| Error::new(e.to_string()))?;
    if json_output {
        println!("{text}");
    } else {
        println!("{}", crate::util::display_safe_block(&text));
    }
    Ok(())
}

/// OS-owned locks are released on process loss, never stolen based on age.
struct Lock(std::fs::File);
impl Lock {
    fn acquire(path: &Path) -> Result<Self> {
        Self::try_acquire(path)?.ok_or_else(|| {
            Error::new(
                "attempt is already owned by a live supervisor; concurrent execution refused",
            )
        })
    }
    fn is_owned(path: &Path) -> Result<bool> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        // Inspection neither creates nor writes the ownership file.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        if !file.metadata()?.is_file() {
            bail!("ownership lock is not a regular file");
        }
        // SAFETY: file owns a valid descriptor; dropping it releases a successful probe.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Ok(true)
        } else {
            Err(error.into())
        }
    }
    fn try_acquire(path: &Path) -> Result<Option<Self>> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        state::confine_file(path)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        // SAFETY: file owns a valid descriptor for the duration of the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(error.into());
        }
        Ok(Some(Self(file)))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: this object still owns the descriptor.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn sanitize(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CMUX_") {
            command.env_remove(key);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        let paths = std::env::split_paths(&path)
            .filter(|p| !p.to_string_lossy().contains("cmux-cli-shims"))
            .collect::<Vec<_>>();
        if let Ok(path) = std::env::join_paths(paths) {
            command.env("PATH", path);
        }
    }
    command.env_remove("AHU_EXPECTED_DIGEST");
}

fn start(dir: &Path, spec: &Spec) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let attempt = attempt_dir(dir, spec);
    confined(&attempt, true)?;
    let stderr = state::create_new_private_file(&attempt.join("supervisor.log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["supervise", "--task-dir"])
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    sanitize(&mut command);
    command
        .env("AHU_RUNTIME_DIR", runtime_root()?)
        .env("AHU_EXPECTED_DIGEST", frozen_digest(dir)?);
    // SAFETY: setsid is async-signal-safe and accesses no Rust state after fork.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if attempt.join("started.json").exists() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            let detail =
                std::fs::read_to_string(attempt.join("supervisor.log")).unwrap_or_default();
            bail!(
                "supervisor exited before startup acknowledgement ({status}); inspect {}: {}",
                attempt.display(),
                detail
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "supervisor startup acknowledgement timed out; task is preserved at {}. Inspect result before retrying",
                dir.display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Events {
    pub session: Option<String>,
    pub terminal: bool,
    pub failed: bool,
    pub summary: String,
    pub blockers: Vec<String>,
    pub unknown_events: u64,
    #[serde(default)]
    pub stderr_diagnostics: Vec<String>,
    #[serde(default)]
    pub stderr_unclassified_lines: u64,
    pub native_observations: Vec<Value>,
    #[serde(default)]
    pub native: crate::native::Observations,
}
impl Events {
    fn observe_stderr(&mut self, line: &[u8]) {
        let text = String::from_utf8_lossy(line);
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let lower = text.to_ascii_lowercase();
        let category = if [
            "permission denied",
            "permission_denied",
            "tool denied",
            "tool execution denied",
            "not authorized",
            "access denied",
        ]
        .iter()
        .any(|p| lower.contains(p))
        {
            Some("permission denial")
        } else if [
            "resource_exhausted",
            "quota exhausted",
            "quota exceeded",
            "quota reached",
            "rate limit exceeded",
        ]
        .iter()
        .any(|p| lower.contains(p))
        {
            Some("quota/rate limit failure")
        } else if [
            "authentication failed",
            "unauthenticated",
            "invalid api key",
            "invalid_api_key",
        ]
        .iter()
        .any(|p| lower.contains(p))
        {
            Some("authentication failure")
        } else {
            None
        };
        if let Some(category) = category {
            self.failed = true;
            if self.stderr_diagnostics.len() < 32 {
                self.stderr_diagnostics.push(format!(
                    "{category}: {}",
                    text.chars().take(512).collect::<String>()
                ));
            }
        } else {
            self.stderr_unclassified_lines += 1;
        }
    }

    pub fn observe(&mut self, harness: &str, line: &[u8]) {
        let event: Value = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(_) => {
                self.failed = true;
                self.blockers.push("malformed or truncated event".into());
                return;
            }
        };
        self.native.observe(harness, &event);
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let session = event
            .get("session_id")
            .or_else(|| event.get("thread_id"))
            .or_else(|| event.get("conversation_id"))
            // OpenCode spells it `sessionID`, on every event including its
            // error events, which is what makes its errors resumable at all.
            .or_else(|| event.get("sessionID"))
            .and_then(Value::as_str);
        if let Some(session) = session {
            if let Some(previous) = &self.session {
                if previous != session {
                    self.failed = true;
                    self.blockers
                        .push("session identity changed within attempt".into());
                }
            } else {
                self.session = Some(session.into());
            }
        }
        if let Some(errors) = event.get("errors").and_then(Value::as_array) {
            for error in errors.iter().take(128) {
                self.blockers.push(
                    error
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| error.to_string()),
                );
            }
            if !errors.is_empty() {
                self.failed = true;
            }
        }
        if let Some(error) = event.get("error").filter(|v| !v.is_null()) {
            self.blockers.push(
                error
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| error.to_string()),
            );
            self.failed = true;
        }
        if kind.contains("denied")
            || event
                .get("permission_denials")
                .and_then(Value::as_array)
                .is_some_and(|v| !v.is_empty())
        {
            self.failed = true;
            self.blockers
                .push("harness reported permission denials; inspect captured events".into());
        }
        if (event
            .get("parent_tool_use_id")
            .is_some_and(|v| !v.is_null())
            || kind.contains("collab"))
            && self.native_observations.len() < 256
        {
            self.native_observations.push(event.clone());
        }
        match (harness, kind) {
            ("codex", "thread.started") | ("claude-code", "system") | ("antigravity", "init") => (),
            ("codex", "turn.completed") => self.finish(false),
            (_, "turn.failed" | "error") => {
                self.failed = true;
                self.blockers.push("harness emitted an error".into());
            }
            ("codex", "item.completed") => {
                if event.pointer("/item/type").and_then(Value::as_str) == Some("agent_message") {
                    self.summary = event
                        .pointer("/item/text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into();
                }
            }
            ("claude-code" | "antigravity", "result") => {
                let status = event
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let subtype = event.get("subtype").and_then(Value::as_str).unwrap_or("");
                let recognized_success = if harness == "claude-code" {
                    subtype == "success"
                        && event.get("is_error").and_then(Value::as_bool) == Some(false)
                        && event.get("result").and_then(Value::as_str).is_some()
                } else {
                    matches!(status.as_str(), "success" | "succeeded" | "ok")
                        && event
                            .get("response")
                            .or_else(|| event.get("result"))
                            .and_then(Value::as_str)
                            .is_some()
                };
                let failed = !recognized_success
                    || event
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    || matches!(
                        status.as_str(),
                        "error" | "failed" | "cancelled" | "timeout"
                    )
                    || subtype.starts_with("error")
                    || event.get("error").is_some_and(|v| !v.is_null());
                self.summary = event
                    .get("result")
                    .or_else(|| event.get("response"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into();
                self.finish(failed);
            }
            // OpenCode's `run --format json` stream, observed on 1.18.30. Every
            // event is `{type, timestamp, sessionID, part}`; the work of a turn
            // is a sequence of steps, and only the reason on a step's finish
            // says whether the turn ended or another step follows.
            ("opencode", "step_start") => (),
            ("opencode", "text") => {
                if let Some(text) = event.pointer("/part/text").and_then(Value::as_str) {
                    self.summary = text.into();
                }
            }
            ("opencode", "tool_use") => {
                // A refused tool call is reported as an ordinary tool error:
                // `state.status: "error"` with the refusal as `state.error`.
                // Observed with a `write` call on a launch that passed no
                // --auto — and the run then ended with no terminal step and
                // still exited 0, which is exactly why the exit status is not
                // allowed to stand in for a result anywhere in this file.
                let refused = event
                    .pointer("/part/state/error")
                    .and_then(Value::as_str)
                    .is_some_and(|error| {
                        error.to_ascii_lowercase().contains("rejected permission")
                    });
                if refused {
                    self.failed = true;
                    self.blockers.push(
                        "harness reported permission denials; inspect captured events".into(),
                    );
                }
            }
            ("opencode", "step_finish") => {
                match event.pointer("/part/reason").and_then(Value::as_str) {
                    Some("stop") => self.finish(false),
                    // The model is about to run tools and another step follows.
                    // Treating this as terminal would score an assignment
                    // complete at its first tool call.
                    Some("tool-calls") | None => (),
                    // Anything else — a token ceiling, an abort — ends the turn
                    // without the model having said it is done. Recorded as a
                    // terminal failure rather than left to look like silence.
                    Some(other) => {
                        self.blockers
                            .push(format!("harness ended the step for reason {other:?}"));
                        self.finish(true);
                    }
                }
            }
            (
                _,
                "assistant" | "user" | "step_update" | "turn.started" | "item.started"
                | "item.updated" | "stream_event",
            ) => (),
            _ => self.unknown_events += 1,
        }
    }
    fn finish(&mut self, failed: bool) {
        if self.terminal {
            self.failed = true;
            self.blockers.push("multiple terminal events".into());
        }
        self.terminal = true;
        self.failed |= failed;
    }
}

const CAPTURE_LIMIT: u64 = 64 * 1024 * 1024;
const LINE_LIMIT: usize = 1024 * 1024;

const MAX_EVENT_JSON_DEPTH: usize = 64;
const WRITE_LIKE_TOOLS: [&str; 10] = [
    "write",
    "edit",
    "multiedit",
    "notebookedit",
    "applypatch",
    "editfile",
    "writefile",
    "createfile",
    "strreplaceeditor",
    "strreplacebasededit",
];
const WRITE_PATH_KEYS: [&str; 7] = [
    "filepath",
    "file",
    "path",
    "notebookpath",
    "abspath",
    "targetfile",
    "content",
];
const WRITE_NAME_KEYS: [&str; 3] = ["tool", "name", "toolname"];
const WRITE_INPUT_KEYS: [&str; 2] = ["input", "arguments"];

/// Lowercase ASCII alphanumerics only: `file_path` and `filePath` both
/// normalize to `filepath`, `_` separators vanish.
fn normalize_token(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Lexically collapse `.` and `..` without touching the filesystem.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => (),
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// Canonicalize, or resolve via the nearest existing ancestor so paths the
/// harness named but never created are still classified. Lexical fallback
/// only when even the ancestor cannot be canonicalized (e.g. CWD removed).
fn real_path(path: &Path) -> PathBuf {
    if let Ok(real) = path.canonicalize() {
        return real;
    }
    let mut tail = Vec::new();
    let mut current = path.to_path_buf();
    while !current.exists() {
        let Some(file_name) = current.file_name().map(Path::to_path_buf) else {
            break;
        }
        current = current.parent().map(Path::to_path_buf).unwrap_or_default();
        tail.push(file_name);
        if current.as_os_str().is_empty() {
            break;
        }
    }
    if let Ok(base) = current.canonicalize() {
        let mut real = base;
        for file_name in tail.iter().rev() {
            real.push(file_name);
        }
        return real;
    }
    lexical_normalize(path)
}

/// Resolve a recorded write-path candidate against the worktree. The stub
/// cwd is the worktree, so relative paths are worktree-relative by
/// construction; absolute paths are kept as the harness named them.
fn resolve_write_path(candidate: &str, worktree: &Path) -> Option<PathBuf> {
    if candidate.is_empty() {
        return None;
    }
    let raw: PathBuf = candidate.into();
    let joined = if raw.is_absolute() {
        raw
    } else {
        worktree.join(raw)
    };
    let resolved = lexical_normalize(&joined);
    if resolved.as_os_str().is_empty() || !resolved.is_absolute() {
        return None;
    }
    Some(resolved)
}

/// Walk a write-tool call's subtree, collecting path candidates and decoding
/// string `input`/`arguments` payloads that embed JSON.
fn collect_write_paths_within(value: &Value, depth: usize, candidates: &mut Vec<String>) {
    let mut stack: Vec<(Cow<'_, Value>, usize)> = vec![(Cow::Borrowed(value), depth)];
    while let Some((value, depth)) = stack.pop() {
        if depth >= MAX_EVENT_JSON_DEPTH {
            continue;
        }
        match value {
            Value::Object(map) => {
                for (key, child) in map.iter() {
                    let key = normalize_token(key);
                    if WRITE_PATH_KEYS.contains(&key.as_str()) {
                        if let Some(path) = child.as_str() {
                            candidates.push(path.to_string());
                        }
                    }
                    if WRITE_INPUT_KEYS.contains(&key.as_str()) {
                        if let Some(embedded) = child.as_str() {
                            if let Ok(decoded) = serde_json::from_str::<Value>(embedded) {
                                stack.push((Cow::Owned(decoded), depth + 1));
                            }
                        }
                    }
                    stack.push((Cow::Borrowed(child), depth + 1));
                }
            }
            Value::Array(items) => {
                for child in items.iter() {
                    stack.push((Cow::Borrowed(child), depth + 1));
                }
            }
            _ => (),
        }
    }
}

/// Walk one event, collecting write-path candidates from any object whose
/// name-ish key names a write-like tool. The main walk does not decode
/// embedded JSON in string `input`/`arguments` values; the within-walk
/// triggered by a matching tool name does.
fn collect_write_paths(event: &Value) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut stack: Vec<(&Value, usize)> = vec![(event, 0)];
    while let Some((value, depth)) = stack.pop() {
        if depth >= MAX_EVENT_JSON_DEPTH {
            continue;
        }
        match value {
            Value::Object(map) => {
                let is_write_tool = map.iter().any(|(key, child)| {
                    normalize_token(key) == "tool"
                        || WRITE_NAME_KEYS.contains(&normalize_token(key).as_str())
                        && child.as_str().map(normalize_token).is_some_and(|tool| {
                            WRITE_LIKE_TOOLS.contains(&tool.as_str())
                        })
                });
                if is_write_tool {
                    collect_write_paths_within(value, depth, &mut candidates);
                }
                for (_, child) in map.iter() {
                    stack.push((child, depth + 1));
                }
            }
            Value::Array(items) => {
                for child in items.iter() {
                    stack.push((child, depth + 1));
                }
            }
            _ => (),
        }
    }
    candidates
}

/// Post-run, disclose-only scan: read the captured event stream and list
/// write-tool call paths that resolve outside the task worktree. Never
/// fails; a missing or unreadable stream scans as empty.
fn scan_writes_outside_worktree(events_path: &Path, worktree: &Path) -> Vec<String> {
    let Ok(file) = std::fs::File::open(events_path) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(file);
    let mut total = 0u64;
    let mut line = Vec::new();
    let mut candidates: Vec<String> = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(read) => total += read as u64,
        }
        if total > CAPTURE_LIMIT {
            break;
        }
        if line.len() > LINE_LIMIT {
            continue;
        }
        while matches!(line.last(), Some(b'\n') | Some(b'\r')) {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        candidates.extend(collect_write_paths(&event));
    }
    let worktree_real = real_path(worktree);
    let mut seen: HashSet<String> = HashSet::new();
    let mut outside = Vec::new();
    for candidate in candidates {
        let Some(resolved) = resolve_write_path(&candidate, &worktree_real) else {
            continue;
        };
        let real = real_path(&resolved);
        if !real.starts_with(&worktree_real) {
            let recorded = real.to_string_lossy().into_owned();
            if seen.insert(recorded.clone()) {
                outside.push(recorded);
            }
        }
    }
    outside.truncate(128);
    outside
}

/// Recorded `writes_outside_worktree` from a finished attempt's result
/// envelope, if it exists and is non-empty. Never fails the caller.
pub(crate) fn recorded_writes_outside_worktree(dir: &Path) -> Option<Vec<String>> {
    let spec: Spec = read_json(&dir.join("headless.json")).ok()?;
    let result_path = attempt_dir(dir, &spec).join("result.json");
    let result: Value = read_json(&result_path).ok()?;
    let paths: Vec<String> = result
        .get("writes_outside_worktree")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    (!paths.is_empty()).then_some(paths)
}

/// Pipe readers never wait for EOF from an escaped descendant: the supervisor
/// polls process completion independently and bounds the final drain.
fn capture(
    mut pipe: impl Read + Send + 'static,
    path: PathBuf,
    harness: Option<String>,
    tx: std::sync::mpsc::Sender<std::result::Result<Events, String>>,
) {
    std::thread::spawn(move || {
        let mut run = || -> Result<Events> {
            let mut file = state::create_new_private_file(&path)?;
            let mut total = 0u64;
            let mut chunk = [0u8; 8192];
            let mut line = Vec::new();
            let mut events = Events::default();
            loop {
                let n = pipe.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                if total > CAPTURE_LIMIT {
                    bail!("capture exceeded 64 MiB; attempt stopped to preserve reviewability");
                }
                file.write_all(&chunk[..n])?;
                {
                    for byte in &chunk[..n] {
                        if *byte == b'\n' {
                            if !line.is_empty() {
                                if let Some(harness) = &harness {
                                    events.observe(harness, &line);
                                } else {
                                    events.observe_stderr(&line);
                                }
                                events.blockers.truncate(128);
                            }
                            line.clear();
                        } else {
                            if line.len() >= LINE_LIMIT {
                                bail!("event exceeds 1 MiB");
                            }
                            line.push(*byte);
                        }
                    }
                }
            }
            if !line.is_empty() {
                if harness.is_none() {
                    events.observe_stderr(&line);
                } else {
                    events.failed = true;
                    events.blockers.push("unterminated event stream".into());
                }
            }
            file.sync_all()?;
            Ok(events)
        };
        let _ = tx.send(run().map_err(|e| e.to_string()));
    });
}

/// Observe exit without reaping: the child PID cannot be reused before group cleanup.
pub(crate) fn child_exited(pid: u32) -> std::io::Result<bool> {
    // SAFETY: zero is a valid initial siginfo_t; waitid initializes it on an event.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: info points to writable memory and pid is a child owned by the caller.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: si_pid reads the initialized SIGCHLD field populated by waitid.
    Ok(unsafe { info.si_pid() } != 0)
}

fn signal_group(pid: u32, signal: i32) {
    // The caller owns a live/reaped child in this fresh process group. No PID
    // from a persisted record is ever used as signal authority.
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: negative child PID selects the group this supervisor created.
        unsafe {
            libc::kill(-pid, signal);
        }
    }
}

fn validate_parent_attempt(repo: &crate::git::Repo, spec: &Spec) -> Result<()> {
    let Some(parent) = &spec.parent_task else {
        return Ok(());
    };
    let dir = lookup(repo, parent)?;
    let parent_spec: Spec = read_json(&dir.join("headless.json"))?;
    if spec.parent_attempt != Some(parent_spec.attempt) {
        bail!("stale parent attempt cannot admit or start a child");
    }
    let attempt = attempt_dir(&dir, &parent_spec);
    if dir.join("cancel.json").exists()
        || attempt.join("admission-closed.json").exists()
        || attempt.join("result.json").exists()
    {
        bail!("parent attempt ended or closed child admission");
    }
    if Lock::try_acquire(&dir.join("owner.lock"))?.is_some() {
        bail!("parent supervisor is no longer live; child dispatch refused");
    }
    let request = spec
        .broker_request
        .as_deref()
        .filter(|id| id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| Error::new("child lacks a broker request identity"))?;
    let claim: Value = read_json(&attempt.join("broker").join(format!("{request}.claim.json")))?;
    if claim["state"] != "dispatching"
        || claim["parent_attempt"] != parent_spec.attempt
        || claim["request_id"] != request
    {
        bail!("child dispatch does not match a consumed request in the live parent attempt");
    }
    Ok(())
}

pub fn supervise(dir: &Path) -> Result<i32> {
    confined(dir, false)?;
    let expected = std::env::var("AHU_EXPECTED_DIGEST")
        .map_err(|_| Error::new("supervise is internal; launch or resume a prepared task"))?;
    supervise_authorized(dir, &expected)
}

fn supervise_authorized(dir: &Path, expected: &str) -> Result<i32> {
    confined(dir, false)?;
    if expected != frozen_digest(dir)? {
        bail!("prepared task/spec integrity mismatch");
    }
    let _owner = Lock::acquire(&dir.join("owner.lock"))?;
    let spec: Spec = read_json(&dir.join("headless.json"))?;
    if spec.schema_version != 1 {
        bail!("unsupported headless schema");
    }
    let attempt = attempt_dir(dir, &spec);
    confined(&attempt, false)?;
    if attempt.join("result.json").exists() || attempt.join("started.json").exists() {
        bail!("attempt already started; implicit replay refused");
    }
    let outcome = run_attempt(dir, &attempt, &spec);
    if let Err(error) = &outcome {
        let _ = durable_json(
            &attempt.join("result.json"),
            &json!({"schema_version":1,"task_id":dir.file_name(),"attempt":spec.attempt,
            "outcome":"supervisor_error","blockers":[error.to_string()],"acceptance":"not assessed","worktree_preserved":true}),
        );
        let _ = task::set_state(dir, task::TaskState::Failed);
    }
    outcome
}

fn run_attempt(dir: &Path, attempt: &Path, spec: &Spec) -> Result<i32> {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let (record, rebuilt, executable) = crate::launch::verify_task(dir, Some(spec))?;
    let repo = crate::git::discover(&record.repo_root)?;
    let real = validate_executable(&executable, &repo)?;
    if real != record.harness_executable
        || digest_bytes(&std::fs::read(&real)?) != spec.executable_digest
    {
        bail!("harness executable changed since submission; refusing execution");
    }
    check_version(&record.identity.harness, &spec.harness_version)?;
    validate_parent_attempt(&repo, spec)?;
    validate_frozen_configuration(&record)?;
    validate_environment(
        &repo,
        &crate::hooks::collect(&record.worktree, &record.identity.harness)?,
    )?;
    if let Some(parent) = &spec.parent_task {
        let parent_dir = lookup(&repo, parent)?;
        if parent_dir.join("cancel.json").exists() {
            bail!("ancestor cancellation blocks this attempt");
        }
    }
    let mut command = Command::new(&real);
    command
        .args(&rebuilt.args)
        .current_dir(&record.worktree)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    sanitize(&mut command);
    command
        .env("AHU_BIN", std::env::current_exe()?)
        .env("AHU_EXECUTION_BACKEND", "headless")
        .env("AHU_PARENT_TASK", &record.task_id)
        .env("AHU_RUNTIME_DIR", runtime_root()?);
    // The feature matrix decides whether the harness's generic CLI log can be
    // pointed at a deterministic external destination.
    if crate::catalog::supports(
        &record.identity.harness,
        crate::catalog::Feature::ExternalLogDestination,
    ) {
        command.arg("--log-file").arg(attempt.join("native.log"));
    }
    let mut broker = crate::broker::Broker::new(&dir.join("requests"), &record, spec)?;
    broker.configure_worker(&mut command);
    if let Some(profile) = &spec.native_profile {
        for key in &profile.env_remove {
            command.env_remove(key);
        }
        for (key, value) in &profile.env {
            command.env(key, value);
        }
    }
    // Serialize the final parent/cancellation check with admission and explicit cancellation.
    let spawn_admission = Lock::acquire(&store(&repo)?.join("launch.lock"))?;
    validate_parent_attempt(&repo, spec)?;
    if dir.join("cancel.json").exists() {
        bail!("task was cancelled before worker spawn");
    }
    durable_json(
        &attempt.join("spawn-intent.json"),
        &json!({"at":task::now_rfc3339(),"attempt":spec.attempt}),
    )?;
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            std::fs::remove_file(attempt.join("spawn-intent.json"))?;
            return Err(error.into());
        }
    };
    drop(spawn_admission);
    let pid = child.id();
    let started = task::now_rfc3339();
    // Retain the owned child on every I/O failure; the guard stops it before return.
    struct Guard(u32);
    impl Drop for Guard {
        fn drop(&mut self) {
            if self.0 != 0 {
                signal_group(self.0, libc::SIGKILL);
            }
        }
    }
    let mut guard = Guard(pid);
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    let (err_tx, err_rx) = std::sync::mpsc::channel();
    capture(
        child
            .stdout
            .take()
            .ok_or_else(|| Error::new("missing stdout"))?,
        attempt.join("events.jsonl"),
        Some(record.identity.harness.clone()),
        out_tx,
    );
    capture(
        child
            .stderr
            .take()
            .ok_or_else(|| Error::new("missing stderr"))?,
        attempt.join("stderr.log"),
        None,
        err_tx,
    );
    durable_json(
        &attempt.join("started.json"),
        &json!({"schema_version":1,"pid":pid,"supervisor_pid":std::process::id(),"started_at":started,"attempt":spec.attempt}),
    )?;
    task::set_state(dir, task::TaskState::Running)?;
    let mut heartbeat = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(spec.options.timeout_seconds);
    let mut stop: Option<(&str, Instant)> = None;
    let mut output = None;
    let mut errors = None;
    let mut broker_failure = None;
    let mut cancelled_tasks = Vec::new();
    loop {
        if output.is_none() {
            output = out_rx.try_recv().ok();
        }
        if errors.is_none() {
            errors = err_rx.try_recv().ok();
        }
        let storage_failed = output.as_ref().is_some_and(|r| r.is_err())
            || errors.as_ref().is_some_and(|r| r.is_err());
        if stop.is_none() {
            let reason = if dir.join("cancel.json").exists() {
                Some("cancelled")
            } else if Instant::now() >= deadline {
                Some("timed_out")
            } else if storage_failed || broker_failure.is_some() {
                Some("capture_failed")
            } else {
                None
            };
            if let Some(reason) = reason {
                cancelled_tasks = cancel_tree(&repo, dir, reason)?;
                signal_group(pid, libc::SIGTERM);
                stop = Some((reason, Instant::now()));
            }
        }
        if stop.is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(2)) {
            signal_group(pid, libc::SIGKILL);
        }
        if child_exited(pid)? {
            break;
        }
        if stop.is_none()
            && broker_failure.is_none()
            && let Err(error) = broker.poll()
        {
            broker_failure = Some(error.to_string());
        }
        if heartbeat.elapsed() >= Duration::from_secs(1) {
            durable_json(
                &attempt.join("heartbeat.json"),
                &json!({"at": task::now_rfc3339(), "attempt":spec.attempt}),
            )?;
            heartbeat = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    durable_json(
        &attempt.join("admission-closed.json"),
        &json!({"at":task::now_rfc3339(),"attempt":spec.attempt}),
    )?;
    // Stop remaining owned OS descendants even if the leader exited normally.
    signal_group(pid, libc::SIGKILL);
    let status = child.wait()?;
    guard.0 = 0;
    if output.is_none() {
        output = out_rx.recv_timeout(Duration::from_secs(2)).ok();
    }
    if errors.is_none() {
        errors = err_rx.recv_timeout(Duration::from_secs(2)).ok();
    }
    let mut events = match output {
        Some(Ok(events)) => events,
        Some(Err(e)) => Events {
            failed: true,
            blockers: vec![e],
            ..Events::default()
        },
        None => Events {
            failed: true,
            blockers: vec!["stdout drain incomplete; escaped descendant cleanup unknown".into()],
            ..Events::default()
        },
    };
    match errors {
        Some(Ok(stderr)) => {
            events.failed |= stderr.failed;
            events.stderr_diagnostics = stderr.stderr_diagnostics;
            events.stderr_unclassified_lines = stderr.stderr_unclassified_lines;
            if !events.stderr_diagnostics.is_empty() {
                events.blockers.push("recognized failure diagnostic on stderr; inspect stderr_diagnostics and the raw stderr artifact".into());
            }
            if events.stderr_unclassified_lines > 0 {
                events.blockers.push("stderr contains unclassified diagnostics; their effect on harness completion is unknown; inspect the raw stderr artifact".into());
            }
        }
        Some(Err(e)) => {
            events.failed = true;
            events.blockers.push(e);
        }
        None => {
            events.failed = true;
            events.blockers.push("stderr drain incomplete".into());
        }
    }
    if !events.terminal {
        events.failed = true;
        events.blockers.push("no terminal harness result".into());
    }
    if events.session.is_none() {
        events
            .blockers
            .push("native session identity unavailable; resume disabled".into());
    }
    if let Some(session) = &spec.session
        && events.session.as_ref() != Some(session)
    {
        events.failed = true;
        events
            .blockers
            .push("resumed session did not match requested identity".into());
    }
    let native_completeness = spec
        .native_profile
        .as_ref()
        .map(|profile| events.native.completeness(profile));
    if let Some(completeness) = &native_completeness {
        let bounded = spec.options.native_helpers == "bounded";
        let failed_helper = events
            .native
            .helpers()
            .iter()
            .any(|helper| helper.status != crate::native::HelperStatus::Completed);
        if (!completeness.complete
            && (bounded
                || record.identity.harness == "claude-code"
                || completeness.unknown_events > 0))
            || (bounded && failed_helper)
        {
            events.failed = true;
            events.blockers.extend(completeness.blockers());
            if failed_helper {
                events
                    .blockers
                    .push("a native helper did not complete successfully".into());
            }
        }
    }
    broker.finish()?;
    if let Some(error) = broker_failure {
        events.failed = true;
        events.blockers.push(error);
    }
    let mut cancellation_results = Vec::new();
    let reconcile_deadline = Instant::now() + Duration::from_secs(5);
    for id in &cancelled_tasks {
        if id == &record.task_id {
            continue;
        }
        let child_dir = lookup(&repo, id)?;
        let value = loop {
            let value = result(&child_dir)
                .unwrap_or_else(|e| json!({"outcome":"unknown","error":e.to_string()}));
            if value["outcome"] != "running" || Instant::now() >= reconcile_deadline {
                break value;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if matches!(
            value["outcome"].as_str(),
            Some("running" | "unknown" | "interrupted")
        ) {
            events.blockers.push(format!(
                "registered descendant {id} cancellation is unconfirmed; inspect its result"
            ));
        }
        cancellation_results
            .push(json!({"task_id":id,"outcome":value["outcome"],"error":value["error"]}));
    }
    let mut ahu_children = Vec::new();
    for child_dir in discover(&repo)? {
        let child_spec: Spec = read_json(&child_dir.join("headless.json"))?;
        if child_spec.parent_task.as_deref() == Some(record.task_id.as_str())
            && child_spec.parent_attempt == Some(spec.attempt)
        {
            let child_result = result(&child_dir)?;
            if child_result["outcome"] != "succeeded" {
                events.failed = true;
                events.blockers.push(format!("registered child {} is {}; inspect its result before accepting this assignment", child_dir.file_name().unwrap_or_default().to_string_lossy(), child_result["outcome"]));
            }
            ahu_children.push(json!({"task_id":child_result["task_id"],"outcome":child_result["outcome"],"attempt":child_result["attempt"]}));
        }
    }
    let agent_claim: Option<Value> = serde_json::from_str(&events.summary).ok();
    if let Some(claim) = &agent_claim
        && matches!(
            claim.get("status").and_then(Value::as_str),
            Some("blocked" | "failed" | "partial")
        )
    {
        events.failed = true;
        events.blockers.push(
            "agent-reported structured status is incomplete; independently review its evidence"
                .into(),
        );
    }
    let outcome = if let Some((reason, _)) = stop {
        reason
    } else if !status.success() || events.failed {
        "failed"
    } else {
        "succeeded"
    };
    let summary = attempt.join("final.txt");
    state::write_private_file(&summary, events.summary.as_bytes())?;
    let changes = crate::git::run_ok(
        &record.worktree,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .ok();
    let revision = crate::git::run_ok(&record.worktree, &["rev-parse", "HEAD"]).ok();
    let result = json!({"schema_version":1,"backend":"headless","task_id":record.task_id,"attempt":spec.attempt,"parent_task":spec.parent_task,"parent_attempt":spec.parent_attempt,"broker_request":spec.broker_request,"root_task":spec.root_task,
        "identity":record.identity,"policy_digest":record.policy_digest,"snapshot_digest":record.config_snapshot_digest,
        "delivery_digest":record.delivery.digest,"capabilities":spec,"outcome":outcome,"started_at":started,"ended_at":task::now_rfc3339(),
        "process":{"exit_code":status.code(),"signal":status.signal()},"harness":events,
        "agent_report":{"text":events.summary,"structured":agent_claim,"trusted":false},"acceptance":"not assessed","completion_verified":false,
        "worktree":record.worktree,"worktree_exists":record.worktree.exists(),"branch":record.branch,"base_commit":record.base_commit,
        "current_revision":revision,"git_status":changes,"validation_evidence":"see agent report; not independently verified",
        "artifacts":{"events":attempt.join("events.jsonl"),"stderr":attempt.join("stderr.log"),"final":summary},
        "descendant_cancellation":cancellation_results,"native_completeness":native_completeness,"ahu_children":ahu_children,"native_cleanup":"unknown for external/provider-managed processes","usage_child_accounting":"unknown","retention":"kept until explicit cleanup; native stores retain their own policies"});
    durable_json(&attempt.join("result.json"), &result)?;
    task::set_state(
        dir,
        if outcome == "succeeded" {
            task::TaskState::Exited
        } else {
            task::TaskState::Failed
        },
    )?;
    Ok(if outcome == "succeeded" { 0 } else { 5 })
}

pub(crate) fn lookup(repo: &crate::git::Repo, id: &str) -> Result<PathBuf> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("invalid headless task id");
    }
    let found: Vec<_> = discover(repo)?
        .into_iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|s| s.to_string_lossy().starts_with(id))
        })
        .collect();
    if found.len() != 1 {
        bail!("headless task id is missing or ambiguous: {id}");
    }
    Ok(found[0].clone())
}

fn result(dir: &Path) -> Result<Value> {
    let spec: Spec = read_json(&dir.join("headless.json"))?;
    result_attempt(dir, &spec)
}
fn result_attempt(dir: &Path, spec: &Spec) -> Result<Value> {
    let path = attempt_dir(dir, spec).join("result.json");
    if path.exists() {
        return read_json(&path);
    }
    let active = Lock::is_owned(&dir.join("owner.lock")).map_err(|error| {
        Error::new(format!(
            "cannot inspect supervisor ownership; liveness unknown: {error}"
        ))
    })?;
    Ok(
        json!({"schema_version":1,"task_id":dir.file_name(),"attempt":spec.attempt,
        "outcome":if active {"running"} else {"interrupted"},"acceptance":"not assessed",
        "blockers":if active {Vec::<String>::new()} else {vec!["no live supervisor owns this attempt; process cleanup unknown, no automatic replay".into()]}}),
    )
}
fn wait(dir: &Path, json_output: bool) -> Result<i32> {
    let spec: Spec = read_json(&dir.join("headless.json"))?;
    loop {
        let value = result_attempt(dir, &spec)?;
        if value["outcome"] != "running" {
            let code = if value["outcome"] == "succeeded" {
                0
            } else {
                5
            };
            emit(&value, json_output)?;
            return Ok(code);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Close admission and cancel registered descendants without signalling recorded PIDs.
fn cancel_tree(repo: &crate::git::Repo, dir: &Path, reason: &str) -> Result<Vec<String>> {
    let admission_path = store(repo)?.join("launch.lock");
    let deadline = Instant::now() + Duration::from_secs(2);
    let _admission = loop {
        if let Some(lock) = Lock::try_acquire(&admission_path)? {
            break lock;
        }
        if Instant::now() >= deadline {
            bail!("cancellation admission lock unavailable; descendant cancellation unconfirmed");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let record = task::load(dir)?;
    let current: Spec = read_json(&dir.join("headless.json"))?;
    let mut selected = vec![(record.task_id, current.attempt, dir.to_path_buf())];
    // Parse the inventory once; never cancel descendants of an older owner attempt.
    let all = discover(repo)?
        .into_iter()
        .map(|path| {
            let spec: Spec = read_json(&path.join("headless.json"))?;
            let id = task::load(&path)?.task_id;
            Ok((id, spec, path))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut i = 0;
    while i < selected.len() {
        for (id, spec, path) in &all {
            if spec.parent_task.as_ref() == Some(&selected[i].0)
                && spec.parent_attempt == Some(selected[i].1)
                && !selected.iter().any(|(found, _, _)| found == id)
            {
                selected.push((id.clone(), spec.attempt, path.clone()));
            }
        }
        i += 1;
    }
    let mut ids = Vec::new();
    for (id, attempt, child) in selected {
        durable_json(
            &child.join(format!("attempt-{attempt}/admission-closed.json")),
            &json!({"at":task::now_rfc3339(),"attempt":attempt,"reason":reason}),
        )?;
        durable_json(
            &child.join("cancel.json"),
            &json!({"requested_at":task::now_rfc3339(),"descendants":true,"reason":reason}),
        )?;
        ids.push(id);
    }
    Ok(ids)
}

pub fn control(
    repo: &crate::git::Repo,
    action: &str,
    id: &str,
    prompt: Option<&Path>,
    json_output: bool,
) -> Result<i32> {
    let dir = lookup(repo, id)?;
    match action {
        "result" => {
            emit(&result(&dir)?, json_output)?;
            Ok(0)
        }
        "wait" => wait(&dir, json_output),
        "cancel" => {
            let ids = cancel_tree(repo, &dir, "cancelled")?;
            emit(
                &json!({"schema_version":1,"cancellation":"requested","tasks":ids,"confirmation":"use wait/result; no recorded PID is signalled"}),
                json_output,
            )?;
            Ok(0)
        }
        "cleanup" => {
            let _owner = Lock::acquire(&dir.join("owner.lock"))?;
            let current = result(&dir)?;
            if !matches!(
                current["outcome"].as_str(),
                Some("succeeded" | "failed" | "cancelled" | "timed_out" | "capture_failed")
            ) {
                bail!(
                    "cleanup requires a known terminal attempt; unknown/interrupted ownership must be resolved first"
                );
            }
            let mut removed = Vec::new();
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !name
                    .strip_prefix("attempt-")
                    .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
                {
                    continue;
                }
                let attempt = ConfinedDir::open(&entry.path())?;
                for file in [
                    "events.jsonl",
                    "stderr.log",
                    "supervisor.log",
                    "native.log",
                    "final.txt",
                ] {
                    if attempt.remove_artifact(file)? {
                        removed.push(entry.path().join(file));
                    }
                }
                durable_json(
                    &entry.path().join("artifacts-removed.json"),
                    &json!({"at":task::now_rfc3339()}),
                )?;
            }
            let inbox = dir.join("requests");
            if inbox.exists() {
                // The listing is read by name and the removals go through the
                // pinned handle, so a directory swapped in mid-scan can only
                // name entries the validated inbox does not have: they are
                // reported as absent instead of deleted somewhere else.
                let mailbox = ConfinedDir::open(&inbox)?;
                for entry in std::fs::read_dir(&inbox)?.take(4097) {
                    let entry = entry?;
                    if mailbox.remove_mailbox_entry(&entry.file_name())? {
                        removed.push(entry.path());
                    }
                }
            }
            emit(
                &json!({"schema_version":1,"removed":removed,"retained":"results, frozen inputs, private broker claims/responses, native session stores, branches and worktrees; inbox cleanup is bounded to 4097 entries per call"}),
                json_output,
            )?;
            Ok(0)
        }
        "resume" => {
            let frozen: Spec = read_json(&dir.join("headless.json"))?;
            if frozen.parent_task.is_some()
                || std::env::var_os("AHU_PARENT_TASK").is_some()
                || std::env::var_os("AHU_BROKER_TOKEN").is_some()
            {
                bail!(
                    "registered child resume and worker-originated resume are unsupported; previous attempt preserved. Submit a new registered assignment through the owning broker, or a new root assignment from the host with explicit grants; do not detach provenance or launch a nested supervisor"
                );
            }
            let _owner = Lock::acquire(&dir.join("owner.lock"))?;
            recover_resume(&dir)?;
            let previous = result(&dir)?;
            if !matches!(
                previous["outcome"].as_str(),
                Some("succeeded" | "failed" | "cancelled" | "timed_out")
            ) {
                bail!(
                    "resume requires a recorded terminal result with known process ownership; interrupted attempts cannot be replayed"
                );
            }
            let mut spec: Spec = read_json(&dir.join("headless.json"))?;
            let session = previous
                .pointer("/harness/session")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::new("no recorded native session; submit a new task"))?;
            if session.is_empty()
                || session.starts_with('-')
                || !session
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                bail!("invalid recorded session id");
            }
            let mut record = task::load(&dir)?;
            let loaded = crate::config::load(&record.worktree)?
                .ok_or_else(|| Error::new("missing task configuration"))?;
            if loaded.digest != record.policy_digest
                || crate::snapshot::collect(&record.worktree)?.digest()
                    != record.config_snapshot_digest
            {
                bail!("task configuration changed; resume refused, submit a new assignment");
            }
            validate_frozen_configuration(&record)?;
            let (_, _, executable) = crate::launch::verify_task(&dir, Some(&spec))?;
            let real = validate_executable(&executable, repo)?;
            if real != record.harness_executable
                || digest_bytes(&std::fs::read(&real)?) != spec.executable_digest
            {
                bail!("harness executable changed; previous attempt preserved, resume refused");
            }
            validate_environment(
                repo,
                &crate::hooks::collect(&record.worktree, &record.identity.harness)?,
            )?;
            let original_spec = spec.clone();
            let original_record = record.clone();
            let original_prompt = task::load_prompt(&dir)?;
            // Frozen widening was explicitly accepted at launch; resume never changes it.
            let prompt = crate::cli::PromptSource::File(
                prompt.ok_or_else(|| Error::new("prompt required"))?.into(),
            )
            .read(&mut std::io::empty(), false)?;
            spec.session = Some(session.into());
            spec.attempt += 1;
            while attempt_dir(&dir, &spec).exists() {
                spec.attempt += 1;
            }
            let (delivered, delivery) = crate::orchestration::deliver_headless_policy(
                record.delivery.agent_instructions.as_deref(),
                &prompt,
                &spec.options.native_helpers,
            )?;
            let command = batch_command(
                &record.identity.harness,
                &LaunchRequest {
                    model: &record.identity.model,
                    prompt: &delivered,
                    cwd: &record.worktree,
                    permissions: record.identity.permissions,
                },
                &spec,
            )?;
            let old_attempt = attempt_dir(&dir, &original_spec);
            durable_json(&old_attempt.join("submission.json"), &record)?;
            state::write_private_file(
                &old_attempt.join("prompt.txt"),
                task::load_prompt(&dir)?.as_bytes(),
            )?;
            record.delivery = delivery;
            record.prompt_digest = digest_bytes(prompt.as_bytes());
            record.launch_command = command.redacted();
            record.state = task::TaskState::Starting;
            // Journal is private recovery metadata, never an automatic replay request.
            durable_json(
                &dir.join("resume-journal.json"),
                &json!({"record":original_record,"spec":original_spec,"prompt":original_prompt,"next_attempt":spec.attempt}),
            )?;
            let transition = task::save(&dir, &record, &prompt)
                .and_then(|()| durable_json(&dir.join("headless.json"), &spec));
            if let Err(error) = transition {
                task::save(&dir, &original_record, &original_prompt)?;
                durable_json(&dir.join("headless.json"), &original_spec)?;
                return Err(error);
            }
            if dir.join("cancel.json").exists() {
                std::fs::remove_file(dir.join("cancel.json"))?;
            }
            drop(_owner);
            if let Err(error) = start(&dir, &spec) {
                if !attempt_dir(&dir, &spec).join("spawn-intent.json").exists()
                    && let Ok(_recovery) = Lock::acquire(&dir.join("owner.lock"))
                {
                    task::save(&dir, &original_record, &original_prompt)?;
                    durable_json(&dir.join("headless.json"), &original_spec)?;
                }
                return Err(error);
            }
            if dir.join("resume-journal.json").exists() {
                std::fs::remove_file(dir.join("resume-journal.json"))?;
            }
            emit(
                &json!({"schema_version":1,"task_id":record.task_id,"attempt":spec.attempt,"session":spec.session,"outcome":"started"}),
                json_output,
            )?;
            Ok(0)
        }
        _ => bail!("unsupported headless operation"),
    }
}

/// Restore only metadata for an attempt that has never acknowledged process start.
/// This is invoked by explicit resume, never by read-only inspection or a timer.
fn recover_resume(dir: &Path) -> Result<()> {
    let journal = dir.join("resume-journal.json");
    if !journal.exists() {
        return Ok(());
    }
    #[derive(Deserialize)]
    struct Journal {
        record: task::TaskRecord,
        spec: Spec,
        prompt: String,
        next_attempt: u32,
    }
    let old: Journal = read_json(&journal)?;
    let next = Spec {
        attempt: old.next_attempt,
        ..old.spec.clone()
    };
    if !attempt_dir(dir, &next).join("spawn-intent.json").exists() {
        task::save(dir, &old.record, &old.prompt)?;
        durable_json(&dir.join("headless.json"), &old.spec)?;
    }
    std::fs::remove_file(journal)?;
    Ok(())
}

pub fn inspection(dir: &Path) -> Result<Value> {
    let spec: Spec = read_json(&dir.join("headless.json"))?;
    let value = result_attempt(dir, &spec)?;
    Ok(
        json!({"number":spec.attempt,"parent_task":spec.parent_task,"outcome":value["outcome"],
        "native_helpers":spec.options.native_helpers,"runtime":dir,"timeout_seconds":spec.options.timeout_seconds,
        "captured_artifacts_removed":attempt_dir(dir, &spec).join("artifacts-removed.json").exists()}),
    )
}
