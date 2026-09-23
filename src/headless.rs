//! Unattended attempts with minimal primary-owned state and an ahu supervisor.
//! Process and harness outcomes are evidence, never work acceptance.
pub(crate) mod review;

use crate::harness::{LaunchCommand, LaunchRequest};
use crate::util::{Error, Result, digest_bytes};
use crate::{bail, state, task};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Read;
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
    #[serde(deserialize_with = "read_spec_version")]
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

fn read_spec_version<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if matches!(version, 1 | 2) {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(
            "unsupported headless schema version",
        ))
    }
}

impl Spec {
    /// Only ahu-owned coordination facts cross the prompt boundary. Native
    /// session identity is a locator, not permission to retrieve its history.
    pub fn coordination_state(&self) -> crate::orchestration::CoordinationState {
        crate::orchestration::CoordinationState {
            root_task: self.root_task.clone(),
            parent_task: self.parent_task.clone(),
            attempt: self.attempt,
            native_session: self.session.clone(),
            child_grants: self
                .child_grants
                .iter()
                .map(|grant| crate::orchestration::GrantedAgent {
                    agent: grant.agent.clone(),
                    identity_digest: grant.identity_digest.clone(),
                    permissions: grant.permissions,
                    native_helpers: grant.native_helpers.clone(),
                })
                .collect(),
        }
    }
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

/// Conventional legacy store, used only to locate existing records.
pub fn runtime_root() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| Error::new("legacy home unavailable"))?;
    crate::storage::external_root(&PathBuf::from(home).join(".local/state/ahu/runtime"))
}

pub fn store(repo: &crate::git::Repo) -> Result<PathBuf> {
    Ok(crate::storage::HeadlessStore::for_repo(repo)?.directory)
}

pub(crate) fn stores(repo: &crate::git::Repo) -> Result<Vec<PathBuf>> {
    let mut stores = vec![store(repo)?];
    stores.extend(
        crate::storage::legacy_runtime_roots(repo)?
            .into_iter()
            .map(|p| p.join(repo.identity())),
    );
    Ok(stores)
}

pub(crate) fn confined_in(repo: &crate::git::Repo, path: &Path, create: bool) -> Result<()> {
    if let Some(store) = crate::storage::HeadlessStore::containing(path)? {
        if store
            .directory
            .parent()
            .and_then(Path::file_name)
            .is_none_or(|n| n != repo.identity().as_str())
        {
            bail!("headless store belongs to another repository");
        }
        return confined_at(&store.directory, path, create);
    }
    let root = crate::storage::legacy_runtime_roots(repo)?
        .into_iter()
        .find(|root| path.starts_with(root))
        .ok_or_else(|| {
            Error::new("task path is outside this repository's configured legacy roots")
        })?;
    confined_at(&root, path, create)
}

pub(crate) fn confined(path: &Path, create: bool) -> Result<()> {
    if let Some(store) = crate::storage::HeadlessStore::containing(path)? {
        return confined_at(&store.directory, path, create);
    }
    if let Ok(root) = runtime_root()
        && path.starts_with(&root)
    {
        return confined_at(&root, path, create);
    }
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(repo) = crate::git::discover(&cwd)
        && crate::storage::legacy_runtime_roots(&repo)?
            .iter()
            .any(|root| path.starts_with(root))
    {
        return confined_in(&repo, path, create);
    }
    // An exact old task reference can identify its owner. The record is evidence
    // only: its repository must explicitly configure this external root before
    // any access is accepted. Never infer a custom store from retired selectors.
    for dir in path.ancestors() {
        let Some(id) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !(task::is_canonical_task_uuid(id)
            || (!id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit())))
        {
            continue;
        }
        let Some(root) = dir.parent().and_then(Path::parent) else {
            continue;
        };
        if crate::storage::external_root(root).ok().as_deref() != Some(root) {
            continue;
        }
        confined_at(root, dir, false)?;
        let record_path = dir.join("task.json");
        if state::confine_file(&record_path)?.is_none_or(|m| m.len() > 1024 * 1024) {
            continue;
        }
        let record = task::load(dir)?;
        let repo = crate::git::discover(&record.repo_root)?;
        if record.task_id != id
            || record.repo_identity != repo.identity()
            || dir
                .parent()
                .and_then(Path::file_name)
                .is_none_or(|n| n != record.repo_identity.as_str())
        {
            bail!("legacy task reference has inconsistent ownership");
        }
        return confined_in(&repo, path, create);
    }
    bail!("task path is outside verified primary coordination and configured legacy roots")
}

