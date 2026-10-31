//! Installed harness runtime boundary. Discovery is inert; calls execute reviewed
//! same-user code, not sandboxed code. Results are data, never execution authority.
//! Consumers must validate operation-specific payloads before applying effects.
use crate::git::Repo;
use crate::interactive::plugins::{local, project};
use crate::plugin_binding::{Binding, Grant};
use crate::util::{Error, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use ahu_harness_api::{Declaration, Descriptor, Mode, Operation};
pub use transport::Session;
mod generic;
mod transport;

struct Prepared {
    repo: Repo,
    binding: Binding,
    declaration: Declaration,
    digest: String,
    snapshot: tempfile::TempDir,
    executable: PathBuf,
    timeout: Duration,
}

/// Frozen installed package, descriptor and executable snapshot. No ambient cache.
pub struct Runtime {
    prepared: Arc<Prepared>,
}

fn candidates(repo: &Repo) -> Result<Vec<(Binding, Declaration)>> {
    let inventory = project::inventory(&repo.root)?;
    verify_candidates(
        repo,
        inventory.bindings.into_iter().map(|entry| entry.binding),
    )
}

fn verify_candidates(
    repo: &Repo,
    bindings: impl IntoIterator<Item = Binding>,
) -> Result<Vec<(Binding, Declaration)>> {
    let mut candidates = Vec::new();
    for binding in bindings {
        if !binding.enabled || !binding.grants.contains(&Grant::HarnessRuntime) {
            continue;
        }
        if binding.compatibility.ahu != env!("CARGO_PKG_VERSION") {
            return Err(Error::new(
                "installed harness runtime requires the exact core version",
            ));
        }
        let store = crate::state::checkout_root(&repo.primary_root()?)?.join("plugins");
        let installation = crate::plugin_install::installed(&store, &binding)?;
        crate::plugin_install::verify_installed(&installation, &binding)?;
        let package_root = installation.join("package");
        let package = local::read_package(&package_root)?;
        let declaration: Declaration = serde_json::from_value(
            package
                .metadata
                .get("harness_runtime")
                .ok_or_else(|| Error::new("runtime grant requires a harness_runtime declaration"))?
                .clone(),
        )
        .map_err(|_| Error::new("invalid harness runtime declaration"))?;
        declaration
            .validate()
            .map_err(|_| Error::new("invalid harness runtime declaration"))?;
        // Discovery checks executable metadata without running or snapshotting it.
        if !declaration.is_generic() {
            local::package_executable(&package_root, &declaration.path)?;
        }
        candidates.push((binding, declaration));
    }
    crate::catalog::validate_runtime_descriptors(candidates.iter().map(|(_, d)| &d.descriptor))?;
    candidates.sort_by(|(_, a), (_, b)| a.descriptor.id.cmp(&b.descriptor.id));
    Ok(candidates)
}

/// Enabled, granted committed bindings backed by verified installations only.
/// Disabled inventory remains available through project plugin inventory.
pub fn discover(repo: &Repo) -> Result<Vec<Descriptor>> {
    candidates(repo).map(|entries| entries.into_iter().map(|(_, d)| d.descriptor).collect())
        .map_err(|_| Error::new("cannot discover verified harness runtimes; check committed bindings and installations"))
}

/// Inert inventory for context locking before a binding activation is committed.
/// This does not authorize execution; `load` always reads committed bindings.
pub fn discover_context(repo: &Repo) -> Result<Vec<Descriptor>> {
    let Some(directory) =
        crate::util::resolve_existing_within(&repo.root, crate::plugin_binding::DIRECTORY)?
    else {
        return Ok(Vec::new());
    };
    let mut bindings = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if bindings.len() >= 32 || !entry.file_type()?.is_file() {
            return Err(Error::new("invalid working plugin binding inventory"));
        }
        let name = entry.file_name();
        let id = name
            .to_str()
            .and_then(|name| name.strip_suffix(".md"))
            .ok_or_else(|| Error::new("invalid working plugin binding filename"))?;
        let (binding, _) = crate::plugin_binding::load(&repo.root, id)?
            .ok_or_else(|| Error::new("working plugin binding disappeared"))?;
        bindings.push(binding);
    }
    verify_candidates(repo, bindings)
        .map(|entries| entries.into_iter().map(|(_, d)| d.descriptor).collect())
}

/// Resolve an exact ID or declared alias, verify pins and freeze executable bytes.
/// No native program is probed and no plugin is executed by this function.
pub fn load(repo: &Repo, harness: &str) -> Result<Runtime> {
    let (binding, declaration) = candidates(repo)?
        .into_iter()
        .find(|(_, d)| {
            d.descriptor.id == harness || d.descriptor.aliases.iter().any(|a| a == harness)
        })
        .ok_or_else(|| {
            Error::new(
                "no enabled installed harness runtime for this identity; no built-in fallback",
            )
        })?;
    let (snapshot, executable, timeout) = if declaration.is_generic() {
        (
            tempfile::tempdir()?,
            PathBuf::new(),
            Duration::from_millis(declaration.timeout_ms),
        )
    } else {
        crate::plugin_exec::prepare_executable(
            repo,
            &binding,
            &declaration.path,
            declaration.timeout_ms,
        )?
    };
    let digest = crate::util::digest_bytes(&serde_json::to_vec(&(&binding, &declaration))?);
    Ok(Runtime {
        prepared: Arc::new(Prepared {
            repo: repo.clone(),
            binding,
            declaration,
            digest,
            snapshot,
            executable,
            timeout,
        }),
    })
}

