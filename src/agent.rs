//! Named agent manifests: `.agents/ahu/agents/<name>.md`.
//!
//! A manifest is an OKF Markdown document whose frontmatter carries
//! `type: ahu:agent` and the fields that make an agent launchable through ahu.
//! The body is either the agent's instructions themselves, or a short pointer
//! to a native definition the manifest references in place — never a copy or
//! a rewrite. Native agent files found elsewhere are onboarding candidates,
//! never implicit registrations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::catalog;
use crate::config::AGENTS_RELATIVE_DIR;
use crate::util::{Error, Result, digest_bytes, is_safe_name, is_semver};

/// The OKF envelope version every manifest this build reads must declare.
pub const OKF_VERSION: &str = "0.2";

/// Where an agent's instructions and harness-specific settings actually live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    /// `.claude/agents/<name>.md`, YAML frontmatter plus a Markdown body.
    ClaudeAgent,
    /// `.codex/agents/<name>.toml`. The source file's text is delivered
    /// verbatim; native TOML fields are not interpreted as instruction
    /// metadata.
    CodexAgent,
    /// `.agents/agents/<name>/agent.md`, YAML frontmatter plus Markdown
    /// instructions.
    AntigravityAgent,
    /// `.opencode/agent/<name>.md`, YAML frontmatter plus Markdown
    /// instructions. Its `model` is provider-qualified, the shape OpenCode's
    /// own `--model` takes, so a declared model is a catalog identifier
    /// rather than a bare name.
    OpenCodeAgent,
}

impl SourceFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceFormat::ClaudeAgent => "claude-agent",
            SourceFormat::CodexAgent => "codex-agent",
            SourceFormat::AntigravityAgent => "antigravity-agent",
            SourceFormat::OpenCodeAgent => "opencode-agent",
        }
    }

    /// Parse a `source_format` value from manifest frontmatter.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude-agent" => Some(SourceFormat::ClaudeAgent),
            "codex-agent" => Some(SourceFormat::CodexAgent),
            "antigravity-agent" => Some(SourceFormat::AntigravityAgent),
            "opencode-agent" => Some(SourceFormat::OpenCodeAgent),
            _ => None,
        }
    }

    /// Whether a file of this format carries YAML frontmatter that is metadata
    /// rather than instruction text.
    ///
    /// Formats determine how source bytes become instruction text. Frontmatter
    /// is excluded from the delivered text, so `ResolvedAgent` records separate
    /// file and instruction digests. No format selects an agent by harness name.
    pub fn has_frontmatter(self) -> bool {
        matches!(
            self,
            SourceFormat::ClaudeAgent
                | SourceFormat::AntigravityAgent
                | SourceFormat::OpenCodeAgent
        )
    }

    /// The harness that owns this native format, when one does.
    pub fn native_harness(self) -> Option<&'static str> {
        match self {
            SourceFormat::ClaudeAgent => Some("claude-code"),
            SourceFormat::CodexAgent => Some("codex"),
            SourceFormat::AntigravityAgent => Some("antigravity"),
            SourceFormat::OpenCodeAgent => Some("opencode"),
        }
    }
}

/// A native definition a manifest references in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSource {
    pub format: SourceFormat,
    /// Repository-root-relative path to the native definition.
    pub path: String,
}

/// An agent manifest's publication state, declared by its author.
///
/// `status` is information for the humans reading the registry: it records
/// intent and never gates launching. A draft can be launched deliberately,
/// and a deprecated agent keeps running for the worktrees that still need it.
/// Absent means `stable`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    Draft,
    #[default]
    Stable,
    Deprecated,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Draft => "draft",
            Status::Stable => "stable",
            Status::Deprecated => "deprecated",
        }
    }
}

/// How much the agent may do without stopping to ask.
///
/// ahu never widens a harness's approval boundary on its own. This is opt-in,
/// per agent, declared in the manifest, and therefore reviewable in Git like any
/// other identity field — and the launch preview states it in full before
/// anything starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permissions {
    /// Pass nothing. The harness's own defaults and prompts apply.
    #[default]
    Prompt,
    /// Request the adapter's edit-approval mode; native settings still apply.
    AcceptEdits,
    /// Request the adapter's automatic approval mode; native settings still apply.
    Auto,
}

