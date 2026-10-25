//! Inert discovery and explicit preparation; activation stays in plugin lifecycle commands.
use super::{apply_plan_with_kiro_updates, safe_new_path, validate_plan};
use crate::git::Repo;
use crate::interactive::plugins::{PluginStatus, local};
use crate::launcher::Console;
use crate::plugin_binding::{Binding, Compatibility, Source, SourceKind};
use crate::util::{Error, Result};
use std::path::Path;

const LIMIT: &str = "Only Codex task launch is routed through the optional codex-plugin-pilot feature. This is an experimental planner, not full native extraction; built-in harnesses have not migrated.";

fn declaration(package: &PluginStatus) -> Result<Option<crate::harness_plugin::Declaration>> {
    package
        .metadata
        .get("harness_launch")
        .map(|value| {
            let declaration: crate::harness_plugin::Declaration =
                serde_json::from_value(value.clone())?;
            declaration.validate()?;
            Ok(declaration)
        })
        .transpose()
}

pub fn list(console: &mut Console<'_>, repo: &Repo) -> Result<i32> {
    report(console, repo)?;
    Ok(0)
}

pub(super) fn report(console: &mut Console<'_>, repo: &Repo) -> Result<()> {
    let mut packages = local::discover()?;
    for (_, installation) in crate::plugin_install::verified_packages(repo)? {
        let mut package = local::read_package(&installation.join("package"))?;
        package.origin = "verified-installed-package";
        packages.push(package);
    }
    let mut candidates = Vec::new();
    for package in packages {
        if let Some(declaration) = declaration(&package)? {
            candidates.push((declaration.harness, package));
        }
    }
    candidates.sort_by(|a, b| (&a.0, &a.1.manifest).cmp(&(&b.0, &b.1.manifest)));
    if candidates.is_empty() {
        return console.say("No harness plugin declarations found in AHU_PLUGIN_PATH or verified installed packages. No package code executed.\n");
    }
    console.say("Harness plugin declarations (metadata only; no package code executed):\n")?;
    for (harness, package) in &candidates {
        let ambiguous = candidates.iter().filter(|(id, _)| id == harness).count() > 1;
        console.say(&format!(
            "  {} for {} at {} [{}]{}\n",
            package.id,
            harness,
            package
                .manifest
                .as_deref()
                .unwrap_or(Path::new("unknown"))
                .display(),
            package.origin,
            if ambiguous {
                " (multiple candidates; no automatic selection)"
            } else {
                ""
            }
        ))?;
    }
    console.say(&format!("{LIMIT}\nTo prepare one package explicitly: ahu setup --harness-plugin /absolute/staged/package\n"))
}

fn binding(root: &Path) -> Result<Binding> {
    let repository = root
        .to_str()
        .filter(|path| crate::plugin_binding::local_path(path))
        .ok_or_else(|| {
            Error::new("setup --harness-plugin requires an absolute plain staged package path")
        })?;
    let digest = crate::plugin_install::digest(root)?;
    let package = local::read_package(root)?;
    let declaration = declaration(&package)?.ok_or_else(|| Error::new(
        "package has no harness_launch declaration; source-only packages must be built and staged separately before preparation"))?;
    if package.id != format!("harness-{}", declaration.harness) {
        return Err(Error::new(
            "harness plugin id must be harness-<declared harness> for launch selection",
        ));
    }
    let cargo_path = package
        .cargo_manifest
        .as_deref()
        .ok_or_else(|| Error::new("package has no Cargo manifest"))?;
    let bytes = crate::okf::read(cargo_path)?;
    let cargo: toml::Value = toml::from_str(
        std::str::from_utf8(&bytes).map_err(|_| Error::new("Cargo manifest is not UTF-8"))?,
    )
    .map_err(|_| Error::new("invalid package Cargo manifest"))?;
    let version = cargo
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::new("package version must be an explicit semantic version"))?;
    let binding = Binding {
        schema_version: 1,
        id: package.id,
        version: version.into(),
        source: Source {
            repository: repository.into(),
            revision: digest.clone(),
            sha256: digest,
            subdirectory: None,
            kind: SourceKind::Local,
        },
        compatibility: Compatibility {
            ahu: env!("CARGO_PKG_VERSION").into(),
            host_api: package.host_api,
        },
        enabled: false,
        grants: Vec::new(),
    };
    crate::plugin_install::verify(root, &binding)?;
    Ok(binding)
}

/// Prepare one disabled binding for review without requiring a terminal or probing a provider.
pub fn prepare(console: &mut Console<'_>, repo: &Repo, root: &Path) -> Result<i32> {
    crate::migration::ensure_idle(&repo.root)?;
    if crate::util::resolve_existing_within(&repo.root, crate::context_lock::LEGACY_LOCK_PATH)?
        .is_some()
    {
        return Err(Error::new("migrate the legacy context lock before setup"));
    }
    let binding = binding(root)?;
    let relative = format!("{}/{}.md", crate::plugin_binding::DIRECTORY, binding.id);
    let document = crate::okf::Document::new(
        "ahu:plugin-binding",
        &binding,
        "\nPrepared local package; review source pins and authority before enabling.\n",
    )?
    .render()?;
    crate::plugin_binding::parse(&document, &binding.id)?;
    let writes = vec![(safe_new_path(&repo.root, &relative)?, document.into_bytes())];
    validate_plan(&writes, &[])?;
    apply_plan_with_kiro_updates(console, &writes, &[])?;
    let snapshot = crate::snapshot::collect(&repo.root)?;
    let lock = crate::context_lock::refresh_for_setup(repo, &snapshot)?;
    console.say(&format!(
        "Prepared disabled {} with no grants. No package code executed, built, downloaded, installed or enabled.\nReview and commit {} and {}. Then run `ahu plugins install {}` (or `ahu plugins update {}` for a retained different version). Install reads committed pins only; keep the staged package unchanged.\nFor activation, run `ahu plugins enable {}`, explicitly add the harness-launch grant to the working binding, then run `ahu lock --update`. Review and commit the enabled binding and lock before launch.\n{LIMIT}\n",
        binding.id, relative, lock.display(), binding.id, binding.id, binding.id))?;
    Ok(0)
}