impl Runtime {
    /// True for an MCP-only harness that core drives without any plugin code.
    pub fn is_generic(&self) -> bool {
        self.prepared.declaration.is_generic()
    }
    pub fn descriptor(&self) -> &Descriptor {
        &self.prepared.declaration.descriptor
    }
    /// Persist this identity with task/attempt state and compare it before reuse.
    pub fn binding_digest(&self) -> String {
        self.prepared.digest.clone()
    }
    /// One request followed by bounded EOF/exit validation and owned-group cleanup.
    pub fn call(&self, operation: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        // Refuse invalid/unsupported operations before spawning any plugin code.
        self.prepared.operation(operation)?;
        if self.prepared.declaration.is_generic() {
            self.prepared.verify_current()?;
            return generic::call(
                &self.prepared.declaration.descriptor,
                self.prepared.timeout,
                operation,
                &params,
            );
        }
        let mut session = self.open_stream()?;
        let result = session.call(operation, params)?;
        session.finish()?;
        Ok(result)
    }
    /// Persistent protocol peer for one decoder/attempt. Never reconnects or retries.
    pub fn open_stream(&self) -> Result<Session> {
        if self.prepared.declaration.is_generic() {
            return Err(Error::new("a generic harness has no runtime stream"));
        }
        self.prepared.verify_current()?;
        Session::open(Arc::clone(&self.prepared))
    }
}
impl Prepared {
    fn operation(&self, value: &str) -> Result<Operation> {
        let operation =
            Operation::parse(value).map_err(|_| Error::new("unknown harness runtime operation"))?;
        if !self.declaration.descriptor.operations.contains(&operation) {
            return Err(Error::new("unsupported harness runtime operation"));
        }
        Ok(operation)
    }
    fn verify_current(&self) -> Result<()> {
        let current = candidates(&self.repo)?;
        if !current.iter().any(|(binding, declaration)| {
            binding == &self.binding && declaration == &self.declaration
        }) {
            return Err(Error::new(
                "harness runtime binding changed or was revoked; reload and compare frozen identity",
            ));
        }
        Ok(())
    }
}

/// Explicit inputs for native inspection. This is an allowlist, not an ambient
/// environment copy: native services receive a canonical project/home, selected
/// executable and declared routing keys. Values must not be persisted or logged.
pub fn native_params(repo: &Repo, descriptor: &Descriptor) -> Result<serde_json::Value> {
    let executable =
        crate::selection::resolve_executable(&descriptor.program).ok_or_else(|| {
            Error::new("declared native executable is unavailable outside repositories")
                .with_kind(crate::util::ErrorKind::Prerequisite)
        })?;
    let home = native_home()?;
    let path = std::env::var_os("PATH")
        .ok_or_else(|| Error::new("native inspection requires an explicit PATH"))?;
    let directories: Vec<_> = std::env::split_paths(&path)
        .filter(|p| p.is_absolute())
        .filter_map(|p| p.canonicalize().ok())
        .filter(|p| p.is_dir() && !crate::selection::is_excluded(p))
        .collect();
    let path = std::env::join_paths(directories)
        .map_err(|_| Error::new("invalid native PATH"))?
        .into_string()
        .map_err(|_| Error::new("native PATH must be UTF-8"))?;
    descriptor
        .validate()
        .map_err(|_| Error::new("invalid native descriptor"))?;
    let mut environment = std::collections::BTreeMap::from([("PATH".to_string(), path)]);
    for key in [
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TERM",
        "TMPDIR",
        "TMP",
        "TEMP",
        "SYSTEMROOT",
        "WINDIR",
    ]
    .into_iter()
    .chain(descriptor.environment_keys.iter().map(String::as_str))
    {
        if let Some(value) = std::env::var_os(key) {
            let value = value
                .into_string()
                .map_err(|_| Error::new("native environment value must be UTF-8"))?;
            if value.len() > 16 * 1024 || value.contains('\0') {
                return Err(Error::new("native environment value exceeds bounds"));
            }
            environment.insert(key.to_string(), value);
        }
    }
    if environment.values().map(String::len).sum::<usize>() > 128 * 1024 {
        return Err(Error::new("native environment exceeds request budget"));
    }
    Ok(serde_json::json!({
        "project": repo.root.canonicalize()?, "home": home,
        "native_executable": executable, "environment": environment,
    }))
}

pub(crate) fn native_home() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
        .ok_or_else(|| Error::new("native operation requires an absolute existing home"))?;
    home.canonicalize()
        .map_err(|_| Error::new("cannot resolve native home"))
}
