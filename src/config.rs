//! Project configuration: `.agents/ahu/config.md`.
//!
//! One policy per project. The file holds the project-agreed harness order, the
//! project-agreed model order within each harness, the required compatibility
//! catalog revision. There are deliberately no personal
//! profiles, environment overrides, or command-line switches that change any of
//! these for one user.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::catalog;
use crate::util::{Error, Result, digest_bytes};

pub const CONFIG_DIR: &str = ".agents/ahu";
pub const CONFIG_RELATIVE_PATH: &str = ".agents/ahu/config.md";
pub const LEGACY_CONFIG_PATH: &str = ".agents/ahu/config.toml";
pub const AGENTS_RELATIVE_DIR: &str = ".agents/ahu/agents";

pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

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

/// Opt-in, project-scoped OpenTelemetry delivery for ahu and its children.
///
/// The endpoint is deliberately local-only in this first implementation. A
/// launch applies these settings to the child process environment; ahu never
/// changes the invoking shell or a harness started outside ahu.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetryConfig {
    pub enabled: bool,
    /// Numeric headless attempt metrics only; independent of OTLP delivery.
    pub local_metrics: bool,
    pub endpoint: String,
}

/// Native harness state that the project explicitly accepts for unattended
/// execution. These stores can select provider authentication and remote
/// configuration which ahu cannot inspect without reading credentials.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct HeadlessConfig {
    /// Trust OpenCode's native account/auth stores for headless runs. This can
    /// activate remote configuration and executable plugins; ahu records the
    /// boundary but does not inspect or constrain that configuration.
    pub trust_opencode_native_state: bool,
}
impl HeadlessConfig {
    fn is_default(&self) -> bool {
        !self.trust_opencode_native_state
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            local_metrics: false,
            endpoint: "http://127.0.0.1:4318".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub schema_version: u32,
    /// Project-agreed harness order for launches without a named agent.
    pub harness_preferences: Vec<String>,
    /// The supported selection policy is `project-ranked`.
    pub model_selection: String,
    /// Required compatibility catalog revision.
    pub catalog_version: String,
    /// Optional exact harness CLI versions. Missing entries float with the
    /// installed CLI; these pins are project policy and are included in the
    /// committed context lock.
    #[serde(default)]
    pub harness_version_pins: std::collections::BTreeMap<String, String>,
    /// Project-agreed model order per harness, best first.
    #[serde(default)]
    pub model_rankings: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub knowledge: Knowledge,
    #[serde(default)]
    #[serde(skip_serializing_if = "HeadlessConfig::is_default")]
    pub headless: HeadlessConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
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

/// Configuration source for one checkout. Snapshot policy stays separate from
/// task persistence: these files remain repository inputs, not operational state.
#[derive(Debug, Clone)]
pub struct ProjectFiles {
    root: PathBuf,
}
impl ProjectFiles {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_RELATIVE_PATH)
    }
    fn existing_config(&self) -> Result<Option<PathBuf>> {
        existing_path(&self.root)
    }
    fn new_config(&self) -> Result<PathBuf> {
        crate::util::resolve_within(&self.root, CONFIG_RELATIVE_PATH, true)
    }
}

pub fn config_path(repo_root: &Path) -> PathBuf {
    crate::storage::CheckoutStorage::new(repo_root)
        .configuration()
        .config_path()
}

/// Load and validate the project configuration.
///
/// Returns `Ok(None)` only when the file does not exist, which is the signal to
/// enter first-run initialization. Malformed configuration is an error: ahu
/// never resets or overwrites a file it could not understand.
pub fn load(repo_root: &Path) -> Result<Option<LoadedConfig>> {
    // Resolved component by component rather than joined: a repository can
    // commit a symlink at `.agents`, `.agents/ahu`, or the policy file itself, and
    // reading through one would let it hand ahu any file the user can read.
    let Some(path) = crate::storage::CheckoutStorage::new(repo_root)
        .configuration()
        .existing_config()?
    else {
        return Ok(None);
    };
    let bytes = crate::okf::read_with_limit(&path, crate::util::MAX_TASK_RECORD_BYTES)
        .map_err(|error| Error::new(format!("cannot read {}: {error}", path.display())))?;
    let digest = digest_bytes(&bytes);
    let config = parse(&bytes, path.ends_with(LEGACY_CONFIG_PATH), &path)?;
    validate(&config, &path)?;
    Ok(Some(LoadedConfig {
        config,
        path,
        digest,
    }))
}

