//! Project configuration: `.agents/ahu/config.toml`.
//!
//! One policy per project. The file holds the project-agreed harness order, the
//! project-agreed model order within each harness, the pinned compatibility
//! catalog, and the context-hygiene cadence. There are deliberately no personal
//! profiles, environment overrides, or command-line switches that change any of
//! these for one user.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::catalog;
use crate::util::{Error, Result, digest_bytes};

pub const CONFIG_DIR: &str = ".agents/ahu";
pub const CONFIG_RELATIVE_PATH: &str = ".agents/ahu/config.toml";
pub const AGENTS_RELATIVE_DIR: &str = ".agents/ahu/agents";
pub const INSTRUCTIONS_RELATIVE_DIR: &str = ".agents/ahu/instructions";

pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextHygiene {
    pub review_on_first_load: bool,
    pub review_interval_days: u32,
}

impl Default for ContextHygiene {
    fn default() -> Self {
        Self {
            review_on_first_load: true,
            review_interval_days: 7,
        }
    }
}

/// Project-agreed knowledge bundles and the policy `ahu knowledge lint` applies
/// to them.
///
/// Optional and additive: a configuration written before this section existed
/// loads with no bundles, which makes `ahu knowledge lint` report that nothing
/// is configured rather than silently pass.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Knowledge {
    /// Repository-relative directories holding OKF bundles, best-known first.
    #[serde(default)]
    pub bundles: Vec<String>,
    /// Whether lint warnings fail the check. Errors always fail it.
    #[serde(default)]
    pub fail_on_warnings: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConfig {
    pub schema_version: u32,
    /// Project-agreed harness order for launches without a named agent.
    pub harness_preferences: Vec<String>,
    /// The supported selection policy is `project-ranked`.
    pub model_selection: String,
    /// Pinned compatibility catalog revision.
    pub catalog_version: String,
    /// Project-agreed model order per harness, best first.
    #[serde(default)]
    pub model_rankings: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub context_hygiene: ContextHygiene,
    #[serde(default)]
    pub knowledge: Knowledge,
}

/// A loaded config plus the identity of the exact bytes it came from.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: ProjectConfig,
    pub path: PathBuf,
    /// SHA-256 of the file as read. Two checkouts showing the same digest hold
    /// the same policy; different digests are different policy snapshots even
    /// when neither is committed.
    pub digest: String,
}

impl LoadedConfig {
    /// Short form used in launch previews and task metadata.
    pub fn short_digest(&self) -> String {
        self.digest[..12].to_string()
    }
}

pub fn config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(CONFIG_RELATIVE_PATH)
}

/// Load and validate the project configuration.
///
/// Returns `Ok(None)` only when the file does not exist, which is the signal to
/// enter first-run initialization. Malformed configuration is an error: ahu
/// never resets or overwrites a file it could not understand.
pub fn load(repo_root: &Path) -> Result<Option<LoadedConfig>> {
    // Resolved component by component rather than joined: a repository can
    // commit a symlink at `.agents`, `.agents/ahu`, or `config.toml` itself, and
    // reading through one would let it hand ahu any file the user can read —
    // whose contents the parse error below then quotes back.
    let Some(path) = crate::util::resolve_existing_within(repo_root, CONFIG_RELATIVE_PATH)? else {
        return Ok(None);
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Error::new(format!("cannot read {}: {e}", path.display())));
        }
    };
    let digest = digest_bytes(&bytes);
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::new(format!("{} is not valid UTF-8", path.display())))?;
    let config: ProjectConfig = toml::from_str(&text).map_err(|e| {
        Error::new(format!(
            "{} is not valid ahu configuration: {e}\n\
             ahu will not rewrite or reset it. Fix the file, or move it aside and run `ahu init`.",
            path.display()
        ))
    })?;
    validate(&config, &path)?;
    Ok(Some(LoadedConfig {
        config,
        path,
        digest,
    }))
}

