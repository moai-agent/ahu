//! Offline inventory of committed bindings, separate from executable selection.
use crate::plugin_binding::{self, Binding};
use crate::util::{Error, Result};
use serde::Serialize;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

const MAX_BINDINGS: usize = 32;
const MAX_TREE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Serialize)]
pub struct Inventory {
    pub schema_version: u32,
    pub origin: &'static str,
    pub revision: Option<String>,
    pub bindings: Vec<BindingStatus>,
}

#[derive(Debug, Serialize)]
pub struct BindingStatus {
    pub path: String,
    pub binding: Binding,
    pub ahu_compatible: bool,
    pub source_verified: bool,
    pub activation_supported: bool,
    pub activation_blockers: Vec<&'static str>,
}

// Plumbing commands only: no checkout, filters, hooks, Cargo or remote access.
// Deny every Git transport, including file/ext helpers, even on older Git
// versions that do not recognize GIT_NO_LAZY_FETCH. Missing objects fail offline.
fn git_output(
    root: &Path,
    args: &[&str],
    limit: u64,
) -> Result<(std::process::ExitStatus, Vec<u8>, u64)> {
    let mut child = Command::new(crate::selection::resolve_utility("git")?)
        .args(["--no-pager", "--no-replace-objects"])
        .args(args)
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", "/dev/null")
        .env("XDG_CONFIG_HOME", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Drain diagnostics without retaining them. Some Git plumbing reports a
    // broken ref as a warning despite exiting successfully.
    let mut stderr = child.stderr.take().expect("piped stderr");
    let diagnostics = std::thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::sink()));
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .expect("piped stdout")
        .take(limit + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() as u64 > limit {
        let _ = child.kill();
        let _ = child.wait();
        let _ = diagnostics.join();
        return Err(Error::new(
            "project plugin Git output is unreadable or exceeds its bound",
        ));
    }
    let status = child.wait()?;
    let diagnostic_bytes = diagnostics
        .join()
        .map_err(|_| Error::new("cannot inspect project plugin Git diagnostics"))??;
    Ok((status, bytes, diagnostic_bytes))
}

fn git(root: &Path, args: &[&str], limit: u64) -> Result<Vec<u8>> {
    let (status, bytes, _) = git_output(root, args, limit)?;
    if !status.success() {
        return Err(Error::new(
            "cannot inspect committed project plugins offline; a local Git commit and its objects are required",
        ));
    }
    Ok(bytes)
}

struct Entry {
    mode: String,
    kind: String,
    oid: String,
    name: String,
}

fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn entries(bytes: &[u8]) -> Result<Vec<Entry>> {
    bytes
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let invalid = || Error::new("invalid committed plugin tree entry");
            let text = std::str::from_utf8(entry).map_err(|_| invalid())?;
            let (header, name) = text.split_once('\t').ok_or_else(invalid)?;
            let fields: Vec<_> = header.split(' ').collect();
            if fields.len() != 3 || !oid(fields[2]) {
                return Err(invalid());
            }
            Ok(Entry {
                mode: fields[0].into(),
                kind: fields[1].into(),
                oid: fields[2].into(),
                name: name.into(),
            })
        })
        .collect()
}

pub fn inventory(root: &Path) -> Result<Inventory> {
    let (status, bytes, _) = git_output(root, &["rev-parse", "--verify", "HEAD^{commit}"], 128)?;
    if !status.success() {
        // An unborn HEAD is a symbolic branch whose ref is absent. A failed
        // revision lookup alone also includes corruption and missing objects.
        let reference = String::from_utf8(git(root, &["symbolic-ref", "--quiet", "HEAD"], 1024)?)
            .map_err(|_| Error::new("invalid project HEAD reference"))?;
        let reference = reference.trim();
        if reference.starts_with("refs/heads/") {
            git(root, &["check-ref-format", reference], 128)?;
            // Older Git supports this bounded ref listing. Do not mistake its
            // successful-but-warning response for an absent branch.
            let (status, refs, diagnostics) = git_output(
                root,
                &[
                    "for-each-ref",
                    "--count=1",
                    "--format=%(refname)",
                    "--",
                    reference,
                ],
                1024,
            )?;
            if status.success() && refs.is_empty() && diagnostics == 0 {
                return Ok(Inventory {
                    schema_version: 1,
                    origin: "committed-project-bindings",
                    revision: None,
                    bindings: Vec::new(),
                });
            }
        }
        return Err(Error::new(
            "cannot inspect committed project plugins offline; HEAD is neither a local commit nor an unborn branch",
        ));
    }
    let revision = String::from_utf8(bytes)
        .map_err(|_| Error::new("invalid project commit"))?
        .trim()
        .to_string();
    if !oid(&revision) {
        return Err(Error::new("invalid project commit"));
    }
    inventory_at(root, &revision)
}

