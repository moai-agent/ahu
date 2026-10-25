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
    let mut candidates = Vec::new();
    for entry in inventory.bindings {
        let binding = entry.binding;
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
        local::package_executable(&package_root, &declaration.path)?;
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
    let (snapshot, executable, timeout) = crate::plugin_exec::prepare_executable(
        repo,
        &binding,
        &declaration.path,
        declaration.timeout_ms,
    )?;
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
        let mut session = self.open_stream()?;
        let result = session.call(operation, params)?;
        session.finish()?;
        Ok(result)
    }
    /// Persistent protocol peer for one decoder/attempt. Never reconnects or retries.
    pub fn open_stream(&self) -> Result<Session> {
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
