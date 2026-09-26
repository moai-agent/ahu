//! Evaluation suites: a fixed, named list of cases with fixed weights.
//!
//! A suite is an OKF Markdown document whose front matter names its cases by
//! path, relative to the suite file's own directory. A path may move up one
//! directory (for example, from `evals/suites/` to `evals/cases/`), but may not
//! escape that parent or traverse a symlink.
//!
//! The weights are the suite's, not the runner's. They are recorded with each
//! run so a report can say which suite a number came from and under which
//! weighting, and they are fixed in the document rather than passed on the
//! command line so two people running the same suite run the same suite.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::bail;
use crate::util::{Error, ErrorKind, Result, display_path};

/// Suite schema versions this ahu can load.
pub const SUPPORTED_SUITE_SCHEMA_VERSIONS: [u32; 1] = [1];

/// Most cases one suite may list.
pub const MAX_SUITE_CASES: usize = 256;

/// Largest suite document ahu will read.
const MAX_SUITE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SuiteDocument {
    okf_version: String,
    #[serde(rename = "type")]
    kind: String,
    schema_version: u32,
    id: String,
    #[serde(alias = "version")]
    suite_version: String,
    cases: Vec<SuiteEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SuiteEntry {
    path: String,
    weight: f64,
}

/// One case of a loaded suite: its weight, and the case itself.
#[derive(Debug, Clone)]
pub struct SuiteCase {
    /// The path exactly as the suite wrote it, for messages. Relative by
    /// construction, so it names nothing about this machine.
    pub declared_path: String,
    pub weight: f64,
    pub case: super::case::EvalCase,
}

/// A validated suite, with every case already loaded and validated.
#[derive(Debug, Clone)]
pub struct EvalSuite {
    pub id: String,
    pub suite_version: String,
    pub schema_version: u32,
    pub purpose: String,
    pub digest: String,
    pub cases: Vec<SuiteCase>,
}

/// Read, validate, and fully load the suite at `path`.
///
/// Every case is loaded here, before the caller can launch anything: an invalid
/// case in the tenth entry has to fail the whole suite rather than nine model
/// runs later.
pub fn load(path: &Path) -> Result<EvalSuite> {
    let bytes = std::fs::read(path).map_err(|error| {
        Error::new(format!(
            "cannot read evaluation suite {}: {error}",
            display_path(path)
        ))
        .with_kind(ErrorKind::Usage)
    })?;
    if bytes.len() > MAX_SUITE_BYTES {
        bail!(kind: ErrorKind::Usage, "evaluation suite exceeds the 256 KiB limit");
    }
    let root = suite_root(path)?;
    let (frontmatter, purpose) = super::case::split(&bytes)?;
    let document: SuiteDocument = yaml_serde::from_slice(frontmatter).map_err(|_| {
        Error::new("evaluation suite front matter is not valid supported YAML")
            .with_kind(ErrorKind::Usage)
    })?;
    if document.okf_version != crate::agent::OKF_VERSION
        || document.kind != "ahu:eval-suite"
        || !SUPPORTED_SUITE_SCHEMA_VERSIONS.contains(&document.schema_version)
        || !super::case::safe_eval_identifier(&document.id)
        || !super::case::safe_eval_identifier(&document.suite_version)
        || purpose.is_empty()
        || purpose.len() > 8192
    {
        bail!(kind: ErrorKind::Usage, "evaluation suite schema, type, or identity is invalid");
    }
    if document.cases.is_empty() || document.cases.len() > MAX_SUITE_CASES {
        bail!(kind: ErrorKind::Usage, "evaluation suite must list between 1 and {MAX_SUITE_CASES} cases");
    }
    let mut declared = BTreeSet::new();
    let mut case_ids = BTreeSet::new();
    let mut cases = Vec::with_capacity(document.cases.len());
    for entry in &document.cases {
        if !entry.weight.is_finite() || entry.weight <= 0.0 {
            bail!(kind: ErrorKind::Usage, "evaluation suite case weights must be finite and greater than zero");
        }
        if !declared.insert(entry.path.clone()) {
            bail!(kind: ErrorKind::Usage, "evaluation suite lists the same case path more than once");
        }
        let resolved = resolve_case(&root, &entry.path)?;
        let case = super::case::load(&resolved)?;
        if !case_ids.insert(case.id.clone()) {
            bail!(kind: ErrorKind::Usage, "evaluation suite lists more than one case with the same case id; a suite's case ids must be unique so its rows stay attributable");
        }
        cases.push(SuiteCase {
            declared_path: entry.path.clone(),
            weight: entry.weight,
            case,
        });
    }
    Ok(EvalSuite {
        id: document.id,
        suite_version: document.suite_version,
        schema_version: document.schema_version,
        purpose: purpose.to_owned(),
        digest: crate::util::digest_bytes(&bytes),
        cases,
    })
}

/// The directory a suite's relative case paths are resolved against.
///
/// The suite file itself must not be reached through a symlink either: its
/// directory is canonicalised, and the file has to be a regular file there.
fn suite_root(path: &Path) -> Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        Error::new(format!(
            "cannot read evaluation suite {}: {error}",
            display_path(path)
        ))
        .with_kind(ErrorKind::Usage)
    })?;
    if !metadata.file_type().is_file() {
        bail!(kind: ErrorKind::Usage, "evaluation suite {} must be a regular file, not a symlink or directory", display_path(path));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    parent.canonicalize().map_err(|error| {
        Error::new(format!(
            "evaluation suite directory {} is unavailable: {error}",
            display_path(parent)
        ))
        .with_kind(ErrorKind::Usage)
    })
}

