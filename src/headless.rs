//! Unattended attempts with minimal primary-owned state and an ahu supervisor.
//! Process and harness outcomes are evidence, never work acceptance.
pub(crate) mod review;
mod skills;

use crate::harness::{LaunchCommand, LaunchRequest};
use crate::util::{Error, Result, digest_bytes};
use crate::{bail, state, task};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const EVAL_OTEL_CHILD_DRAIN: Duration = Duration::from_millis(750);

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
    let isolation = crate::harness::isolation::profile(harness, &spec.harness_version);
    if let Some(profile) = isolation
        && !spec.native_controls.iter().any(|id| id == profile.id)
    {
        bail!("headless isolation profile is missing or differs from the frozen controls");
    }
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
                Permissions::Auto => {
                    add(&["-c", "sandbox_mode=\"workspace-write\""]);
                    // `approval_policy="never"` prevents interactive prompts;
                    // Codex separately denies MCP tools that still require
                    // approval. Auto agents may use only ahu's local,
                    // read-only tools unattended. Provider-backed tools remain
                    // approval-gated and require an eval-specific opt-in.
                    for tool in crate::harness::codex::AUTO_LOCAL_MCP_TOOLS {
                        let config = crate::harness::codex::auto_local_mcp_config(tool);
                        add(&["-c", config.as_str()]);
                    }
                }
                Permissions::AcceptEdits if spec.session.is_none() => add(&["--approve-for-me"]),
                Permissions::AcceptEdits => bail!(
                    "Codex exec resume has no validated accept-edits mapping; submit a new assignment instead"
                ),
            }
            if spec.session.is_none() {
                add(&["--color", "never"]);
            }
            if let Some(profile) = isolation {
                add(profile.args);
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
            if let Some(profile) = isolation {
                add(profile.args);
            }
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
        .ok_or_else(|| Error::new("project configuration is missing; run ahu setup"))?;
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
    if let Some(isolation) =
        crate::harness::isolation::profile(&plan.pair.harness, &spec.harness_version)
    {
        spec.native_controls.push(isolation.id.into());
    }
    spec.gaps.extend(profile.gaps.clone());
    spec.gaps.push("Native integration evidence is bounded; unsupported configuration and hooks are refused. ahu itself never invokes cmux in this backend; arbitrary shell commands remain outside integration inspection. Same-UID code is not isolated from supervisor state.".into());
    if !spec.child_grants.is_empty() {
        spec.gaps.push("Broker grants approve only frozen registered identities and profiles. Codex workspace-write receives the task's requests directory as a narrow additional write root; read-only Codex transport is refused. Limits: 128 tasks per root grant, depth 8, 16 active/interrupted tasks per repository, no hidden queue or global token cap.".into());
    }

    if spec.options.native_helpers == "bounded" {
        spec.gaps.push("The ENTIRE assignment has a read-only MODEL TOOL ceiling: parent and helpers have no model tools for editing, building, shell commands or shell-launching registered children. Settings-defined hooks are outside that tool ceiling and their side effects are not proven read-only. MCP tools and slash commands are disabled; repository settings remain discoverable. Roles are requested/observed, not an allowlist; total helper count is not capped. Budget is 5 USD per attempt, concurrency 1, depth 1, helper model equals manifest model.".into());
    }
    spec.native_profile = Some(profile);
    let cmux_integration = validate_environment(
        repo,
        &repo.root,
        &plan.pair.harness,
        &spec.harness_version,
        &plan.harness_executable,
    )?;
    crate::selection::check_launch_compatibility_with_policy(
        &plan.harness_executable,
        &plan.pair.harness,
        &plan.pair.model,
        permissions,
        &repo.root,
        Some(&cmux_integration.headless),
    )?;
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
    crate::commands::preflight(console, repo, &loaded, &plan)?;
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
    executable: &Path,
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
    let loaded = crate::config::load(config_root)?
        .ok_or_else(|| Error::new("missing project configuration"))?;
    crate::catalog::check_harness_version(
        harness,
        version,
        loaded
            .config
            .harness_version_pins
            .get(harness)
            .map(String::as_str),
    )?;
    crate::cmux::integration::enforce_headless_executable(config_root, harness, version, executable)
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub observed_at: Option<String>,
    #[serde(default = "crate::telemetry::SkillEvidence::unverified")]
    pub evidence: crate::telemetry::SkillEvidence,
    #[serde(default = "crate::telemetry::SkillEvidence::unverified")]
    pub execution: crate::telemetry::SkillEvidence,
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

/// USD amounts reported by the harness. These are estimates/engine values,
/// not provider billing records.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReportedCost {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl TokenUsage {
    /// Shared field names for OTEL and the opt-in local metrics projection.
    pub(crate) fn normalized_fields(&self) -> [(&'static str, Option<u64>); 6] {
        [
            ("ahu.tokens.input", self.input),
            ("ahu.tokens.output", self.output),
            ("ahu.tokens.cached", self.cached),
            ("ahu.tokens.cache_write", self.cache_write),
            ("ahu.tokens.reasoning", self.reasoning),
            ("ahu.tokens.total", self.total),
        ]
    }

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

impl ReportedCost {
    fn observe_claude_result(&mut self, event: &Value) {
        if event.get("type").and_then(Value::as_str) != Some("result") {
            return;
        }
        self.observe(event.get("total_cost_usd"), "claude_code_result_total");
    }

    fn observe_opencode_step(
        &mut self,
        event: &Value,
        seen: &mut std::collections::BTreeSet<String>,
    ) {
        if event.get("type").and_then(Value::as_str) != Some("step_finish") {
            return;
        }
        let part = match event.get("part") {
            Some(part) => part,
            None => return,
        };
        let Some(id) = part.get("id").and_then(Value::as_str) else {
            return;
        };
        if !seen.insert(id.to_owned()) {
            return;
        }
        self.observe(part.get("cost"), "opencode_step_finish_sum");
    }

    fn observe(&mut self, value: Option<&Value>, source: &str) {
        let Some(value) = value.and_then(Value::as_f64) else {
            return;
        };
        if !value.is_finite() || value < 0.0 {
            return;
        }
        let total = self.usd.unwrap_or(0.0) + value;
        if !total.is_finite() {
            return;
        }
        self.usd = Some(total);
        self.source = Some(source.to_owned());
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

/// Attribute an observed invocation only to exact local catalog entries.
/// Duplicate paths with identical bytes establish the digest but not which
/// copy the harness loaded; conflicting copies establish neither provenance.
fn attribute_skill_provenance(skills: &mut [SkillInvocation], catalog: &[SkillCatalogEntry]) {
    for skill in skills {
        let matches: Vec<_> = catalog
            .iter()
            .filter(|entry| entry.name == skill.name)
            .collect();
        if matches.is_empty() {
            continue;
        }

        let digests: std::collections::BTreeSet<_> =
            matches.iter().map(|entry| entry.digest.as_str()).collect();
        if digests.len() == 1 {
            skill.digest = Some(matches[0].digest.clone());
        }
        if matches.len() == 1 {
            skill.source = Some(matches[0].source.clone());
        }
    }
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
    pub stderr_informational_lines: u64,
    #[serde(default)]
    pub usage: TokenUsage,
    #[serde(default)]
    pub trajectory: crate::eval::trajectory::Observer,
    #[serde(default)]
    pub cost: ReportedCost,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub skills: Vec<SkillInvocation>,
    #[serde(default)]
    pub skill_unknown_events: u64,
    #[serde(skip)]
    skill_calls: Vec<skills::Call>,
    #[serde(default = "crate::telemetry::SkillEvidence::unverified")]
    pub skill_observation: crate::telemetry::SkillEvidence,
    #[serde(skip_serializing)]
    pub native_observations: Vec<Value>,
    #[serde(default)]
    #[serde(skip_serializing)]
    pub native: crate::native::Observations,
    #[serde(default, skip_serializing)]
    pub writes_outside_worktree: Vec<String>,
    #[serde(skip)]
    native_event_count: usize,
    #[serde(skip)]
    cost_step_ids: std::collections::BTreeSet<String>,
}
impl Events {
    fn observe_stderr(&mut self, line: &[u8]) {
        let text = String::from_utf8_lossy(line);
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        // Exact native stdin progress messages carry no failure or private data.
        // Keep their count without making every noninteractive Codex run unknown.
        if matches!(
            text,
            "Reading prompt from stdin..." | "Reading additional input from stdin..."
        ) {
            self.stderr_informational_lines += 1;
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
                self.trajectory.malformed();
                self.failed = true;
                self.blockers.push("malformed or truncated event".into());
                return;
            }
        };
        self.usage.observe(&event);
        self.trajectory.observe(harness, &event);
        match harness {
            "claude-code" => self.cost.observe_claude_result(&event),
            "opencode" => self
                .cost
                .observe_opencode_step(&event, &mut self.cost_step_ids),
            _ => (),
        }
        self.observe_skill(harness, &event);
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
            (_, "tool_use") | ("antigravity", "tool_result") => (),
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

/// Resolve existing prefixes before applying `..`: a symlink followed by a
/// parent component must retain filesystem traversal semantics, including when
/// the final write target does not yet exist.
fn real_path(path: &Path) -> Result<PathBuf> {
    fn resolve(path: &Path, links: usize) -> Result<PathBuf> {
        if links > 40 {
            bail!("reported write path exceeds the symlink traversal bound");
        }
        let mut resolved = PathBuf::new();
        let mut components = path.components();
        while let Some(component) = components.next() {
            match component {
                std::path::Component::CurDir => (),
                std::path::Component::ParentDir => {
                    resolved.pop();
                }
                other => {
                    resolved.push(other);
                    match std::fs::symlink_metadata(&resolved) {
                        Ok(metadata) if metadata.file_type().is_symlink() => {
                            let target = std::fs::read_link(&resolved)?;
                            resolved.pop();
                            resolved.push(target);
                            resolved.push(components.as_path());
                            return resolve(&resolved, links + 1);
                        }
                        Ok(_) => (),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
        Ok(resolved)
    }
    resolve(path, 0)
}

/// Only absolute paths are classified; a relative path's tool working
/// directory is not known from the event. Preserve `..` until symlinks resolve.
fn resolve_write_path(candidate: &str) -> Option<PathBuf> {
    let raw: PathBuf = candidate.into();
    raw.is_absolute().then_some(raw)
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
        // Retain only known completion or collaboration markers, never payloads.
        return if kind.contains("collab")
            || event
                .pointer("/item/type")
                .and_then(Value::as_str)
                .is_some_and(|s| s.contains("collab"))
        {
            json!({"type":"collab"})
        } else if harness == "codex" && kind == "turn.completed" {
            json!({"type":"turn.completed"})
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
    let worktree = match real_path(worktree) {
        Ok(worktree) => worktree,
        Err(_) => {
            events.failed = true;
            events
                .blockers
                .push("task worktree could not be resolved for write observation".into());
            return;
        }
    };
    for candidate in collect_write_paths(&event) {
        if candidate.len() > 4096 {
            events.failed = true;
            continue;
        }
        let Some(resolved) = resolve_write_path(&candidate) else {
            continue;
        };
        let real = match real_path(&resolved) {
            Ok(real) => real,
            Err(_) => {
                events.failed = true;
                events.blockers.push("reported write target could not be resolved within the symlink traversal bound".into());
                continue;
            }
        };
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

/// The pipe reader can finish after the process-exit poll. Reconcile its final
/// boundary evidence before accepting a successful exit or joining children.
fn boundary_after_drain(events: &mut Events, stop: &mut Option<(&'static str, Instant)>) -> bool {
    if stop.is_some() || events.writes_outside_worktree.is_empty() {
        return false;
    }
    events.blockers.push(
        "harness reported a write target outside the task worktree in its final event drain".into(),
    );
    *stop = Some(("boundary_violation", Instant::now()));
    true
}

/// Recorded `writes_outside_worktree` from a finished attempt's result
/// envelope, if it exists and is non-empty. Never fails the caller.
pub fn recorded_writes_outside_worktree(dir: &Path) -> Option<Vec<String>> {
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
            || !matches!(record.schema_version, 2..=4)
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
                    events.trajectory.incomplete();
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

// Persist only an allowlisted category, never arbitrary subprocess or filesystem
// error text, which can contain paths, request contents or credentials.
fn supervisor_failure_code(error: &Error) -> &'static str {
    match error.to_string().as_str() {
        "Git ownership changed during storage verification" => "git_ownership_changed",
        "storage ownership verification unavailable" => "storage_verification_unavailable",
        "stdout evaluator unavailable" => "stream_evaluator_unavailable",
        _ => "unclassified",
    }
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
            "outcome":"supervisor_error","failure_phase":phase,"failure_category":format!("{:?}",error.kind()),"failure_code":supervisor_failure_code(error),"blockers":["supervisor execution failed"],"acceptance":"not assessed","worktree_preserved":true}),
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
        &real,
    )?;
    crate::selection::check_launch_compatibility_with_policy(
        &real,
        &record.identity.harness,
        &record.identity.model,
        record.identity.permissions,
        &record.worktree,
        Some(&cmux_integration.headless),
    )?;
    crate::auth_binding::capture_task_for_model(
        &repo,
        &record.identity.harness,
        &record.identity.model,
        dir,
    )?;
    let eval_otel_capture = crate::telemetry::eval_endpoint_override().is_some();
    if let Some(parent) = &spec.parent_task {
        let parent_dir = lookup(&repo, parent)?;
        if parent_dir.join("cancel.json").exists() {
            bail!("ancestor cancellation blocks this attempt");
        }
    }
    let measurement_started = Instant::now();
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
    telemetry_span.set_u64("ahu.task.attempt", u64::from(spec.attempt));
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
        Some(spec.attempt),
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
            } else if stdout_events
                .lock()
                .map(|events| !events.writes_outside_worktree.is_empty())
                .unwrap_or(false)
            {
                Some("boundary_violation")
            } else if Instant::now() >= deadline {
                Some("timed_out")
            } else if storage_failed || broker_failure.is_some() {
                Some("capture_failed")
            } else {
                None
            };
            if let Some(reason) = reason {
                if reason == "boundary_violation"
                    && let Ok(mut events) = stdout_events.lock()
                {
                    events.blockers.push(
                        "harness reported a write target outside the task worktree; process stopped after observing the event".into(),
                    );
                }
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
    // When a harness exits, its MCP subprocess sees stdio EOF and needs a
    // moment to emit its session summary and flush OTLP before we kill any
    // remaining descendants. Keep this grace limited to local eval capture;
    // cancelled and timed-out attempts still follow their termination path.
    if stop.is_none() && eval_otel_capture {
        std::thread::sleep(EVAL_OTEL_CHILD_DRAIN);
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
    if !matches!(&output, Some(Ok(()))) {
        events.trajectory.incomplete();
    }
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
    events.stderr_informational_lines = stderr.stderr_informational_lines;
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
    if boundary_after_drain(&mut events, &mut stop) {
        cancelled_tasks = cancel_tree(&repo, dir, "boundary_violation")?;
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
    attribute_skill_provenance(&mut events.skills, &skill_catalog);
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
    for (key, value) in events.usage.normalized_fields() {
        if let Some(value) = value {
            telemetry_span.set_u64(key, value);
        }
    }
    if let Some(cost_usd) = events.cost.usd {
        telemetry_span.set_f64("ahu.cost.harness_reported_usd", cost_usd);
        if let Some(source) = events.cost.source.as_deref() {
            telemetry_span.set_string("ahu.cost.source", source);
        }
    }
    telemetry_span.set_u64("ahu.skills.available", skill_catalog.len() as u64);
    telemetry_span.set_string("ahu.skills.observation", events.skill_observation.as_str());
    if events.skill_observation == crate::telemetry::SkillEvidence::Observed {
        telemetry_span.set_u64("ahu.skills.invoked.count", events.skills.len() as u64);
    }
    telemetry_span.set_u64("ahu.skills.unclassified.count", events.skill_unknown_events);
    if events
        .skills
        .iter()
        .any(|skill| skill.execution == crate::telemetry::SkillEvidence::Observed)
    {
        for (status, field) in [
            ("completed", "ahu.skills.completed.count"),
            ("failed", "ahu.skills.failed.count"),
        ] {
            telemetry_span.set_u64(
                field,
                events
                    .skills
                    .iter()
                    .filter(|skill| skill.status == status)
                    .count() as u64,
            );
        }
    }
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
        let provenance: Vec<_> = events
            .skills
            .iter()
            .filter_map(|skill| {
                skill
                    .digest
                    .as_ref()
                    .map(|digest| format!("{}=sha256:{digest}", skill.name))
            })
            .collect();
        if !provenance.is_empty() {
            telemetry_span.set_string("ahu.skills.invoked.provenance", provenance.join(","));
        }
    }
    let helpers: Vec<Value> = events.native.helpers().iter().map(|h| json!({
        "task_id":h.task_id,"role":h.role,"depth":h.depth,"backgrounded":h.backgrounded,"status":h.status,
        "output_reference":{"location":h.output_file,"source":"harness event stream","verified_exists":false},"total_tokens":h.total_tokens
    })).collect();
    events.blockers.truncate(128);
    let mut result = json!({"schema_version":2,"backend":"headless","task_id":record.task_id,"attempt":spec.attempt,
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
    let elapsed_ms = Some(
        measurement_started
            .elapsed()
            .as_millis()
            .min(u64::MAX as u128) as u64,
    );
    if let Some(metrics) =
        crate::telemetry::local_metrics(&telemetry, &events.usage, &events.cost, elapsed_ms)
    {
        result["metrics"] = serde_json::to_value(metrics)?;
    }
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

/// Return only the opt-in, bounded numeric projections for completed headless
/// attempts. Callers must already have established task ownership and opt-in.
pub(crate) fn private_attempt_metrics(
    dir: &Path,
    expected_harness: &str,
    observation_limit: usize,
) -> Result<Vec<PrivateAttemptMetrics>> {
    let spec_bytes = review::read_bytes(&dir.join("headless.json"), Some(1024 * 1024))?;
    let spec: Spec = serde_json::from_slice(&spec_bytes)
        .map_err(|_| Error::new("headless attempt metadata is malformed"))?;
    if !matches!(spec.schema_version, 1 | 2) || spec.attempt == 0 || spec.attempt > 4096 {
        bail!("unsupported headless attempt metadata");
    }
    let id = dir.file_name().unwrap_or_default().to_string_lossy();
    let mut output = Vec::new();
    for number in 1..=spec.attempt {
        let path = dir.join(format!("attempt-{number}")).join("result.json");
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        }
        if output.len() >= observation_limit {
            bail!("private report exceeds its observation bound");
        }
        let bytes = review::read_bytes(&path, Some(1024 * 1024))?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| Error::new("invalid headless result metadata"))?;
        review::validate_result(&value, &id, number)?;
        let metrics = match value.get("metrics") {
            Some(metrics) if metrics["schema_version"] == 1 => Some((metrics, 1_u64)),
            Some(metrics) if metrics["schema_version"] == 2 => Some((metrics, 2_u64)),
            Some(_) => bail!("unsupported local metrics projection version"),
            None => None,
        };
        let empty_values = serde_json::Map::new();
        let values = match &metrics {
            Some((metrics, _)) => metrics["values"]
                .as_object()
                .ok_or_else(|| Error::new("invalid local metrics projection"))?,
            None => &empty_values,
        };
        let amount = |name: &str| -> Option<u64> {
            let value = values.get(name)?;
            (value["kind"] == "observed")
                .then(|| value["value"].as_u64())
                .flatten()
        };
        let cost_usd = match values.get("ahu.cost.harness_reported_usd") {
            Some(cost_value) if cost_value["kind"] == "observed_float" => cost_value["value"]
                .as_f64()
                .filter(|amount| amount.is_finite() && *amount >= 0.0),
            Some(cost_value) if cost_value["kind"] == "unavailable" => None,
            None if metrics.is_none_or(|(_, version)| version == 1) => None,
            _ => bail!("invalid local cost projection"),
        };
        let elapsed_ms = match metrics.and_then(|(metrics, _)| metrics.get("elapsed_ms")) {
            Some(value) if value["kind"] == "observed" => value["value"].as_u64(),
            Some(value) if value["kind"] == "unavailable" => None,
            None => None,
            _ => bail!("invalid elapsed time projection"),
        };
        // Cost provenance is fixed by the adapter and checked against the
        // frozen harness; raw event data is never copied into the report.
        let result_harness = value.pointer("/identity/harness").and_then(Value::as_str);
        let outcome = value["outcome"].as_str().unwrap_or("unknown");
        if result_harness.is_some_and(|harness| harness != expected_harness)
            || (result_harness.is_none() && outcome != "supervisor_error")
        {
            bail!("headless result identity unavailable or inconsistent");
        }
        let source = match expected_harness {
            "claude-code" if cost_usd.is_some() => Some("claude_code_result_total".to_owned()),
            "opencode" if cost_usd.is_some() => Some("opencode_step_finish_sum".to_owned()),
            _ => None,
        };
        if cost_usd.is_some() != source.is_some() {
            bail!("harness cost source does not match frozen identity");
        }
        output.push(PrivateAttemptMetrics {
            attempt: number,
            outcome: outcome.to_owned(),
            native_complete: value
                .pointer("/native_completeness/complete")
                .and_then(Value::as_bool),
            elapsed_ms,
            metrics_observed: metrics.is_some(),
            usage: TokenUsage {
                input: amount("ahu.tokens.input"),
                output: amount("ahu.tokens.output"),
                cached: amount("ahu.tokens.cached"),
                cache_write: amount("ahu.tokens.cache_write"),
                reasoning: amount("ahu.tokens.reasoning"),
                total: amount("ahu.tokens.total"),
            },
            cost: ReportedCost {
                usd: cost_usd,
                source,
            },
        });
    }
    Ok(output)
}

#[derive(Debug)]
pub(crate) struct PrivateAttemptMetrics {
    pub attempt: u32,
    pub outcome: String,
    pub native_complete: Option<bool>,
    pub elapsed_ms: Option<u64>,
    pub metrics_observed: bool,
    pub usage: TokenUsage,
    pub cost: ReportedCost,
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
                Some(
                    "succeeded"
                        | "failed"
                        | "cancelled"
                        | "timed_out"
                        | "capture_failed"
                        | "boundary_violation"
                )
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
            let task_record = task::load(&dir)?;
            crate::auth_binding::verify_task_resume_for_model(
                repo,
                &task_record.identity.harness,
                &task_record.identity.model,
                &dir,
            )?;
            recover_resume(&dir)?;
            let previous = result(&dir)?;
            if !matches!(
                previous["outcome"].as_str(),
                Some("succeeded" | "failed" | "cancelled" | "timed_out" | "boundary_violation")
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
            if real != record.harness_executable {
                bail!(
                    "harness executable path changed; previous attempt preserved, resume refused"
                );
            }
            let version = crate::selection::probe_version(
                real.to_str()
                    .ok_or_else(|| Error::new("harness executable path is not UTF-8"))?,
            )
            .ok_or_else(|| {
                Error::new("cannot determine installed harness version before resume")
            })?;
            let loaded = crate::config::load(&record.worktree)?
                .ok_or_else(|| Error::new("missing task configuration"))?;
            let pin = loaded
                .config
                .harness_version_pins
                .get(&record.identity.harness)
                .map(String::as_str);
            crate::catalog::check_harness_version(&record.identity.harness, &version, pin)?;
            let current_digest = digest_bytes(&std::fs::read(&real)?);
            if pin.is_some() && current_digest != spec.executable_digest {
                bail!(
                    "pinned harness executable changed; previous attempt preserved, resume refused"
                );
            }
            validate_environment(
                repo,
                &record.worktree,
                &record.identity.harness,
                &version,
                &real,
            )?;
            // Keep the finished attempt's provenance immutable. A floating
            // project may resume with a newer installed CLI after the same
            // native isolation checks pass; the next attempt records its own
            // version and executable digest.
            let previous_spec = spec.clone();
            let previous_record = record.clone();
            spec.harness_version = version.clone();
            spec.executable_digest = current_digest;
            record.enforcement.harness_version = Some(version);
            record.harness_executable = real;
            let original_spec = previous_spec.clone();
            let original_record = previous_record.clone();
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
            let approval_lease =
                crate::approval::retire_for_resume(&dir, &record.task_id, previous_spec.attempt)?;
            let old_attempt = attempt_dir(&dir, &previous_spec);
            durable_json(&old_attempt.join("submission.json"), &previous_record)?;
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
            drop(approval_lease);
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
    use crate::state;

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
        assert!(events.terminal);
        let profile = crate::native::profile(&crate::native::Request {
            harness: "codex",
            harness_version: "0.157.1",
            policy: crate::native::DISABLED,
            session_model: "test-model",
            helper_model: None,
            helper_role: "unused",
            max_concurrent: 1,
            max_depth: 1,
            budget_usd: None,
            assignment_writes: true,
        })
        .unwrap();
        assert!(events.native.completeness(&profile).complete);
        assert_eq!(
            super::native_metadata(
                "codex",
                &serde_json::json!({
                    "type":"turn.completed", "message":"private", "usage":{"input_tokens":29}
                })
            ),
            serde_json::json!({"type":"turn.completed"})
        );
    }

    #[test]
    fn private_attempt_reader_extracts_only_opted_in_numeric_projections() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(root.path())
                .status()
                .unwrap()
                .success()
        );
        let repo = crate::git::discover(root.path()).unwrap();
        let id = "00000000-0000-7000-8000-000000000001";
        let dir = super::store(&repo).unwrap().join(id);
        state::create_private_dir_all(&dir).unwrap();
        let spec = super::Spec {
            schema_version: 2,
            options: super::Options::default(),
            harness_version: "test".into(),
            executable_digest: "digest".into(),
            parent_task: None,
            parent_attempt: None,
            root_task: None,
            broker_request: None,
            child_grants: Vec::new(),
            depth: 0,
            attempt: 3,
            session: None,
            broker_dir: None,
            native_profile: None,
            native_controls: Vec::new(),
            gaps: Vec::new(),
        };
        state::write_json(&dir.join("headless.json"), &spec).unwrap();
        let result = serde_json::json!({
            "schema_version":2,
            "task_id":id,
            "attempt":1,
            "outcome":"succeeded",
            "identity":{"harness":"claude-code"},
            "process":{"exit_code":0},
            "harness":{"terminal":true,"failed":false,"summary":"private-payload"},
            "metrics":{"schema_version":2,"elapsed_ms":{"kind":"observed","value":314},"values":{
                "ahu.tokens.input":{"kind":"observed","value":0},
                "ahu.tokens.output":{"kind":"unavailable"},
                "ahu.tokens.cached":{"kind":"unavailable"},
                "ahu.tokens.cache_write":{"kind":"unavailable"},
                "ahu.tokens.reasoning":{"kind":"unavailable"},
                "ahu.tokens.total":{"kind":"unavailable"},
                "ahu.cost.harness_reported_usd":{"kind":"observed_float","value":0.0125}
            }}
        });
        let attempt = dir.join("attempt-1");
        state::create_private_dir_all(&attempt).unwrap();
        state::write_json(&attempt.join("result.json"), &result).unwrap();

        let legacy_attempt = dir.join("attempt-2");
        state::create_private_dir_all(&legacy_attempt).unwrap();
        state::write_json(
            &legacy_attempt.join("result.json"),
            &serde_json::json!({
                "schema_version":2,
                "task_id":id,
                "attempt":2,
                "outcome":"succeeded",
                "identity":{"harness":"claude-code"},
                "process":{"exit_code":0},
                "harness":{"terminal":true,"failed":false},
                "metrics":{"schema_version":1,"token_aggregation":"maximum-reported-per-field","values":{
                    "ahu.tokens.input":{"kind":"observed","value":9},
                    "ahu.tokens.output":{"kind":"observed","value":4},
                    "ahu.tokens.cached":{"kind":"unavailable"},
                    "ahu.tokens.cache_write":{"kind":"unavailable"},
                    "ahu.tokens.reasoning":{"kind":"unavailable"},
                    "ahu.tokens.total":{"kind":"unavailable"}
                }}
            }),
        )
        .unwrap();

        let failed_attempt = dir.join("attempt-3");
        state::create_private_dir_all(&failed_attempt).unwrap();
        state::write_json(
            &failed_attempt.join("result.json"),
            &serde_json::json!({
                "schema_version":2,
                "task_id":id,
                "attempt":3,
                "outcome":"supervisor_error",
                "failure_category":"temporary",
                "blockers":["supervisor execution failed"]
            }),
        )
        .unwrap();

        let observations = super::private_attempt_metrics(&dir, "claude-code", 3).unwrap();
        assert_eq!(observations.len(), 3);
        assert_eq!(observations[0].attempt, 1);
        assert_eq!(observations[0].elapsed_ms, Some(314));
        assert!(observations[0].metrics_observed);
        assert_eq!(observations[0].usage.input, Some(0));
        assert_eq!(observations[0].usage.output, None);
        assert_eq!(observations[0].cost.usd, Some(0.0125));
        assert_eq!(
            observations[0].cost.source.as_deref(),
            Some("claude_code_result_total")
        );
        assert!(!format!("{:?}", observations[0].outcome).contains("private-payload"));
        assert!(observations[1].metrics_observed);
        assert_eq!(observations[1].usage.input, Some(9));
        assert_eq!(observations[1].usage.output, Some(4));
        assert_eq!(observations[1].elapsed_ms, None);
        assert_eq!(observations[1].cost.usd, None);
        assert!(!observations[2].metrics_observed);
        assert_eq!(observations[2].outcome, "supervisor_error");
        assert_eq!(observations[2].usage.input, None);
        assert_eq!(observations[2].cost.usd, None);
        assert!(super::private_attempt_metrics(&dir, "claude-code", 2).is_err());

        std::fs::write(dir.join("headless.json"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert!(super::private_attempt_metrics(&dir, "claude-code", 3).is_err());
    }

    #[test]
    fn reported_cost_is_normalized_without_claiming_billing() {
        let mut claude = Events::default();
        claude.observe(
            "claude-code",
            br#"{"type":"result","subtype":"success","total_cost_usd":0.0125,"usage":{"input_tokens":10}}"#,
        );
        assert_eq!(claude.cost.usd, Some(0.0125));
        assert_eq!(
            claude.cost.source.as_deref(),
            Some("claude_code_result_total")
        );

        let mut opencode = Events::default();
        let step =
            br#"{"type":"step_finish","part":{"id":"step-1","cost":0.003,"reason":"tool-calls"}}"#;
        opencode.observe("opencode", step);
        opencode.observe("opencode", step);
        opencode.observe(
            "opencode",
            br#"{"type":"step_finish","part":{"id":"step-2","cost":0.002,"reason":"stop"}}"#,
        );
        assert_eq!(opencode.cost.usd, Some(0.005));
        assert_eq!(
            opencode.cost.source.as_deref(),
            Some("opencode_step_finish_sum")
        );

        let mut unsupported = Events::default();
        unsupported.observe(
            "codex",
            br#"{"type":"turn.completed","usage":{"input_tokens":10},"total_cost_usd":99}"#,
        );
        assert_eq!(unsupported.cost.usd, None);
        unsupported.observe(
            "antigravity",
            br#"{"type":"result","status":"success","response":"ok","total_cost_usd":99}"#,
        );
        assert_eq!(unsupported.cost.usd, None);

        let mut invalid = Events::default();
        invalid.observe("claude-code", br#"{"type":"result","total_cost_usd":-1}"#);
        assert_eq!(invalid.cost.usd, None);

        let encoded = serde_json::to_value(&opencode.cost).unwrap();
        assert_eq!(encoded["usd"], 0.005);
        assert_eq!(encoded["source"], "opencode_step_finish_sum");
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

#[cfg(test)]
mod write_tracking_tests {
    use super::*;

    #[test]
    fn write_path_discovery_handles_normalized_keys_nested_tools_and_embedded_json() {
        let event = serde_json::json!({
            "outer": [{
                "tool_name": "WriteFile",
                "arguments": r#"{"file_path":"/tmp/project/../outside/new.txt"}"#
            }],
            "tool": "Read",
            "path": "/tmp/not-a-write.txt"
        });
        let paths = collect_write_paths(&event);
        assert!(
            paths
                .iter()
                .any(|path| path == "/tmp/project/../outside/new.txt")
        );
        assert!(!paths.iter().any(|path| path == "/tmp/not-a-write.txt"));
        assert_eq!(
            resolve_write_path("/tmp/project/../outside/new.txt").unwrap(),
            PathBuf::from("/tmp/project/../outside/new.txt")
        );
        assert!(resolve_write_path("").is_none());
        assert!(resolve_write_path("relative/file.txt").is_none());
    }

    #[test]
    fn final_stream_drain_cannot_accept_a_reported_outside_write() {
        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Events::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        let stream = format!(
            "{}\n{}\n",
            json!({"tool":"Write", "input":{"path":outside.path().join("new.txt")}}),
            json!({"type":"turn.completed"})
        );
        capture(
            std::io::Cursor::new(stream.into_bytes()),
            worktree.path().join("unused.json"),
            worktree.path().to_path_buf(),
            Some(SessionOwner {
                task_id: "synthetic".into(),
                attempt: 1,
                harness: "codex".into(),
            }),
            shared.clone(),
            tx,
        );
        // The process-exit poll has already completed with no stop reason.
        let mut stop = None;
        rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        let mut events = shared.lock().unwrap();
        assert!(events.terminal);
        assert!(boundary_after_drain(&mut events, &mut stop));
        assert_eq!(stop.unwrap().0, "boundary_violation");
        assert!(!boundary_after_drain(&mut events, &mut stop));
        let mut cancelled = Some(("cancelled", Instant::now()));
        assert!(!boundary_after_drain(&mut events, &mut cancelled));
        assert_eq!(cancelled.unwrap().0, "cancelled");
    }

    #[test]
    fn dangling_write_symlinks_are_classified_and_cycles_fail_closed() {
        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("not-created.txt");
        let link = worktree.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let event = serde_json::to_vec(&json!({"tool":"Write", "input":{"path":link}})).unwrap();
        let mut events = Events::default();
        observe_writes(&mut events, &event, worktree.path());
        assert_eq!(
            events.writes_outside_worktree,
            vec![
                outside
                    .path()
                    .canonicalize()
                    .unwrap()
                    .join("not-created.txt")
                    .to_string_lossy()
                    .into_owned()
            ]
        );
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("link", &link).unwrap();
        let mut events = Events::default();
        observe_writes(&mut events, &event, worktree.path());
        assert!(events.failed);
        assert!(
            events
                .blockers
                .iter()
                .any(|blocker| blocker.contains("symlink traversal bound"))
        );
    }

    #[test]
    fn missing_write_targets_resolve_symlinks_before_parent_components() {
        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(outside.path().join("child")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("child"), worktree.path().join("link"))
            .unwrap();
        let target = worktree.path().join("link/../new/file.txt");
        let event = json!({"tool":"Write", "input":{"path":target}});
        let mut events = Events::default();
        observe_writes(
            &mut events,
            &serde_json::to_vec(&event).unwrap(),
            worktree.path(),
        );
        assert_eq!(
            events.writes_outside_worktree,
            vec![
                outside
                    .path()
                    .canonicalize()
                    .unwrap()
                    .join("new/file.txt")
                    .to_string_lossy()
                    .into_owned()
            ]
        );

        // The reverse alias stays inside and must not cause a false violation.
        std::fs::create_dir(worktree.path().join("child")).unwrap();
        std::os::unix::fs::symlink(worktree.path().join("child"), outside.path().join("link"))
            .unwrap();
        let event =
            json!({"tool":"Write", "input":{"path":outside.path().join("link/../new.txt")}});
        let mut events = Events::default();
        observe_writes(
            &mut events,
            &serde_json::to_vec(&event).unwrap(),
            worktree.path(),
        );
        assert!(events.writes_outside_worktree.is_empty());
    }

    #[test]
    fn observed_write_events_record_only_unique_paths_outside_the_worktree() {
        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let inside_path = worktree.path().join("edited.txt");
        let outside_path = outside.path().join("edited.txt");
        let event = serde_json::json!({
            "tool": "Write",
            "input": {"filePath": inside_path, "targetFile": outside_path}
        });
        let encoded = serde_json::to_vec(&event).unwrap();
        let mut events = Events::default();
        observe_writes(&mut events, &encoded, worktree.path());
        observe_writes(&mut events, &encoded, worktree.path());
        assert_eq!(events.writes_outside_worktree.len(), 1);
        assert!(events.writes_outside_worktree[0].ends_with("edited.txt"));

        observe_writes(&mut events, b"not json", worktree.path());
        assert_eq!(events.writes_outside_worktree.len(), 1);
    }

    #[test]
    fn native_metadata_is_projected_to_only_supported_helper_fields() {
        let progress = native_metadata(
            "claude-code",
            &serde_json::json!({
                "type":"system", "subtype":"task_progress", "task_id":"child",
                "usage":{"total_tokens":12}, "secret":"ignore"
            }),
        );
        assert_eq!(progress["task_id"], "child");
        assert_eq!(progress["usage"]["total_tokens"], 12);
        assert!(progress.get("secret").is_none());

        let result = native_metadata(
            "claude-code",
            &serde_json::json!({
                "type":"result", "subagent_stats":{
                    "spawned":2, "completed":1, "max_depth":3,
                    "refused":{"budget":4, "secret":9},
                    "by_type":{"worker":1, "invalid":"x"}
                }, "huge_payload":"ignored"
            }),
        );
        assert_eq!(result["subagent_stats"]["spawned"], 2);
        assert_eq!(result["subagent_stats"]["refused"]["budget"], 4);
        assert!(result["subagent_stats"]["refused"].get("secret").is_none());
        assert!(result.get("huge_payload").is_none());

        assert_eq!(
            native_metadata("codex", &serde_json::json!({"type":"item.started"})),
            serde_json::json!({})
        );
        assert_eq!(
            native_metadata("codex", &serde_json::json!({"type":"collab.started"})),
            serde_json::json!({"type":"collab"})
        );
    }

    #[test]
    fn native_metadata_bounds_and_session_validation_fail_closed() {
        assert!(bounded_native_metadata(
            &serde_json::json!({"a": [1, true, null]})
        ));
        assert!(!bounded_native_metadata(&serde_json::json!(
            "x".repeat(4097)
        )));
        assert!(!bounded_native_metadata(&serde_json::json!(vec![0; 257])));
        assert!(!bounded_native_metadata(&serde_json::json!(u64::MAX)));
        assert!(valid_session("session-1"));
        assert!(!valid_session(" \n"));
        assert!(!valid_session("bad\nlocator"));
        assert!(!valid_session(&"x".repeat(4097)));
    }

    #[test]
    fn stream_capture_checkpoints_sessions_and_classifies_incomplete_streams() {
        let dir = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(init.success());
        let repo = crate::git::discover(dir.path()).unwrap();
        let store = crate::storage::HeadlessStore::for_repo(&repo).unwrap();
        let checkpoint = store.directory.join("task-1/attempt-2/session.json");
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Events::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        capture(
            std::io::Cursor::new(
                b"{\"type\":\"step_start\",\"sessionID\":\"opencode-session\"}\n".to_vec(),
            ),
            checkpoint.clone(),
            dir.path().to_path_buf(),
            Some(SessionOwner {
                task_id: "task-1".into(),
                attempt: 2,
                harness: "opencode".into(),
            }),
            shared.clone(),
            tx,
        );
        rx.recv().unwrap().unwrap();
        assert_eq!(
            shared.lock().unwrap().session.as_deref(),
            Some("opencode-session")
        );
        let saved: SessionCheckpoint =
            serde_json::from_slice(&std::fs::read(checkpoint).unwrap()).unwrap();
        assert_eq!(saved.task_id, "task-1");
        assert_eq!(saved.attempt, 2);

        let incomplete = std::sync::Arc::new(std::sync::Mutex::new(Events::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        capture(
            std::io::Cursor::new(b"{\"type\":\"turn.started\"}".to_vec()),
            dir.path().join("unused.json"),
            dir.path().to_path_buf(),
            Some(SessionOwner {
                task_id: "task-1".into(),
                attempt: 2,
                harness: "codex".into(),
            }),
            incomplete.clone(),
            tx,
        );
        rx.recv().unwrap().unwrap();
        assert!(incomplete.lock().unwrap().failed);
        assert!(
            incomplete
                .lock()
                .unwrap()
                .trajectory
                .observation
                .stream_incomplete
        );
        assert!(
            incomplete
                .lock()
                .unwrap()
                .blockers
                .contains(&"unterminated event stream".into())
        );
    }

    #[test]
    fn stderr_capture_keeps_only_categories_or_a_count() {
        let dir = tempfile::tempdir().unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Events::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        capture(
            std::io::Cursor::new(b"permission denied\nordinary diagnostic\n\n".to_vec()),
            dir.path().join("unused.json"),
            dir.path().to_path_buf(),
            None,
            events.clone(),
            tx,
        );
        rx.recv().unwrap().unwrap();
        let events = events.lock().unwrap();
        assert!(events.failed);
        assert_eq!(events.stderr_diagnostics, ["permission denial"]);
        assert_eq!(events.stderr_unclassified_lines, 1);
    }

    #[test]
    fn stdin_progress_is_informational_but_nearby_diagnostics_are_not_suppressed() {
        let mut events = Events::default();
        events.observe_stderr(b"Reading prompt from stdin...");
        events.observe_stderr(b"Reading additional input from stdin...\n");
        assert_eq!(events.stderr_informational_lines, 2);
        assert_eq!(events.stderr_unclassified_lines, 0);
        assert!(!events.failed);
        events.observe_stderr(b"Reading prompt from stdin... warning");
        events.observe_stderr(
            b"Could not create otel exporter: failed to build OTLP metrics exporter",
        );
        assert_eq!(events.stderr_unclassified_lines, 2);
        events.observe_stderr(b"Reading prompt from stdin... permission denied");
        assert!(events.failed);
        assert_eq!(events.stderr_diagnostics, ["permission denial"]);
    }
}

#[cfg(test)]
mod profile_and_metadata_tests {
    use super::*;

    #[test]
    fn schema_reader_accepts_only_supported_headless_spec_versions() {
        #[derive(Deserialize)]
        struct Version {
            #[serde(deserialize_with = "read_spec_version")]
            schema_version: u32,
        }
        for version in [1, 2] {
            let parsed: Version =
                serde_json::from_value(json!({"schema_version":version})).unwrap();
            assert_eq!(parsed.schema_version, version);
        }
        for version in [0, 3, 99] {
            assert!(serde_json::from_value::<Version>(json!({"schema_version":version})).is_err());
        }
    }

    #[test]
    fn skill_catalog_is_sorted_and_excludes_hidden_or_incomplete_entries() {
        let root = tempfile::tempdir().unwrap();
        for path in [
            ".agents/skills/zeta",
            ".agents/skills/alpha",
            ".claude/skills/native",
        ] {
            std::fs::create_dir_all(root.path().join(path)).unwrap();
        }
        std::fs::create_dir_all(root.path().join(".agents/skills/.hidden")).unwrap();
        std::fs::write(root.path().join(".agents/skills/zeta/SKILL.md"), "zeta").unwrap();
        std::fs::write(root.path().join(".agents/skills/alpha/SKILL.md"), "alpha").unwrap();
        std::fs::write(root.path().join(".claude/skills/native/SKILL.md"), "native").unwrap();
        let catalog = skill_catalog(root.path());
        assert_eq!(catalog.len(), 3);
        assert!(
            catalog
                .windows(2)
                .all(|pair| pair[0].source <= pair[1].source)
        );
        assert!(catalog.iter().all(|entry| !entry.name.starts_with('.')));
        assert_eq!(
            catalog
                .iter()
                .find(|entry| entry.name == "zeta")
                .unwrap()
                .digest,
            digest_bytes(b"zeta")
        );
    }

    #[test]
    fn skill_invocation_provenance_requires_unambiguous_catalog_evidence() {
        let invocation = |name: &str| SkillInvocation {
            name: name.into(),
            source: None,
            digest: None,
            status: "invoked".into(),
            completed_at: None,
            elapsed_ms: None,
            harness: Some("opencode".into()),
            observed_at: None,
            evidence: crate::telemetry::SkillEvidence::Observed,
            execution: crate::telemetry::SkillEvidence::Unverified,
        };
        let entry = |source: &str, digest: &str| SkillCatalogEntry {
            name: "review".into(),
            source: source.into(),
            digest: digest.into(),
        };

        let mut skills = vec![invocation("review"), invocation("external")];
        attribute_skill_provenance(
            &mut skills,
            &[entry(".agents/skills/review/SKILL.md", &"a".repeat(64))],
        );
        assert_eq!(
            skills[0].source.as_deref(),
            Some(".agents/skills/review/SKILL.md")
        );
        assert!(
            skills[0]
                .digest
                .as_deref()
                .is_some_and(|digest| digest == "a".repeat(64))
        );
        assert!(skills[1].source.is_none());
        assert!(skills[1].digest.is_none());

        let mut identical_copies = vec![invocation("review")];
        attribute_skill_provenance(
            &mut identical_copies,
            &[
                entry(".agents/skills/review/SKILL.md", &"b".repeat(64)),
                entry(".claude/skills/review/SKILL.md", &"b".repeat(64)),
            ],
        );
        assert!(identical_copies[0].source.is_none());
        assert!(
            identical_copies[0]
                .digest
                .as_deref()
                .is_some_and(|digest| digest == "b".repeat(64))
        );

        let mut conflicting_copies = vec![invocation("review")];
        attribute_skill_provenance(
            &mut conflicting_copies,
            &[
                entry(".agents/skills/review/SKILL.md", &"c".repeat(64)),
                entry(".claude/skills/review/SKILL.md", &"d".repeat(64)),
            ],
        );
        assert!(conflicting_copies[0].source.is_none());
        assert!(conflicting_copies[0].digest.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn executable_validation_rejects_repository_binaries_and_cmux_wrappers() {
        use std::os::unix::fs::PermissionsExt;

        let repo_root = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo_root.path())
            .status()
            .unwrap();
        assert!(init.success());
        let repo = crate::git::discover(repo_root.path()).unwrap();
        let inside = repo_root.path().join("agent");
        std::fs::write(&inside, "#!/bin/sh\necho ok\n").unwrap();
        let mut permissions = std::fs::metadata(&inside).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&inside, permissions.clone()).unwrap();
        assert!(
            validate_executable(&inside, &repo)
                .unwrap_err()
                .to_string()
                .contains("outside the repository")
        );

        let outside_root = tempfile::tempdir().unwrap();
        let wrapper = outside_root.path().join("wrapper");
        std::fs::write(&wrapper, "#!/bin/sh\n# cmux wrapper\n").unwrap();
        std::fs::set_permissions(&wrapper, permissions.clone()).unwrap();
        assert!(
            validate_executable(&wrapper, &repo)
                .unwrap_err()
                .to_string()
                .contains("script wrapper")
        );

        let valid = outside_root.path().join("agent");
        std::fs::write(&valid, "#!/bin/sh\necho ok\n").unwrap();
        std::fs::set_permissions(&valid, permissions).unwrap();
        assert_eq!(
            validate_executable(&valid, &repo).unwrap(),
            valid.canonicalize().unwrap()
        );
    }

    #[test]
    fn missing_result_reports_supervisor_liveness_without_claiming_completion() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root.path().join("task-1");
        let spec = sample_spec();
        std::fs::create_dir_all(&task_dir).unwrap();
        let owner = Lock::acquire(&task_dir.join("owner.lock")).unwrap();
        let running = result_attempt(&task_dir, &spec).unwrap();
        assert_eq!(running["outcome"], "running");
        assert_eq!(running["acceptance"], "not assessed");
        assert_eq!(running["completion_verified"], false);
        drop(owner);
        let interrupted = result_attempt(&task_dir, &spec).unwrap();
        assert_eq!(interrupted["outcome"], "interrupted");
        assert!(
            interrupted["blockers"][0]
                .as_str()
                .unwrap()
                .contains("no live supervisor")
        );
    }

    #[test]
    fn recorded_write_paths_ignore_missing_malformed_and_empty_evidence() {
        let root = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(init.success());
        let repo = crate::git::discover(root.path()).unwrap();
        let store = crate::storage::HeadlessStore::for_repo(&repo).unwrap();
        let task_dir = store.directory.join("task-1");
        state::create_private_dir_all(&task_dir).unwrap();
        assert!(recorded_writes_outside_worktree(&task_dir).is_none());
        state::write_private_file(&task_dir.join("headless.json"), b"bad json").unwrap();
        assert!(recorded_writes_outside_worktree(&task_dir).is_none());

        let spec = sample_spec();
        state::write_private_file(
            &task_dir.join("headless.json"),
            &serde_json::to_vec(&spec).unwrap(),
        )
        .unwrap();
        let result_file = attempt_dir(&task_dir, &spec).join("result.json");
        state::create_private_dir_all(result_file.parent().unwrap()).unwrap();
        state::write_private_file(&result_file, br#"{"writes_outside_worktree":[]}"#).unwrap();
        assert!(recorded_writes_outside_worktree(root.path()).is_none());
        state::write_private_file(
            &result_file,
            br#"{"writes_outside_worktree":["/outside/a",4,"/outside/b"]}"#,
        )
        .unwrap();
        let _: Value = read_json(&result_file).unwrap();
        assert_eq!(
            recorded_writes_outside_worktree(&task_dir).unwrap(),
            ["/outside/a", "/outside/b"]
        );
    }

    #[test]
    fn result_reading_validates_attempt_and_schema_before_returning_evidence() {
        let (_root, repo) = super::filesystem_behavior_tests::repository();
        let dir = store(&repo).unwrap().join("abc");
        let spec = sample_spec();
        let attempt = attempt_dir(&dir, &spec);
        state::create_private_dir_all(&attempt).unwrap();
        state::write_private_file(
            &dir.join("headless.json"),
            &serde_json::to_vec(&spec).unwrap(),
        )
        .unwrap();
        let valid = json!({"schema_version":2,"task_id":"abc","attempt":1,
            "outcome":"failed","blockers":["synthetic failure"]});
        let path = attempt.join("result.json");
        state::write_private_file(&path, &serde_json::to_vec(&valid).unwrap()).unwrap();
        assert_eq!(result(&dir).unwrap(), valid);
        let reviewed = review_attempt(&dir, &spec, review::read).unwrap();
        assert_eq!(reviewed["outcome"], "failed");
        assert_eq!(reviewed["capabilities"]["attempt"], 1);
        assert_eq!(reviewed["task_handle"], reviewed["review"]["task_handle"]);
        for (field, value, message) in [
            ("schema_version", json!(1), "schema does not match"),
            ("task_id", json!("def"), "another attempt"),
            ("attempt", json!(2), "another attempt"),
            ("blockers", json!([false]), "blockers are malformed"),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            state::write_private_file(&path, &serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(
                result(&dir).unwrap_err().to_string().contains(message),
                "{field}"
            );
            assert!(
                review_attempt(&dir, &spec, review::read)
                    .unwrap_err()
                    .to_string()
                    .contains(message),
                "{field}"
            );
        }
        for (schema_version, attempt) in [(3, 1), (2, 0)] {
            let unsupported = Spec {
                schema_version,
                attempt,
                ..spec.clone()
            };
            assert!(
                review_attempt(&dir, &unsupported, |_| panic!(
                    "invalid spec must not read a result"
                ))
                .unwrap_err()
                .to_string()
                .contains("unsupported headless attempt")
            );
        }
    }

    #[test]
    fn supervisor_failure_codes_never_persist_arbitrary_error_text() {
        for (message, expected) in [
            (
                "Git ownership changed during storage verification",
                "git_ownership_changed",
            ),
            (
                "storage ownership verification unavailable",
                "storage_verification_unavailable",
            ),
            (
                "stdout evaluator unavailable",
                "stream_evaluator_unavailable",
            ),
            ("provider failed with secret-marker", "unclassified"),
            (
                "Git ownership changed during storage verification secret-marker",
                "unclassified",
            ),
        ] {
            assert_eq!(
                super::supervisor_failure_code(&Error::new(message)),
                expected
            );
        }
    }

    #[test]
    fn result_review_normalizes_only_its_own_legacy_supervisor_error_identifier() {
        let (_root, repo) = super::filesystem_behavior_tests::repository();
        let dir = store(&repo).unwrap().join("abc");
        let spec = sample_spec();
        let path = attempt_dir(&dir, &spec).join("result.json");
        state::create_private_dir_all(path.parent().unwrap()).unwrap();
        let mut value = json!({"schema_version":2,"task_id":dir.file_name(),"attempt":1,
            "outcome":"supervisor_error"});
        state::write_private_file(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            review_attempt(&dir, &spec, review::read).unwrap()["task_id"],
            "abc"
        );
        value["task_id"] = serde_json::to_value(Path::new("def").file_name()).unwrap();
        state::write_private_file(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            review_attempt(&dir, &spec, review::read)
                .unwrap_err()
                .to_string()
                .contains("another attempt")
        );
    }

    #[test]
    fn result_without_readable_ownership_reports_unknown_liveness() {
        let root = tempfile::tempdir().unwrap();
        let spec = sample_spec();
        assert!(
            result_attempt(root.path(), &spec)
                .unwrap_err()
                .to_string()
                .contains("liveness unknown")
        );
        assert!(
            review_attempt(root.path(), &spec, review::read)
                .unwrap_err()
                .to_string()
                .contains("liveness unknown")
        );
        assert!(!root.path().join("owner.lock").exists());
    }

    #[test]
    fn batch_command_rebuilds_frozen_helper_profile_and_refuses_policy_drift() {
        let root = tempfile::tempdir().unwrap();
        let request = LaunchRequest {
            model: "claude-sonnet-4-6",
            prompt: "--literal prompt",
            cwd: root.path(),
            permissions: crate::agent::Permissions::Prompt,
        };
        let mut spec = sample_spec();
        spec.options.native_helpers = "bounded".into();
        spec.harness_version = "Claude Code 2.1.270".into();
        assert!(
            batch_command("claude-code", &request, &spec)
                .unwrap_err()
                .to_string()
                .contains("bounded native helpers are unavailable")
        );
        let profile = build_native_profile("claude-code", request.model, &spec).unwrap();
        spec.native_profile = Some(profile.clone());
        let command = batch_command("claude-code", &request, &spec).unwrap();
        assert_eq!(command.program, "claude");
        assert_eq!(command.args[command.prompt_arg.unwrap()], request.prompt);
        assert_eq!(command.args[command.args.len() - 2], "--");
        assert!(
            command
                .args
                .windows(profile.args.len())
                .any(|args| args == profile.args)
        );
        spec.native_profile.as_mut().unwrap().helper_model = "different-model".into();
        assert!(
            batch_command("claude-code", &request, &spec)
                .unwrap_err()
                .to_string()
                .contains("differs from the frozen profile")
        );
        spec.native_profile = Some(profile);
        spec.harness_version = "2.1.269".into();
        assert!(
            batch_command("claude-code", &request, &spec)
                .unwrap_err()
                .to_string()
                .contains("not validated")
        );
    }

    #[test]
    fn batch_resume_preserves_permission_mapping_and_literal_prompt() {
        use crate::agent::Permissions;
        let root = tempfile::tempdir().unwrap();
        let spec = Spec {
            session: Some("session-123".into()),
            ..sample_spec()
        };
        for (harness, permissions, program, expected) in [
            (
                "claude-code",
                Permissions::Auto,
                "claude",
                vec![
                    "--print",
                    "--model",
                    "synthetic-model",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--permission-prompts",
                    "none",
                    "--disallowedTools",
                    "Agent,Task,TeamCreate,TeamDelete",
                    "--permission-mode",
                    "auto",
                    "--resume",
                    "session-123",
                    "--",
                ],
            ),
            (
                "claude-code",
                Permissions::AcceptEdits,
                "claude",
                vec![
                    "--print",
                    "--model",
                    "synthetic-model",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--permission-prompts",
                    "none",
                    "--disallowedTools",
                    "Agent,Task,TeamCreate,TeamDelete",
                    "--permission-mode",
                    "acceptEdits",
                    "--resume",
                    "session-123",
                    "--",
                ],
            ),
            (
                "antigravity",
                Permissions::AcceptEdits,
                "agy",
                vec![
                    "--model",
                    "synthetic-model",
                    "--output-format",
                    "stream-json",
                    "--print-timeout",
                    "1800s",
                    "--mode",
                    "accept-edits",
                    "--conversation",
                    "session-123",
                    "--print",
                ],
            ),
            (
                "antigravity",
                Permissions::Auto,
                "agy",
                vec![
                    "--model",
                    "synthetic-model",
                    "--output-format",
                    "stream-json",
                    "--print-timeout",
                    "1800s",
                    "--dangerously-skip-permissions",
                    "--conversation",
                    "session-123",
                    "--print",
                ],
            ),
        ] {
            let request = LaunchRequest {
                model: "synthetic-model",
                prompt: "--literal $(prompt)\nnext line",
                cwd: root.path(),
                permissions,
            };
            let command = batch_command(harness, &request, &spec).unwrap();
            assert_eq!(command.program, program);
            assert_eq!(command.prompt_arg, Some(expected.len()));
            assert_eq!(&command.args[..expected.len()], expected.as_slice());
            assert_eq!(command.args.last().unwrap(), request.prompt);
            assert_eq!(command.args.len(), expected.len() + 1);
        }
        for (harness, model, permissions, message) in [
            ("codex", "", Permissions::Prompt, "exact model is required"),
            (
                "codex",
                "--model",
                Permissions::Prompt,
                "exact model is required",
            ),
            (
                "codex",
                "synthetic-model",
                Permissions::AcceptEdits,
                "resume has no validated accept-edits mapping",
            ),
            (
                "unknown",
                "synthetic-model",
                Permissions::Prompt,
                "no headless adapter",
            ),
        ] {
            let request = LaunchRequest {
                model,
                prompt: "synthetic",
                cwd: root.path(),
                permissions,
            };
            assert!(
                batch_command(harness, &request, &spec)
                    .unwrap_err()
                    .to_string()
                    .contains(message)
            );
        }
    }

    #[test]
    fn event_observation_enforces_helper_count_and_metadata_bounds() {
        let mut events = Events::default();
        let progress = br#"{"type":"system","subtype":"task_progress","task_id":"synthetic"}"#;
        for _ in 0..256 {
            events.observe("claude-code", progress);
        }
        assert!(!events.failed);
        assert_eq!(events.native_event_count, 256);
        events.observe("claude-code", progress);
        assert!(events.failed);
        assert_eq!(events.native_event_count, 257);
        assert_eq!(events.blockers, ["native helper evaluation limit exceeded"]);

        let mut events = Events::default();
        let oversized =
            json!({"type": "system", "subtype": "task_progress", "task_id": "x".repeat(4097)});
        events.observe("claude-code", &serde_json::to_vec(&oversized).unwrap());
        assert!(events.failed);
        assert_eq!(
            events.blockers,
            ["native metadata evaluation bound exceeded"]
        );
        assert!(events.native.helpers().is_empty());
    }

    fn saved_attempt(repo: &crate::git::Repo, id: &str, spec: &Spec) -> PathBuf {
        let dir = store(repo).unwrap().join(id);
        let record: task::TaskRecord = serde_json::from_value(json!({
            "schema_version": 2, "task_id": id, "title": "Synthetic attempt",
            "created_at": "2026-01-01T00:00:00Z", "repo_identity": repo.identity(),
            "repo_root": repo.root, "branch": "ahu/synthetic", "worktree": repo.root,
            "identity": {"mode": "automatic", "agent": "auto", "permissions": "prompt",
                "harness": "codex", "model": "synthetic-model"},
            "policy_digest": "synthetic", "catalog_version": crate::catalog::CATALOG_VERSION,
            "config_snapshot": {"entries": [], "skipped_directories": []},
            "config_snapshot_digest": "synthetic",
            "materialize": {"written": [], "removed": [], "concurrently_modified": []},
            "launch_command": {"program": "synthetic-executable", "args": []},
            "delivery": {"nonce": "synthetic", "agent_instructions": null, "digest": "synthetic"},
            "prompt_digest": digest_bytes(b"original prompt"),
            "enforcement": {"harness": "codex", "harness_version": null,
                "model_fixed_for_session": false, "gaps": [], "applied_controls": []},
            "state": "exited"
        }))
        .unwrap();
        task::save(&dir, &record, "original prompt").unwrap();
        durable_json(&dir.join("headless.json"), spec).unwrap();
        confined(&attempt_dir(&dir, spec), true).unwrap();
        dir
    }

    #[test]
    fn resume_recovery_restores_only_attempts_without_spawn_intent() {
        let (_root, repo) = super::filesystem_behavior_tests::repository();
        for spawned in [false, true] {
            let old = sample_spec();
            let dir = saved_attempt(&repo, if spawned { "abc" } else { "def" }, &old);
            recover_resume(&dir).unwrap();
            let original = task::load(&dir).unwrap();
            let next = Spec {
                attempt: 2,
                session: Some("session-2".into()),
                ..old.clone()
            };
            let mut changed = original.clone();
            changed.state = task::TaskState::Starting;
            changed.prompt_digest = digest_bytes(b"new prompt");
            task::save(&dir, &changed, "new prompt").unwrap();
            durable_json(&dir.join("headless.json"), &next).unwrap();
            durable_json(
                &dir.join("resume-journal.json"),
                &json!({
                    "record": original, "spec": old, "prompt": "original prompt", "next_attempt": 2
                }),
            )
            .unwrap();
            let evidence = attempt_dir(&dir, &old).join("result.json");
            state::write_private_file(&evidence, b"preserved previous result").unwrap();
            if spawned {
                durable_json(
                    &attempt_dir(&dir, &next).join("spawn-intent.json"),
                    &json!({}),
                )
                .unwrap();
            }
            recover_resume(&dir).unwrap();
            assert!(!dir.join("resume-journal.json").exists());
            assert_eq!(
                task::load(&dir).unwrap(),
                if spawned { changed } else { original }
            );
            assert_eq!(
                task::load_prompt(&dir).unwrap(),
                if spawned {
                    "new prompt"
                } else {
                    "original prompt"
                }
            );
            let recovered: Spec = read_json(&dir.join("headless.json")).unwrap();
            assert_eq!(
                serde_json::to_value(recovered).unwrap(),
                serde_json::to_value(if spawned { next } else { old }).unwrap()
            );
            assert_eq!(
                std::fs::read(&evidence).unwrap(),
                b"preserved previous result"
            );
            recover_resume(&dir).unwrap();
        }
    }

    #[test]
    fn parent_admission_requires_live_matching_attempt_and_consumed_request() {
        let (_root, repo) = super::filesystem_behavior_tests::repository();
        let parent = sample_spec();
        let dir = saved_attempt(&repo, "abc", &parent);
        validate_parent_attempt(&repo, &sample_spec()).unwrap();
        let mut child = Spec {
            parent_task: Some("abc".into()),
            parent_attempt: Some(1),
            broker_request: Some("0123456789abcdef".into()),
            ..sample_spec()
        };
        let refused = |spec: &Spec, message: &str| {
            assert!(
                validate_parent_attempt(&repo, spec)
                    .unwrap_err()
                    .to_string()
                    .contains(message)
            );
        };
        child.parent_attempt = Some(2);
        refused(&child, "stale parent attempt");
        child.parent_attempt = Some(1);
        refused(&child, "no longer live");
        let _owner = Lock::acquire(&dir.join("owner.lock")).unwrap();
        for path in [
            dir.join("cancel.json"),
            attempt_dir(&dir, &parent).join("admission-closed.json"),
            attempt_dir(&dir, &parent).join("result.json"),
        ] {
            durable_json(&path, &json!({})).unwrap();
            refused(&child, "ended or closed child admission");
            std::fs::remove_file(path).unwrap();
        }
        for request in [None, Some("short"), Some("0123456789abcdeg")] {
            child.broker_request = request.map(str::to_owned);
            refused(&child, "lacks a broker request identity");
        }
        child.broker_request = Some("0123456789abcdef".into());
        let claim = attempt_dir(&dir, &parent).join("broker/0123456789abcdef.claim.json");
        let valid =
            json!({"state": "dispatching", "parent_attempt": 1, "request_id": "0123456789abcdef"});
        for (key, value) in [
            ("state", json!("consumed")),
            ("parent_attempt", json!(2)),
            ("request_id", json!("fedcba9876543210")),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = value;
            durable_json(&claim, &invalid).unwrap();
            refused(&child, "does not match a consumed request");
        }
        durable_json(&claim, &valid).unwrap();
        validate_parent_attempt(&repo, &child).unwrap();
    }

    #[test]
    fn cancellation_follows_current_attempt_descendants_only() {
        let (_root, repo) = super::filesystem_behavior_tests::repository();
        let parent = sample_spec();
        let root = saved_attempt(&repo, "aaa", &parent);
        let child = Spec {
            parent_task: Some("aaa".into()),
            parent_attempt: Some(1),
            attempt: 2,
            ..sample_spec()
        };
        let child_dir = saved_attempt(&repo, "bbb", &child);
        let grandchild = Spec {
            parent_task: Some("bbb".into()),
            parent_attempt: Some(2),
            ..sample_spec()
        };
        let grandchild_dir = saved_attempt(&repo, "ccc", &grandchild);
        let stale = Spec {
            parent_attempt: Some(0),
            ..child.clone()
        };
        let stale_dir = saved_attempt(&repo, "ddd", &stale);
        let unrelated = saved_attempt(&repo, "eee", &sample_spec());
        let mut ids = cancel_tree(&repo, &root, "synthetic cancellation").unwrap();
        ids.sort();
        assert_eq!(ids, ["aaa", "bbb", "ccc"]);
        for (dir, spec) in [
            (&root, &parent),
            (&child_dir, &child),
            (&grandchild_dir, &grandchild),
        ] {
            let closed: Value =
                read_json(&attempt_dir(dir, spec).join("admission-closed.json")).unwrap();
            assert_eq!(closed["attempt"], spec.attempt);
            assert_eq!(closed["reason"], "synthetic cancellation");
            let cancel: Value = read_json(&dir.join("cancel.json")).unwrap();
            assert_eq!(cancel["descendants"], true);
            assert_eq!(cancel["reason"], "synthetic cancellation");
            assert_eq!(task::load_prompt(dir).unwrap(), "original prompt");
        }
        for (dir, spec) in [(&stale_dir, &stale), (&unrelated, &parent)] {
            assert!(!dir.join("cancel.json").exists());
            assert!(
                !attempt_dir(dir, spec)
                    .join("admission-closed.json")
                    .exists()
            );
        }
    }

    #[test]
    fn cleanup_requires_terminal_result_and_preserves_coordination_evidence() {
        let (_root, repo) = super::filesystem_behavior_tests::repository();
        let spec = sample_spec();
        let dir = saved_attempt(&repo, "abc", &spec);
        let attempt = attempt_dir(&dir, &spec);
        let capture = attempt.join("events.jsonl");
        state::write_private_file(&capture, b"synthetic capture").unwrap();
        assert!(
            control(&repo, "cleanup", "abc", None, true)
                .unwrap_err()
                .to_string()
                .contains("known terminal attempt")
        );
        assert!(capture.exists());
        let result =
            json!({"schema_version": 2, "task_id": "abc", "attempt": 1, "outcome": "failed"});
        durable_json(&attempt.join("result.json"), &result).unwrap();
        for name in ["stderr.log", "supervisor.log", "native.log", "final.txt"] {
            state::write_private_file(&attempt.join(name), b"synthetic capture").unwrap();
        }
        durable_json(
            &attempt.join("broker/request.claim.json"),
            &json!({"retained": true}),
        )
        .unwrap();
        durable_json(&dir.join("requests/request.json"), &json!({})).unwrap();
        durable_json(&dir.join("requests/private/nested.json"), &json!({})).unwrap();
        durable_json(&dir.join("attempt-not-a-number/events.jsonl"), &json!({})).unwrap();
        assert_eq!(control(&repo, "cleanup", "abc", None, true).unwrap(), 0);
        for name in [
            "events.jsonl",
            "stderr.log",
            "supervisor.log",
            "native.log",
            "final.txt",
        ] {
            assert!(!attempt.join(name).exists());
        }
        assert!(!dir.join("requests/request.json").exists());
        for path in [
            "task.json",
            "prompt.txt",
            "headless.json",
            "attempt-1/result.json",
            "attempt-1/artifacts-removed.json",
            "attempt-1/broker/request.claim.json",
            "requests/private/nested.json",
            "attempt-not-a-number/events.jsonl",
        ] {
            assert!(dir.join(path).is_file(), "{path}");
        }
        assert_eq!(super::result(&dir).unwrap(), result);
        assert_eq!(control(&repo, "cleanup", "abc", None, true).unwrap(), 0);
    }

    #[test]
    fn isolation_owned_process_cancellation_reaps_child() {
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        for signal in [libc::SIGTERM, libc::SIGKILL] {
            let mut child = Command::new("/bin/sleep")
                .arg("30")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .unwrap();
            signal_group(child.id(), signal);
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert_eq!(status.signal(), Some(signal));
                    break;
                }
                if Instant::now() > deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("owned process did not terminate");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn isolation_profile_is_frozen_and_applies_on_resume_without_removing_context() {
        let root = tempfile::tempdir().unwrap();
        let request = LaunchRequest {
            model: "synthetic-model",
            prompt: "literal prompt",
            cwd: root.path(),
            permissions: crate::agent::Permissions::Prompt,
        };
        let mut spec = sample_spec();
        spec.harness_version = "2.1.283 (Claude Code)".into();
        assert!(batch_command("claude-code", &request, &spec).is_err());
        let profile =
            crate::harness::isolation::profile("claude-code", &spec.harness_version).unwrap();
        spec.native_controls.push(profile.id.into());
        for session in [None, Some("synthetic-session".into())] {
            spec.session = session;
            let command = batch_command("claude-code", &request, &spec).unwrap();
            assert!(command.args.windows(2).any(|pair| pair == profile.args));
            for forbidden in [
                "--bare",
                "--safe-mode",
                "--strict-mcp-config",
                "--disable-slash-commands",
                "--setting-sources",
            ] {
                assert!(!command.args.iter().any(|arg| arg == forbidden));
            }
            assert_eq!(command.args[command.prompt_arg.unwrap()], "literal prompt");
        }
        assert!(crate::harness::isolation::profile("codex", "0.157.2").is_none());
        assert!(crate::harness::isolation::profile("claude-code", "2.1.284").is_none());
    }

    #[test]
    fn codex_effective_profile_is_frozen_on_launch_and_resume() {
        let root = tempfile::tempdir().unwrap();
        let request = LaunchRequest {
            model: "synthetic-model",
            prompt: "literal prompt",
            cwd: root.path(),
            permissions: crate::agent::Permissions::Prompt,
        };
        let mut spec = sample_spec();
        spec.harness_version = "codex-cli 0.157.1".into();
        assert!(batch_command("codex", &request, &spec).is_err());
        let profile = crate::harness::isolation::profile("codex", &spec.harness_version).unwrap();
        spec.native_controls.push(profile.id.into());
        for session in [None, Some("synthetic-session".into())] {
            spec.session = session;
            let command = batch_command("codex", &request, &spec).unwrap();
            assert!(
                command
                    .args
                    .windows(profile.args.len())
                    .any(|args| args == profile.args)
            );
            assert!(
                !command
                    .args
                    .iter()
                    .any(|v| v == "features.hooks=false" || v == "--dangerously-bypass-hook-trust")
            );
            assert_eq!(command.args[command.prompt_arg.unwrap()], "literal prompt");
        }
    }

    #[test]
    fn codex_auto_headless_approves_local_tools_and_checkpoint_request() {
        use crate::agent::Permissions;

        let root = tempfile::tempdir().unwrap();
        let request = LaunchRequest {
            model: "synthetic-model",
            prompt: "literal prompt",
            cwd: root.path(),
            permissions: Permissions::Auto,
        };
        for session in [None, Some("session-123".to_string())] {
            let spec = Spec {
                harness_version: "codex-cli 0.155.1".into(),
                session,
                ..sample_spec()
            };
            let command = batch_command("codex", &request, &spec).unwrap();
            for tool in crate::harness::codex::AUTO_LOCAL_MCP_TOOLS {
                let config = crate::harness::codex::auto_local_mcp_config(tool);
                assert!(command.args.contains(&config), "missing {config:?}");
            }
            for tool in ["ahu_typed_decide", "ahu_skills_suggest"] {
                assert!(
                    !command.args.iter().any(|arg| arg.contains(tool)),
                    "provider-backed tool {tool} must retain its approval gate"
                );
            }
            assert_eq!(command.args[command.prompt_arg.unwrap()], request.prompt);
        }
    }

    fn sample_spec() -> Spec {
        Spec {
            schema_version: 2,
            options: Options::default(),
            harness_version: "1.0.0".into(),
            executable_digest: "digest".into(),
            parent_task: None,
            parent_attempt: None,
            root_task: None,
            broker_request: None,
            child_grants: Vec::new(),
            depth: 0,
            attempt: 1,
            session: None,
            broker_dir: None,
            native_profile: None,
            native_controls: Vec::new(),
            gaps: Vec::new(),
        }
    }
}

#[cfg(test)]
mod filesystem_behavior_tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    pub(super) fn repository() -> (tempfile::TempDir, crate::git::Repo) {
        let root = tempfile::tempdir().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(root.path())
                .status()
                .unwrap()
                .success()
        );
        let repo = crate::git::discover(root.path()).unwrap();
        (root, repo)
    }

    #[test]
    fn confinement_creates_only_private_directories_and_rejects_redirection() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        let nested = root.join("task/attempt");
        confined_at(&root, &nested, false).unwrap();
        assert!(!root.exists());
        confined_at(&root, &nested, true).unwrap();
        for dir in [&root, &root.join("task"), &nested] {
            assert_eq!(
                std::fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert!(
            confined_at(&root, temp.path(), false)
                .unwrap_err()
                .to_string()
                .contains("outside its verified store")
        );
        assert!(
            confined_at(&root, &root.join("../escape"), true)
                .unwrap_err()
                .to_string()
                .contains("invalid runtime component")
        );
        assert!(!temp.path().join("escape").exists());
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        symlink(&outside, root.join("redirect")).unwrap();
        std::fs::write(root.join("file"), "keep").unwrap();
        for name in ["redirect", "file"] {
            assert!(
                confined_at(&root, &root.join(name).join("child"), true)
                    .unwrap_err()
                    .to_string()
                    .contains("redirected or not a directory")
            );
        }
        assert!(!outside.join("child").exists());
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            confined_at(&root, &nested, false)
                .unwrap_err()
                .to_string()
                .contains("owner-only directory")
        );
    }

    #[test]
    fn missing_external_paths_resolve_ancestors_but_refuse_traversal_and_dangling_links() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let alias = temp.path().join("alias");
        symlink(&target, &alias).unwrap();
        assert_eq!(
            resolve_missing_path(&alias.join("new/state")).unwrap(),
            target.canonicalize().unwrap().join("new/state")
        );
        assert!(!target.join("new").exists());
        for path in [PathBuf::from("relative/state"), target.join("../state")] {
            assert!(
                resolve_missing_path(&path)
                    .unwrap_err()
                    .to_string()
                    .contains("absolute without '..'")
            );
        }
        let dangling = temp.path().join("dangling");
        symlink(temp.path().join("absent"), &dangling).unwrap();
        assert!(
            resolve_missing_path(&dangling.join("state"))
                .unwrap_err()
                .to_string()
                .contains("dangling symlink")
        );
    }

    #[test]
    fn cleanup_pins_directory_and_preserves_redirected_targets() {
        let (_root, repo) = repository();
        let store = store(&repo).unwrap();
        let attempt = store.join("abc/attempt-1");
        confined_in(&repo, &attempt, true).unwrap();
        let pinned = ConfinedDir::open(&attempt).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("stdout"), "outside").unwrap();
        std::fs::write(attempt.join("stdout"), "capture").unwrap();
        let moved = store.join("abc/original-attempt");
        std::fs::rename(&attempt, &moved).unwrap();
        symlink(outside.path(), &attempt).unwrap();
        assert!(pinned.remove_artifact("stdout").unwrap());
        assert!(!moved.join("stdout").exists());
        assert_eq!(
            std::fs::read_to_string(outside.path().join("stdout")).unwrap(),
            "outside"
        );
        assert!(!pinned.remove_artifact("stdout").unwrap());
        assert!(
            !pinned
                .unlink(&ConfinedDir::entry_name("stdout").unwrap())
                .unwrap()
        );
        symlink(outside.path().join("stdout"), moved.join("link")).unwrap();
        std::fs::create_dir(moved.join("directory")).unwrap();
        for name in ["link", "directory"] {
            assert!(
                pinned
                    .remove_artifact(name)
                    .unwrap_err()
                    .to_string()
                    .contains("expected a regular file")
            );
        }
        assert!(!pinned.remove_mailbox_entry("directory".as_ref()).unwrap());
        assert!(pinned.remove_mailbox_entry("link".as_ref()).unwrap());
        assert!(!pinned.remove_mailbox_entry("link".as_ref()).unwrap());
        assert!(outside.path().join("stdout").exists());
        assert!(moved.join("directory").is_dir());
        assert!(
            pinned
                .unlink(&ConfinedDir::entry_name("directory").unwrap())
                .unwrap_err()
                .to_string()
                .contains("cannot remove runtime entry")
        );
        assert!(
            pinned
                .remove_artifact("bad\0name")
                .unwrap_err()
                .to_string()
                .contains("interior NUL")
        );
        assert!(ConfinedDir::open(&attempt).is_err());
        assert!(
            ConfinedDir::open(&store.join("abc/missing"))
                .err()
                .unwrap()
                .to_string()
                .contains("refusing runtime directory")
        );
    }

    #[test]
    fn discovery_selects_task_names_with_records_and_refuses_foreign_stores() {
        let (_root, repo) = repository();
        let domain = store(&repo).unwrap();
        assert!(discover_domain(&repo, &domain).unwrap().is_empty());
        for name in [
            "abc",
            "01a0e53c-de9e-75a4-821f-9d197a88d800",
            "not-a-task",
            "def",
        ] {
            let dir = domain.join(name);
            confined_in(&repo, &dir, true).unwrap();
            if name != "def" {
                state::write_private_file(&dir.join("task.json"), b"{}").unwrap();
            }
        }
        let mut found = discover_domain(&repo, &domain).unwrap();
        found.sort();
        assert_eq!(
            found,
            [
                domain.join("01a0e53c-de9e-75a4-821f-9d197a88d800"),
                domain.join("abc")
            ]
        );
        let (_other_root, other_repo) = repository();
        assert!(
            confined_in(&other_repo, &domain, false)
                .unwrap_err()
                .to_string()
                .contains("another repository")
        );
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), domain.join("bad")).unwrap();
        assert!(
            discover_domain(&repo, &domain)
                .unwrap_err()
                .to_string()
                .contains("symlink")
        );
    }

    #[test]
    fn ownership_contention_refuses_second_supervisor_and_releases_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("owner.lock");
        assert!(supervisor_owns_attempt(root.path()).is_err());
        assert!(!path.exists());
        let owner = Lock::acquire(&path).unwrap();
        assert!(supervisor_owns_attempt(root.path()).unwrap());
        assert!(Lock::try_acquire(&path).unwrap().is_none());
        assert!(
            Lock::acquire(&path)
                .err()
                .unwrap()
                .to_string()
                .contains("concurrent execution refused")
        );
        drop(owner);
        assert!(!supervisor_owns_attempt(root.path()).unwrap());
        assert!(Lock::try_acquire(&path).unwrap().is_some());
    }
}
