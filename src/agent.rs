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
    /// Instructions the harness receives, for the committed-context snapshot.
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

    /// The instruction source as a reviewer refers to it: repository-relative
    /// when the file is inside the checkout, absolute when it somehow is not.
    ///
    /// Task records store this form, and drift names the file that changed, so
    /// both go through one conversion rather than each spelling a path its own
    /// way.
    pub fn relative_source(&self, repo_root: &Path) -> String {
        self.source_path
            .strip_prefix(repo_root)
            .unwrap_or(&self.source_path)
            .to_string_lossy()
            .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest whose frontmatter is valid and whose body is its instructions.
    fn manifest_text(extra: &str, body: &str) -> String {
        format!(
            "---\nokf_version: {OKF_VERSION}\ntype: ahu:agent\ntitle: builder\nversion: \
             1.0.0\ndescription: builds\nharness: claude-code\nmodel: \
             claude-sonnet-5\n{extra}---\n\n{body}"
        )
    }

    fn parse(text: &str) -> Result<(AgentManifest, String)> {
        parse_manifest(text, Path::new("/repo/.agents/ahu/agents/builder.md"))
    }

    fn parse_err(text: &str) -> String {
        parse(text).unwrap_err().to_string()
    }

    #[test]
    fn source_formats_round_trip_their_manifest_spelling_and_own_harness() {
        let cases = [
            (
                SourceFormat::ClaudeAgent,
                "claude-agent",
                "claude-code",
                true,
            ),
            (SourceFormat::CodexAgent, "codex-agent", "codex", false),
            (
                SourceFormat::AntigravityAgent,
                "antigravity-agent",
                "antigravity",
                true,
            ),
            (
                SourceFormat::OpenCodeAgent,
                "opencode-agent",
                "opencode",
                true,
            ),
        ];
        for (format, spelling, harness, has_frontmatter) in cases {
            assert_eq!(format.as_str(), spelling);
            assert_eq!(SourceFormat::parse(spelling), Some(format));
            assert_eq!(format.native_harness(), Some(harness));
            assert_eq!(format.has_frontmatter(), has_frontmatter);
        }
        for unknown in ["", "claude", "cursor-agent", "CLAUDE-AGENT"] {
            assert_eq!(SourceFormat::parse(unknown), None);
        }
    }

    #[test]
    fn status_and_permissions_spell_themselves_for_the_registry_and_the_preview() {
        assert_eq!(Status::default(), Status::Stable);
        for (status, spelling) in [
            (Status::Draft, "draft"),
            (Status::Stable, "stable"),
            (Status::Deprecated, "deprecated"),
        ] {
            assert_eq!(status.as_str(), spelling);
        }

        assert_eq!(Permissions::default(), Permissions::Prompt);
        for (permissions, spelling, widens) in [
            (Permissions::Prompt, "prompt", false),
            (Permissions::AcceptEdits, "accept-edits", true),
            (Permissions::Auto, "auto", true),
        ] {
            assert_eq!(permissions.as_str(), spelling);
            assert_eq!(permissions.widens_defaults(), widens);
            assert!(!permissions.disclosure().is_empty());
        }
        // The prompt disclosure must not claim a boundary ahu did not set.
        assert!(
            Permissions::Prompt
                .disclosure()
                .contains("ahu passes no permission flag")
        );
        assert!(
            Permissions::AcceptEdits
                .disclosure()
                .contains("other tools still prompt")
        );
        assert!(Permissions::Auto.disclosure().contains("unattended"));
    }

    #[test]
    fn a_manifest_needs_frontmatter_that_opens_and_closes() {
        let missing = parse_err("title: builder\n\nInstructions.\n");
        assert!(missing.contains("not an ahu agent manifest"), "{missing}");

        let unclosed = parse_err("---\nokf_version: 0.2\ntype: ahu:agent\n\nInstructions.\n");
        assert!(unclosed.contains("never closed"), "{unclosed}");

        // CRLF frontmatter delimiters open and close the same document.
        let crlf = format!(
            "---\r\nokf_version: {OKF_VERSION}\r\ntype: ahu:agent\r\ntitle: builder\r\nversion: \
             1.0.0\r\nharness: claude-code\r\nmodel: claude-sonnet-5\r\n---\r\n\r\nInstructions.\r\n"
        );
        let (manifest, body) = parse(&crlf).unwrap();
        assert_eq!(manifest.name, "builder");
        assert!(body.contains("Instructions."));
    }

    #[test]
    fn frontmatter_lines_must_be_key_value_pairs_and_comments_are_skipped() {
        let broken = parse_err(&manifest_text("just-a-bare-line\n", "Instructions.\n"));
        assert!(broken.contains("is not a `key: value` pair"), "{broken}");

        // Comments, blank lines, and unknown keys leave a manifest valid: it is
        // an OKF document first, so ahu reads the fields it owns and no others.
        let (manifest, _) = parse(&manifest_text(
            "# a comment\n\nauthor: someone\nquoted: \"value\"\n",
            "Instructions.\n",
        ))
        .unwrap();
        assert_eq!(manifest.name, "builder");
        assert_eq!(manifest.description, "builds");
    }

    #[test]
    fn the_okf_envelope_and_the_required_identity_fields_are_all_enforced() {
        let wrong_version = parse_err(
            &manifest_text("", "Instructions.\n").replace("okf_version: 0.2", "okf_version: 0.1"),
        );
        assert!(
            wrong_version.contains("is not supported by this ahu build"),
            "{wrong_version}"
        );

        let wrong_kind = parse_err(
            &manifest_text("", "Instructions.\n").replace("type: ahu:agent", "type: ahu:skill"),
        );
        assert!(
            wrong_kind.contains("is not an ahu agent manifest"),
            "{wrong_kind}"
        );

        // Each of the four identity fields is individually required.
        for removed in [
            "title: builder\n",
            "version: 1.0.0\n",
            "harness: claude-code\n",
            "model: claude-sonnet-5\n",
        ] {
            let text = manifest_text("", "Instructions.\n").replace(removed, "");
            let error = parse_err(&text);
            assert!(
                error.contains("title, version, harness, and model are all required"),
                "{removed:?} -> {error}"
            );
        }
    }

    #[test]
    fn status_permissions_and_native_helpers_accept_only_their_declared_values() {
        for (declared, expected) in [
            ("", Status::Stable),
            ("status: stable\n", Status::Stable),
            ("status: draft\n", Status::Draft),
            ("status: deprecated\n", Status::Deprecated),
        ] {
            let (manifest, _) = parse(&manifest_text(declared, "Instructions.\n")).unwrap();
            assert_eq!(manifest.status, expected);
        }
        let bad_status = parse_err(&manifest_text("status: retired\n", "Instructions.\n"));
        assert!(
            bad_status.contains("is not one of: stable, draft, deprecated"),
            "{bad_status}"
        );

        for (declared, expected) in [
            ("", Permissions::Prompt),
            ("permissions: prompt\n", Permissions::Prompt),
            ("permissions: accept-edits\n", Permissions::AcceptEdits),
            ("permissions: auto\n", Permissions::Auto),
        ] {
            let (manifest, _) = parse(&manifest_text(declared, "Instructions.\n")).unwrap();
            assert_eq!(manifest.permissions, expected);
        }
        let bad_permissions = parse_err(&manifest_text("permissions: yolo\n", "Instructions.\n"));
        assert!(
            bad_permissions.contains("is not one of: prompt, accept-edits, auto"),
            "{bad_permissions}"
        );

        for (declared, expected) in [
            ("", None),
            ("native_helpers: disabled\n", Some("disabled")),
            ("native_helpers: bounded\n", Some("bounded")),
        ] {
            let (manifest, _) = parse(&manifest_text(declared, "Instructions.\n")).unwrap();
            assert_eq!(manifest.native_helpers.as_deref(), expected);
        }
        let bad_helpers = parse_err(&manifest_text("native_helpers: all\n", "Instructions.\n"));
        assert!(
            bad_helpers.contains("is not one of: disabled, bounded"),
            "{bad_helpers}"
        );
    }

    #[test]
    fn a_source_reference_needs_both_halves_and_a_known_format() {
        let (manifest, _) = parse(&manifest_text(
            "source_format: claude-agent\nsource_path: .claude/agents/builder.md\n",
            "See the native definition.\n",
        ))
        .unwrap();
        assert_eq!(
            manifest.source,
            Some(AgentSource {
                format: SourceFormat::ClaudeAgent,
                path: ".claude/agents/builder.md".to_string(),
            })
        );

        // No source pair at all means the body is the instructions.
        let (plain, _) = parse(&manifest_text("", "Instructions.\n")).unwrap();
        assert_eq!(plain.source, None);

        let unknown_format = parse_err(&manifest_text(
            "source_format: cursor-agent\nsource_path: .cursor/agents/builder.md\n",
            "See the native definition.\n",
        ));
        assert!(
            unknown_format.contains("source_format \"cursor-agent\" is not one of"),
            "{unknown_format}"
        );

        // Half a reference is never a reference: neither half implies the other.
        for half in [
            "source_format: claude-agent\n",
            "source_path: .claude/agents/builder.md\n",
        ] {
            let error = parse_err(&manifest_text(half, "See the native definition.\n"));
            assert!(
                error.contains("source_format and source_path must appear together"),
                "{half:?} -> {error}"
            );
        }
    }

    #[test]
    fn an_empty_body_is_refused_and_the_reason_depends_on_the_source_reference() {
        let own_instructions = parse_err(&manifest_text("", "   \n\n"));
        assert!(
            own_instructions.contains("The body is this agent's instructions"),
            "{own_instructions}"
        );

        let referenced = parse_err(&manifest_text(
            "source_format: claude-agent\nsource_path: .claude/agents/builder.md\n",
            "\n",
        ));
        assert!(
            referenced.contains("must still tell a human reader where the instructions live"),
            "{referenced}"
        );
    }

    /// A repository root carrying one registry manifest per `write_manifest`
    /// call. Nothing is committed: `load_all` reads the working tree.
    fn registry() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(AGENTS_RELATIVE_DIR)).unwrap();
        root
    }

    fn write(root: &Path, relative: &str, contents: &str) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// A manifest whose body is its own instructions, written into the registry.
    fn write_manifest(root: &Path, name: &str, extra: &str, body: &str) -> PathBuf {
        write(
            root,
            &format!("{AGENTS_RELATIVE_DIR}/{name}.md"),
            &format!(
                "---\nokf_version: {OKF_VERSION}\ntype: ahu:agent\ntitle: {name}\nversion: \
                 1.0.0\ndescription: builds\nharness: claude-code\nmodel: \
                 claude-sonnet-5\n{extra}---\n\n{body}"
            ),
        )
    }

    fn load_err(root: &Path) -> String {
        load_all(root).unwrap_err().to_string()
    }

    #[test]
    fn an_unregistered_repository_has_no_agents_and_says_so_when_one_is_named() {
        let root = tempfile::tempdir().unwrap();
        // No `.agents/ahu/agents` at all: an empty registry, not an error.
        assert!(load_all(root.path()).unwrap().is_empty());
        let error = find(root.path(), "builder").unwrap_err();
        assert_eq!(error.kind(), crate::util::ErrorKind::UnknownAgent);
        assert!(
            error.to_string().contains("has no ahu agents yet"),
            "{error}"
        );

        // An empty registry directory is equally not an error.
        let root = registry();
        assert!(load_all(root.path()).unwrap().is_empty());
    }

    #[test]
    fn a_named_agent_is_never_substituted_and_the_alternatives_are_listed() {
        let root = registry();
        write_manifest(root.path(), "builder", "", "Instructions.\n");
        write_manifest(root.path(), "reviewer", "", "Instructions.\n");

        let found = find(root.path(), "builder").unwrap();
        assert_eq!(found.manifest.name, "builder");
        assert_eq!(found.label(), "builder@1.0.0");

        let error = find(root.path(), "missing").unwrap_err();
        assert_eq!(error.kind(), crate::util::ErrorKind::UnknownAgent);
        let text = error.to_string();
        assert!(text.contains("@builder"), "{text}");
        assert!(text.contains("@reviewer"), "{text}");
        assert!(text.contains("never substitutes"), "{text}");
    }

    #[test]
    fn the_registry_reads_manifests_in_order_and_skips_what_is_not_one() {
        let root = registry();
        write_manifest(root.path(), "reviewer", "", "Instructions.\n");
        write_manifest(root.path(), "builder", "", "Instructions.\n");
        // Reserved knowledge-bundle documents and non-Markdown files are never
        // manifests, so malformed ones must not fail the load.
        write(
            root.path(),
            &format!("{AGENTS_RELATIVE_DIR}/index.md"),
            "not a manifest\n",
        );
        write(
            root.path(),
            &format!("{AGENTS_RELATIVE_DIR}/log.md"),
            "not a manifest\n",
        );
        write(
            root.path(),
            &format!("{AGENTS_RELATIVE_DIR}/notes.txt"),
            "not a manifest\n",
        );

        let agents = load_all(root.path()).unwrap();
        let names: Vec<&str> = agents.iter().map(|a| a.manifest.name.as_str()).collect();
        assert_eq!(names, ["builder", "reviewer"]);
    }

    #[test]
    fn one_malformed_manifest_fails_the_whole_registry_load() {
        let root = registry();
        write_manifest(root.path(), "builder", "", "Instructions.\n");
        write(
            root.path(),
            &format!("{AGENTS_RELATIVE_DIR}/broken.md"),
            "not a manifest at all\n",
        );
        // A partial list must never be presented as the project's registry.
        assert!(load_err(root.path()).contains("not an ahu agent manifest"));
        assert!(find(root.path(), "builder").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn the_registry_refuses_to_read_through_a_symlinked_manifest_or_directory() {
        use std::os::unix::fs::symlink;

        let root = registry();
        let outside = tempfile::tempdir().unwrap();
        let target = write(
            outside.path(),
            "elsewhere.md",
            "---\nokf_version: 0.2\n---\n",
        );
        symlink(
            &target,
            root.path()
                .join(format!("{AGENTS_RELATIVE_DIR}/builder.md")),
        )
        .unwrap();
        let error = load_err(root.path());
        assert!(
            error.contains("refusing to act through a symlink"),
            "{error}"
        );
        // The refusal names the path inside the repository, not the target.
        assert!(error.contains("builder.md"), "{error}");

        // A symlinked registry directory is refused before any manifest is read.
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".agents/ahu")).unwrap();
        symlink(outside.path(), root.path().join(AGENTS_RELATIVE_DIR)).unwrap();
        assert!(load_err(root.path()).contains("refusing to act through a symlink"));
    }

    #[test]
    fn agent_names_are_unique_because_each_manifest_must_match_its_file_name() {
        let root = registry();
        write_manifest(root.path(), "builder", "", "Instructions.\n");
        write_manifest(root.path(), "reviewer", "", "Instructions.\n");
        // The registry is one flat directory, so no two manifests share a file
        // stem, and a manifest whose title differs from its stem is refused.
        // Together those make duplicate agent names unreachable; the duplicate
        // check in `load_all` is a guard on that invariant, not a reachable
        // branch. The invariant itself is what a test can assert.
        let path = root
            .path()
            .join(format!("{AGENTS_RELATIVE_DIR}/reviewer.md"));
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("title: reviewer", "title: builder");
        std::fs::write(&path, text).unwrap();
        let error = load_err(root.path());
        assert!(error.contains("does not match the file stem"), "{error}");
        assert!(error.contains("reviewer"), "{error}");
    }

    #[test]
    fn a_manifest_must_match_its_file_name_and_carry_a_usable_name_and_version() {
        let root = registry();
        write_manifest(root.path(), "builder", "", "Instructions.\n");
        let path = root
            .path()
            .join(format!("{AGENTS_RELATIVE_DIR}/builder.md"));
        let original = std::fs::read_to_string(&path).unwrap();

        let stem_mismatch = original.replace("title: builder", "title: other");
        std::fs::write(&path, &stem_mismatch).unwrap();
        assert!(load_err(root.path()).contains("does not match the file stem"));

        // A name that cannot be a selector, branch segment, or session title.
        // The file is named to match so the stem check passes first.
        let unsafe_path = root
            .path()
            .join(format!("{AGENTS_RELATIVE_DIR}/Builder.md"));
        std::fs::remove_file(&path).unwrap();
        std::fs::write(
            &unsafe_path,
            original.replace("title: builder", "title: Builder"),
        )
        .unwrap();
        let error = load_err(root.path());
        assert!(error.contains("is not usable as a selector"), "{error}");
        std::fs::remove_file(&unsafe_path).unwrap();

        std::fs::write(&path, original.replace("version: 1.0.0", "version: 1.0")).unwrap();
        let error = load_err(root.path());
        assert!(error.contains("is not a semantic version"), "{error}");

        // A valid pre-release and build version is accepted.
        std::fs::write(
            &path,
            original.replace("version: 1.0.0", "version: 2.1.0-rc.1+build.5"),
        )
        .unwrap();
        assert_eq!(
            load_all(root.path()).unwrap()[0].manifest.version,
            "2.1.0-rc.1+build.5"
        );
    }

    #[test]
    fn the_harness_and_model_must_both_be_in_the_compatibility_catalog() {
        let root = registry();
        write_manifest(root.path(), "builder", "", "Instructions.\n");
        let path = root
            .path()
            .join(format!("{AGENTS_RELATIVE_DIR}/builder.md"));
        let original = std::fs::read_to_string(&path).unwrap();

        std::fs::write(
            &path,
            original.replace("harness: claude-code", "harness: cursor"),
        )
        .unwrap();
        let error = load_err(root.path());
        assert!(error.contains("is not in compatibility catalog"), "{error}");
        assert!(error.contains(catalog::CATALOG_VERSION), "{error}");

        // A catalog harness with a model that belongs to another harness.
        std::fs::write(
            &path,
            original.replace("model: claude-sonnet-5", "model: gpt-5.5"),
        )
        .unwrap();
        let error = load_err(root.path());
        assert!(
            error.contains("is not a catalog model for harness"),
            "{error}"
        );
        // The refusal lists what the catalog does offer instead of substituting.
        assert!(error.contains("claude-sonnet-5"), "{error}");
        assert!(
            error.contains("will not substitute a different model"),
            "{error}"
        );
    }

    #[test]
    fn a_manifest_carrying_its_own_instructions_digests_the_manifest_file() {
        let root = registry();
        let path = write_manifest(root.path(), "builder", "", "Do the work.\n");
        let agent = find(root.path(), "builder").unwrap();

        assert_eq!(agent.manifest_path, path);
        assert_eq!(agent.source_path, path);
        // The source is the manifest, so the file digests are the same file's.
        assert_eq!(agent.source_digest, agent.manifest_digest);
        // The delivered text is the body alone, so its digest differs from the
        // file's: frontmatter is metadata, never instruction text.
        assert_eq!(agent.instructions, "Do the work.\n");
        assert_eq!(agent.instructions_digest, digest_bytes(b"Do the work.\n"));
        assert_ne!(agent.instructions_digest, agent.source_digest);
        assert_eq!(agent.native_model, None);
        assert!(agent.native_settings.is_empty());
        assert_eq!(
            agent.relative_source(root.path()),
            format!("{AGENTS_RELATIVE_DIR}/builder.md")
        );
        // A root the file is not under leaves the absolute path in place.
        assert_eq!(
            agent.relative_source(Path::new("/nowhere")),
            path.to_string_lossy()
        );
    }

    #[test]
    fn a_referenced_claude_definition_supplies_the_instructions_and_its_settings() {
        let root = registry();
        write_manifest(
            root.path(),
            "builder",
            "source_format: claude-agent\nsource_path: .claude/agents/builder.md\n",
            "Instructions live in the native definition.\n",
        );
        let native = write(
            root.path(),
            ".claude/agents/builder.md",
            "---\nname: builder\nmodel: claude-sonnet-5\ntools: Read\n---\n\nNative instructions.\n",
        );

        let agent = find(root.path(), "builder").unwrap();
        assert_eq!(agent.source_path, native.canonicalize().unwrap());
        assert_eq!(agent.instructions, "Native instructions.\n");
        assert_eq!(agent.native_model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(
            agent.native_settings.get("tools").map(String::as_str),
            Some("Read")
        );
        // The manifest file and the instruction source are different files.
        assert_ne!(agent.source_digest, agent.manifest_digest);
        assert_ne!(agent.instructions_digest, agent.source_digest);
        // Identity covers both file digests, so changing either is drift.
        let before = agent.identity_digest();
        std::fs::write(
            &native,
            "---\nname: builder\nmodel: claude-sonnet-5\ntools: Read\n---\n\nChanged.\n",
        )
        .unwrap();
        assert_ne!(
            find(root.path(), "builder").unwrap().identity_digest(),
            before
        );
    }

    #[test]
    fn a_codex_source_is_delivered_verbatim_because_it_has_no_frontmatter() {
        let root = registry();
        let path = root
            .path()
            .join(format!("{AGENTS_RELATIVE_DIR}/builder.md"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                "---\nokf_version: {OKF_VERSION}\ntype: ahu:agent\ntitle: builder\nversion: \
                 1.0.0\nharness: codex\nmodel: gpt-5.5\nsource_format: \
                 codex-agent\nsource_path: .codex/agents/builder.toml\n---\n\nSee the TOML.\n"
            ),
        )
        .unwrap();
        let native_text = "model = \"gpt-5.5\"\ninstructions = \"Do the work.\"\n";
        write(root.path(), ".codex/agents/builder.toml", native_text);

        let agent = find(root.path(), "builder").unwrap();
        // TOML fields are not interpreted as instruction metadata, so the whole
        // file is the delivered text and the two digests cover identical bytes.
        assert_eq!(agent.instructions, native_text);
        assert_eq!(agent.instructions_digest, agent.source_digest);
        assert_eq!(agent.native_model, None);
        assert!(agent.native_settings.is_empty());
    }

    #[test]
    fn ahu_never_translates_an_agent_between_harnesses_or_guesses_a_model() {
        let root = registry();
        // A Claude-format source under a manifest that selects Codex.
        let path = root
            .path()
            .join(format!("{AGENTS_RELATIVE_DIR}/builder.md"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mismatched = format!(
            "---\nokf_version: {OKF_VERSION}\ntype: ahu:agent\ntitle: builder\nversion: \
             1.0.0\nharness: codex\nmodel: gpt-5.5\nsource_format: \
             claude-agent\nsource_path: .claude/agents/builder.md\n---\n\nSee it.\n"
        );
        std::fs::write(&path, &mismatched).unwrap();
        write(
            root.path(),
            ".claude/agents/builder.md",
            "---\nname: builder\n---\n\nNative.\n",
        );
        let error = load_err(root.path());
        assert!(
            error.contains("does not translate an agent from one harness to another"),
            "{error}"
        );

        // A native definition that declares a different model than the manifest.
        write_manifest(
            root.path(),
            "builder",
            "source_format: claude-agent\nsource_path: .claude/agents/builder.md\n",
            "See it.\n",
        );
        write(
            root.path(),
            ".claude/agents/builder.md",
            "---\nname: builder\nmodel: claude-opus-5\n---\n\nNative.\n",
        );
        let error = load_err(root.path());
        assert!(error.contains("but"), "{error}");
        assert!(error.contains("declares"), "{error}");
        assert!(error.contains("Make them agree"), "{error}");

        // `inherit` and an empty declaration defer to the manifest rather than
        // conflicting with it.
        for declared in ["model: inherit", "model:"] {
            write(
                root.path(),
                ".claude/agents/builder.md",
                &format!("---\nname: builder\n{declared}\n---\n\nNative.\n"),
            );
            let agent = find(root.path(), "builder").unwrap();
            assert_eq!(agent.manifest.model, "claude-sonnet-5");
        }
    }

    #[test]
    fn an_unreadable_or_non_utf8_source_is_reported_against_the_manifest() {
        let root = registry();
        write_manifest(
            root.path(),
            "builder",
            "source_format: claude-agent\nsource_path: .claude/agents/builder.md\n",
            "See it.\n",
        );
        // A source_path that resolves to nothing at all.
        let error = load_err(root.path());
        assert!(error.contains("cannot resolve source_path"), "{error}");

        // A source file whose bytes are not valid UTF-8 cannot be instructions.
        let native = root.path().join(".claude/agents/builder.md");
        std::fs::create_dir_all(native.parent().unwrap()).unwrap();
        std::fs::write(&native, [0x2d, 0x2d, 0x2d, 0x0a, 0xff, 0xfe]).unwrap();
        let error = load_err(root.path());
        assert!(error.contains("is not valid UTF-8"), "{error}");
        assert!(error.contains("agent source"), "{error}");
    }

    #[test]
    fn a_non_utf8_manifest_is_refused_before_it_is_parsed() {
        let root = registry();
        std::fs::write(
            root.path()
                .join(format!("{AGENTS_RELATIVE_DIR}/builder.md")),
            [0x2d, 0x2d, 0x2d, 0x0a, 0xff, 0xfe],
        )
        .unwrap();
        let error = load_err(root.path());
        assert!(error.contains("is not valid UTF-8"), "{error}");
    }

    #[test]
    fn a_source_path_must_be_a_plain_relative_file_inside_the_repository() {
        let root = tempfile::tempdir().unwrap();
        let manifest = root.path().join(".agents/ahu/agents/builder.md");
        let target = write(root.path(), ".claude/agents/builder.md", "Native.\n");

        let resolved =
            resolve_source_path(root.path(), ".claude/agents/builder.md", &manifest).unwrap();
        // The validated path is returned, so the read cannot re-resolve a link.
        assert_eq!(resolved, target.canonicalize().unwrap());
        assert!(resolved.is_absolute());
        // A `./` prefix is a plain path, not traversal.
        assert_eq!(
            resolve_source_path(root.path(), "./.claude/agents/builder.md", &manifest).unwrap(),
            resolved
        );

        let cases = [
            ("", "source_path is empty"),
            ("/etc/passwd", "is absolute"),
            ("../outside.md", "escapes the repository root"),
            (".claude/../../outside.md", "escapes the repository root"),
            (".claude/agents/missing.md", "cannot resolve source_path"),
            (".claude/agents", "is not a file"),
        ];
        for (relative, expected) in cases {
            let error = resolve_source_path(root.path(), relative, &manifest)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{relative:?} -> {error}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_source_path_cannot_reach_outside_the_repository_through_a_symlink() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let manifest = root.path().join(".agents/ahu/agents/builder.md");
        let outside = tempfile::tempdir().unwrap();
        let external = write(outside.path(), "external.md", "Native.\n");
        std::fs::create_dir_all(root.path().join(".claude/agents")).unwrap();
        symlink(&external, root.path().join(".claude/agents/builder.md")).unwrap();

        let error = resolve_source_path(root.path(), ".claude/agents/builder.md", &manifest)
            .unwrap_err()
            .to_string();
        assert!(error.contains("resolves outside the repository"), "{error}");
        assert!(
            error.contains("cannot carry an external file into a task worktree"),
            "{error}"
        );
    }

    #[test]
    fn find_frontmatter_end_reports_the_closing_delimiter_or_nothing() {
        assert_eq!(find_frontmatter_end("a: 1\n---\nbody"), Some((5, 4)));
        // A CRLF delimiter line is recognised, and its length includes the \r.
        assert_eq!(find_frontmatter_end("a: 1\r\n---\r\nbody"), Some((6, 5)));
        assert_eq!(find_frontmatter_end("a: 1\nno delimiter\n"), None);
    }

    #[test]
    fn parse_frontmatter_splits_metadata_from_delivered_text() {
        // No frontmatter: the whole file is the instructions and nothing is read
        // as metadata.
        let (body, model, settings) = parse_frontmatter("Just instructions.\n");
        assert_eq!(body, "Just instructions.\n");
        assert_eq!(model, None);
        assert!(settings.is_empty());

        // Opened but never closed is not frontmatter either, so the text is
        // delivered whole rather than half-stripped.
        let (body, model, settings) = parse_frontmatter("---\nmodel: x\nno close\n");
        assert_eq!(body, "---\nmodel: x\nno close\n");
        assert_eq!(model, None);
        assert!(settings.is_empty());

        let (body, model, settings) = parse_frontmatter(
            "---\n# comment\n\na bare line with no colon\nname: builder\nmodel: \
             'claude-sonnet-5'\ntools: Read, Write\n---\n\nInstructions.\n",
        );
        assert_eq!(body, "Instructions.\n");
        assert_eq!(model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(settings.get("name").map(String::as_str), Some("builder"));
        assert_eq!(
            settings.get("tools").map(String::as_str),
            Some("Read, Write")
        );
        // Comments and non-pair lines are not settings.
        assert!(!settings.contains_key("# comment"));
        assert_eq!(settings.len(), 3);
    }
}
