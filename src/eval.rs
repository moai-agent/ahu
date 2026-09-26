//! Comparison of local evaluation runs from external JSONL records.
//!
//! `ahu eval report` reads the compact records `scripts/local_eval.py record`
//! appends and groups them into one row per comparable configuration. It is a
//! reader: it never launches a candidate or an evaluator, and it never writes
//! anything.
//!
//! Three constraints shape this module.
//!
//!   - Run evidence stays outside the checkout. The recorder already refuses to
//!     write inside the repository, and this command refuses to read from
//!     inside it, so a record file cannot become repository content by being
//!     summarised from one. The path is canonicalised first, so a symlink that
//!     points into the checkout is refused too.
//!   - A missing observation is not a zero. Token, timing, and decision-call
//!     figures are reported as coverage counts alongside means taken over the
//!     records that carried them, so a configuration that reported nothing is
//!     visibly uncovered rather than silently cheap and fast.
//!   - The records are external input. Every field is validated, the line
//!     number travels with any complaint about one, and every value is escaped
//!     before it reaches the terminal.
//!
//! The record shape is the one `scripts/local_eval.py` writes. Unknown fields
//! are ignored so a newer recorder stays readable here, but a record missing a
//! field this report groups or scores by is an error rather than a guess.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::util::{Error, ErrorKind, Result, display_path, display_safe};

/// Version of the `--output json` report contract.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

/// Record schema versions this report knows how to read.
///
/// A record that declares a newer schema is refused rather than interpreted
/// under the old field meanings.
pub const SUPPORTED_RECORD_SCHEMA_VERSIONS: [u64; 1] = [1];

/// Largest record file this command will read.
///
/// The records are one compact numeric object per run, so a file beyond this is
/// not the longitudinal record described in `evals/README.md`.
const MAX_RECORDS_BYTES: u64 = 64 * 1024 * 1024;

/// The value substituted for a field the recorder left unset.
const UNSPECIFIED: &str = "unspecified";

/// The stage a record belongs to when it does not name one.
const DEFAULT_STAGE: &str = "candidate";

/// One run, as `scripts/local_eval.py record` appends it.
///
/// Unknown fields are accepted: the recorder may add fields, and this report
/// only has to keep meaning the same thing for the ones it reads.
#[derive(Debug, Clone, Deserialize)]
struct Record {
    /// Absent on records written before the field existed; when present it must
    /// be a version this report understands.
    #[serde(default)]
    schema_version: Option<u64>,
    case_id: String,
    model: String,
    harness: String,
    score: f64,
    passed: bool,
    #[serde(default)]
    corpus_version: Option<String>,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    agent_version: Option<String>,
    #[serde(default)]
    evaluator: Option<String>,
    #[serde(default)]
    evaluator_version: Option<String>,
    #[serde(default)]
    harness_version: Option<String>,
    #[serde(default)]
    ahu_revision: Option<String>,
    #[serde(default)]
    skill_digest: Option<String>,
    /// Observed token metrics, keyed by the recorder's metric names. An empty
    /// or absent map is no observation; a map of zeros is an observation of
    /// zero.
    #[serde(default)]
    reported_tokens: Option<BTreeMap<String, serde_json::Value>>,
    #[serde(default)]
    decision_call_count: Option<f64>,
    #[serde(default)]
    elapsed_ms: Option<f64>,
}

/// Everything one row of the report is grouped by.
///
/// Ordered as the report prints it, so sorting the map sorts the report.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GroupKey {
    pub case_id: String,
    pub corpus_version: String,
    pub stage: String,
    pub agent: String,
    pub agent_version: String,
    pub evaluator: String,
    pub evaluator_version: String,
    pub model: String,
    pub harness: String,
    pub harness_version: String,
    pub ahu_revision: String,
    pub skill_digest: String,
}

/// How many runs in a group carried each kind of observation.
///
/// Kept apart from the measurements themselves: `decision_calls: 0` over four
/// runs means four runs reported no count, not four runs that made no call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Coverage {
    pub tokens: usize,
    pub timing: usize,
    pub decision_calls: usize,
}

