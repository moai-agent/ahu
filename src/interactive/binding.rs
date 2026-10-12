//! Core-owned durable host identity. Neither a receipt nor a manifest grants execution.
use super::{external::Executable, plugins::PluginStatus};
use crate::git::Repo;
use crate::plugin_binding::{Binding, Grant};
use crate::util::{Error, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenHost {
    pub binding: Binding,
    pub policy_revision: String,
    pub package: String,
    pub executable: Executable,
    pub executable_sha256: String,
    /// None resolves the content-addressed installation in this repository.
    /// A discovered package is reverified in place and never substituted.
    pub discovered_root: Option<PathBuf>,
}

impl FrozenHost {
    pub fn key(&self) -> Result<String> {
        // Policy revisions and discovery locations do not change package identity.
        Ok(crate::util::digest_bytes(&serde_json::to_vec(&(
            crate::plugin_install::key(&self.binding)?,
            &self.package,
            &self.executable,
            &self.executable_sha256,
        ))?))
    }
}

fn refused(message: &str) -> Error {
    Error::new(message).with_kind(ErrorKind::Prerequisite)
}

fn authorize(binding: &Binding) -> Result<()> {
    if !binding.enabled || !binding.grants.contains(&Grant::InteractiveBackend) {
        return Err(refused(
            "session host requires committed enablement and interactive-backend grant",
        ));
    }
    if binding.compatibility.ahu != env!("CARGO_PKG_VERSION") {
        return Err(refused(
            "session host binding requires a different ahu version",
        ));
    }
    Ok(())
}

fn package(repo: &Repo, binding: &Binding, discovered: Option<&PathBuf>) -> Result<PluginStatus> {
    let root = if let Some(root) = discovered {
        crate::plugin_install::verify(root, binding)?;
        root.clone()
    } else {
        let store = crate::state::checkout_root(&repo.primary_root()?)?.join("plugins");
        let path = crate::plugin_install::installed(&store, binding)?;
        crate::plugin_install::verify_installed(&path, binding)?;
        path.join("package")
    };
    super::plugins::local::read_package(&root)
}

fn executable_digest(status: &PluginStatus) -> Result<String> {
    let mut file = super::plugins::local::executable_file(status)?;
    Ok(crate::util::digest_bytes(&crate::util::read_limited(
        &mut file,
        64 * 1024 * 1024,
    )?))
}

pub(crate) fn select(repo: &Repo, id: &str) -> Result<(PluginStatus, FrozenHost)> {
    let inventory = super::plugins::project::inventory(&repo.root)?;
    let binding = inventory
        .bindings
        .into_iter()
        .find(|s| s.binding.id == id)
        .ok_or_else(|| refused("session host requires a committed project binding"))?
        .binding;
    authorize(&binding)?;
    let discovered_root = super::plugins::local::discover()?
        .into_iter()
        .find(|p| p.id == id)
        .and_then(|p| p.manifest)
        .and_then(|p| p.parent().map(ToOwned::to_owned));
    let status = package(repo, &binding, discovered_root.as_ref())?;
    let executable = status
        .executable
        .clone()
        .ok_or_else(|| refused("package has no executable host declaration"))?;
    let frozen = FrozenHost {
        binding,
        policy_revision: inventory
            .revision
            .ok_or_else(|| refused("session host requires a committed project binding"))?,
        package: status.package.clone(),
        executable,
        executable_sha256: executable_digest(&status)?,
        discovered_root,
    };
    Ok((status, frozen))
}

/// Verify current authority and the original committed identity without executing code.
pub(crate) fn verify(repo: &Repo, frozen: &FrozenHost) -> Result<PluginStatus> {
    let current = super::plugins::project::inventory(&repo.root)?
        .bindings
        .into_iter()
        .find(|s| s.binding.id == frozen.binding.id)
        .ok_or_else(|| refused("recorded session host binding is absent"))?
        .binding;
    authorize(&current)?;
    let original = super::plugins::project::inventory_at(&repo.root, &frozen.policy_revision)?
        .bindings
        .into_iter()
        .find(|s| s.binding.id == frozen.binding.id)
        .ok_or_else(|| refused("recorded session host has no committed provenance"))?
        .binding;
    authorize(&original)?;
    if original != frozen.binding {
        return Err(refused(
            "recorded session host differs from its committed binding",
        ));
    }
    let status = package(repo, &frozen.binding, frozen.discovered_root.as_ref())?;
    if status.package != frozen.package
        || status.executable.as_ref() != Some(&frozen.executable)
        || executable_digest(&status)? != frozen.executable_sha256
    {
        return Err(refused(
            "recorded session host executable identity mismatch",
        ));
    }
    Ok(status)
}