impl Permissions {
    pub fn as_str(self) -> &'static str {
        match self {
            Permissions::Prompt => "prompt",
            Permissions::AcceptEdits => "accept-edits",
            Permissions::Auto => "auto",
        }
    }

    /// Whether this widens the harness's own default.
    pub fn widens_defaults(self) -> bool {
        !matches!(self, Permissions::Prompt)
    }

    /// What the user is agreeing to, spelled out for the launch preview.
    pub fn disclosure(self) -> &'static str {
        match self {
            Permissions::Prompt => {
                // Native settings determine the effective boundary. Disclose
                // the flags ahu passes without inferring the session policy.
                "ahu passes no permission flag. The effective approval boundary is set by the \
                 harness's own settings, including any settings this repository carries into the \
                 task worktree — see the settings summary above"
            }
            Permissions::AcceptEdits => {
                "file edits are approved automatically; other tools still prompt"
            }
            Permissions::Auto => {
                "tool use is approved automatically and the agent runs unattended, \
                 including commands it chooses to run in the task worktree"
            }
        }
    }
}

/// The fields ahu validates and uses from an OKF agent manifest.
///
/// `okf_version` and `type` are the OKF envelope: checked while parsing and
/// not stored, because they describe the document, not the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentManifest {
    /// The agent's name and selector, taken from the document title.
    pub name: String,
    /// Required. A named agent without a semantic version cannot be released,
    /// compared, or reported as drifted.
    pub version: String,
    pub description: String,
    pub harness: String,
    /// Exact model identifier. No aliases, no `inherit`.
    pub model: String,
    /// Opt-in approval widening. Absent means the harness's own defaults.
    pub permissions: Permissions,
    /// The native definition this manifest references in place. `None` means
    /// the manifest body is the agent's instructions.
    pub source: Option<AgentSource>,
    /// Publication state declared by the author. Never gates launching.
    pub status: Status,
    /// Whether the harness's native helper tools are available to the agent,
    /// when the manifest opts in or out explicitly. `None` means the harness's
    /// own default applies; the two declared values are `disabled` and
    /// `bounded`.
    pub native_helpers: Option<String>,
}

/// Parse an ahu agent manifest: OKF frontmatter plus a Markdown body.
///
/// Returns the manifest and its body. The body is the agent's instructions
/// when no `source_format`/`source_path` pair selects a native definition, and
/// a pointer paragraph for human readers when one does. Unknown frontmatter
/// fields are ignored so a manifest stays an OKF document first and an ahu
/// manifest second.
pub fn parse_manifest(text: &str, path: &Path) -> Result<(AgentManifest, String)> {
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        bail!(
            "{}: not an ahu agent manifest: expected YAML frontmatter opening with `---` on the \
             first line.",
            path.display()
        );
    };
    let Some((front_end, delimiter_len)) = find_frontmatter_end(rest) else {
        bail!(
            "{}: frontmatter opened with `---` but never closed with a `---` line.",
            path.display()
        );
    };
    let (front, body) = rest.split_at(front_end);
    let body = body[delimiter_len..].trim_start_matches('\n').to_string();

    let mut okf_version = String::new();
    let mut kind = String::new();
    let mut name = String::new();
    let mut version = String::new();
    let mut description = String::new();
    let mut harness = String::new();
    let mut model = String::new();
    let mut permissions = String::new();
    let mut status = String::new();
    let mut source_format = String::new();
    let mut source_path = String::new();
    let mut native_helpers = String::new();

    for line in front.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            bail!(
                "{}: frontmatter line {:?} is not a `key: value` pair.",
                path.display(),
                trimmed
            );
        };
        let key = key.trim();
        let value = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        match key {
            "okf_version" => okf_version = value,
            "type" => kind = value,
            "title" => name = value,
            "version" => version = value,
            "description" => description = value,
            "harness" => harness = value,
            "model" => model = value,
            "permissions" => permissions = value,
            "status" => status = value,
            "source_format" => source_format = value,
            "source_path" => source_path = value,
            "native_helpers" => native_helpers = value,
            _ => {}
        }
    }

    if okf_version != OKF_VERSION {
        bail!(
            "{}: okf_version {okf_version:?} is not supported by this ahu build (expected \
             {OKF_VERSION}).",
            path.display()
        );
    }
    if kind != "ahu:agent" {
        bail!(
            "{}: type {kind:?} is not an ahu agent manifest (expected \"ahu:agent\").",
            path.display()
        );
    }
    if name.is_empty() || version.is_empty() || harness.is_empty() || model.is_empty() {
        bail!(
            "{}: title, version, harness, and model are all required in an ahu agent manifest.",
            path.display()
        );
    }
    let status = match status.as_str() {
        "" | "stable" => Status::Stable,
        "draft" => Status::Draft,
        "deprecated" => Status::Deprecated,
        other => bail!(
            "{}: status {other:?} is not one of: stable, draft, deprecated.",
            path.display()
        ),
    };
    let permissions = match permissions.as_str() {
        "" | "prompt" => Permissions::Prompt,
        "accept-edits" => Permissions::AcceptEdits,
        "auto" => Permissions::Auto,
        other => bail!(
            "{}: permissions {other:?} is not one of: prompt, accept-edits, auto.",
            path.display()
        ),
    };
    let native_helpers = match native_helpers.as_str() {
        "" => None,
        "disabled" | "bounded" => Some(native_helpers.clone()),
        other => bail!(
            "{}: native_helpers {other:?} is not one of: disabled, bounded.",
            path.display()
        ),
    };
    let source = match (source_format.is_empty(), source_path.is_empty()) {
        (false, false) => {
            let format = SourceFormat::parse(&source_format).ok_or_else(|| {
                Error::new(format!(
                    "{}: source_format {source_format:?} is not one of: {}.",
                    path.display(),
                    "claude-agent, codex-agent, antigravity-agent, opencode-agent"
                ))
            })?;
            Some(AgentSource {
                format,
                path: source_path,
            })
        }
        (true, true) => None,
        _ => bail!(
            "{}: source_format and source_path must appear together — the format ahu reads and \
             the native file to read it from.",
            path.display()
        ),
    };

    if body.trim().is_empty() {
        match source {
            None => bail!(
                "{}: the manifest body is empty. The body is this agent's instructions; write \
                 them below the frontmatter, or reference a native definition with source_format \
                 and source_path.",
                path.display()
            ),
            Some(_) => bail!(
                "{}: the manifest body is empty. When source_format and source_path select a \
                 native definition, the body must still tell a human reader where the \
                 instructions live.",
                path.display()
            ),
        }
    }

    Ok((
        AgentManifest {
            name,
            version,
            description,
            harness,
            model,
            permissions,
            source,
            status,
            native_helpers,
        },
        body,
    ))
}

