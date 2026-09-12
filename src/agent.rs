//! Named agent manifests: `.agents/ahu/agents/<name>.toml`.
//!
//! A manifest is the only thing that makes an agent launchable through ahu.
//! Native agent files found elsewhere are onboarding candidates, never implicit
//! registrations. The manifest references a native definition in place rather
//! than copying or rewriting it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::catalog;
use crate::config::{AGENTS_RELATIVE_DIR, SUPPORTED_SCHEMA_VERSION};
use crate::util::{Error, Result, digest_bytes, is_safe_name, is_semver};

/// Where an agent's instructions and harness-specific settings actually live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceFormat {
    /// `.claude/agents/<name>.md`, YAML frontmatter plus a Markdown body.
    #[serde(rename = "claude-agent")]
    ClaudeAgent,
    /// Plain Markdown instructions explicitly selected by the project.
    #[serde(rename = "markdown")]
    Markdown,
    /// `.codex/agents/<name>.toml`. Recognised, but not launchable in 0.1.1.
    #[serde(rename = "codex-agent")]
    CodexAgent,
    /// `.agents/agents/<name>/agent.md`. Recognised, not launchable in 0.1.1.
    #[serde(rename = "antigravity-agent")]
    AntigravityAgent,
}

impl SourceFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceFormat::ClaudeAgent => "claude-agent",
            SourceFormat::Markdown => "markdown",
            SourceFormat::CodexAgent => "codex-agent",
            SourceFormat::AntigravityAgent => "antigravity-agent",
        }
    }

    /// The harness that owns this native format, when one does.
    pub fn native_harness(self) -> Option<&'static str> {
        match self {
            SourceFormat::ClaudeAgent => Some("claude-code"),
            SourceFormat::CodexAgent => Some("codex"),
            SourceFormat::AntigravityAgent => Some("antigravity"),
            SourceFormat::Markdown => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSource {
    pub format: SourceFormat,
    /// Repository-root-relative path to the native definition.
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentManifest {
    pub schema_version: u32,
    pub name: String,
    /// Required. A named agent without a semantic version cannot be released,
    /// compared, or reported as drifted.
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub harness: String,
    /// Exact model identifier. No aliases, no `inherit`.
    pub model: String,
    pub source: AgentSource,
}

/// A manifest that has been validated against the catalog and the working tree.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    pub manifest: AgentManifest,
    pub manifest_path: PathBuf,
    pub manifest_digest: String,
    /// Absolute path to the native definition inside the repository.
    pub source_path: PathBuf,
    pub source_digest: String,
    /// Instructions the harness will load, for the context inventory.
    pub instructions: String,
    /// Model the native file declares, when it declares one.
    pub native_model: Option<String>,
    /// Native settings ahu recognised and will preserve by leaving them in place.
    pub native_settings: BTreeMap<String, String>,
}

impl ResolvedAgent {
    /// `chris@1.2.0`, the form used to refer to a launch.
    pub fn label(&self) -> String {
        format!("{}@{}", self.manifest.name, self.manifest.version)
    }

    /// Digest binding the manifest and the native definition together.
    pub fn identity_digest(&self) -> String {
        digest_bytes(
            format!(
                "{}\n{}\n{}\n{}\n{}",
                self.manifest.name,
                self.manifest.version,
                self.manifest.harness,
                self.manifest.model,
                self.source_digest
            )
            .as_bytes(),
        )
    }
}

fn agents_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(AGENTS_RELATIVE_DIR)
}

/// Load every registered agent, keyed by name.
///
/// A single malformed manifest fails the whole load: ahu must not present a
/// partial agent list as if it were the project's registry.
pub fn load_all(repo_root: &Path) -> Result<Vec<ResolvedAgent>> {
    let dir = agents_dir(repo_root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => bail!("cannot read {}: {e}", dir.display()),
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            paths.push(path);
        }
    }
    paths.sort();
    let mut agents = Vec::new();
    for path in paths {
        agents.push(load_one(repo_root, &path)?);
    }
    let mut seen = BTreeMap::new();
    for agent in &agents {
        if let Some(previous) =
            seen.insert(agent.manifest.name.clone(), agent.manifest_path.clone())
        {
            bail!(
                "two manifests define agent {:?}: {} and {}.\n\
                 Agent names must be unique; rename one of them.",
                agent.manifest.name,
                previous.display(),
                agent.manifest_path.display()
            );
        }
    }
    Ok(agents)
}