pub(crate) fn inventory_at(root: &Path, revision: &str) -> Result<Inventory> {
    if !oid(revision) {
        return Err(Error::new("invalid project commit"));
    }
    let revision = revision.to_owned();
    let mut report = Inventory {
        schema_version: 1,
        origin: "committed-project-bindings",
        revision: Some(revision.clone()),
        bindings: Vec::new(),
    };
    // Walk each committed ancestor: a symlink or submodule must not masquerade
    // as an absent directory. Working-tree and index contents are never read.
    let mut tree = revision;
    for component in plugin_binding::DIRECTORY.split('/') {
        let listing = entries(&git(
            root,
            &["ls-tree", "--full-tree", "-z", &tree, "--", component],
            MAX_TREE_BYTES,
        )?)?;
        let Some(entry) = listing.into_iter().next() else {
            return Ok(report);
        };
        if entry.name != component || entry.mode != "040000" || entry.kind != "tree" {
            return Err(Error::new(
                "committed plugin binding ancestors must be directories",
            ));
        }
        tree = entry.oid;
    }
    let listing = entries(&git(
        root,
        &["ls-tree", "--full-tree", "-z", &tree],
        MAX_TREE_BYTES,
    )?)?;
    if listing.len() > MAX_BINDINGS {
        return Err(Error::new("project plugin bindings exceed 32 entries"));
    }
    for entry in listing {
        if !matches!(entry.mode.as_str(), "100644" | "100755") || entry.kind != "blob" {
            return Err(Error::new(
                "committed plugin bindings must be regular files",
            ));
        }
        let id = entry
            .name
            .strip_suffix(".md")
            .ok_or_else(|| Error::new("committed plugin bindings require plain .md filenames"))?;
        let bytes = git(
            root,
            &["cat-file", "blob", &entry.oid],
            crate::okf::MAX_BYTES,
        )?;
        let text =
            std::str::from_utf8(&bytes).map_err(|_| Error::new("plugin binding is not UTF-8"))?;
        let (binding, _) = plugin_binding::parse(text, id)?;
        let ahu_compatible = binding.compatibility.ahu == env!("CARGO_PKG_VERSION");
        let mut blockers = Vec::new();
        if !binding.enabled {
            blockers.push("disabled");
        }
        if !ahu_compatible {
            blockers.push("ahu-version-mismatch");
        }
        if !binding
            .grants
            .contains(&plugin_binding::Grant::InteractiveBackend)
        {
            blockers.push("interactive-backend-not-granted");
        }
        blockers.push("source-unverified");
        report.bindings.push(BindingStatus {
            path: format!("{}/{name}", plugin_binding::DIRECTORY, name = entry.name),
            binding,
            ahu_compatible,
            source_verified: false,
            activation_supported: true,
            activation_blockers: blockers,
        });
    }
    report
        .bindings
        .sort_by(|a, b| a.binding.id.cmp(&b.binding.id));
    Ok(report)
}

pub fn list(root: &Path, output_json: bool) -> Result<()> {
    let report = inventory(root)?;
    if output_json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!(
            "Project plugin bindings at {} (committed files only)",
            report.revision.as_deref().unwrap_or("unborn HEAD")
        );
        for status in report.bindings {
            println!(
                "{}@{}: enabled={}, ahu_compatible={}; blockers: {}",
                status.binding.id,
                status.binding.version,
                status.binding.enabled,
                status.ahu_compatible,
                status.activation_blockers.join(", ")
            );
        }
        println!(
            "Source pins and grants are declarations, not verified installation or execution permission. No sources fetched or plugins activated; index and working-tree edits are excluded."
        );
    }
    Ok(())
}
