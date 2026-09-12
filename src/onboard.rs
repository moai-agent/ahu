//! Read-only onboarding preview and additive registration.
//!
//! Adoption is additive. A teammate who never installs ahu keeps using the
//! repository's existing harness setup unchanged. Onboarding writes nothing
//! except `.agents/ahu/agents/<name>.toml` files that the user explicitly
//! selected, creates them exclusively, and never touches a native definition,
//! the Git index, or the branch.

use std::path::{Path, PathBuf};

use crate::agent::{self, AgentManifest, SourceFormat};
use crate::bail;
use crate::catalog;
use crate::config::AGENTS_RELATIVE_DIR;
use crate::util::{Error, Result, display_safe, is_safe_name, resolve_within};

/// A native agent definition ahu found. Finding it is not registering it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    /// Repository-root-relative path to the native definition.
    pub path: String,
    pub format: SourceFormat,
    /// Model the native file declares, if any.
    pub native_model: Option<String>,
    /// Whether an ahu manifest already registers this name.
    pub already_registered: bool,
    /// Reasons this candidate cannot be registered as-is.
    pub blockers: Vec<String>,
    /// Native fields ahu saw and will leave exactly where they are.
    pub preserved_fields: Vec<String>,
    /// Description declared by the native definition, carried into the manifest.
    pub description: String,
}

impl Candidate {
    pub fn registrable(&self) -> bool {
        self.blockers.is_empty() && !self.already_registered
    }
}

/// Inspect the repository without running anything in it.
///
/// This reads files only. It does not execute repository scripts, hooks, or a
/// harness session: detecting a definition is not authorization to run code.
pub fn preview(repo_root: &Path) -> Result<Vec<Candidate>> {
    let registered: Vec<String> = agent::load_all(repo_root)
        .map(|agents| agents.into_iter().map(|a| a.manifest.name).collect())
        .unwrap_or_default();
    let mut candidates = Vec::new();

    // Claude Code: `.claude/agents/<name>.md`.
    let claude_agents = repo_root.join(".claude/agents");
    if let Ok(entries) = std::fs::read_dir(&claude_agents) {
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
            .collect();
        paths.sort();
        for path in paths {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let relative = format!(".claude/agents/{name}.md");
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let (native_model, preserved_fields, description) = frontmatter_summary(&text);
            let mut blockers = Vec::new();
            if !is_safe_name(&name) {
                blockers.push(format!(
                    "{name:?} is not usable as an ahu selector, branch segment, and session title"
                ));
            }
            if let Some(model) = native_model.as_deref()
                && model != "inherit"
                && catalog::model("claude-code", model).is_none()
            {
                blockers.push(format!(
                    "the definition declares model {model:?}, which is not in compatibility catalog {}",
                    catalog::CATALOG_VERSION
                ));
            }
            candidates.push(Candidate {
                already_registered: registered.contains(&name),
                name,
                path: relative,
                format: SourceFormat::ClaudeAgent,
                native_model,
                blockers,
                preserved_fields,
                description,
            });
        }
    }

    // Definitions for harnesses ahu recognises but cannot launch yet. They are
    // reported so onboarding tells the truth about what is in the repository.
    for (dir, pattern, format) in [
        (".codex/agents", "toml", SourceFormat::CodexAgent),
        (".agents/agents", "md", SourceFormat::AntigravityAgent),
    ] {
        let root = repo_root.join(dir);
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let (name, relative) = if path.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                let inner = path.join("agent.md");
                if !inner.is_file() {
                    continue;
                }
                (name.clone(), format!("{dir}/{name}/agent.md"))
            } else {
                if path.extension().and_then(|s| s.to_str()) != Some(pattern) {
                    continue;
                }
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                (name.clone(), format!("{dir}/{name}.{pattern}"))
            };
            let harness = format.native_harness().unwrap_or("unknown");
            candidates.push(Candidate {
                already_registered: registered.contains(&name),
                name,
                path: relative,
                format,
                native_model: None,
                blockers: vec![format!(
                    "ahu 0.1.1 has no validated adapter for harness {harness:?}, so this definition cannot be launched through ahu"
                )],
                preserved_fields: Vec::new(),
                description: String::new(),
            });
        }
    }

    Ok(candidates)
}

/// The exact manifest ahu proposes for a candidate.
pub fn proposed_manifest(candidate: &Candidate, model: &str, version: &str) -> String {
    format!(
        "schema_version = 1\n\
         name = {:?}\n\
         version = {version:?}\n\
         description = {:?}\n\
         harness = {:?}\n\
         model = {model:?}\n\
         \n\
         [source]\n\
         format = {:?}\n\
         path = {:?}\n",
        candidate.name,
        candidate.description,
        candidate.format.native_harness().unwrap_or("claude-code"),
        candidate.format.as_str(),
        candidate.path,
    )
}