/// A manifest that has been validated against the catalog and the working tree.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    pub manifest: AgentManifest,
    pub manifest_path: PathBuf,
    pub manifest_digest: String,
    /// Absolute path to the file the instructions are read from: the native
    /// definition for a manifest that references one, and the manifest itself
    /// when the body is the instructions.
    pub source_path: PathBuf,
    /// SHA-256 of the complete file at `source_path`, bytes exactly as they are
    /// on disk — frontmatter included.
    ///
    /// This is what a reviewer compares against the repository, and what
    /// identifies the file as a file. It is **not** what ahu puts in front of
    /// the model.
    pub source_digest: String,
    /// SHA-256 of exactly the text ahu delivers, byte for byte.
    ///
    /// That is `instructions` below: the file at `source_path` with its YAML
    /// frontmatter stripped when it has any. It is identical to the bytes that
    /// land inside the `<<<ahu-agent-...>>>` fence in the delivered prompt.
    ///
    /// For a source file with no frontmatter this covers the same bytes as
    /// `source_digest`, and it is computed the same way rather than being left
    /// absent — a digest that is sometimes missing is a digest a reader has to
    /// reason about, and the point of having two is that neither needs
    /// reasoning about.
    ///
    /// Separate digests let readers compare the source file and the delivered
    /// body without treating frontmatter as instruction text.
    pub instructions_digest: String,
    /// Instructions the harness receives, for the context inventory.
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
    ///
    /// Both file digests are mixed in. The file digest alone would miss nothing
    /// today — stripping is deterministic, so a changed body implies a changed
    /// file — but the delivered digest is the one that describes what reached
    /// the model, and drift should not depend on that implication holding if the
    /// parse ever changes. A change to either is drift.
    pub fn identity_digest(&self) -> String {
        digest_bytes(
            format!(
                "{}\n{}\n{}\n{}\n{}\n{}",
                self.manifest.name,
                self.manifest.version,
                self.manifest.harness,
                self.manifest.model,
                self.source_digest,
                self.instructions_digest
            )
            .as_bytes(),
        )
    }
}