fn confined_at(root: &Path, path: &Path, create: bool) -> Result<()> {
    if !path.starts_with(root) {
        bail!("task path is outside its verified store");
    }
    let mut cursor = root.to_path_buf();
    if let Ok(meta) = std::fs::symlink_metadata(root) {
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
        .strip_prefix(root)
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
    discover_stores(repo, stores(repo)?)
}

fn discover_domain(repo: &crate::git::Repo, domain: &Path) -> Result<Vec<PathBuf>> {
    discover_stores(repo, vec![domain.to_path_buf()])
}

fn discover_stores(repo: &crate::git::Repo, domains: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for dir in domains {
        confined_in(repo, &dir, false)?;
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if crate::task::is_canonical_task_uuid(&name)
                || (!name.is_empty() && name.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                confined_in(repo, &entry.path(), false)?;
                if entry.path().join("task.json").exists() {
                    out.push(entry.path());
                }
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
    let mut body = serde_json::to_vec(value).map_err(|e| Error::new(e.to_string()))?;
    body.push(b'\n');
    crate::private_io::atomic_write(path, &body, crate::private_io::Durability::Durable)
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&review::read_bytes(path, None)?)
        .map_err(|_| Error::new(format!("invalid metadata at {}", path.display())))
}

fn attempt_dir(dir: &Path, spec: &Spec) -> PathBuf {
    dir.join(format!("attempt-{}", spec.attempt))
}
fn frozen_digest(dir: &Path) -> Result<String> {
    let mut bytes = review::read_bytes(&dir.join("task.json"), None)?;
    bytes.extend(review::read_bytes(&dir.join("headless.json"), None)?);
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
        match agent
            .as_ref()
            .ok_or_else(|| Error::new("named agent required"))?
            .manifest
            .native_helpers
            .clone()
        {
            Some(policy) => options.native_helpers = policy,
            None => {
                let project: toml::Value = toml::from_str(&std::fs::read_to_string(&loaded.path)?)
                    .map_err(|e| Error::new(e.to_string()))?;
                if let Some(value) = project
                    .get("execution")
                    .and_then(|v| v.get("native_helpers"))
                {
                    options.native_helpers = value
                        .as_str()
                        .ok_or_else(|| Error::new("native_helpers must be disabled or bounded"))?
                        .into();
                }
            }
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
    confined(&store(repo)?, false)?;
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
    crate::catalog::check_headless_version(&plan.pair.harness, &version)?;
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
        if parent_spec.schema_version != 2 {
            bail!(
                "legacy dispatch is unsupported; use the original runner and its owning supervisor"
            );
        }
        parent_spec.root_task.unwrap_or_else(|| parent.clone())
    } else {
        plan.task_id.clone()
    };
    let mut spec = Spec { schema_version: 2, options, harness_version: version, executable_digest: digest_bytes(&std::fs::read(&executable)?),
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
        "Stderr classification recognizes a bounded set of permission/authentication/quota signatures conservatively; other stderr is discarded with explicit diagnostic uncertainty. Authenticated agy denial coverage remains unvalidated.".into(),
        "Automatic timeout/capture failure and explicit cancellation stop registered descendants of the current attempt; results record bounded reconciliation and unknown cleanup. Supervisor crash and provider-managed work remain outside guaranteed cleanup.".into(),
        "Broker scans at most 32 entries per tick, retains at most 256 consumed requests per attempt, and stops admission above 4096 entries per scan. Cleanup removes at most 4097 inbox entries per call while retaining private claims/responses; arbitrary worker filesystem writes require external disk quotas.".into(),
        "Child resume and worker-originated resume are unsupported: submit a new registered assignment through the broker or a new root from the host with explicit grants.".into(),
    ]);
    let profile = build_native_profile(&plan.pair.harness, &plan.pair.model, &spec)?;
    spec.native_controls = profile.control_ids();
    spec.gaps.extend(profile.gaps.clone());
    spec.gaps.push("Native integration evidence is bounded; unsupported configuration and hooks are refused. ahu itself never invokes cmux in this backend; arbitrary shell commands remain outside integration inspection. Same-UID code is not isolated from supervisor state.".into());
    if !spec.child_grants.is_empty() {
        spec.gaps.push("Broker grants approve only frozen registered identities and profiles. Codex workspace-write receives the task's requests directory as a narrow additional write root; read-only Codex transport is refused. Limits: 128 tasks per root grant, depth 8, 16 active/interrupted tasks per repository, no hidden queue or global token cap.".into());
    }

    if spec.options.native_helpers == "bounded" {
        spec.gaps.push("The ENTIRE assignment has a read-only MODEL TOOL ceiling: parent and helpers have no model tools for editing, building, shell commands or shell-launching registered children. Settings-defined hooks are outside that tool ceiling and their side effects are not proven read-only. MCP tools and slash commands are disabled; repository settings remain discoverable. Roles are requested/observed, not an allowlist; total helper count is not capped. Budget is 5 USD per attempt, concurrency 1, depth 1, helper model equals manifest model.".into());
    }
    spec.native_profile = Some(profile);
    let cmux_integration =
        validate_environment(repo, &repo.root, &plan.pair.harness, &spec.harness_version)?;
    plan.cmux_integration = cmux_integration.clone();
    spec.gaps.push(format!("cmux admission allowed for inspected native components; evidence SHA-256 {}. Live conformance remains unverified.", cmux_integration.headless.evidence_digest));
    let (delivered, delivery) = crate::orchestration::deliver_composed(
        plan.agent.as_ref().map(|a| a.instructions.as_str()),
        prompt,
        crate::orchestration::Composition {
            mode: crate::orchestration::Mode::headless(&spec.options.native_helpers)?,
            metadata: Some(crate::orchestration::Metadata {
                task_id: plan.task_id.clone(),
                agent: plan.agent_label(),
                harness: plan.pair.harness.clone(),
                model: plan.pair.model.clone(),
                permissions,
            }),
            state: Some(spec.coordination_state()),
        },
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
    if !dry_run {
        confined(plan.task_dir.parent().expect("store"), true)?;
    }
    crate::commands::preflight(console, repo, &loaded, &plan, prompt, dry_run)?;
    let mut preview = json!({"schema_version":1,"backend":"headless","task_id":plan.task_id,"worktree":plan.worktree,
        "task_handle_candidate":format!("@{}",plan.task_name.clone().unwrap_or_else(||crate::task_handles::generated_name(&plan.title))), "task_handle_reserved":false,
        "branch":plan.branch,"command":plan.command.redacted(),"executable":plan.harness_executable,"capabilities":spec,
        "runtime":plan.task_dir,"identity": {"agent":plan.agent_label(),"harness":plan.pair.harness,"model":plan.pair.model},
        "acceptance":"not assessed", "cmux_integration":cmux_integration});
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
    let previous = discover_domain(repo, &store(repo)?)?;
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
    let handle =
        crate::task_handles::reserve(repo, &plan.task_id, plan.task_name.as_deref(), &plan.title)?;
    preview["task_handle"] = json!(handle);
    preview["task_handle_reserved"] = json!(true);
    console.say(&format!(
        "Task {handle} ({})\n",
        crate::task_ref::display(&plan.task_id)
    ))?;
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
    crate::task_index::register(
        &repo.identity(),
        &plan.task_id,
        &repo.root,
        crate::task_index::StoreKind::Headless,
    )?;
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
    config_root: &Path,
    harness: &str,
    version: &str,
) -> Result<crate::cmux::integration::Status> {
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
    crate::catalog::check_headless_version(harness, version)?;
    Ok(crate::cmux::integration::enforce(config_root, harness)?.with_version(Some(version)))
}

pub(crate) fn emit(value: &Value, json_output: bool) -> Result<()> {
    if !json_output && value["review"].is_object() {
        let review = &value["review"];
        let canonical = crate::task_ref::display(review["task_id"].as_str().unwrap_or("unknown"));
        let label = match review["task_handle"].as_str() {
            Some(handle) => format!("{handle} ({canonical})"),
            None => canonical,
        };
        println!(
            "{} [headless; recorded state {}]",
            review::safe(&label),
            review::safe(review["session_state"].as_str().unwrap_or("unknown"))
        );
        print!("{}", review::render(review, false));
        if let Some(summary) = value["harness"]["summary"]
            .as_str()
            .filter(|s| !s.is_empty())
        {
            println!("  report excerpt (untrusted) {}", review::safe(summary));
        }
        if let Some(paths) = value["writes_outside_worktree"]
            .as_array()
            .filter(|p| !p.is_empty())
        {
            println!("  writes outside worktree (recorded):");
            for path in paths.iter().take(8).filter_map(Value::as_str) {
                println!("    {}", review::safe(path));
            }
            if paths.len() > 8 {
                println!("    additional entries omitted; inspect result JSON");
            }
        }
        return Ok(());
    }
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
        crate::storage::validate_owned_metadata(&file.metadata()?, true)?;
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
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        crate::storage::validate_owned_metadata(&file.metadata()?, true)?;
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

fn sanitize(command: &mut Command, policy: Option<&crate::cmux::integration::HeadlessPolicy>) {
    crate::cmux::integration::sanitize(command, policy);
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

    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["supervise", "--task-dir"])
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    sanitize(&mut command, None);
    command
        .env("AHU_EXPECTED_DIGEST", frozen_digest(dir)?)
        .env_remove("AHU_RUNTIME_DIR")
        .env_remove("AHU_TASK_INDEX_DIR");
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
            bail!(
                "supervisor exited before startup acknowledgement ({status}); inspect {}",
                attempt.display()
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillInvocation {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillCatalogEntry {
    pub name: String,
    pub source: String,
    pub digest: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cached: Option<u64>,
    #[serde(default)]
    pub cache_write: Option<u64>,
    #[serde(default)]
    pub reasoning: Option<u64>,
    pub total: Option<u64>,
}

impl TokenUsage {
    fn observe(&mut self, event: &Value) {
        fn max_slot(slot: &mut Option<u64>, value: Option<u64>) {
            if let Some(value) = value {
                *slot = Some(slot.unwrap_or(0).max(value));
            }
        }
        for usage in [
            event.get("usage"),
            event.pointer("/part/usage"),
            event.pointer("/result/usage"),
            event.pointer("/response/usage"),
            event.pointer("/step_update/usage"),
            event.pointer("/result/result/usage"),
            event.pointer("/step_update/tool_info/usage"),
            event.get("tokens"),
            event.pointer("/part/tokens"),
        ]
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        {
            let number = |keys: &[&str]| {
                keys.iter()
                    .find_map(|key| usage.get(*key).and_then(Value::as_u64))
            };
            max_slot(&mut self.input, number(&["input_tokens", "prompt_tokens"]));
            max_slot(
                &mut self.output,
                number(&["output_tokens", "completion_tokens"]),
            );
            max_slot(
                &mut self.cached,
                number(&[
                    "cached_tokens",
                    "cache_read_input_tokens",
                    "cached_input_tokens",
                ])
                .or_else(|| {
                    usage
                        .get("cache")
                        .and_then(|cache| cache.get("read"))
                        .and_then(Value::as_u64)
                }),
            );
            max_slot(
                &mut self.cache_write,
                number(&["cache_write_input_tokens", "cache_creation_input_tokens"]).or_else(
                    || {
                        usage
                            .get("cache")
                            .and_then(|cache| cache.get("write"))
                            .and_then(Value::as_u64)
                    },
                ),
            );
            max_slot(
                &mut self.reasoning,
                number(&["reasoning_output_tokens", "thinking_tokens", "reasoning"]),
            );
            max_slot(&mut self.total, number(&["total_tokens"]));
        }
    }
}

fn skill_catalog(worktree: &Path) -> Vec<SkillCatalogEntry> {
    let roots = [
        ".agents/skills",
        ".claude/skills",
        ".gemini/antigravity-cli/skills",
    ];
    let mut entries = Vec::new();
    for root in roots {
        let path = worktree.join(root);
        let Ok(children) = std::fs::read_dir(&path) else {
            continue;
        };
        for child in children.flatten() {
            let name = child.file_name().to_string_lossy().into_owned();
            if name.is_empty() || name.starts_with('.') {
                continue;
            }
            let file = child.path().join("SKILL.md");
            let Ok(bytes) = std::fs::read(&file) else {
                continue;
            };
            entries.push(SkillCatalogEntry {
                name,
                source: format!("{root}/{}/SKILL.md", child.file_name().to_string_lossy()),
                digest: digest_bytes(&bytes),
            });
        }
    }
    entries.sort_by(|left, right| left.source.cmp(&right.source));
    entries
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Events {
    pub session: Option<String>,
    pub terminal: bool,
    pub failed: bool,
    #[serde(skip_serializing)]
    pub summary: String,
    pub blockers: Vec<String>,
    pub unknown_events: u64,
    #[serde(default)]
    pub stderr_diagnostics: Vec<String>,
    #[serde(default)]
    pub stderr_unclassified_lines: u64,
    #[serde(default)]
    pub usage: TokenUsage,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub skills: Vec<SkillInvocation>,
    #[serde(skip_serializing)]
    pub native_observations: Vec<Value>,
    #[serde(default)]
    #[serde(skip_serializing)]
    pub native: crate::native::Observations,
    #[serde(default, skip_serializing)]
    pub writes_outside_worktree: Vec<String>,
    #[serde(skip)]
    native_event_count: usize,
}
impl Events {
    fn observe_skill(&mut self, event: &Value) {
        fn skill_name(value: &Value) -> Option<String> {
            let object = value.as_object()?;
            for key in ["skill", "skill_name", "name"] {
                if let Some(name) = object.get(key).and_then(Value::as_str)
                    && !name.trim().is_empty()
                {
                    return Some(name.to_string());
                }
            }
            None
        }
        fn tool_is_skill(value: &Value) -> bool {
            value
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case("skill") || name.ends_with(".skill"))
        }
        let mut found = Vec::new();
        if tool_is_skill(event.get("name").unwrap_or(&Value::Null)) {
            found.extend(
                event
                    .get("input")
                    .and_then(skill_name)
                    .or_else(|| skill_name(event)),
            );
        }
        for path in ["/item", "/part", "/tool"] {
            if let Some(value) = event.pointer(path)
                && (tool_is_skill(value.get("name").unwrap_or(&Value::Null))
                    || tool_is_skill(value.get("tool").unwrap_or(&Value::Null))
                    || tool_is_skill(value.get("type").unwrap_or(&Value::Null)))
            {
                found.extend(
                    value
                        .get("input")
                        .and_then(skill_name)
                        .or_else(|| {
                            value
                                .get("state")
                                .and_then(|state| state.get("input"))
                                .and_then(skill_name)
                        })
                        .or_else(|| skill_name(value)),
                );
            }
        }
        if let Some(content) = event.pointer("/message/content").and_then(Value::as_array) {
            for value in content {
                if tool_is_skill(value.get("name").unwrap_or(&Value::Null)) {
                    found.extend(value.get("input").and_then(skill_name));
                }
            }
        }
        for value in [
            event.pointer("/step_update/tool_name"),
            event.pointer("/step_update/tool_info/name"),
        ] {
            if tool_is_skill(value.unwrap_or(&Value::Null)) {
                found.extend(
                    event
                        .pointer("/step_update/tool_info/parameters")
                        .and_then(skill_name),
                );
            }
        }
        for name in found {
            if self.skills.len() >= 128 {
                self.failed = true;
                self.blockers.push("skill invocation limit exceeded".into());
                break;
            }
            self.skills.push(SkillInvocation {
                name,
                source: None,
                digest: None,
                status: "invoked".into(),
            });
        }
    }

    fn resolve_skills(&mut self, catalog: &[SkillCatalogEntry]) {
        for invocation in &mut self.skills {
            if let Some(entry) = catalog.iter().find(|entry| entry.name == invocation.name) {
                invocation.source = Some(entry.source.clone());
                invocation.digest = Some(entry.digest.clone());
            }
        }
    }

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
                self.stderr_diagnostics.push(category.into());
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
        self.usage.observe(&event);
        self.observe_skill(&event);
        if self.model.is_none() {
            self.model = event
                .get("model")
                .or_else(|| event.get("model_name"))
                .or_else(|| event.pointer("/part/model"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        let metadata = native_metadata(harness, &event);
        let helper_event = metadata["type"] == "system" || metadata["type"] == "collab";
        if helper_event && self.native_event_count >= 256 {
            self.failed = true;
            self.blockers
                .push("native helper evaluation limit exceeded".into());
        } else if bounded_native_metadata(&metadata) {
            self.native.observe(harness, &metadata);
        } else {
            self.failed = true;
            self.blockers
                .push("native metadata evaluation bound exceeded".into());
        }
        if helper_event {
            self.native_event_count += 1;
        }
        let kind = event
            .get("type")
            .or_else(|| event.get("event"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let session = event
            .get("session_id")
            .or_else(|| event.get("thread_id"))
            .or_else(|| event.get("conversation_id"))
            // OpenCode spells it `sessionID`, on every event including its
            // error events, which is what makes its errors resumable at all.
            .or_else(|| event.get("sessionID"))
            .or_else(|| event.pointer("/step_update/conversation_id"))
            .and_then(Value::as_str);
        if let Some(session) = session.filter(|s| valid_session(s)) {
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
        if event
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|v| !v.is_empty())
            || event.get("error").is_some_and(|v| !v.is_null())
        {
            self.blockers.push("harness reported an error".into());
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
                .push("harness reported permission denials".into());
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
                let result = event.get("result").filter(|value| value.is_object());
                let status = result
                    .and_then(|value| value.get("status"))
                    .or_else(|| event.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let subtype = result
                    .and_then(|value| value.get("subtype"))
                    .or_else(|| event.get("subtype"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let recognized_success = if harness == "claude-code" {
                    subtype == "success"
                        && event.get("is_error").and_then(Value::as_bool) == Some(false)
                        && event.get("result").and_then(Value::as_str).is_some()
                } else {
                    matches!(status.as_str(), "success" | "succeeded" | "ok")
                        && result
                            .and_then(|value| value.get("response"))
                            .or_else(|| event.get("response"))
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
                    .get("response")
                    .or_else(|| result.and_then(|value| value.get("response")))
                    .or_else(|| event.get("result"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into();
                self.finish(failed);
            }
            ("antigravity", "step_update") => (),
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
                    self.blockers
                        .push("harness reported permission denials".into());
                }
            }
            (_, "tool_use") => (),
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
                    Some(_) => {
                        self.blockers
                            .push("harness ended the step without a stop reason".into());
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

const STREAM_EVALUATION_LIMIT: u64 = 64 * 1024 * 1024;
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
const WRITE_PATH_KEYS: [&str; 6] = [
    "filepath",
    "file",
    "path",
    "notebookpath",
    "abspath",
    "targetfile",
];
const WRITE_NAME_KEYS: [&str; 3] = ["tool", "name", "toolname"];
const WRITE_INPUT_KEYS: [&str; 2] = ["input", "arguments"];

/// Lowercase ASCII alphanumerics only: `file_path` and `filePath` both
/// normalize to `filepath`, `_` separators vanish.
fn normalize_token(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
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
        let Some(file_name) = current.file_name().map(ToOwned::to_owned) else {
            break;
        };
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

/// Resolve a recorded write-path candidate. Only absolute paths are
/// classified; relative paths cannot be attributed to the worktree
/// confidently after the run ends.
fn resolve_write_path(candidate: &str) -> Option<PathBuf> {
    if candidate.is_empty() {
        return None;
    }
    let raw: PathBuf = candidate.into();
    if !raw.is_absolute() {
        return None;
    }
    let resolved = lexical_normalize(&raw);
    if resolved.as_os_str().is_empty() {
        return None;
    }
    Some(resolved)
}

/// Walk a write-tool call's subtree, collecting path candidates and decoding
/// string `input`/`arguments` payloads that embed JSON. Recursion is bounded
/// by `MAX_EVENT_JSON_DEPTH`.
fn collect_write_paths_within(value: &Value, depth: usize, candidates: &mut Vec<String>) {
    if depth >= MAX_EVENT_JSON_DEPTH {
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter() {
                let key = normalize_token(key);
                if WRITE_PATH_KEYS.contains(&key.as_str())
                    && let Some(path) = child.as_str()
                {
                    candidates.push(path.to_string());
                }
                if WRITE_INPUT_KEYS.contains(&key.as_str())
                    && let Some(embedded) = child.as_str()
                    && let Ok(decoded) = serde_json::from_str::<Value>(embedded)
                {
                    collect_write_paths_within(&decoded, depth + 1, candidates);
                }
                collect_write_paths_within(child, depth + 1, candidates);
            }
        }
        Value::Array(items) => {
            for child in items.iter() {
                collect_write_paths_within(child, depth + 1, candidates);
            }
        }
        _ => (),
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
                    WRITE_NAME_KEYS.contains(&normalize_token(key).as_str())
                        && child
                            .as_str()
                            .map(normalize_token)
                            .is_some_and(|tool| WRITE_LIKE_TOOLS.contains(&tool.as_str()))
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

/// Project exactly the fields consumed by helper accounting. Ordinary tool
/// payloads and native text use the stream bounds, not helper metadata bounds.
fn native_metadata(harness: &str, event: &Value) -> Value {
    let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
    let subtype = event.get("subtype").and_then(Value::as_str).unwrap_or("");
    if harness != "claude-code" {
        // The foreign observer only tests these two strings for this marker.
        return if kind.contains("collab")
            || event
                .pointer("/item/type")
                .and_then(Value::as_str)
                .is_some_and(|s| s.contains("collab"))
        {
            json!({"type":"collab"})
        } else {
            json!({})
        };
    }
    let mut out = json!({});
    let fields: &[&str] = match (kind, subtype) {
        ("system", "task_started") => &[
            "task_id",
            "task_type",
            "subagent_type",
            "spawn_depth",
            "is_backgrounded",
        ],
        ("system", "task_progress") => &["task_id"],
        ("system", "task_notification") => &["task_id", "status", "output_file"],
        ("result", _) => &[],
        _ => return out,
    };
    out["type"] = json!(kind);
    // Only the three known system subtypes are consumed; terminal subtypes
    // and arbitrary payload strings must not enter helper accounting.
    if kind == "system" {
        out["subtype"] = json!(subtype);
    }
    for field in fields {
        let value = event.get(field).filter(|v| match *field {
            "spawn_depth" => v.is_u64(),
            "is_backgrounded" => v.is_boolean(),
            _ => v.is_string(),
        });
        if let Some(value) = value {
            out[field] = value.clone();
        }
    }
    if matches!(
        (kind, subtype),
        ("system", "task_progress" | "task_notification")
    ) && let Some(tokens) = event.pointer("/usage/total_tokens").and_then(Value::as_u64)
    {
        out["usage"] = json!({"total_tokens":tokens});
    }
    if kind == "result"
        && let Some(stats) = event.get("subagent_stats")
    {
        let mut selected = json!({});
        for field in ["spawned", "completed", "max_depth"] {
            if let Some(value) = stats.get(field).and_then(Value::as_u64) {
                selected[field] = json!(value);
            }
        }
        for field in ["depth_limit", "concurrency_limit", "budget"] {
            if let Some(value) = stats
                .get("refused")
                .and_then(|v| v.get(field))
                .and_then(Value::as_u64)
            {
                if selected["refused"].is_null() {
                    selected["refused"] = json!({});
                }
                selected["refused"][field] = json!(value);
            }
        }
        if let Some(roles) = stats.get("by_type").and_then(Value::as_object) {
            selected["by_type"] = Value::Object(
                roles
                    .iter()
                    .filter(|(_, count)| count.is_u64())
                    .map(|(role, count)| (role.clone(), count.clone()))
                    .collect(),
            );
        }
        out["subagent_stats"] = selected;
    }
    out
}

fn bounded_native_metadata(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.len() <= 256
                && map
                    .iter()
                    .all(|(key, value)| key.len() <= 4096 && bounded_native_metadata(value))
        }
        Value::Array(items) => items.len() <= 256 && items.iter().all(bounded_native_metadata),
        Value::String(text) => text.len() <= 4096,
        // Native refusal accounting adds three provider-supplied counters.
        // Reject values outside the safe evaluation range before that parser.
        Value::Number(number) => number.as_u64().is_none_or(|n| n <= u64::MAX / 3),
        _ => true,
    }
}

fn observe_writes(events: &mut Events, line: &[u8], worktree: &Path) {
    let Ok(event) = serde_json::from_slice::<Value>(line) else {
        return;
    };
    let worktree = real_path(worktree);
    for candidate in collect_write_paths(&event) {
        if candidate.len() > 4096 {
            events.failed = true;
            continue;
        }
        let Some(resolved) = resolve_write_path(&candidate) else {
            continue;
        };
        let real = real_path(&resolved);
        if !real.starts_with(&worktree) {
            let path = real.to_string_lossy().into_owned();
            if !events.writes_outside_worktree.contains(&path) {
                if events.writes_outside_worktree.len() >= 128 {
                    events.failed = true;
                    events
                        .blockers
                        .push("reported write target limit exceeded".into());
                    break;
                }
                events.writes_outside_worktree.push(path);
            }
        }
    }
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

/// A session locator is observed evidence only for its exact owning attempt.
/// Native data remains harness-owned; this schema records no inferred location.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionCheckpoint {
    schema_version: u32,
    task_id: String,
    attempt: u32,
    harness: String,
    session: String,
    native_data_location: Option<String>,
}
impl SessionCheckpoint {
    fn validate(&self, record: &task::TaskRecord, spec: &Spec, id: &str) -> Result<()> {
        if self.schema_version != 2
            || self.task_id != id
            || record.task_id != id
            || !matches!(record.schema_version, 2 | 3)
            || self.attempt == 0
            || self.attempt != spec.attempt
            || self.harness != record.identity.harness
            || self.harness.is_empty()
            || !valid_session(&self.session)
            || self.native_data_location.is_some()
        {
            bail!("native session checkpoint does not belong to this supported attempt");
        }
        Ok(())
    }
}
fn valid_session(session: &str) -> bool {
    !session.trim().is_empty() && session.len() <= 4096 && !session.chars().any(char::is_control)
}
struct SessionOwner {
    task_id: String,
    attempt: u32,
    harness: String,
}

/// Pipe readers never wait for EOF from an escaped descendant: the supervisor
/// polls process completion independently and bounds the final drain.
fn capture(
    mut pipe: impl Read + Send + 'static,
    checkpoint: PathBuf,
    worktree: PathBuf,
    owner: Option<SessionOwner>,
    events: std::sync::Arc<std::sync::Mutex<Events>>,
    tx: std::sync::mpsc::Sender<std::result::Result<(), String>>,
) {
    std::thread::spawn(move || {
        let run = || -> Result<()> {
            let mut total = 0u64;
            let mut chunk = [0u8; 8192];
            let mut line = Vec::new();
            loop {
                let n = pipe.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                if total > STREAM_EVALUATION_LIMIT {
                    bail!("stream evaluation exceeded 64 MiB");
                }
                for byte in &chunk[..n] {
                    if *byte == b'\n' {
                        if !line.is_empty() {
                            let mut events = events
                                .lock()
                                .map_err(|_| Error::new("stream evaluator unavailable"))?;
                            if let Some(owner) = &owner {
                                let had_session = events.session.is_some();
                                events.observe(&owner.harness, &line);
                                observe_writes(&mut events, &line, &worktree);
                                if !had_session && events.session.is_some() {
                                    durable_json(
                                        &checkpoint,
                                        &SessionCheckpoint {
                                            schema_version: 2,
                                            task_id: owner.task_id.clone(),
                                            attempt: owner.attempt,
                                            harness: owner.harness.clone(),
                                            session: events
                                                .session
                                                .clone()
                                                .expect("session was just observed"),
                                            native_data_location: None,
                                        },
                                    )?;
                                }
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
            if !line.is_empty() {
                let mut events = events
                    .lock()
                    .map_err(|_| Error::new("stream evaluator unavailable"))?;
                if owner.is_none() {
                    events.observe_stderr(&line);
                } else {
                    events.failed = true;
                    events.blockers.push("unterminated event stream".into());
                }
            }
            Ok(())
        };
        let mut run = run;
        let _ = tx.send(run().map_err(|_| "stream evaluation failed or exceeded its bound".into()));
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

pub(crate) fn signal_group(pid: u32, signal: i32) {
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
    if spec.schema_version != 2 {
        bail!(
            "legacy or unsupported headless execution: use the original runner for this task; this binary only starts schema 2 attempts"
        );
    }
    let attempt = attempt_dir(dir, &spec);
    confined(&attempt, false)?;
    if attempt.join("result.json").exists() || attempt.join("started.json").exists() {
        bail!("attempt already started; implicit replay refused");
    }
    let mut phase = "frozen_execution_validation";
    let outcome = run_attempt(dir, &attempt, &spec, &mut phase);
    if let Err(error) = &outcome {
        let _ = durable_json(
            &attempt.join("result.json"),
            &json!({"schema_version":2,"task_id":dir.file_name().unwrap_or_default().to_string_lossy(),"attempt":spec.attempt,
            "outcome":"supervisor_error","failure_phase":phase,"failure_category":format!("{:?}",error.kind()),"blockers":["supervisor execution failed"],"acceptance":"not assessed","worktree_preserved":true}),
        );
        let _ = task::set_state(dir, task::TaskState::Failed);
    }
    outcome
}

fn run_attempt(dir: &Path, attempt: &Path, spec: &Spec, phase: &mut &'static str) -> Result<i32> {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let (record, rebuilt, executable) = crate::launch::verify_task(dir, Some(spec))?;
    if record.delivery.layout_version != 3 {
        bail!("primary-owned execution requires delivery layout 3; submit a new assignment");
    }
    let repo = crate::git::discover(&record.repo_root)?;
    let real = validate_executable(&executable, &repo)?;
    if real != record.harness_executable
        || digest_bytes(&std::fs::read(&real)?) != spec.executable_digest
    {
        bail!("harness executable changed since submission; refusing execution");
    }
    crate::catalog::check_headless_version(&record.identity.harness, &spec.harness_version)?;
    validate_parent_attempt(&repo, spec)?;
    validate_frozen_configuration(&record)?;
    let cmux_integration = validate_environment(
        &repo,
        &record.worktree,
        &record.identity.harness,
        &spec.harness_version,
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
    sanitize(&mut command, Some(&cmux_integration.headless));
    let telemetry = crate::config::load(&record.worktree)?
        .map(|loaded| loaded.config.telemetry)
        .unwrap_or_default();
    crate::telemetry::initialize(&telemetry)?;
    let mut telemetry_span = crate::telemetry::span(
        "ahu.harness.run",
        [
            ("ahu.agent.name", record.agent_label()),
            ("ahu.harness", record.identity.harness.clone()),
            ("ahu.model.requested", record.identity.model.clone()),
            ("ahu.version", env!("CARGO_PKG_VERSION").to_string()),
            ("ahu.task.id", record.task_id.clone()),
        ],
    );
    if let Some(version) = record.identity.agent_version.as_deref() {
        telemetry_span.set_string("ahu.agent.version", version);
    }
    telemetry_span.set_string("ahu.harness.version", &spec.harness_version);
    crate::telemetry::configure_child(
        &mut command,
        &telemetry,
        &record.agent_label(),
        &record.identity.harness,
        &record.identity.model,
        record.identity.agent_version.as_deref(),
        Some(&spec.harness_version),
        Some(&record.task_id),
    );
    command
        .env("AHU_BIN", std::env::current_exe()?)
        .env("AHU_EXECUTION_BACKEND", "headless")
        .env("AHU_PARENT_TASK", &record.task_id)
        .env("AHU_WORKER_SESSION", "headless")
        .env("AHU_TASK_ID", &record.task_id)
        .env("AHU_TASK_DIR", dir)
        .env_remove("AHU_RUNTIME_DIR")
        .env_remove("AHU_TASK_INDEX_DIR");
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
    *phase = "worker_spawn";
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
    *phase = "stream_evaluation_and_lifecycle";
    let stdout_events = std::sync::Arc::new(std::sync::Mutex::new(Events::default()));
    let stderr_events = std::sync::Arc::new(std::sync::Mutex::new(Events::default()));
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    let (err_tx, err_rx) = std::sync::mpsc::channel();
    capture(
        child
            .stdout
            .take()
            .ok_or_else(|| Error::new("missing stdout"))?,
        attempt.join("native-session.json"),
        record.worktree.clone(),
        Some(SessionOwner {
            task_id: record.task_id.clone(),
            attempt: spec.attempt,
            harness: record.identity.harness.clone(),
        }),
        stdout_events.clone(),
        out_tx,
    );
    capture(
        child
            .stderr
            .take()
            .ok_or_else(|| Error::new("missing stderr"))?,
        attempt.join("native-session.json"),
        record.worktree.clone(),
        None,
        stderr_events.clone(),
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
    let mut events = std::mem::take(
        &mut *stdout_events
            .lock()
            .map_err(|_| Error::new("stdout evaluator unavailable"))?,
    );
    match output {
        Some(Ok(())) => (),
        Some(Err(error)) => {
            events.failed = true;
            events.blockers.push(error);
        }
        None => {
            events.failed = true;
            events
                .blockers
                .push("stdout drain incomplete; escaped descendant cleanup unknown".into());
        }
    }
    let stderr = std::mem::take(
        &mut *stderr_events
            .lock()
            .map_err(|_| Error::new("stderr evaluator unavailable"))?,
    );
    events.failed |= stderr.failed;
    events.stderr_diagnostics = stderr.stderr_diagnostics;
    events.stderr_unclassified_lines = stderr.stderr_unclassified_lines;
    if !events.stderr_diagnostics.is_empty() {
        events
            .blockers
            .push("recognized failure diagnostic on stderr".into());
    }
    match errors {
        Some(Ok(())) => (),
        Some(Err(error)) => {
            events.failed = true;
            events.blockers.push(error);
        }
        None => {
            events.failed = true;
            events.blockers.push("stderr drain incomplete".into());
        }
    }
    if events.stderr_unclassified_lines > 0 {
        events
            .blockers
            .push("stderr contained unclassified diagnostics; effect on completion unknown".into());
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
    *phase = "child_reconciliation";
    broker.finish()?;
    if broker_failure.is_some() {
        events.failed = true;
        events.blockers.push("broker dispatch failed".into());
    }
    let mut cancellation_results = Vec::new();
    let reconcile_deadline = Instant::now() + Duration::from_secs(5);
    for id in &cancelled_tasks {
        if id == &record.task_id {
            continue;
        }
        let child_dir = lookup(&repo, id)?;
        let value = loop {
            let value = result(&child_dir).unwrap_or_else(
                |_| json!({"outcome":"unknown","error":"child result unavailable"}),
            );
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
    for child_dir in discover_domain(
        &repo,
        dir.parent()
            .ok_or_else(|| Error::new("missing owning store"))?,
    )? {
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
    let skill_catalog = skill_catalog(&record.worktree);
    events.resolve_skills(&skill_catalog);
    let outcome = if let Some((reason, _)) = stop {
        reason
    } else if !status.success() || events.failed {
        "failed"
    } else {
        "succeeded"
    };
    if let Some(model) = events.model.as_deref() {
        telemetry_span.set_string("ahu.model.resolved", model);
    }
    telemetry_span.set_string("ahu.status", outcome);
    if let Some(value) = events.usage.input {
        telemetry_span.set_u64("ahu.tokens.input", value);
    }
    if let Some(value) = events.usage.output {
        telemetry_span.set_u64("ahu.tokens.output", value);
    }
    if let Some(value) = events.usage.cached {
        telemetry_span.set_u64("ahu.tokens.cached", value);
    }
    if let Some(value) = events.usage.cache_write {
        telemetry_span.set_u64("ahu.tokens.cache_write", value);
    }
    if let Some(value) = events.usage.reasoning {
        telemetry_span.set_u64("ahu.tokens.reasoning", value);
    }
    if let Some(value) = events.usage.total {
        telemetry_span.set_u64("ahu.tokens.total", value);
    }
    telemetry_span.set_u64("ahu.skills.available", skill_catalog.len() as u64);
    telemetry_span.set_u64("ahu.skills.invoked.count", events.skills.len() as u64);
    if !events.skills.is_empty() {
        telemetry_span.set_string(
            "ahu.skills.invoked",
            events
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    let helpers: Vec<Value> = events.native.helpers().iter().map(|h| json!({
        "task_id":h.task_id,"role":h.role,"depth":h.depth,"backgrounded":h.backgrounded,"status":h.status,
        "output_reference":{"location":h.output_file,"source":"harness event stream","verified_exists":false},"total_tokens":h.total_tokens
    })).collect();
    events.blockers.truncate(128);
    let result = json!({"schema_version":2,"backend":"headless","task_id":record.task_id,"attempt":spec.attempt,
        "parent_task":spec.parent_task,"parent_attempt":spec.parent_attempt,"broker_request":spec.broker_request,"root_task":spec.root_task,
        "identity":record.identity,"worktree":record.worktree,"branch":record.branch,
        "outcome":outcome,"started_at":started,"finished_at":task::now_rfc3339(),
        "process":{"exit_code":status.code(),"signal":status.signal()},"harness":events,
        "skill_catalog":skill_catalog,
        "native_reference":{"session":events.session,"source":"harness event stream","harness":record.identity.harness,"harness_version":spec.harness_version,"data_location":null,"location_status":"unknown; harness-owned"},
        "native_helpers":helpers,"native_shell_tasks":events.native.shell_tasks(),"native_refusals":events.native.refusals(),"acceptance":"not assessed","completion_verified":false,
        "writes_outside_worktree":events.writes_outside_worktree,"write_evidence":"reported tool targets; not proof of writes",
        "descendant_cancellation":cancellation_results,"native_completeness":native_completeness,"ahu_children":ahu_children,
        "native_cleanup":"unknown for external/provider-managed processes"});
    *phase = "result_persistence";
    if serde_json::to_vec(&result)?.len() > 1024 * 1024 {
        bail!("coordination result exceeded its 1 MiB evaluation bound");
    }
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
    let id = crate::task_ref::resolve(repo, id)?;
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        bail!("invalid headless task id");
    }
    let inventory = discover(repo)?;
    let exact: Vec<_> = inventory
        .iter()
        .filter(|p| p.file_name().is_some_and(|s| s == id.as_str()))
        .collect();
    if exact.len() == 1 {
        let record = task::load(exact[0])?;
        if record.repo_identity != repo.identity() || record.task_id != id {
            bail!("headless task ownership mismatch");
        }
        return Ok(exact[0].clone());
    }
    let found: Vec<_> = inventory
        .into_iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|s| s.to_string_lossy().starts_with(&id))
        })
        .collect();
    if found.len() != 1 {
        bail!("headless task id is missing or ambiguous: {id}");
    }
    let record = task::load(&found[0])?;
    if record.repo_identity != repo.identity()
        || found[0]
            .file_name()
            .is_none_or(|s| s != record.task_id.as_str())
    {
        bail!("headless task ownership mismatch");
    }
    Ok(found[0].clone())
}

fn result(dir: &Path) -> Result<Value> {
    let spec: Spec = read_json(&dir.join("headless.json"))?;
    result_attempt(dir, &spec)
}
// Lifecycle consumers retain the existing result envelope semantics. Display limits
// belong to inspection, never child reconciliation, resume, or cleanup.
fn result_attempt(dir: &Path, spec: &Spec) -> Result<Value> {
    let path = attempt_dir(dir, spec).join("result.json");
    if path.exists() {
        let value: Value = read_json(&path)?;
        review::validate_result(
            &value,
            &dir.file_name().unwrap_or_default().to_string_lossy(),
            spec.attempt,
        )?;
        if value["schema_version"] != spec.schema_version {
            bail!("result schema does not match its owning spec");
        }
        return Ok(value);
    }
    let active = Lock::is_owned(&dir.join("owner.lock")).map_err(|error| {
        Error::new(format!(
            "cannot inspect supervisor ownership; liveness unknown: {error}"
        ))
    })?;
    Ok(json!({"schema_version":spec.schema_version,
        "task_id":dir.file_name().unwrap_or_default().to_string_lossy(),"attempt":spec.attempt,
        "outcome":if active {"running"} else {"interrupted"},"acceptance":"not assessed",
        "completion_verified":false,
        "blockers":if active {Vec::<String>::new()} else {vec!["no live supervisor owns this attempt; process cleanup unknown, no automatic replay".into()]}}))
}

fn review_attempt(
    dir: &Path,
    spec: &Spec,
    read_result: impl FnOnce(&Path) -> Result<Value>,
) -> Result<Value> {
    if !matches!(spec.schema_version, 1 | 2) || spec.attempt == 0 {
        bail!("unsupported headless attempt metadata");
    }
    let id = dir.file_name().unwrap_or_default().to_string_lossy();
    let path = attempt_dir(dir, spec).join("result.json");
    let mut value = match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            let mut value = read_result(&path)?;
            // Older supervisor-error envelopes serialized the directory OsStr.
            // Normalize only the exact encoding of this task's own identifier.
            if value["outcome"] == "supervisor_error"
                && value["task_id"] == serde_json::to_value(dir.file_name())?
            {
                value["task_id"] = json!(id);
            }
            review::validate_result(&value, &id, spec.attempt)?;
            if value["schema_version"] != spec.schema_version {
                bail!("result schema does not match its owning spec");
            }
            value
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let active = Lock::is_owned(&dir.join("owner.lock")).map_err(|error| {
                Error::new(format!(
                    "cannot inspect supervisor ownership; liveness unknown: {error}"
                ))
            })?;
            json!({"schema_version":spec.schema_version,"task_id":id,"attempt":spec.attempt,
                "outcome":if active {"running"} else {"interrupted"},"acceptance":"not assessed",
                "completion_verified":false,
                "blockers":if active {Vec::<String>::new()} else {vec!["no live supervisor owns this attempt; process cleanup unknown, no automatic replay".into()]}})
        }
        Err(error) => return Err(error.into()),
    };
    value["review"] = review::projection(dir, Some(spec), Some(&value), None);
    value["task_handle"] = value["review"]["task_handle"].clone();
    value["capabilities"] = serde_json::to_value(spec)?;
    Ok(value)
}
fn wait(dir: &Path, json_output: bool) -> Result<i32> {
    let spec: Spec = read_json(&dir.join("headless.json"))?;
    loop {
        // Validate public review output without applying the interactive display-size
        // limit to wait's existing full-envelope API.
        let value = review_attempt(dir, &spec, read_json)?;
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
    let admission_path = dir
        .parent()
        .ok_or_else(|| Error::new("task store missing"))?
        .join("launch.lock");
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
    let all = discover_domain(
        repo,
        dir.parent()
            .ok_or_else(|| Error::new("missing owning store"))?,
    )?
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
            let spec: Spec = review::read(&dir.join("headless.json"))?;
            emit(&review_attempt(&dir, &spec, review::read)?, json_output)?;
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
            if frozen.schema_version != 2 {
                bail!(
                    "legacy resume is unsupported by this binary; use the original runner and its legacy store, or submit a new assignment"
                );
            }
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
                &record.worktree,
                &record.identity.harness,
                &spec.harness_version,
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
            let (delivered, delivery) = crate::orchestration::deliver_composed(
                record.delivery.agent_instructions.as_deref(),
                &prompt,
                crate::orchestration::Composition {
                    mode: crate::orchestration::Mode::headless(&spec.options.native_helpers)?,
                    metadata: Some(crate::orchestration::Metadata {
                        task_id: record.task_id.clone(),
                        agent: record.agent_label(),
                        harness: record.identity.harness.clone(),
                        model: record.identity.model.clone(),
                        permissions: record.identity.permissions,
                    }),
                    state: Some(spec.coordination_state()),
                },
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
    let spec = match review::read::<Spec>(&dir.join("headless.json")) {
        Ok(spec) if matches!(spec.schema_version, 1 | 2) && spec.attempt > 0 => spec,
        _ => {
            return Ok(review::projection(
                dir,
                None,
                None,
                Some(
                    "headless.json unavailable: missing, unreadable, malformed, unsupported or beyond the inspection bound",
                ),
            ));
        }
    };
    match review_attempt(dir, &spec, review::read) {
        Ok(value) => Ok(value["review"].clone()),
        Err(_) => Ok(review::projection(
            dir,
            Some(&spec),
            None,
            Some(
                "attempt inspection unavailable: inspect headless.json, result.json and owner.lock; metadata may be missing, unreadable, malformed or beyond the inspection bound",
            ),
        )),
    }
}

/// Whether a live supervisor holds this attempt's ownership lock.
///
/// A read-only probe of `owner.lock`: neither this function nor anything it
/// calls creates or writes the file, so a listing never disturbs a running
/// task. An unreadable signal is the caller's `unknown`, not a claim that no
/// supervisor is running.
pub(crate) fn supervisor_owns_attempt(dir: &Path) -> Result<bool> {
    Lock::is_owned(&dir.join("owner.lock"))
}

#[cfg(test)]
mod telemetry_usage_tests {
    use super::Events;

    #[test]
    fn usage_normalization_keeps_common_cumulative_snapshot_fields() {
        let mut events = Events::default();
        events.observe(
            "codex",
            br#"{"type":"turn.completed","usage":{"input_tokens":12,"output_tokens":7,"cached_tokens":3,"total_tokens":19}}"#,
        );
        events.observe(
            "codex",
            br#"{"type":"turn.completed","usage":{"input_tokens":20,"output_tokens":9,"cached_tokens":5,"total_tokens":29}}"#,
        );
        assert_eq!(events.usage.input, Some(20));
        assert_eq!(events.usage.output, Some(9));
        assert_eq!(events.usage.cached, Some(5));
        assert_eq!(events.usage.total, Some(29));
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::Lock;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn review_regression_hardlinked_lock_never_borrows_live_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner.lock");
        let _held = Lock::acquire(&path).unwrap();
        let alias = dir.path().join("alias.lock");
        std::fs::hard_link(&path, &alias).unwrap();
        assert!(Lock::is_owned(&alias).is_err());
        assert!(Lock::try_acquire(&alias).is_err());
    }

    #[test]
    fn review_regression_lock_modes_checked_on_both_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner.lock");
        drop(Lock::acquire(&path).unwrap());
        for mode in [0o640, 0o666] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(Lock::is_owned(&path).is_err());
            assert!(Lock::try_acquire(&path).is_err());
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert!(!Lock::is_owned(&path).unwrap());
    }
}