/// Dual-format policy is always an error, even if the effective values agree.
pub fn existing_path(root: &Path) -> Result<Option<PathBuf>> {
    crate::migration::ensure_idle(root)?;
    let current = crate::util::resolve_existing_within(root, CONFIG_RELATIVE_PATH)?;
    let legacy = crate::util::resolve_existing_within(root, LEGACY_CONFIG_PATH)?;
    if current.is_some() && legacy.is_some() {
        bail!(
            "conflicting project formats; inspect `ahu migrate --preview` and resolve the conflict explicitly"
        );
    }
    Ok(current.or(legacy))
}

pub(crate) fn parse(bytes: &[u8], legacy: bool, path: &Path) -> Result<ProjectConfig> {
    if bytes.len() as u64 > crate::util::MAX_TASK_RECORD_BYTES {
        bail!("configuration exceeds byte limit");
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Error::new("configuration is not UTF-8"))?;
    let config = if legacy {
        // TOML's diagnostic contains field names and source positions, not the
        // document text. Preserve the useful legacy unknown-field detail.
        toml::from_str(text).map_err(|error| {
            Error::new(format!(
                "not valid ahu configuration: {error}; ahu will not rewrite or reset it"
            ))
        })?
    } else {
        crate::okf::Document::parse(text, "ahu:project")
            .and_then(|document| document.settings())
            .map_err(|error| {
                Error::new(format!(
                    "not valid ahu configuration: {error}; ahu will not rewrite or reset it"
                ))
            })?
    };
    validate(&config, path)?;
    Ok(config)
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
    if config.headless.trust_opencode_native_state
        && !catalog::supports("opencode", catalog::Feature::HeadlessLaunch)
    {
        bail!(
            "{}: headless.trust_opencode_native_state requires an ahu build with the OpenCode headless adapter.",
            path.display()
        );
    }
    for (harness_id, version) in &config.harness_version_pins {
        if catalog::harness(harness_id).is_none() {
            bail!(
                "{}: harness_version_pins names unknown harness {harness_id:?}.",
                path.display()
            );
        }
        if !crate::util::is_semver(version) {
            bail!(
                "{}: harness_version_pins.{harness_id} must be an exact semantic version (MAJOR.MINOR.PATCH); found {version:?}.",
                path.display()
            );
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
    catalog::require_version(&config.catalog_version)?;
    crate::telemetry::validate_config(&config.telemetry, path)?;
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

/// Render new shared policy exclusively as OKF. Existing documents can retain
/// descriptive metadata with Document::set_settings and Document::render.
pub fn render(config: &ProjectConfig) -> String {
    try_render(config).expect("bounded project configuration is serializable")
}

pub fn try_render(config: &ProjectConfig) -> Result<String> {
    let text = crate::okf::Document::new("ahu:project", config,
        "\n# Project policy\n\nOne policy for every ahu user in this project. Review and commit this file.\n")
        .and_then(|document| document.render())?;
    // Rendering must not publish a policy that the bounded loader cannot read.
    if text.len() as u64 > crate::util::MAX_TASK_RECORD_BYTES {
        bail!("configuration exceeds byte limit");
    }
    Ok(text)
}

/// Write a new configuration, creating only the missing ahu directories.
///
/// The file is created exclusively: a concurrent `ahu setup` that got there first
/// keeps its result and this call reports the collision instead of overwriting.
pub fn write_new(repo_root: &Path, config: &ProjectConfig) -> Result<PathBuf> {
    validate(config, &config_path(repo_root))?;
    if existing_path(repo_root)?.is_some() {
        bail!(
            "project configuration already exists; nothing was overwritten; use explicit migration for legacy policy"
        );
    }
    // Component-by-component, so a symlinked `.agents` or `.agents/ahu` cannot
    // redirect where the project's configuration is created.
    let path = crate::storage::CheckoutStorage::new(repo_root)
        .configuration()
        .new_config()?;
    let body = try_render(config)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!(
                "{} already exists. Another ahu setup finished first; \
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