/// Load every registered agent, keyed by name.
///
/// A single malformed manifest fails the whole load: ahu must not present a
/// partial agent list as if it were the project's registry.
pub fn load_all(repo_root: &Path) -> Result<Vec<ResolvedAgent>> {
    // The registry directory is resolved component by component, and each
    // manifest is checked before it is opened. `register` and `unregister`
    // already refuse to act through a symlinked `.agents/ahu/agents`; reading
    // needs the same refusal, because a symlinked manifest hands ahu a file
    // outside the repository whose contents the parse error quotes back.
    let Some(dir) = crate::util::resolve_existing_within(repo_root, AGENTS_RELATIVE_DIR)? else {
        return Ok(Vec::new());
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => bail!("cannot read {}: {e}", dir.display()),
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        // `index.md` and `log.md` are reserved document names in ahu
        // knowledge, and the registry directory is a knowledge bundle; neither
        // is ever an agent manifest.
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if file_name == "index.md" || file_name == "log.md" {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| Error::new(format!("cannot inspect {}: {e}", path.display())))?;
        if meta.file_type().is_symlink() {
            let shown = path
                .strip_prefix(repo_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            return Err(crate::util::symlink_refusal(&path, &shown));
        }
        paths.push(path);
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
        return Err(Error::new(format!(
            "no agent named {name:?} is registered, and this project has no ahu agents yet.\n\
             Register one under {}, or submit without an @agent to use automatic selection.",
            AGENTS_RELATIVE_DIR
        ))
        .with_kind(crate::util::ErrorKind::UnknownAgent));
    }
    Err(Error::new(format!(
        "no agent named {name:?} is registered. Available: {}.\n\
         ahu never substitutes another agent or an automatic selection for a named one.",
        known.join(", ")
    ))
    .with_kind(crate::util::ErrorKind::UnknownAgent))
}

fn load_one(repo_root: &Path, path: &Path) -> Result<ResolvedAgent> {
    let bytes = std::fs::read(path)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    let manifest_digest = digest_bytes(&bytes);
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::new(format!("{} is not valid UTF-8", path.display())))?;
    let (manifest, body) = parse_manifest(&text, path)?;

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if stem != manifest.name {
        bail!(
            "{}: manifest title {:?} does not match the file stem {stem:?}.",
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
            "{}: ahu has no validated adapter for harness {:?}, so it cannot launch this agent.\n\
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
    if let Some(source) = &manifest.source
        && let Some(native) = source.format.native_harness()
        && native != manifest.harness
    {
        bail!(
            "{}: source format {:?} belongs to harness {native:?}, but the manifest selects {:?}.\n\
             ahu does not translate an agent from one harness to another.",
            path.display(),
            source.format.as_str(),
            manifest.harness
        );
    }

    // The instruction text is what ahu delivers in the prompt, so where it is
    // read from determines what is delivered: the manifest body when the
    // manifest carries its own instructions, or the native definition it
    // references in place. Frontmatter is metadata ahu reads (for the
    // model-conflict check and the native-settings disclosure) and does not
    // put in front of the model; the body is the instructions.
    let (source_path, source_digest, instructions, native_model, native_settings) = match &manifest
        .source
    {
        Some(source) => {
            let source_path = resolve_source_path(repo_root, &source.path, path)?;
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
            let (instructions, native_model, native_settings) = if source.format.has_frontmatter() {
                parse_frontmatter(&source_text)
            } else {
                (source_text.clone(), None, BTreeMap::new())
            };
            (
                source_path,
                source_digest,
                instructions,
                native_model,
                native_settings,
            )
        }
        None => (
            path.to_path_buf(),
            manifest_digest.clone(),
            body,
            None,
            BTreeMap::new(),
        ),
    };
    // Computed the same way as `source_digest`, over the bytes ahu will actually
    // deliver. For a source file without frontmatter the two cover identical
    // bytes and come out equal, which is the correct answer rather than a
    // special case.
    let instructions_digest = digest_bytes(instructions.as_bytes());

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
        instructions_digest,
        instructions,
        native_model,
        native_settings,
    })
}

/// Resolve a manifest `source_path` to a real file inside the repository.
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
        bail!("{}: source_path is empty.", manifest_path.display());
    }
    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        bail!(
            "{}: source_path {relative:?} is absolute. Use a repository-root-relative path so the \
             definition travels into task worktrees.",
            manifest_path.display()
        );
    }
    for component in candidate.components() {
        match component {
            std::path::Component::Normal(_) => {}
            std::path::Component::CurDir => {}
            _ => bail!(
                "{}: source_path {relative:?} escapes the repository root.",
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
            "{}: cannot resolve source_path {relative:?} ({e}).",
            manifest_path.display()
        ))
    })?;
    if !canonical.starts_with(&canonical_root) {
        bail!(
            "{}: source_path {relative:?} resolves outside the repository ({}).\n\
             ahu cannot carry an external file into a task worktree.",
            manifest_path.display(),
            canonical.display()
        );
    }
    if !canonical.is_file() {
        bail!(
            "{}: source_path {relative:?} is not a file.",
            manifest_path.display()
        );
    }
    // Return the path that was actually validated. Returning the pre-canonical
    // join would re-resolve symlinks at read time, leaving a window in which the
    // file read is not the file that was checked.
    Ok(canonical)
}

/// Split an agent file into its YAML frontmatter and Markdown body.
///
/// ahu reads the frontmatter to detect a model conflict and to report the native
/// settings it is preserving. It never rewrites the file.
fn parse_frontmatter(text: &str) -> (String, Option<String>, BTreeMap<String, String>) {
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