/// Resolve one declared case path under the suite's directory.
fn resolve_case(root: &Path, declared: &str) -> Result<PathBuf> {
    if declared.is_empty() || declared.len() > 512 || declared.starts_with('/') {
        bail!(kind: ErrorKind::Usage, "evaluation suite case paths must be relative to the suite file; {declared:?} is not");
    }
    if declared.contains('\\') || declared.bytes().any(|byte| byte < 0x20) {
        bail!(kind: ErrorKind::Usage, "evaluation suite case paths must be plain forward-slash paths");
    }
    let (base, relative) = if let Some(relative) = declared.strip_prefix("../") {
        let parent = root.parent().ok_or_else(|| {
            Error::new("evaluation suite has no safe parent for a sibling case path")
                .with_kind(ErrorKind::Usage)
        })?;
        (parent, relative)
    } else {
        (root, declared)
    };
    let resolved = crate::util::resolve_existing_within(base, relative)
        .map_err(|error| error.with_kind(ErrorKind::Usage))?
        .ok_or_else(|| {
            Error::new(format!(
                "evaluation suite names case {declared:?}, which does not exist beside the suite"
            ))
            .with_kind(ErrorKind::Usage)
        })?;
    let metadata = std::fs::symlink_metadata(&resolved).map_err(|error| {
        Error::new(format!(
            "cannot read evaluation suite case {declared:?}: {error}"
        ))
        .with_kind(ErrorKind::Usage)
    })?;
    if !metadata.file_type().is_file() {
        bail!(kind: ErrorKind::Usage, "evaluation suite case {declared:?} must be a regular file");
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create dir");
        }
        std::fs::write(&path, body).expect("write");
        path
    }

    fn case_document(id: &str) -> String {
        format!(
            "---\nokf_version: '0.2'\ntype: ahu:eval-case\nschema_version: 2\nid: {id}\n\
             corpus_version: '1.0.0'\nstate: {{subject: s}}\n\
             questions: {{route: {{type: choice}}}}\nexpected: {{route: billing}}\n\
             rubric: {{route: routes to billing}}\n\
             scoring: {{route: 1.0, exact_match_pass_threshold: 1.0}}\n---\n\nA case.\n"
        )
    }

    fn suite_document(entries: &str) -> String {
        format!(
            "---\nokf_version: '0.2'\ntype: ahu:eval-suite\nschema_version: 1\n\
             id: routing-suite\nsuite_version: '1.0.0'\ncases:\n{entries}---\n\nA suite.\n"
        )
    }

    #[test]
    fn a_suite_loads_its_cases_in_order_with_their_declared_weights() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        write(dir.path(), "cases/one.md", &case_document("case-one"));
        write(dir.path(), "cases/two.md", &case_document("case-two"));
        let suite = write(
            dir.path(),
            "suite.md",
            &suite_document(
                "  - {path: cases/two.md, weight: 2.0}\n  - {path: cases/one.md, weight: 0.5}\n",
            ),
        );
        let loaded = load(&suite).expect("suite loads");
        assert_eq!(loaded.id, "routing-suite");
        assert_eq!(loaded.suite_version, "1.0.0");
        assert_eq!(loaded.digest.len(), 64);
        assert_eq!(loaded.purpose, "A suite.");
        // Suite order, not alphabetical order: the runner's order is the
        // document's so a rerun repeats it.
        assert_eq!(
            loaded
                .cases
                .iter()
                .map(|entry| (entry.case.id.as_str(), entry.weight))
                .collect::<Vec<_>>(),
            [("case-two", 2.0), ("case-one", 0.5)]
        );
        assert_eq!(loaded.cases[0].declared_path, "cases/two.md");
    }

    #[test]
    fn a_suite_can_name_cases_in_a_sibling_directory_without_leaving_its_parent() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        write(dir.path(), "cases/one.md", &case_document("case-one"));
        let suite = write(
            dir.path(),
            "suites/suite.md",
            &suite_document("  - {path: ../cases/one.md, weight: 1.0}\n")
                .replace("suite_version:", "version:"),
        );
        let loaded = load(&suite).expect("safe sibling case loads");
        assert_eq!(loaded.cases[0].case.id, "case-one");

        let outside = write(
            dir.path(),
            "suites/escape.md",
            &suite_document("  - {path: ../../outside.md, weight: 1.0}\n"),
        );
        let error = load(&outside).expect_err("parent escape refused");
        assert_eq!(error.kind(), ErrorKind::Usage);
    }

    #[test]
    fn unsafe_paths_duplicate_entries_and_duplicate_case_ids_are_refused() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let outside = tempfile::TempDir::new().expect("temp dir");
        write(dir.path(), "cases/one.md", &case_document("case-one"));
        write(dir.path(), "cases/copy.md", &case_document("case-one"));
        write(outside.path(), "secret.md", &case_document("outside-case"));
        std::os::unix::fs::symlink(
            outside.path().join("secret.md"),
            dir.path().join("cases/link.md"),
        )
        .expect("symlink");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("elsewhere")).expect("symlink");

        for (entries, needle) in [
            (
                "  - {path: ../../escape.md, weight: 1.0}\n",
                "not a plain path",
            ),
            ("  - {path: /etc/passwd, weight: 1.0}\n", "must be relative"),
            ("  - {path: cases/link.md, weight: 1.0}\n", "symlink"),
            ("  - {path: elsewhere/secret.md, weight: 1.0}\n", "symlink"),
            (
                "  - {path: cases/absent.md, weight: 1.0}\n",
                "does not exist",
            ),
            ("  - {path: cases, weight: 1.0}\n", "regular file"),
            (
                "  - {path: cases/one.md, weight: 1.0}\n  - {path: cases/one.md, weight: 1.0}\n",
                "same case path more than once",
            ),
            (
                "  - {path: cases/one.md, weight: 1.0}\n  - {path: cases/copy.md, weight: 1.0}\n",
                "same case id",
            ),
            ("  - {path: cases/one.md, weight: 0}\n", "greater than zero"),
            (
                "  - {path: cases/one.md, weight: -1}\n",
                "greater than zero",
            ),
            (
                "  - {path: 'cases\\one.md', weight: 1.0}\n",
                "plain forward-slash",
            ),
        ] {
            let suite = write(dir.path(), "suite.md", &suite_document(entries));
            let error = load(&suite).expect_err("refused");
            assert_eq!(error.kind(), ErrorKind::Usage, "{entries}");
            assert!(error.to_string().contains(needle), "{entries}: {error}");
        }
    }

    #[test]
    fn a_wrong_type_version_or_empty_case_list_is_refused_before_anything_loads() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        write(dir.path(), "cases/one.md", &case_document("case-one"));
        for body in [
            suite_document("").replace("cases:\n", "cases: []\n"),
            suite_document("  - {path: cases/one.md, weight: 1.0}\n")
                .replace("ahu:eval-suite", "ahu:eval-case"),
            suite_document("  - {path: cases/one.md, weight: 1.0}\n")
                .replace("schema_version: 1", "schema_version: 2"),
            suite_document("  - {path: cases/one.md, weight: 1.0}\n")
                .replace("id: routing-suite", "id: 'not a safe id'"),
            suite_document("  - {path: cases/one.md, weight: 1.0}\n").replace("\nA suite.\n", "\n"),
            suite_document("  - {path: cases/one.md, weight: 1.0, extra: 1}\n"),
        ] {
            let suite = write(dir.path(), "suite.md", &body);
            let error = load(&suite).expect_err("refused");
            assert_eq!(error.kind(), ErrorKind::Usage, "{body}");
        }
    }

    #[test]
    fn an_invalid_case_fails_the_whole_suite_rather_than_part_of_a_run() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        write(dir.path(), "cases/one.md", &case_document("case-one"));
        write(
            dir.path(),
            "cases/broken.md",
            &case_document("case-broken")
                .replace("scoring: {route: 1.0,", "scoring: {route: -1.0,"),
        );
        let suite = write(
            dir.path(),
            "suite.md",
            &suite_document(
                "  - {path: cases/one.md, weight: 1.0}\n  - {path: cases/broken.md, weight: 1.0}\n",
            ),
        );
        let error = load(&suite).expect_err("refused");
        assert_eq!(error.kind(), ErrorKind::Usage);
        assert!(error.to_string().contains("scoring"), "{error}");
    }

    #[test]
    fn the_suite_file_itself_may_not_be_a_symlink() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        write(dir.path(), "cases/one.md", &case_document("case-one"));
        let real = write(
            dir.path(),
            "real-suite.md",
            &suite_document("  - {path: cases/one.md, weight: 1.0}\n"),
        );
        let link = dir.path().join("suite.md");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let error = load(&link).expect_err("refused");
        assert_eq!(error.kind(), ErrorKind::Usage);
        assert!(error.to_string().contains("regular file"), "{error}");
    }
}
