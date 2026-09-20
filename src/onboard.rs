//! Read-only onboarding preview and additive registration.
//!
//! Adoption is additive. A teammate who never installs ahu keeps using the
//! repository's existing harness setup unchanged. Onboarding writes nothing
//! except `.agents/ahu/agents/<name>.md` files that the user explicitly
//! selected, creates them exclusively, and never touches a native definition,
//! the Git index, or the branch.

use std::path::{Path, PathBuf};

use crate::agent::{self, SourceFormat};
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

    // Definitions ahu can read well enough to propose a manifest for: one
    // Markdown file per agent, named by the file, with YAML frontmatter whose
    // `model` and `description` are the only fields ahu interprets. A format
    // belongs here when its declared model is an identifier the catalog can be
    // asked about; everything else is listed below with a blocker instead.
    for (dir, format) in [
        (".claude/agents", SourceFormat::ClaudeAgent),
        (".opencode/agent", SourceFormat::OpenCodeAgent),
    ] {
        let Ok(entries) = std::fs::read_dir(repo_root.join(dir)) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
            .collect();
        paths.sort();
        // Every one of these formats names a harness; `unwrap_or` keeps the
        // lookup total without inventing a second source of truth for it.
        let harness = format.native_harness().unwrap_or("claude-code");
        for path in paths {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let relative = format!("{dir}/{name}.md");
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let (native_model, preserved_fields, description) = frontmatter_summary(&text);
            let mut blockers = Vec::new();
            if !is_safe_name(&name) {
                blockers.push(format!(
                    "{name:?} is not usable as an ahu selector, branch segment, and session title"
                ));
            }
            // OpenCode spells an inherited model as no `model` key at all,
            // where Claude Code writes `inherit`; both land as "ahu needs an
            // explicit --model", which is the same conversation either way.
            if let Some(model) = native_model.as_deref()
                && model != "inherit"
                && catalog::model(harness, model).is_none()
            {
                blockers.push(format!(
                    "the definition declares model {model:?}, which is not a {harness} model in compatibility catalog {}",
                    catalog::CATALOG_VERSION
                ));
            }
            candidates.push(Candidate {
                already_registered: registered.contains(&name),
                name,
                path: relative,
                format,
                native_model,
                blockers,
                preserved_fields,
                description,
            });
        }
    }

    // Additional native definitions are listed with registration blockers.
    // This discovery path does not parse their metadata or offer registration.
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
                    "native onboarding for harness {harness:?} does not support registering this definition; use an explicit manifest with a supported instruction source"
                )],
                preserved_fields: Vec::new(),
                description: String::new(),
            });
        }
    }

    Ok(candidates)
}

/// Quote `value` as a YAML double-quoted string.
///
/// YAML's double-quoted escape surface is the one ahu's manifest parser
/// accepts, and it matches the escapes used before migration. Use
/// YAML-compatible escapes so control and bidi characters cannot break
/// registration or change terminal presentation.
fn yaml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            // YAML forbids raw control characters in a double-quoted string,
            // and the characters that reorder text have no business in a
            // manifest field either. Both take YAML's own escape, not Rust's.
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c if crate::util::is_display_hostile_char(c) => {
                let code = c as u32;
                if code <= 0xffff {
                    out.push_str(&format!("\\u{code:04X}"));
                } else {
                    out.push_str(&format!("\\U{code:08X}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The exact manifest ahu proposes for a candidate.
///
/// The manifest is OKF Markdown that references the native candidate in place:
/// the body is a short pointer, never a copy, so the native definition stays
/// the single editable source.
pub fn proposed_manifest(candidate: &Candidate, model: &str, version: &str) -> String {
    let description = if candidate.description.is_empty() {
        "\"\"".to_string()
    } else {
        yaml_string(&candidate.description)
    };
    format!(
        "---\n\
         okf_version: 0.2\n\
         type: ahu:agent\n\
         title: {}\n\
         description: {}\n\
         status: stable\n\
         tags: [agents]\n\
         harness: {}\n\
         model: {}\n\
         permissions: prompt\n\
         version: {}\n\
         source_format: {}\n\
         source_path: {}\n\
         \n\
         ---\n\
         \n\
         Instructions live in the native definition at `{}`, referenced in place and never edited.\n",
        yaml_string(&candidate.name),
        description,
        yaml_string(candidate.format.native_harness().unwrap_or("claude-code")),
        yaml_string(model),
        yaml_string(version),
        yaml_string(candidate.format.as_str()),
        yaml_string(&candidate.path),
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
        &format!("{AGENTS_RELATIVE_DIR}/{}.md", candidate.name),
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
        return Err(Error::new(format!(
            "{name:?} is not a valid ahu agent name, so it cannot name a registration to remove."
        ))
        .with_kind(crate::util::ErrorKind::Usage));
    }
    // Same reasoning as `register`, and more important here: this deletes.
    // A symlinked `agents` directory would otherwise let `--remove config`
    // unlink an arbitrary `config.md` outside the repository.
    let path = resolve_within(
        repo_root,
        &format!("{AGENTS_RELATIVE_DIR}/{name}.md"),
        false,
    )?;
    if !path.is_file() {
        return Err(
            Error::new(format!("{} is not a registered ahu agent.", name))
                .with_kind(crate::util::ErrorKind::UnknownAgent),
        );
    }
    // Only ahu's own manifests are removable, so a same-named unrelated file
    // cannot be deleted through this command.
    let body = std::fs::read_to_string(&path)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    if agent::parse_manifest(&body, &path).is_err() {
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
             Create an ahu agent explicitly instead: a manifest under .agents/ahu/agents/<name>.md\n\
             that either carries its own instructions in its body or references a native definition\n\
             in place.\n",
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
        "\nRegistering adds only .agents/ahu/agents/<name>.md. Native definitions are\n\
         referenced in place and never edited, and removing a registration deletes only that\n\
         manifest.\n",
    );
    out
}