fn validate(config: &ProjectConfig, path: &Path) -> Result<()> {
    if config.schema_version != SUPPORTED_SCHEMA_VERSION {
        bail!(
            "{} declares schema_version {}, but this ahu build supports {SUPPORTED_SCHEMA_VERSION}.\n\
             Install an ahu release that understands this schema rather than editing the file to match.",
            path.display(),
            config.schema_version
        );
    }
    if config.model_selection != "project-ranked" {
        bail!(
            "{}: model_selection must be \"project-ranked\" in this release, found {:?}.",
            path.display(),
            config.model_selection
        );
    }
    if config.harness_preferences.is_empty() {
        bail!(
            "{}: harness_preferences is empty, so ahu has no project-agreed order to follow.\n\
             Add at least one harness.",
            path.display()
        );
    }
    let mut seen = std::collections::BTreeSet::new();
    for harness_id in &config.harness_preferences {
        if !seen.insert(harness_id.clone()) {
            bail!(
                "{}: harness {harness_id:?} is listed twice in harness_preferences.",
                path.display()
            );
        }
        if catalog::harness(harness_id).is_none() {
            bail!(
                "{}: harness {harness_id:?} is not in compatibility catalog {}.\n\
                 Known harnesses: {}.",
                path.display(),
                config.catalog_version,
                catalog::HARNESSES
                    .iter()
                    .map(|h| h.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    for (harness_id, models) in &config.model_rankings {
        if catalog::harness(harness_id).is_none() {
            bail!(
                "{}: model_rankings names unknown harness {harness_id:?}.",
                path.display()
            );
        }
        let mut seen_models = std::collections::BTreeSet::new();
        for model_id in models {
            if !seen_models.insert(model_id.clone()) {
                bail!(
                    "{}: model {model_id:?} is listed twice for harness {harness_id:?}.",
                    path.display()
                );
            }
            if catalog::model(harness_id, model_id).is_none() {
                bail!(
                    "{}: {model_id:?} is not a catalog model for harness {harness_id:?}.\n\
                     Catalog {} lists: {}.",
                    path.display(),
                    config.catalog_version,
                    catalog::models_for(harness_id)
                        .iter()
                        .map(|m| m.model)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }
    let mut seen_bundles = std::collections::BTreeSet::new();
    for bundle in &config.knowledge.bundles {
        validate_bundle_path(bundle, path)?;
        if !seen_bundles.insert(bundle.clone()) {
            bail!(
                "{}: knowledge.bundles lists {bundle:?} twice.",
                path.display()
            );
        }
    }
    if config.context_hygiene.review_interval_days == 0 {
        bail!(
            "{}: context_hygiene.review_interval_days must be at least 1.",
            path.display()
        );
    }
    catalog::require_version(&config.catalog_version)?;
    Ok(())
}

/// A knowledge bundle path must name a plain location inside the checkout.
///
/// The value reaches a third-party validator as a directory argument, so it is
/// checked here rather than normalized: an absolute path, a `..` component, or
/// an empty segment would point that validator — and ahu's own tree scan — at
/// something other than a directory of this repository. Display-hostile
/// characters are refused too, because the path is printed back in findings.
fn validate_bundle_path(bundle: &str, path: &Path) -> Result<()> {
    let reject = |reason: &str| -> Error {
        Error::new(format!(
            "{}: knowledge.bundles entry {bundle:?} {reason}.\n\
             Each bundle is a repository-relative directory such as \"docs/knowledge\".",
            path.display()
        ))
    };
    if bundle.is_empty() {
        return Err(reject("is empty"));
    }
    if bundle.starts_with('/') {
        return Err(reject("is absolute"));
    }
    if bundle.contains('\\') {
        return Err(reject("contains a backslash"));
    }
    if bundle.chars().any(crate::util::is_display_hostile_char) {
        return Err(reject("contains a control or direction-changing character"));
    }
    for segment in bundle.split('/') {
        if segment.is_empty() {
            return Err(reject("has an empty path segment"));
        }
        if segment == "." || segment == ".." {
            return Err(reject("has a \".\" or \"..\" segment"));
        }
    }
    Ok(())
}

/// Quote a string as TOML.
///
/// Rust's `{:?}` is close enough to TOML for the catalog-constrained values
/// elsewhere in this file, which are ASCII by construction. It is not close
/// enough for a bundle path, which the project writes and which may hold any
/// character the validation accepts: Rust escapes a non-ASCII character in the
/// `\u{...}` form, which TOML does not accept, so rendering a configuration
/// that loaded could produce one that no longer parses. The TOML serializer is
/// the only thing that knows TOML's own rules.
fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

/// Render a config to the exact TOML ahu writes during initialization.
pub fn render(config: &ProjectConfig) -> String {
    let mut out = String::new();
    out.push_str("# ahu project configuration.\n");
    out.push_str("# One policy for every ahu user in this project. Share it by committing it;\n");
    out.push_str("# ahu never stages, commits, or pushes this file for you.\n");
    out.push_str(&format!("schema_version = {}\n", config.schema_version));
    out.push_str(&format!(
        "harness_preferences = [{}]\n",
        config
            .harness_preferences
            .iter()
            .map(|h| format!("{h:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!("model_selection = {:?}\n", config.model_selection));
    out.push_str(&format!("catalog_version = {:?}\n", config.catalog_version));
    out.push_str("\n[model_rankings]\n");
    for (harness_id, models) in &config.model_rankings {
        out.push_str(&format!(
            "{:?} = [{}]\n",
            harness_id,
            models
                .iter()
                .map(|m| format!("{m:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out.push_str("\n[context_hygiene]\n");
    out.push_str(&format!(
        "review_on_first_load = {}\n",
        config.context_hygiene.review_on_first_load
    ));
    out.push_str(&format!(
        "review_interval_days = {}\n",
        config.context_hygiene.review_interval_days
    ));
    out.push_str("\n[knowledge]\n");
    out.push_str("# Repository-relative OKF bundle directories for `ahu knowledge lint`.\n");
    out.push_str(&format!(
        "bundles = [{}]\n",
        config
            .knowledge
            .bundles
            .iter()
            .map(|b| toml_string(b))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!(
        "fail_on_warnings = {}\n",
        config.knowledge.fail_on_warnings
    ));
    out
}

/// Write a new configuration, creating only the missing ahu directories.
///
/// The file is created exclusively: a concurrent `ahu init` that got there first
/// keeps its result and this call reports the collision instead of overwriting.
pub fn write_new(repo_root: &Path, config: &ProjectConfig) -> Result<PathBuf> {
    // Component-by-component, so a symlinked `.agents` or `.agents/ahu` cannot
    // redirect where the project's configuration is created.
    let path = crate::util::resolve_within(repo_root, CONFIG_RELATIVE_PATH, true)?;
    let body = render(config);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!(
                "{} already exists. Another ahu initialization finished first; \
                 nothing was overwritten.",
                path.display()
            );
        }
        Err(e) => bail!("cannot create {}: {e}", path.display()),
    };
    use std::io::Write;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    Ok(path)
}
