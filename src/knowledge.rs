//! Knowledge-bundle linting through the `okf` toolkit.
//!
//! `ahu knowledge lint` checks the OKF bundles this project agreed on, named in
//! `[knowledge]` in the project configuration. It is a read-only check: ahu
//! never fetches anything, never generates an index, and never edits a file in
//! a bundle.
//!
//! Three constraints shape this module.
//!
//!   - The validator is `okf`, resolved from `PATH` through the same resolver
//!     every harness goes through, so a binary committed inside the checkout is
//!     never the one that runs. There is no configurable command and nothing is
//!     interpreted by a shell.
//!   - `okf lint` suppresses error findings and exits 0 whenever it produced a
//!     report at all, so it cannot be the only run: `okf validate` runs first,
//!     and the two sets of findings are combined and deduplicated.
//!   - `okf` walks a bundle itself, so a symlink anywhere in the tree would let
//!     the repository have an external directory read and its contents quoted
//!     back in findings. ahu scans the tree first and refuses the bundle rather
//!     than handing it over. That preflight closes the committed-symlink case,
//!     which is the one a repository controls; it cannot close a race against
//!     someone writing to the checkout while the check is running, because okf
//!     opens the files itself, afterwards.
//!
//! okf is otherwise trusted the way any tool ahu runs is trusted: it executes
//! with the invoking user's privileges and ahu does not sandbox it. What ahu
//! does not do is trust its *output*. A report that is missing a field, uses a
//! severity ahu does not know, disagrees with its own counts, or arrives with
//! an exit status that does not match what it says, is a failure of the check
//! rather than a bundle that passed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::config::LoadedConfig;
use crate::util::{Error, ErrorKind, Result, display_path, display_safe};

/// The validator ahu runs. Not configurable: the command is part of the check.
pub const OKF_EXECUTABLE: &str = "okf";

/// The subcommands run for every bundle, in order.
///
/// `validate` reports errors and warnings; `lint` reports warnings only. Both
/// run because neither is a superset of the other across okf versions, and the
/// combined findings are deduplicated.
const OKF_SUBCOMMANDS: [&str; 2] = ["validate", "lint"];

/// Version of the `--output json` report contract.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

/// How deep ahu will verify a bundle tree before refusing it.
///
/// The scan exists to prove no symlink is present anywhere okf will look, so a
/// tree it cannot finish walking is refused rather than trusted.
const MAX_BUNDLE_DEPTH: usize = 32;

/// Largest okf report ahu will parse.
///
/// This does not bound what the subprocess produced or what ahu has already
/// read: the output is collected in full before anything here runs. It bounds
/// the JSON parsing and the finding list built from it, and it refuses a report
/// that is implausible for a knowledge bundle rather than working through it.
const MAX_REPORT_BYTES: usize = 16 * 1024 * 1024;

/// A severity ahu knows how to act on.
///
/// A closed set on purpose. If a future okf emits a third severity, a finding
/// carrying it must not be deserialized into something this module quietly
/// counts as harmless; it fails the check instead, and adding the variant is a
/// deliberate decision about what that severity means for an exit status.
///
/// The ordering is the display and sort order: errors before warnings.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    #[serde(rename = "ERROR")]
    Error,
    #[serde(rename = "WARN")]
    Warn,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
        }
    }
}

/// One finding, as okf reports it.
///
/// Field names match okf's JSON so the report ahu emits stays recognisable to
/// anyone who has read okf's own output. Every field is required: a report that
/// omits one is not the report contract ahu verified, and guessing a default
/// for it is how a missing severity becomes a clean bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub severity: Severity,
    pub concept_id: String,
    pub rule: String,
    pub message: String,
}

impl Finding {
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    pub fn is_warning(&self) -> bool {
        self.severity == Severity::Warn
    }
}

/// What okf printed for one subcommand.
///
/// Deliberately not `deny_unknown_fields`: a later okf may add a field, and
/// that is not a reason to fail. Every field ahu relies on is required, and the
/// summary counts are kept so they can be checked against the findings.
#[derive(Debug, Deserialize)]
struct OkfReport {
    command: String,
    valid: bool,
    errors: u64,
    warnings: u64,
    /// An empty finding list may arrive as `[]` or as `null`; it may not arrive
    /// by being absent, which would make any object at all a clean bundle.
    #[serde(deserialize_with = "null_as_empty")]
    findings: Vec<Finding>,
}

fn null_as_empty<'de, D>(deserializer: D) -> std::result::Result<Vec<Finding>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Vec<Finding>>::deserialize(deserializer)?.unwrap_or_default())
}

/// The error envelope okf prints instead of a report.
#[derive(Debug, Deserialize)]
struct OkfError {
    error: OkfErrorBody,
}

#[derive(Debug, Deserialize)]
struct OkfErrorBody {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    message: String,
}