/// Write one manifest. Exclusive creation only; nothing else is touched.
pub fn register(
    repo_root: &Path,
    candidate: &Candidate,
    model: &str,
    version: &str,
) -> Result<PathBuf> {
    if !candidate.blockers.is_empty() {
        bail!(
            "cannot register {:?}: {}",
            candidate.name,
            candidate.blockers.join("; ")
        );
    }
    // Resolved component by component: a repository can commit a symlink at
    // `.agents`, `.agents/ahu`, or `.agents/ahu/agents`, and following one would
    // let it choose where ahu creates files.
    let path = resolve_within(
        repo_root,
        &format!("{AGENTS_RELATIVE_DIR}/{}.toml", candidate.name),
        true,
    )?;
    let body = proposed_manifest(candidate, model, version);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!(
                "{} already exists; nothing was overwritten.",
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

/// Remove one ahu registration.
///
/// This deletes the manifest and nothing else: the native definition, the
/// repository's skills, existing sessions, and task worktrees are untouched.
pub fn unregister(repo_root: &Path, name: &str) -> Result<PathBuf> {
    // `register` constrains names through the candidate checks; this path takes
    // a name straight from the command line, so it is validated here too rather
    // than being joined into a path unchecked.
    if !is_safe_name(name) {
        bail!(
            "{name:?} is not a valid ahu agent name, so it cannot name a registration to remove."
        );
    }
    // Same reasoning as `register`, and more important here: this deletes.
    // A symlinked `agents` directory would otherwise let `--remove config`
    // unlink an arbitrary `config.toml` outside the repository.
    let path = resolve_within(
        repo_root,
        &format!("{AGENTS_RELATIVE_DIR}/{name}.toml"),
        false,
    )?;
    if !path.is_file() {
        bail!("{} is not a registered ahu agent.", name);
    }
    // Only ahu's own manifests are removable, so a same-named unrelated file
    // cannot be deleted through this command.
    let body = std::fs::read_to_string(&path)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    if toml::from_str::<AgentManifest>(&body).is_err() {
        bail!(
            "{} is not an ahu agent manifest, so ahu will not delete it.",
            path.display()
        );
    }
    std::fs::remove_file(&path)
        .map_err(|e| Error::new(format!("cannot remove {}: {e}", path.display())))?;
    Ok(path)
}

fn frontmatter_summary(text: &str) -> (Option<String>, Vec<String>, String) {
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (None, Vec::new(), String::new());
    };
    let mut model = None;
    let mut description = String::new();
    let mut fields = Vec::new();
    for line in rest.lines() {
        if line.trim_end() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        let cleaned = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        match key.as_str() {
            "model" => model = Some(cleaned),
            "description" => description = cleaned,
            _ => {}
        }
        fields.push(key);
    }
    (model, fields, description)
}

/// Render a preview for a terminal.
pub fn render(candidates: &[Candidate], repo_root: &Path) -> String {
    let mut out = String::new();
    out.push_str(&format!("Onboarding preview for {}\n", repo_root.display()));
    out.push_str("This reads files only. Nothing has been written, staged, or committed.\n\n");
    if candidates.is_empty() {
        out.push_str(
            "No native agent definitions were found.\n\
             A repository holding only skills or an AGENTS.md has nothing ahu can convert into a\n\
             launchable agent: a skill is not an agent, and AGENTS.md is not an agent registry.\n\
             Create an ahu agent explicitly instead, with its own instructions under\n\
             .agents/ahu/instructions/<name>.md.\n",
        );
        return out;
    }
    for candidate in candidates {
        let status = if candidate.already_registered {
            "already registered"
        } else if candidate.registrable() {
            "can be registered"
        } else {
            "cannot be registered"
        };
        // `native_model`, `preserved_fields`, and `path` are all taken verbatim
        // from repository files, so they are sanitized before display.
        out.push_str(&format!(
            "  {} [{}] {}\n",
            display_safe(&candidate.name),
            candidate.format.as_str(),
            status
        ));
        out.push_str(&format!("      at {}\n", display_safe(&candidate.path)));
        if let Some(model) = &candidate.native_model {
            out.push_str(&format!("      declares model {}\n", display_safe(model)));
        }
        if !candidate.preserved_fields.is_empty() {
            out.push_str(&format!(
                "      native fields left in place: {}\n",
                display_safe(&candidate.preserved_fields.join(", "))
            ));
        }
        for blocker in &candidate.blockers {
            out.push_str(&format!("      blocked: {}\n", display_safe(blocker)));
        }
    }
    out.push_str(
        "\nRegistering adds only .agents/ahu/agents/<name>.toml. Native definitions are\n\
         referenced in place and never edited, and removing a registration deletes only that\n\
         manifest.\n",
    );
    out
}