/// Look up one registered agent by name.
pub fn find(repo_root: &Path, name: &str) -> Result<ResolvedAgent> {
    let all = load_all(repo_root)?;
    if let Some(found) = all.iter().find(|a| a.manifest.name == name) {
        return Ok(found.clone());
    }
    let known = all
        .iter()
        .map(|a| format!("@{}", a.manifest.name))
        .collect::<Vec<_>>();
    if known.is_empty() {
        bail!(
            "no agent named {name:?} is registered, and this project has no ahu agents yet.\n\
             Register one under {}, or submit without an @agent to use automatic selection.",
            AGENTS_RELATIVE_DIR
        );
    }
    bail!(
        "no agent named {name:?} is registered. Available: {}.\n\
         ahu never substitutes another agent or an automatic selection for a named one.",
        known.join(", ")
    );
}

fn load_one(repo_root: &Path, path: &Path) -> Result<ResolvedAgent> {
    let bytes = std::fs::read(path)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    let manifest_digest = digest_bytes(&bytes);
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::new(format!("{} is not valid UTF-8", path.display())))?;
    let manifest: AgentManifest = toml::from_str(&text).map_err(|e| {
        Error::new(format!(
            "{} is not a valid ahu agent manifest: {e}",
            path.display()
        ))
    })?;

    if manifest.schema_version != SUPPORTED_SCHEMA_VERSION {
        bail!(
            "{}: schema_version {} is not supported by this ahu build (expected {SUPPORTED_SCHEMA_VERSION}).",
            path.display(),
            manifest.schema_version
        );
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if stem != manifest.name {
        bail!(
            "{}: manifest name {:?} does not match the file stem {stem:?}.",
            path.display(),
            manifest.name
        );
    }
    if !is_safe_name(&manifest.name) {
        bail!(
            "{}: agent name {:?} is not usable as a selector, branch segment, and session title.\n\
             Use lowercase letters, digits, `-`, and `_`.",
            path.display(),
            manifest.name
        );
    }
    if !is_semver(&manifest.version) {
        bail!(
            "{}: version {:?} is not a semantic version.\n\
             Every named agent needs one so behaviour changes can be bundled and compared.",
            path.display(),
            manifest.version
        );
    }

    let harness = catalog::harness(&manifest.harness).ok_or_else(|| {
        Error::new(format!(
            "{}: harness {:?} is not in compatibility catalog {}.",
            path.display(),
            manifest.harness,
            catalog::CATALOG_VERSION
        ))
    })?;
    if !harness.adapter_available {
        bail!(
            "{}: ahu 0.1.1 has no validated adapter for harness {:?}, so it cannot launch this agent.\n\
             Use the harness directly, or wait for an ahu release that supports it.",
            path.display(),
            manifest.harness
        );
    }
    if catalog::model(&manifest.harness, &manifest.model).is_none() {
        bail!(
            "{}: {:?} is not a catalog model for harness {:?}.\n\
             Catalog {} lists: {}.\n\
             ahu will not substitute a different model.",
            path.display(),
            manifest.model,
            manifest.harness,
            catalog::CATALOG_VERSION,
            catalog::models_for(&manifest.harness)
                .iter()
                .map(|m| m.model)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if let Some(native) = manifest.source.format.native_harness()
        && native != manifest.harness
    {
        bail!(
            "{}: source format {:?} belongs to harness {native:?}, but the manifest selects {:?}.\n\
             ahu does not translate an agent from one harness to another.",
            path.display(),
            manifest.source.format.as_str(),
            manifest.harness
        );
    }

    let source_path = resolve_source_path(repo_root, &manifest.source.path, path)?;
    let source_bytes = std::fs::read(&source_path).map_err(|e| {
        Error::new(format!(
            "{}: cannot read the agent's source {}: {e}",
            path.display(),
            source_path.display()
        ))
    })?;
    let source_digest = digest_bytes(&source_bytes);
    let source_text = String::from_utf8(source_bytes).map_err(|_| {
        Error::new(format!(
            "{}: agent source {} is not valid UTF-8",
            path.display(),
            source_path.display()
        ))
    })?;

    let (instructions, native_model, native_settings) = match manifest.source.format {
        SourceFormat::ClaudeAgent => parse_claude_agent(&source_text),
        SourceFormat::Markdown => (source_text.clone(), None, BTreeMap::new()),
        SourceFormat::CodexAgent | SourceFormat::AntigravityAgent => {
            (source_text.clone(), None, BTreeMap::new())
        }
    };

    if let Some(native) = native_model.as_deref()
        && native != manifest.model
        && !(native == "inherit" || native.is_empty())
    {
        bail!(
            "{}: the manifest selects model {:?} but {} declares {:?}.\n\
             ahu will not rewrite either file or pick one silently. Make them agree.",
            path.display(),
            manifest.model,
            source_path.display(),
            native
        );
    }

    Ok(ResolvedAgent {
        manifest,
        manifest_path: path.to_path_buf(),
        manifest_digest,
        source_path,
        source_digest,
        instructions,
        native_model,
        native_settings,
    })
}

/// Resolve a manifest `source.path` to a real file inside the repository.
///
/// Repository-root-relative only. Absolute paths, parent traversal, and links
/// leaving the repository are rejected: the source has to travel into a task
/// worktree, so it must actually be part of the repository.
pub fn resolve_source_path(
    repo_root: &Path,
    relative: &str,
    manifest_path: &Path,
) -> Result<PathBuf> {
    if relative.is_empty() {
        bail!("{}: source.path is empty.", manifest_path.display());
    }
    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        bail!(
            "{}: source.path {relative:?} is absolute. Use a repository-root-relative path so the \
             definition travels into task worktrees.",
            manifest_path.display()
        );
    }
    for component in candidate.components() {
        match component {
            std::path::Component::Normal(_) => {}
            std::path::Component::CurDir => {}
            _ => bail!(
                "{}: source.path {relative:?} escapes the repository root.",
                manifest_path.display()
            ),
        }
    }
    let joined = repo_root.join(candidate);
    let canonical_root = repo_root
        .canonicalize()
        .map_err(|e| Error::new(format!("cannot resolve {}: {e}", repo_root.display())))?;
    let canonical = joined.canonicalize().map_err(|e| {
        Error::new(format!(
            "{}: cannot resolve source.path {relative:?} ({e}).",
            manifest_path.display()
        ))
    })?;
    if !canonical.starts_with(&canonical_root) {
        bail!(
            "{}: source.path {relative:?} resolves outside the repository ({}).\n\
             ahu cannot carry an external file into a task worktree.",
            manifest_path.display(),
            canonical.display()
        );
    }
    if !canonical.is_file() {
        bail!(
            "{}: source.path {relative:?} is not a file.",
            manifest_path.display()
        );
    }
    // Return the path that was actually validated. Returning the pre-canonical
    // join would re-resolve symlinks at read time, leaving a window in which the
    // file read is not the file that was checked.
    Ok(canonical)
}

/// Split a Claude Code agent file into its YAML frontmatter and Markdown body.
///
/// ahu reads the frontmatter to detect a model conflict and to report the native
/// settings it is preserving. It never rewrites the file.
fn parse_claude_agent(text: &str) -> (String, Option<String>, BTreeMap<String, String>) {
    let mut settings = BTreeMap::new();
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (text.to_string(), None, settings);
    };
    let Some(end) = find_frontmatter_end(rest) else {
        return (text.to_string(), None, settings);
    };
    let (front, body) = rest.split_at(end.0);
    let body = &body[end.1..];
    let mut model = None;
    for line in front.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        if key == "model" {
            model = Some(value.clone());
        }
        settings.insert(key.to_string(), value);
    }
    (body.trim_start_matches('\n').to_string(), model, settings)
}

/// Offset of the closing `---` line and the length of that delimiter line.
fn find_frontmatter_end(rest: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((offset, line.len()));
        }
        offset += line.len();
    }
    None
}