/// The findings for one configured bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleReport {
    /// The path exactly as configured, repository-relative.
    pub path: String,
    /// Deduplicated findings from every okf subcommand, errors first.
    pub findings: Vec<Finding>,
}

impl BundleReport {
    pub fn errors(&self) -> usize {
        self.findings.iter().filter(|f| f.is_error()).count()
    }

    pub fn warnings(&self) -> usize {
        self.findings.iter().filter(|f| f.is_warning()).count()
    }
}

/// The whole check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintReport {
    pub bundles: Vec<BundleReport>,
    /// The project's warning policy, carried so the rendering can explain the
    /// exit status without re-reading the configuration.
    pub fail_on_warnings: bool,
    /// Absolute path of the okf binary that produced these findings.
    pub okf: PathBuf,
}

impl LintReport {
    pub fn errors(&self) -> usize {
        self.bundles.iter().map(BundleReport::errors).sum()
    }

    pub fn warnings(&self) -> usize {
        self.bundles.iter().map(BundleReport::warnings).sum()
    }

    /// Whether this result should exit 0.
    pub fn passed(&self) -> bool {
        self.errors() == 0 && !(self.fail_on_warnings && self.warnings() > 0)
    }
}

/// Run the configured check.
///
/// Every prerequisite is reported as a prerequisite: no configuration, no
/// configured bundles, or no okf on `PATH`. Anything that happens once the
/// check is actually running — an unusable bundle tree, a validator that fails
/// or answers with something ahu cannot read — is a run failure.
pub fn lint(repo_root: &Path, loaded: &LoadedConfig) -> Result<LintReport> {
    let bundles = &loaded.config.knowledge.bundles;
    if bundles.is_empty() {
        return Err(Error::new(format!(
            "no knowledge bundles are configured, so there is nothing to lint.\n\
             Add the directories this project keeps its OKF bundles in to {}:\n\
             \x20 [knowledge]\n\
             \x20 bundles = [\"docs/knowledge\"]",
            display_path(&loaded.path)
        ))
        .with_kind(ErrorKind::Prerequisite));
    }
    let okf = crate::selection::resolve_executable(OKF_EXECUTABLE).ok_or_else(|| {
        Error::new(format!(
            "{OKF_EXECUTABLE} was not found on PATH outside this repository, so knowledge \
             bundles cannot be checked.\n\
             Install the OKF toolkit, or remove the [knowledge] bundles from {}.",
            display_path(&loaded.path)
        ))
        .with_kind(ErrorKind::Prerequisite)
    })?;
    let okf = PathBuf::from(okf);

    let mut reports = Vec::new();
    for bundle in bundles {
        let directory = resolve_bundle(repo_root, bundle)?;
        let mut findings: BTreeSet<Finding> = BTreeSet::new();
        for subcommand in OKF_SUBCOMMANDS {
            findings.extend(run_okf(&okf, subcommand, repo_root, &directory, bundle)?);
        }
        // `Finding`'s own ordering already puts errors first, then groups by
        // concept, rule, and message, so the set iterates in display order.
        let findings: Vec<Finding> = findings.into_iter().collect();
        reports.push(BundleReport {
            path: bundle.clone(),
            findings,
        });
    }
    Ok(LintReport {
        bundles: reports,
        fail_on_warnings: loaded.config.knowledge.fail_on_warnings,
        okf,
    })
}

/// Resolve a configured bundle to a directory ahu is willing to hand to okf.
fn resolve_bundle(repo_root: &Path, bundle: &str) -> Result<PathBuf> {
    // Component-by-component, so neither the bundle directory nor any parent of
    // it may be a symlink: following one would point okf at a tree outside the
    // checkout and put its contents into findings ahu then prints.
    let Some(directory) = crate::util::resolve_existing_within(repo_root, bundle)? else {
        bail!(
            "knowledge bundle {} does not exist in this checkout.\n\
             Create it, or remove it from knowledge.bundles.",
            display_safe(bundle)
        );
    };
    let meta = std::fs::symlink_metadata(&directory)
        .map_err(|e| Error::new(format!("cannot inspect {}: {e}", display_path(&directory))))?;
    if !meta.is_dir() {
        bail!(
            "knowledge bundle {} is not a directory.",
            display_safe(bundle)
        );
    }
    // okf reports an empty directory as a valid bundle with no findings, so a
    // mistyped or emptied path would otherwise pass the check while nothing was
    // checked. A bundle that holds no concept document is reported as the
    // configuration problem it is.
    if scan(&directory, &directory, 0, bundle)? == 0 {
        bail!(
            "knowledge bundle {} holds no concept documents, so checking it proves nothing.\n\
             An OKF concept is a Markdown file other than the reserved index.md and log.md. \
             Point knowledge.bundles at the directory that holds them, or remove this entry.",
            display_safe(bundle)
        );
    }
    Ok(directory)
}