/// One comparable configuration.
#[derive(Debug, Clone)]
pub struct Group {
    pub key: GroupKey,
    pub runs: usize,
    pub passes: usize,
    pub mean_score: f64,
    pub pass_rate: f64,
    pub coverage: Coverage,
    /// Mean over the runs that reported a time; `None` when none did.
    pub mean_elapsed_ms: Option<f64>,
    /// Mean over the runs that reported a count; `None` when none did.
    pub mean_decision_calls: Option<f64>,
    /// Every token metric name observed anywhere in the group.
    pub token_fields: BTreeSet<String>,
}

/// The whole comparison, sorted by group key.
#[derive(Debug, Clone)]
pub struct Report {
    /// Where the records were read from. Kept for the terminal view only: the
    /// JSON contract carries no local path.
    pub records: PathBuf,
    pub record_count: usize,
    pub groups: Vec<Group>,
}

/// Round as `scripts/local_eval.py trend` does, so the two agree on a value.
fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// A recorder field that may be absent, null, or empty, with its substitute.
fn or_unspecified(value: &Option<String>, fallback: &str) -> String {
    value
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// Confirm the record file is outside this repository, and return its real path.
///
/// Canonicalisation happens before the comparison, so neither a relative path
/// nor a symlink outside the checkout that resolves into it gets past this.
/// Both the invoking checkout and the primary checkout are refused, which also
/// covers every task worktree, because those live under the primary root.
pub fn records_path_outside_repository(repo: &crate::git::Repo, records: &Path) -> Result<PathBuf> {
    let real = records.canonicalize().map_err(|error| {
        Error::new(format!(
            "cannot read eval records {}: {error}",
            display_path(records)
        ))
        .with_kind(ErrorKind::Usage)
    })?;
    let mut roots = vec![repo.root.clone()];
    if let Ok(primary) = repo.primary_root() {
        roots.push(primary);
    }
    for root in roots {
        let Ok(root) = root.canonicalize() else {
            continue;
        };
        if real.starts_with(&root) {
            return Err(Error::new(format!(
                "eval records must live outside this repository, and {} resolves to {} inside {}.\n\
                 Run evidence is kept in a user-owned directory; ahu neither reads nor writes it \
                 inside the checkout.",
                display_path(records),
                display_path(&real),
                display_path(&root)
            ))
            .with_kind(ErrorKind::Usage));
        }
    }
    Ok(real)
}

/// Read, validate, and group the records at `records`.
pub fn report(repo: &crate::git::Repo, records: &Path) -> Result<Report> {
    let real = records_path_outside_repository(repo, records)?;
    let metadata = std::fs::metadata(&real).map_err(|error| {
        Error::new(format!(
            "cannot read eval records {}: {error}",
            display_path(records)
        ))
    })?;
    if !metadata.is_file() {
        return Err(Error::new(format!(
            "eval records must be a JSONL file; {} is not a regular file.",
            display_path(records)
        ))
        .with_kind(ErrorKind::Usage));
    }
    if metadata.len() > MAX_RECORDS_BYTES {
        return Err(Error::new(format!(
            "eval records {} is {} bytes, beyond the {MAX_RECORDS_BYTES} byte limit for a run record file.",
            display_path(records),
            metadata.len()
        ))
        .with_kind(ErrorKind::Usage));
    }
    let text = std::fs::read_to_string(&real).map_err(|error| {
        Error::new(format!(
            "cannot read eval records {}: {error}",
            display_path(records)
        ))
    })?;
    let parsed = parse_records(records, &text)?;
    Ok(Report {
        records: records.to_path_buf(),
        record_count: parsed.len(),
        groups: group(&parsed),
    })
}

/// Parse one JSON object per non-blank line.
///
/// Each line is parsed on its own, so a syntax error names the line it is on
/// rather than an offset into the whole file.
fn parse_records(path: &Path, text: &str) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if line.trim().is_empty() {
            continue;
        }
        // Parsed twice on purpose: once to insist the line is an object rather
        // than some other JSON document, and once into the record, from the
        // text, because deserializing from a `Value` loses the position that
        // tells the reader which field on the line was wrong.
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|error| malformed(path, number, &error))?;
        if !value.is_object() {
            return Err(malformed_text(
                path,
                number,
                "expected a JSON object, one run record per line",
            ));
        }
        let record: Record =
            serde_json::from_str(line).map_err(|error| malformed(path, number, &error))?;
        if let Some(version) = record.schema_version
            && !SUPPORTED_RECORD_SCHEMA_VERSIONS.contains(&version)
        {
            return Err(malformed_text(
                path,
                number,
                &format!(
                    "record schema_version {version} is not one this ahu reads ({})",
                    SUPPORTED_RECORD_SCHEMA_VERSIONS
                        .iter()
                        .map(u64::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        if !record.score.is_finite() {
            return Err(malformed_text(
                path,
                number,
                "score must be a finite number",
            ));
        }
        records.push(record);
    }
    Ok(records)
}

/// A parser's complaint about one record, carrying the line it is on.
///
/// Each line is parsed as its own document, so serde's own line number is
/// always 1 and only its column says anything; the file line comes from here.
fn malformed(path: &Path, line: usize, error: &serde_json::Error) -> Error {
    malformed_text(path, line, &error.to_string().replace("at line 1 ", "at "))
}

/// A complaint about one record, carrying the line it is on.
///
/// The message quotes parser words about repository-external input, so it is
/// escaped here rather than at the printer.
fn malformed_text(path: &Path, line: usize, message: &str) -> Error {
    Error::new(format!(
        "{}:{line}: {}",
        display_path(path),
        display_safe(message)
    ))
    .with_kind(ErrorKind::Usage)
}

fn key_for(record: &Record) -> GroupKey {
    GroupKey {
        case_id: record.case_id.clone(),
        corpus_version: or_unspecified(&record.corpus_version, UNSPECIFIED),
        stage: or_unspecified(&record.stage, DEFAULT_STAGE),
        agent: or_unspecified(&record.agent, UNSPECIFIED),
        agent_version: or_unspecified(&record.agent_version, UNSPECIFIED),
        evaluator: or_unspecified(&record.evaluator, UNSPECIFIED),
        evaluator_version: or_unspecified(&record.evaluator_version, UNSPECIFIED),
        model: record.model.clone(),
        harness: record.harness.clone(),
        harness_version: or_unspecified(&record.harness_version, UNSPECIFIED),
        ahu_revision: or_unspecified(&record.ahu_revision, UNSPECIFIED),
        skill_digest: or_unspecified(&record.skill_digest, UNSPECIFIED),
    }
}

fn group(records: &[Record]) -> Vec<Group> {
    let mut grouped: BTreeMap<GroupKey, Vec<&Record>> = BTreeMap::new();
    for record in records {
        grouped.entry(key_for(record)).or_default().push(record);
    }
    grouped
        .into_iter()
        .map(|(key, items)| {
            let runs = items.len();
            let passes = items.iter().filter(|item| item.passed).count();
            let total: f64 = items.iter().map(|item| item.score).sum();
            let mut coverage = Coverage::default();
            let mut token_fields = BTreeSet::new();
            let mut elapsed = Vec::new();
            let mut calls = Vec::new();
            for item in &items {
                if let Some(tokens) = &item.reported_tokens
                    && !tokens.is_empty()
                {
                    coverage.tokens += 1;
                    token_fields.extend(tokens.keys().cloned());
                }
                // A reported zero is an observation; only an absent field is not.
                if let Some(value) = item.elapsed_ms {
                    coverage.timing += 1;
                    elapsed.push(value);
                }
                if let Some(value) = item.decision_call_count {
                    coverage.decision_calls += 1;
                    calls.push(value);
                }
            }
            Group {
                key,
                runs,
                passes,
                mean_score: round4(total / runs as f64),
                pass_rate: round4(passes as f64 / runs as f64),
                coverage,
                mean_elapsed_ms: mean(&elapsed),
                mean_decision_calls: mean(&calls),
                token_fields,
            }
        })
        .collect()
}

/// The mean of the observations there are, or `None` when there are none.
fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(round4(values.iter().sum::<f64>() / values.len() as f64))
}

/// The terminal view: one block per comparable configuration.
pub fn render(report: &Report) -> String {
    use crate::style::{self, Role};
    let style = style::stdout();
    let mut out = format!(
        "eval report  {} record(s) from {}\n             {} comparable configuration(s)\n",
        report.record_count,
        display_path(&report.records),
        report.groups.len()
    );
    if report.groups.is_empty() {
        out.push_str(&style.paint(
            Role::Hint,
            "\nNo run records. Append one with `scripts/local_eval.py record`.\n",
        ));
        return out;
    }
    for group in &report.groups {
        let key = &group.key;
        // Every value below is external record content, so all of it is escaped.
        out.push_str(&format!(
            "\n{}  corpus {}  stage {}\n",
            style.paint(Role::Heading, &display_safe(&key.case_id)),
            display_safe(&key.corpus_version),
            display_safe(&key.stage)
        ));
        out.push_str(&format!(
            "  agent      {} {}\n  evaluator  {} {}\n  model      {}\n  harness    {} {}\n  ahu        {}  skill digest {}\n",
            style.paint(Role::Agent, &display_safe(&key.agent)),
            display_safe(&key.agent_version),
            display_safe(&key.evaluator),
            display_safe(&key.evaluator_version),
            style.paint(Role::Runtime, &display_safe(&key.model)),
            display_safe(&key.harness),
            display_safe(&key.harness_version),
            display_safe(&key.ahu_revision),
            display_safe(&key.skill_digest)
        ));
        out.push_str(&format!(
            "  runs {}  mean score {:.4}  pass rate {:.4} ({}/{})\n",
            group.runs, group.mean_score, group.pass_rate, group.passes, group.runs
        ));
        out.push_str(&format!(
            "  coverage   tokens {}/{}  timing {}/{}  decision calls {}/{}\n",
            group.coverage.tokens,
            group.runs,
            group.coverage.timing,
            group.runs,
            group.coverage.decision_calls,
            group.runs
        ));
        out.push_str(&format!(
            "  observed   elapsed ms {}  decision calls {}  token fields {}\n",
            measurement(group.mean_elapsed_ms),
            measurement(group.mean_decision_calls),
            if group.token_fields.is_empty() {
                style.paint(Role::Gap, "none observed")
            } else {
                group
                    .token_fields
                    .iter()
                    .map(|field| display_safe(field))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
    }
    out.push_str(&style.paint(
        Role::Hint,
        "\nMeans cover only the runs that reported the measurement.\n\
         Coverage below the run count is missing observation, not a measured zero.\n",
    ));
    out
}

/// A mean that may have had nothing to average.
fn measurement(value: Option<f64>) -> String {
    match value {
        Some(value) => format!("{value}"),
        None => crate::style::stdout().paint(crate::style::Role::Gap, "none observed"),
    }
}

/// The `--output json` contract: a versioned, machine-readable comparison.
///
/// It carries no local path: the caller supplied the records location, and the
/// record content itself is deliberately free of paths and transcripts.
pub fn render_json(report: &Report) -> Result<String> {
    let value = serde_json::json!({
        "schema_version": REPORT_SCHEMA_VERSION,
        "command": "eval report",
        "record_count": report.record_count,
        "groups": report.groups.iter().map(|group| serde_json::json!({
            "case_id": group.key.case_id,
            "corpus_version": group.key.corpus_version,
            "stage": group.key.stage,
            "agent": group.key.agent,
            "agent_version": group.key.agent_version,
            "evaluator": group.key.evaluator,
            "evaluator_version": group.key.evaluator_version,
            "model": group.key.model,
            "harness": group.key.harness,
            "harness_version": group.key.harness_version,
            "ahu_revision": group.key.ahu_revision,
            "skill_digest": group.key.skill_digest,
            "runs": group.runs,
            "passed": group.passes,
            "mean_score": group.mean_score,
            "pass_rate": group.pass_rate,
            "coverage": {
                "token_observations": group.coverage.tokens,
                "timing_observations": group.coverage.timing,
                "decision_call_observations": group.coverage.decision_calls,
            },
            "observed": {
                "mean_elapsed_ms": group.mean_elapsed_ms,
                "mean_decision_calls": group.mean_decision_calls,
                "token_fields": group.token_fields,
            },
        })).collect::<Vec<_>>(),
    });
    Ok(serde_json::to_string(&value)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid record, with `overrides` replacing or adding raw JSON fields.
    ///
    /// Built from a map so an override never emits a duplicate key: the parser
    /// refuses those, which is the point of building the fixture this way.
    fn record(overrides: &[(&str, &str)]) -> String {
        let mut fields: BTreeMap<&str, &str> = BTreeMap::from([
            ("schema_version", "1"),
            ("case_id", "\"c1\""),
            ("corpus_version", "\"1.0.0\""),
            ("model", "\"m\""),
            ("harness", "\"h\""),
            ("score", "1.0"),
            ("passed", "true"),
        ]);
        fields.extend(overrides.iter().copied());
        let body = fields
            .iter()
            .map(|(name, value)| format!("\"{name}\":{value}"))
            .collect::<Vec<_>>()
            .join(",");
        format!("{{{body}}}")
    }

    #[test]
    fn blank_lines_are_skipped_and_defaults_fill_unset_identity_fields() {
        let text = format!("\n{}\n\n   \n", record(&[]));
        let records = parse_records(Path::new("runs.jsonl"), &text).expect("parses");
        assert_eq!(records.len(), 1);
        let groups = group(&records);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key.stage, "candidate");
        assert_eq!(groups[0].key.agent, UNSPECIFIED);
        assert_eq!(groups[0].key.skill_digest, UNSPECIFIED);
        assert_eq!(groups[0].runs, 1);
    }

    #[test]
    fn a_null_identity_field_groups_with_an_absent_one() {
        let text = format!(
            "{}\n{}\n",
            record(&[]),
            record(&[("agent", "null"), ("skill_digest", "\"\"")])
        );
        let records = parse_records(Path::new("runs.jsonl"), &text).expect("parses");
        assert_eq!(group(&records).len(), 1);
    }

    #[test]
    fn differing_identity_fields_split_groups() {
        for field in [
            ("agent", "\"@a\""),
            ("agent_version", "\"2.0.0\""),
            ("evaluator", "\"@j\""),
            ("evaluator_version", "\"2.0.0\""),
            ("harness_version", "\"9\""),
            ("ahu_revision", "\"deadbee\""),
            ("skill_digest", "\"sha256:1\""),
            ("stage", "\"evaluator\""),
            ("corpus_version", "\"2.0.0\""),
            ("case_id", "\"c2\""),
            ("model", "\"other\""),
            ("harness", "\"other\""),
        ] {
            let text = format!("{}\n{}\n", record(&[]), record(&[field]));
            let records = parse_records(Path::new("runs.jsonl"), &text).expect("parses");
            assert_eq!(group(&records).len(), 2, "{field:?} must split the group");
        }
    }

    #[test]
    fn a_reported_zero_is_covered_and_a_missing_metric_is_not() {
        let text = format!(
            "{}\n{}\n",
            record(&[
                ("elapsed_ms", "0"),
                ("decision_call_count", "0"),
                ("reported_tokens", r#"{"input":0}"#),
            ]),
            record(&[("reported_tokens", "{}")])
        );
        let records = parse_records(Path::new("runs.jsonl"), &text).expect("parses");
        let groups = group(&records);
        assert_eq!(groups.len(), 1);
        let only = &groups[0];
        assert_eq!(only.runs, 2);
        assert_eq!(only.coverage.timing, 1);
        assert_eq!(only.coverage.decision_calls, 1);
        assert_eq!(only.coverage.tokens, 1);
        // The observation that exists is zero; the one that does not is absent.
        assert_eq!(only.mean_elapsed_ms, Some(0.0));
        assert_eq!(only.mean_decision_calls, Some(0.0));
        assert_eq!(
            only.token_fields.iter().cloned().collect::<Vec<_>>(),
            vec!["input".to_string()]
        );
    }

    #[test]
    fn means_and_rates_average_only_the_runs_that_reported() {
        let text = format!(
            "{}\n{}\n{}\n",
            record(&[("elapsed_ms", "100")]),
            record(&[("elapsed_ms", "300")]),
            record(&[("score", "0.0"), ("passed", "false")])
        );
        let records = parse_records(Path::new("runs.jsonl"), &text).expect("parses");
        let only = &group(&records)[0];
        assert_eq!(only.runs, 3);
        assert_eq!(only.mean_score, 0.6667);
        assert_eq!(only.pass_rate, 0.6667);
        // 100 and 300 over the two runs that timed, not over all three.
        assert_eq!(only.mean_elapsed_ms, Some(200.0));
        assert_eq!(only.coverage.timing, 2);
    }

    #[test]
    fn malformed_lines_name_the_line_they_are_on() {
        let good = record(&[]);
        for (body, expected, needle) in [
            (format!("{good}\n{{oops\n"), 2, "key must be a string"),
            (format!("{good}\n[]\n"), 2, "expected a JSON object"),
            (
                format!(
                    "{good}\n{{\"model\":\"m\",\"harness\":\"h\",\"score\":1,\"passed\":true}}\n"
                ),
                2,
                "missing field `case_id`",
            ),
            (
                "{\"case_id\":\"c\",\"model\":\"m\",\"harness\":\"h\",\"score\":\"high\",\"passed\":true}\n"
                    .to_string(),
                1,
                "expected f64 at column",
            ),
            (
                format!(
                    "{good}\n{{\"case_id\":\"a\",\"case_id\":\"b\",\"model\":\"m\",\"harness\":\"h\",\"score\":1,\"passed\":true}}\n"
                ),
                2,
                "duplicate field `case_id`",
            ),
            (
                format!(
                    "{good}\n{{\"schema_version\":99,\"case_id\":\"c\",\"model\":\"m\",\"harness\":\"h\",\"score\":1,\"passed\":true}}\n"
                ),
                2,
                "schema_version 99",
            ),
        ] {
            let error = parse_records(Path::new("runs.jsonl"), &body).expect_err("refused");
            let message = error.to_string();
            assert!(
                message.starts_with(&format!("runs.jsonl:{expected}: ")),
                "{message} must point at line {expected}"
            );
            assert!(message.contains(needle), "{message} must mention {needle}");
            assert_eq!(error.kind(), ErrorKind::Usage);
        }
    }

    #[test]
    fn an_unknown_field_is_ignored_so_a_newer_recorder_stays_readable() {
        let text = record(&[("trace_id", "\"abc\""), ("future_field", r#"{"nested":1}"#)]);
        let records = parse_records(Path::new("runs.jsonl"), &text).expect("parses");
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn the_json_contract_is_versioned_and_carries_no_local_path() {
        let records = parse_records(
            Path::new("runs.jsonl"),
            &record(&[("agent", "\"@triage\""), ("elapsed_ms", "12")]),
        )
        .expect("parses");
        let report = Report {
            records: PathBuf::from("/home/someone/evals/runs.jsonl"),
            record_count: records.len(),
            groups: group(&records),
        };
        let json: serde_json::Value =
            serde_json::from_str(&render_json(&report).expect("renders")).expect("valid JSON");
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["command"], "eval report");
        assert_eq!(json["record_count"], 1);
        let group = &json["groups"][0];
        assert_eq!(group["agent"], "@triage");
        assert_eq!(group["runs"], 1);
        assert_eq!(group["coverage"]["timing_observations"], 1);
        assert_eq!(group["coverage"]["token_observations"], 0);
        assert_eq!(group["observed"]["mean_elapsed_ms"], 12.0);
        // A metric nothing reported is null, never 0.
        assert!(group["observed"]["mean_decision_calls"].is_null());
        assert!(
            !render_json(&report)
                .expect("renders")
                .contains("/home/someone"),
            "the JSON contract must not carry a local path"
        );
    }

    #[test]
    fn the_terminal_view_escapes_record_content_and_names_uncovered_metrics() {
        let text = "{\"case_id\":\"c\\u001b[31m1\",\"model\":\"m\",\"harness\":\"h\",\
                    \"score\":1,\"passed\":true}";
        let records = parse_records(Path::new("runs.jsonl"), text).expect("parses");
        let rendered = render(&Report {
            records: PathBuf::from("runs.jsonl"),
            record_count: records.len(),
            groups: group(&records),
        });
        assert!(!rendered.contains('\u{1b}'), "{rendered}");
        assert!(rendered.contains("none observed"), "{rendered}");
        assert!(rendered.contains("timing 0/1"), "{rendered}");
    }

    #[test]
    fn an_empty_record_file_reports_nothing_rather_than_failing() {
        let records = parse_records(Path::new("runs.jsonl"), "\n\n").expect("parses");
        assert!(records.is_empty());
        let rendered = render(&Report {
            records: PathBuf::from("runs.jsonl"),
            record_count: 0,
            groups: Vec::new(),
        });
        assert!(rendered.contains("No run records"), "{rendered}");
    }
}