/// Reserved OKF file names, which are bundle structure rather than concepts.
const RESERVED_DOCUMENTS: [&str; 2] = ["index.md", "log.md"];

/// Whether a file is an OKF concept document.
fn is_concept_document(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    // okf identifies concepts by extension, case-sensitively.
    name.ends_with(".md") && !RESERVED_DOCUMENTS.contains(&name)
}

/// Refuse a bundle tree that okf could read outside the repository, and count
/// the concept documents in it.
///
/// okf opens every file under the bundle root itself, so checking only the root
/// would leave a committed symlink at `docs/knowledge/tables/secrets.md` free to
/// name any file the user can read — whose contents come back in a finding
/// message. Every entry is classified with `symlink_metadata`, which never
/// follows, and anything that is not a regular file or a directory is refused
/// too: a fifo or device node would make the validator block or read a device.
///
/// The count comes from the same walk because the caller needs both answers
/// about the same tree, and walking it twice would only widen the window in
/// which it can change underneath.
fn scan(root: &Path, dir: &Path, depth: usize, bundle: &str) -> Result<usize> {
    if depth > MAX_BUNDLE_DEPTH {
        bail!(
            "knowledge bundle {} nests deeper than {MAX_BUNDLE_DEPTH} directories at {}, \
             so ahu cannot verify the whole tree is free of symlinks.",
            display_safe(bundle),
            display_path(&relative_to(root, dir))
        );
    }
    let entries = std::fs::read_dir(dir)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", display_path(dir))))?;
    let mut children: Vec<PathBuf> = Vec::new();
    for entry in entries {
        children.push(entry?.path());
    }
    children.sort();
    let mut concepts = 0;
    for path in children {
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| Error::new(format!("cannot inspect {}: {e}", display_path(&path))))?;
        if meta.file_type().is_symlink() {
            return Err(Error::new(format!(
                "{}\nknowledge bundle {} was not checked.",
                crate::util::symlink_refusal(&path, bundle),
                display_safe(bundle)
            )));
        }
        if meta.is_dir() {
            concepts += scan(root, &path, depth + 1, bundle)?;
        } else if meta.is_file() {
            if is_concept_document(&path) {
                concepts += 1;
            }
        } else {
            bail!(
                "knowledge bundle {} contains {}, which is neither a regular file nor a \
                 directory. ahu will not hand that tree to {OKF_EXECUTABLE}.",
                display_safe(bundle),
                display_path(&relative_to(root, &path))
            );
        }
    }
    Ok(concepts)
}

fn relative_to(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

/// Run one okf subcommand over one bundle and return its findings.
///
/// The binary is an absolute path resolved outside the repository and the
/// bundle is a single argument; nothing here is interpreted by a shell. stdin
/// is closed so the validator cannot consume the caller's input.
fn run_okf(
    okf: &Path,
    subcommand: &str,
    repo_root: &Path,
    directory: &Path,
    bundle: &str,
) -> Result<Vec<Finding>> {
    let output = Command::new(okf)
        .arg(subcommand)
        .arg(directory)
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            Error::new(format!(
                "cannot run {} {subcommand}: {e}",
                display_path(okf)
            ))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = output.status.code();
    let ran = format!("{} {subcommand}", display_path(okf));

    if stdout.len() > MAX_REPORT_BYTES {
        bail!(
            "{ran} printed more than {} MiB for knowledge bundle {}; ahu will not parse a \
             report that large.",
            MAX_REPORT_BYTES / (1024 * 1024),
            display_safe(bundle)
        );
    }
    // okf answers with an error envelope instead of a report when it cannot
    // load the bundle at all — an unparseable concept file, for instance. That
    // is a failed check either way, so it is read before the status is judged.
    if let Ok(reported) = serde_json::from_str::<OkfError>(&stdout) {
        bail!(
            "{ran} could not check knowledge bundle {}: {}{}",
            display_safe(bundle),
            display_safe(&reported.error.message),
            if reported.error.kind.is_empty() {
                String::new()
            } else {
                format!(" ({})", display_safe(&reported.error.kind))
            }
        );
    }
    let report: OkfReport = serde_json::from_str(&stdout).map_err(|e| {
        Error::new(format!(
            "{ran} printed output ahu could not interpret as an okf report for knowledge \
             bundle {}: {e}\n\
             ahu expects the JSON report contract of okf 0.5; a build that reports \
             differently is not one ahu can turn into a pass or a fail.",
            display_safe(bundle)
        ))
    })?;

    let found_errors = report.findings.iter().filter(|f| f.is_error()).count() as u64;
    let found_warnings = report.findings.iter().filter(|f| f.is_warning()).count() as u64;
    // `lint` reports the same counts as `validate` but suppresses the error
    // findings themselves, so only `validate` is expected to list them.
    let expected_errors = if subcommand == "validate" {
        report.errors
    } else {
        0
    };
    // `validate` exits 1 when the bundle has errors and 0 otherwise; `lint`
    // exits 0 whenever it produced a report at all.
    let expected_code = if subcommand == "validate" && report.errors > 0 {
        1
    } else {
        0
    };
    let inconsistency = if report.command != subcommand {
        Some(format!(
            "it reports command {:?}",
            display_safe(&report.command)
        ))
    } else if report.valid != (report.errors == 0) {
        Some(format!(
            "it reports valid = {} alongside {} error(s)",
            report.valid, report.errors
        ))
    } else if found_errors != expected_errors {
        Some(format!(
            "it reports {} error(s) but lists {found_errors}",
            report.errors
        ))
    } else if found_warnings != report.warnings {
        Some(format!(
            "it reports {} warning(s) but lists {found_warnings}",
            report.warnings
        ))
    } else if code != Some(expected_code) {
        Some(format!(
            "it exited {} where {expected_code} was the status for that report",
            code.map(|c| c.to_string())
                .unwrap_or_else(|| "on a signal".to_string())
        ))
    } else {
        None
    };
    if let Some(inconsistency) = inconsistency {
        bail!(
            "{ran} produced a report ahu will not act on for knowledge bundle {}: \
             {inconsistency}.\n\
             ahu does not guess which half of an inconsistent report is right, so the bundle \
             is neither passed nor failed on it.{}",
            display_safe(bundle),
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!("\n{}", crate::util::display_safe_block(stderr.trim()))
            }
        );
    }
    Ok(report.findings)
}

/// The terminal view: every finding, then the verdict and why it is the verdict.
pub fn render(report: &LintReport) -> String {
    use crate::style::{self, Role};
    let style = style::stdout();
    let mut out = String::new();
    out.push_str(&format!(
        "knowledge    {} bundle(s) checked with {}\n",
        report.bundles.len(),
        display_path(&report.okf)
    ));
    for bundle in &report.bundles {
        let (errors, warnings) = (bundle.errors(), bundle.warnings());
        out.push_str(&format!(
            "\n{}  {errors} error(s), {warnings} warning(s)\n",
            style.paint(Role::Heading, &display_safe(&bundle.path))
        ));
        if bundle.findings.is_empty() {
            out.push_str(&style.paint(Role::Hint, "  no findings\n"));
        }
        for finding in &bundle.findings {
            let role = match finding.severity {
                Severity::Error => Role::Error,
                Severity::Warn => Role::Warning,
            };
            // The concept id, rule, and message are validator output derived
            // from repository content, so all of it is escaped before it
            // reaches the terminal. The severity is ahu's own word for it.
            out.push_str(&format!(
                "  {} {}  {}\n        {}\n",
                style.paint(role, &format!("{:<5}", finding.severity.as_str())),
                display_safe(&finding.concept_id),
                style.paint(Role::Hint, &display_safe(&finding.rule)),
                display_safe(&finding.message)
            ));
        }
    }
    let (errors, warnings) = (report.errors(), report.warnings());
    let verdict = match (errors, warnings, report.fail_on_warnings) {
        (0, 0, _) => "\nNo knowledge findings.\n".to_string(),
        (0, w, false) => format!(
            "\n{w} warning(s), and knowledge.fail_on_warnings is false, so they do not fail this check.\n"
        ),
        (0, w, true) => {
            format!(
                "\n{w} warning(s), and knowledge.fail_on_warnings is true, so they fail this check.\n"
            )
        }
        (e, 0, _) => format!("\n{e} error(s) must be fixed.\n"),
        (e, w, _) => format!("\n{e} error(s) must be fixed, alongside {w} warning(s).\n"),
    };
    out.push_str(&style.paint(
        if report.passed() {
            Role::Success
        } else {
            Role::Error
        },
        &verdict,
    ));
    out
}

/// The `--output json` contract: a versioned, machine-readable report.
pub fn render_json(report: &LintReport) -> Result<String> {
    let value = serde_json::json!({
        "schema_version": REPORT_SCHEMA_VERSION,
        "command": "knowledge lint",
        "okf": report.okf,
        "fail_on_warnings": report.fail_on_warnings,
        "errors": report.errors(),
        "warnings": report.warnings(),
        "passed": report.passed(),
        "bundles": report
            .bundles
            .iter()
            .map(|bundle| serde_json::json!({
                "path": bundle.path,
                "errors": bundle.errors(),
                "warnings": bundle.warnings(),
                "findings": bundle.findings,
            }))
            .collect::<Vec<_>>(),
    });
    Ok(serde_json::to_string(&value)?)
}
