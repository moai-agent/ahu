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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;

use crate::bail;
use crate::table;
use crate::util::{Error, ErrorKind, Result, display_path, display_safe};

pub mod case;
pub mod decision;
pub mod fingerprint;
pub mod stats;
pub mod suite;
pub mod trajectory;

pub use case::{PromptProfile, ToolExpectationStatus, ToolExpectations, score_tool_expectations};
pub use fingerprint::{AgentFingerprint, Blinding, BuildIdentity, InputFingerprint, SuiteIdentity};
pub use stats::Interval;
pub use suite::EvalSuite;

static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Version of the `--output json` report contract.
pub const REPORT_SCHEMA_VERSION: u32 = 2;

/// Current JSONL record schema version.
pub const RECORD_SCHEMA_VERSION: u64 = 2;

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
    schema_version: u64,
    case_id: String,
    model: String,
    harness: String,
    score: Option<f64>,
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
    evaluator_kind: Option<String>,
    #[serde(default)]
    decision_evaluator_policy_digest: Option<String>,
    #[serde(default)]
    decision_evaluator_policy: Option<serde_json::Value>,
    #[serde(default)]
    evaluator_metrics: Option<decision::Observation>,
    #[serde(default)]
    evaluation_elapsed_ms: Option<f64>,
    #[serde(default)]
    evaluator_version: Option<String>,
    #[serde(default)]
    evaluator_model: Option<String>,
    #[serde(default)]
    evaluator_harness: Option<String>,
    #[serde(default)]
    evaluator_harness_version: Option<String>,
    #[serde(default)]
    harness_version: Option<String>,
    #[serde(default)]
    skill_digest: Option<String>,
    #[serde(default)]
    selection_policy_digest: Option<String>,
    #[serde(default)]
    skill_selection: Option<crate::skill_selection::Selection>,
    #[serde(default)]
    total_elapsed_ms: Option<f64>,
    #[serde(default)]
    selection_telemetry_observed: Option<bool>,
    #[serde(default)]
    candidate_selection: BTreeMap<String, f64>,
    /// Observed token metrics, keyed by the recorder's metric names. An empty
    /// or absent map is no observation; a map of zeros is an observation of
    /// zero.
    #[serde(default)]
    reported_tokens: Option<BTreeMap<String, serde_json::Value>>,
    #[serde(default)]
    decision_call_count: Option<f64>,
    #[serde(default)]
    elapsed_ms: Option<f64>,
    #[serde(default)]
    decision_service_duration_ms: Option<f64>,
    #[serde(default)]
    decision_request_bytes: Option<u64>,
    #[serde(default)]
    decision_request_observations: u64,
    #[serde(default)]
    mcp_observed: Option<bool>,
    #[serde(default)]
    mcp_request_count: Option<f64>,
    #[serde(default)]
    mcp_tool_list_count: Option<f64>,
    #[serde(default)]
    mcp_tool_call_count: Option<f64>,
    #[serde(default)]
    mcp_tool_error_count: Option<f64>,
    #[serde(default)]
    typed_decision_error_count: Option<f64>,
    #[serde(default)]
    mcp_tools: Option<BTreeMap<String, f64>>,
    #[serde(default)]
    mcp_tool_errors_by_name: Option<BTreeMap<String, f64>>,
    #[serde(default)]
    telemetry_receiver: Option<crate::eval_otel::ReceiverStats>,
    // Version 2 evaluation evidence.
    #[serde(default)]
    case_schema_version: Option<u64>,
    #[serde(default)]
    case_digest: Option<String>,
    #[serde(default)]
    prompt_profile: Option<String>,
    #[serde(default)]
    prompt_version: Option<u64>,
    #[serde(default)]
    scoring_version: Option<u64>,
    #[serde(default)]
    suite_id: Option<String>,
    #[serde(default)]
    suite_version: Option<String>,
    #[serde(default)]
    suite_digest: Option<String>,
    #[serde(default)]
    agent_identity_digest: Option<String>,
    #[serde(default)]
    evaluator_identity_digest: Option<String>,
    #[serde(default)]
    evaluator_skill_digest: Option<String>,
    #[serde(default)]
    evaluator_repo_head: Option<String>,
    #[serde(default)]
    blinding: Option<String>,
    #[serde(default)]
    tool_definitions_digest: Option<String>,
    #[serde(default)]
    ahu_version: Option<String>,
    #[serde(default)]
    ahu_build_digest: Option<String>,
    #[serde(default)]
    target_repo_head: Option<String>,
    #[serde(default)]
    input_fingerprint: Option<String>,
    #[serde(default)]
    fingerprint_completeness: Option<String>,
    /// The deterministic answer score, separate from any judge's opinion of it.
    #[serde(default)]
    answer_score: Option<f64>,
    #[serde(default)]
    answer_passed: Option<bool>,
    #[serde(default)]
    judge_status: Option<String>,
    #[serde(default)]
    judge_score: Option<f64>,
    #[serde(default)]
    judge_passed: Option<bool>,
    #[serde(default)]
    judge_criterion_scores: Option<BTreeMap<String, f64>>,
    #[serde(default)]
    judge_reason_codes: Option<Vec<String>>,
    #[serde(default)]
    tool_expectation_status: Option<String>,
    #[serde(default)]
    telemetry_coverage: Option<String>,
    #[serde(default)]
    terminal_status: Option<String>,
    #[serde(default)]
    attempts: Option<f64>,
    /// Whether a valid answer existed to score at all. `scored` or `no_answer`.
    ///
    /// Absent in a record written before this field existed, so the reader falls
    /// back to the terminal status, which has always said the same thing.
    #[serde(default)]
    answer_status: Option<String>,
    /// Wall-clock milliseconds the runner spent on the candidate launch.
    ///
    /// Collected by the runner rather than the harness, so it survives a launch
    /// that failed or timed out and never reached the telemetry receiver.
    #[serde(default)]
    launch_elapsed_ms: Option<f64>,
    #[serde(default)]
    trajectory: Option<trajectory::Observation>,
    #[serde(default)]
    trajectory_budget_status: Option<trajectory::BudgetStatus>,
}

/// Terminal statuses that mean no valid answer existed to score.
///
/// Shared by the runner that writes a record and the report that reads one, so
/// the two cannot drift on what counts as a failed attempt.
pub fn is_no_answer_status(status: &str) -> bool {
    matches!(
        status,
        "candidate_launch_failed" | "candidate_timed_out" | "candidate_cancelled"
    ) || status.starts_with("candidate_answer_")
}

impl Record {
    /// Whether this run's answer passed, as a binary outcome.
    fn answer_pass(&self) -> bool {
        self.answer_passed.unwrap_or(self.passed)
    }

    /// Whether this run produced a valid answer for the scorer to judge.
    ///
    /// A launch that failed, timed out, or produced no usable `answer.json` did
    /// not answer wrongly; it did not answer. The two have to stay apart, so the
    /// quality rate below is taken only over the runs this returns true for.
    fn answer_scored(&self) -> bool {
        match self.answer_status.as_deref() {
            Some("scored") => true,
            Some("no_answer") => false,
            // Older records, and manually recorded rows, say the same thing
            // through the terminal status.
            _ => !is_no_answer_status(self.terminal_status.as_deref().unwrap_or(UNSPECIFIED)),
        }
    }

    fn tool_status(&self) -> ToolExpectationStatus {
        match self.tool_expectation_status.as_deref() {
            Some("pass") => ToolExpectationStatus::Pass,
            Some("fail") => ToolExpectationStatus::Fail,
            Some("unknown") => ToolExpectationStatus::Unknown,
            // A record that says nothing about tool expectations stated none.
            _ => ToolExpectationStatus::NotApplicable,
        }
    }
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
    pub evaluator_kind: String,
    pub decision_evaluator_policy_digest: String,
    pub decision_evaluator_policy: String,
    pub evaluator_version: String,
    pub evaluator_model: String,
    pub evaluator_harness: String,
    pub evaluator_harness_version: String,
    pub model: String,
    pub harness: String,
    pub harness_version: String,
    pub skill_digest: String,
    pub selection_policy_digest: String,
    pub selection_mode: String,
    pub case_schema_version: String,
    pub case_digest: String,
    pub prompt_profile: String,
    pub prompt_version: String,
    pub scoring_version: String,
    pub suite_id: String,
    pub suite_version: String,
    pub suite_digest: String,
    pub agent_identity_digest: String,
    pub evaluator_identity_digest: String,
    pub evaluator_skill_digest: String,
    pub evaluator_repo_head: String,
    pub blinding: String,
    pub tool_definitions_digest: String,
    pub ahu_version: String,
    pub ahu_build_digest: String,
    pub target_repo_head: String,
    /// `complete` or `partial`. Part of the key, so a run whose inputs are only
    /// partly known is never averaged with runs whose inputs are fully known.
    pub fingerprint_completeness: String,
    /// The runner's own digest over every input above. Grouping by the fields
    /// individually is what tells a reader *which* input differed; this is the
    /// short form, and a mismatch against the fields is itself visible.
    pub input_fingerprint: String,
}

/// How many runs in a group carried each kind of observation.
///
/// Kept apart from the measurements themselves: `decision_calls: 0` over four
/// runs means four runs reported no count, not four runs that made no call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Coverage {
    pub tokens: usize,
    pub timing: usize,
    /// Runs that carried the runner's own launch wall-clock time. Collected
    /// outside the harness, so a failed or timed-out attempt still has one.
    pub launch_timing: usize,
    pub decision_calls: usize,
    pub mcp: usize,
    /// Runs whose telemetry carried the MCP session summary.
    pub telemetry_complete: usize,
    /// Runs with spans but no session summary: counts there are a floor.
    pub telemetry_partial: usize,
    /// Runs for which no telemetry arrived at all.
    pub telemetry_none: usize,
    /// Runs that recorded local OTLP receiver drop/error counters.
    pub telemetry_receiver: usize,
}

/// One comparable configuration.
#[derive(Debug, Clone)]
pub struct Group {
    pub trajectory: trajectory::Summary,
    pub key: GroupKey,
    pub runs: usize,
    pub passes: usize,
    /// Mean over trials that produced a score; failed/unscored trials are excluded.
    pub mean_score: Option<f64>,
    pub score_observations: usize,
    pub pass_rate: f64,
    pub coverage: Coverage,
    /// Receiver-level export drops/errors observed across runs with counters.
    pub telemetry_receiver: Option<crate::eval_otel::ReceiverStats>,
    /// Mean over the runs that reported a time; `None` when none did.
    pub mean_elapsed_ms: Option<f64>,
    /// Mean launch wall-clock time over the runs that reported one, failed and
    /// timed-out attempts included.
    pub mean_launch_elapsed_ms: Option<f64>,
    pub mean_total_elapsed_ms: Option<f64>,
    pub evaluator_metrics: decision::Summary,
    pub mean_selection_elapsed_ms: Option<f64>,
    pub selection_fallbacks: usize,
    pub selection_telemetry_observations: usize,
    pub mean_selection_input_tokens: Option<f64>,
    pub mean_selection_output_tokens: Option<f64>,
    pub selection_input_complete_runs: usize,
    pub selection_output_complete_runs: usize,
    pub selection_input_partial_runs: usize,
    pub selection_output_partial_runs: usize,
    pub mean_selection_partial_input_tokens: Option<f64>,
    pub mean_selection_partial_output_tokens: Option<f64>,
    pub selection_reported_models: BTreeMap<String, usize>,
    pub mean_candidate_selection: BTreeMap<String, f64>,
    pub candidate_selection_runs: usize,
    /// Mean over the runs that reported a count; `None` when none did.
    pub mean_decision_calls: Option<f64>,
    /// Mean sum of successful typed-decision service durations per run.
    pub mean_decision_service_duration_ms: Option<f64>,
    /// Mean measured MCP argument byte sum per run, not tokens or provider bytes.
    pub mean_decision_request_bytes: Option<f64>,
    pub decision_request_runs: usize,
    pub decision_request_observations: u64,
    /// Every token metric name observed anywhere in the group.
    pub token_fields: BTreeSet<String>,
    /// Mean reported amount per token field, over the runs that reported a
    /// number for it. A field a recorder named but could not measure has no
    /// mean here, and nothing in this report derives one.
    pub mean_tokens: BTreeMap<String, f64>,
    /// How many runs reported an amount for each token field, so a mean is
    /// read against the sample it was taken over rather than the run count.
    pub token_field_observations: BTreeMap<String, usize>,
    /// Harness-reported USD fields, kept separate from token amounts.
    pub reported_cost_fields: BTreeSet<String>,
    pub mean_reported_cost_usd: BTreeMap<String, f64>,
    pub reported_cost_observations: BTreeMap<String, usize>,
    pub mean_mcp_requests: Option<f64>,
    pub mean_mcp_tool_lists: Option<f64>,
    pub mean_mcp_tool_calls: Option<f64>,
    pub mean_mcp_tool_errors: Option<f64>,
    pub mean_typed_decision_errors: Option<f64>,
    pub mcp_tools: BTreeMap<String, f64>,
    pub mcp_tool_errors: BTreeMap<String, f64>,
    /// Answer reliability: passes over *every* attempt in the group, with a 95%
    /// Wilson interval. A failed launch counts against this, because a
    /// configuration that cannot produce an answer is not a reliable one.
    ///
    /// This is the deterministic answer check, not a judge's opinion of it.
    pub answer_passes: usize,
    pub answer_pass_rate: f64,
    pub answer_pass_interval: Option<stats::Interval>,
    /// Attempts that produced a valid answer at all, and the passes among them.
    ///
    /// Answer *quality* is this pair: how often a real answer was right. It is
    /// kept apart from reliability above so a failed launch is never reported as
    /// a wrong answer, and there is no interval when nothing was answered --
    /// an unanswered sample is not a quality of zero.
    pub answer_observations: usize,
    pub answer_quality_passes: usize,
    pub answer_quality_rate: Option<f64>,
    pub answer_quality_interval: Option<stats::Interval>,
    /// Attempts that produced no valid answer. The complement of
    /// `answer_observations`, named because it is what a reader of the
    /// conditional latency and token means has to see beside them.
    pub attempts_without_answer: usize,
    /// Tool-expectation outcomes. The rate and interval are taken over the
    /// decided runs only, so `unknown` never counts as either a pass or a fail.
    pub tool_pass: usize,
    pub tool_fail: usize,
    pub tool_unknown: usize,
    pub tool_not_applicable: usize,
    pub tool_pass_rate: Option<f64>,
    pub tool_pass_interval: Option<stats::Interval>,
    /// Judge evidence, kept apart from the deterministic score above.
    pub judge_scored: usize,
    pub judge_failed: usize,
    pub judge_passes: usize,
    pub mean_judge_score: Option<f64>,
    pub mean_answer_score: Option<f64>,
    /// Every criterion the judge scored anywhere in the group, with its mean.
    pub judge_criterion_means: BTreeMap<String, f64>,
    /// Every reason code the judge emitted anywhere in the group, with a count.
    pub judge_reason_codes: BTreeMap<String, usize>,
    pub mean_attempts: Option<f64>,
    /// How each run in the group ended, counted by terminal status. A group with
    /// failures is visibly a group with failures rather than a lower mean.
    pub terminal_statuses: BTreeMap<String, usize>,
}

impl Group {
    /// The mean total tokens, when a recorder reported a total.
    ///
    /// Only a field the recorder itself called a total counts. Adding the
    /// input and output fields that happened to be reported would be an
    /// estimate, and this report makes none.
    pub fn mean_total_tokens(&self) -> Option<f64> {
        self.mean_tokens
            .get("ahu.tokens.total")
            .or_else(|| self.mean_tokens.get("total"))
            .copied()
    }
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

use stats::{mean, round4};

/// The amount a recorder reported for one token field, if it reported one.
///
/// Two shapes are in use. `ahu eval run` writes the local-metrics measurement,
/// `{"kind": "observed", "value": n}`, whose `unavailable` form is a field the
/// harness named but never measured; a helper that projects only what it saw
/// writes the bare number. Anything else is not an amount, and nothing here
/// turns an absent one into a zero.
fn token_amount(value: &serde_json::Value) -> Option<f64> {
    let amount = match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::Object(fields) => {
            if matches!(
                fields.get("kind").and_then(serde_json::Value::as_str),
                Some("observed" | "observed_float")
            ) {
                fields.get("value").and_then(serde_json::Value::as_f64)
            } else {
                None
            }
        }
        _ => None,
    };
    amount.filter(|amount| amount.is_finite() && *amount >= 0.0)
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

fn observed_token(value: &serde_json::Value) -> bool {
    let number = value.as_f64().or_else(|| {
        (matches!(
            value.get("kind").and_then(serde_json::Value::as_str),
            Some("observed" | "observed_float")
        ))
        .then(|| value.get("value").and_then(serde_json::Value::as_f64))
        .flatten()
    });
    number.is_some_and(|number| number.is_finite() && number >= 0.0)
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
        if record.schema_version != RECORD_SCHEMA_VERSION {
            return Err(malformed_text(
                path,
                number,
                &format!(
                    "record schema_version {} is unsupported; expected {RECORD_SCHEMA_VERSION}",
                    record.schema_version
                ),
            ));
        }
        if record.candidate_selection.iter().any(|(key, v)| {
            ![
                "observations",
                "duration_ms_known",
                "input_tokens_known",
                "output_tokens_known",
                "input_complete_observations",
                "output_complete_observations",
            ]
            .contains(&key.as_str())
                || !v.is_finite()
                || *v < 0.0
        }) {
            return Err(malformed_text(
                path,
                number,
                "invalid candidate selection observations",
            ));
        }
        if record.skill_selection.as_ref().is_some_and(|s| {
            !["disabled", "suggested", "abstained", "fallback"].contains(&s.status.as_str())
                || s.candidate_count > 40
                || s.selected.len() > 3
                || s.service.as_ref().is_some_and(|service| {
                    ["prompt_tokens", "generated_tokens"].iter().any(|key| {
                        service
                            .get(key)
                            .is_some_and(|v| !v.is_null() && v.as_u64().is_none())
                    })
                })
        }) {
            return Err(malformed_text(
                path,
                number,
                "invalid skill selection observations",
            ));
        }
        if record
            .evaluator_metrics
            .as_ref()
            .is_some_and(|m| !m.validate())
        {
            return Err(malformed_text(
                path,
                number,
                "invalid evaluator observations",
            ));
        }
        if record.decision_request_bytes.is_some() != (record.decision_request_observations > 0)
            || record.score.is_some_and(|score| !score.is_finite())
            || [
                record.elapsed_ms,
                record.total_elapsed_ms,
                record.evaluation_elapsed_ms,
                record.skill_selection.as_ref().map(|s| s.elapsed_ms),
                record.decision_service_duration_ms,
                record.decision_call_count,
                record.mcp_request_count,
                record.mcp_tool_list_count,
                record.mcp_tool_call_count,
                record.mcp_tool_error_count,
                record.typed_decision_error_count,
            ]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite() || value < 0.0)
            || [&record.mcp_tools, &record.mcp_tool_errors_by_name]
                .into_iter()
                .flatten()
                .any(|tools| {
                    tools.len() > crate::mcp::TOOL_NAMES.len()
                        || tools.iter().any(|(name, count)| {
                            !crate::mcp::TOOL_NAMES.contains(&name.as_str())
                                || !count.is_finite()
                                || *count < 0.0
                        })
                })
            || [record.answer_score, record.judge_score]
                .into_iter()
                .flatten()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
            || record
                .judge_criterion_scores
                .as_ref()
                .is_some_and(|scores| {
                    scores.len() > 64
                        || scores
                            .values()
                            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
                })
            || record.judge_reason_codes.as_ref().is_some_and(|codes| {
                codes.len() > 32
                    || codes.iter().any(|code| {
                        code.is_empty()
                            || code.len() > 64
                            || !code
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    })
            })
            || [record.attempts, record.launch_elapsed_ms]
                .into_iter()
                .flatten()
                .any(|value| !value.is_finite() || value < 0.0)
            || record
                .answer_status
                .as_deref()
                .is_some_and(|status| !["scored", "no_answer"].contains(&status))
            // A reported token amount is a count. A non-numeric value stays
            // tolerated -- it is simply not an amount -- but a number that is
            // not a count is bad input rather than a metric to average.
            || record.reported_tokens.as_ref().is_some_and(|tokens| {
                tokens.values().any(|value| {
                    value.as_f64().is_some_and(|amount| !amount.is_finite() || amount < 0.0)
                        || value
                            .get("value")
                            .and_then(serde_json::Value::as_f64)
                            .is_some_and(|amount| !amount.is_finite() || amount < 0.0)
                })
            })
        {
            return Err(malformed_text(
                path,
                number,
                "scores and observed metrics must be finite, non-negative values for supported MCP tools",
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
    let number = |value: Option<u64>| {
        value.map_or_else(|| UNSPECIFIED.to_owned(), |value| value.to_string())
    };
    GroupKey {
        case_id: record.case_id.clone(),
        corpus_version: or_unspecified(&record.corpus_version, UNSPECIFIED),
        stage: or_unspecified(&record.stage, DEFAULT_STAGE),
        agent: or_unspecified(&record.agent, UNSPECIFIED),
        agent_version: or_unspecified(&record.agent_version, UNSPECIFIED),
        evaluator: or_unspecified(&record.evaluator, UNSPECIFIED),
        evaluator_kind: or_unspecified(&record.evaluator_kind, UNSPECIFIED),
        decision_evaluator_policy: record
            .decision_evaluator_policy
            .as_ref()
            .map_or_else(|| "null".into(), |p| p.to_string()),
        decision_evaluator_policy_digest: or_unspecified(
            &record.decision_evaluator_policy_digest,
            UNSPECIFIED,
        ),
        evaluator_version: or_unspecified(&record.evaluator_version, UNSPECIFIED),
        evaluator_model: or_unspecified(&record.evaluator_model, UNSPECIFIED),
        evaluator_harness: or_unspecified(&record.evaluator_harness, UNSPECIFIED),
        evaluator_harness_version: or_unspecified(&record.evaluator_harness_version, UNSPECIFIED),
        model: record.model.clone(),
        harness: record.harness.clone(),
        harness_version: or_unspecified(&record.harness_version, UNSPECIFIED),
        skill_digest: or_unspecified(&record.skill_digest, UNSPECIFIED),
        selection_policy_digest: or_unspecified(&record.selection_policy_digest, "none"),
        selection_mode: record
            .skill_selection
            .as_ref()
            .map_or("none", |s| s.mode.as_str())
            .into(),
        case_schema_version: number(record.case_schema_version),
        case_digest: or_unspecified(&record.case_digest, UNSPECIFIED),
        prompt_profile: or_unspecified(&record.prompt_profile, UNSPECIFIED),
        prompt_version: number(record.prompt_version),
        scoring_version: number(record.scoring_version),
        suite_id: or_unspecified(&record.suite_id, "none"),
        suite_version: or_unspecified(&record.suite_version, "none"),
        suite_digest: or_unspecified(&record.suite_digest, "none"),
        agent_identity_digest: or_unspecified(&record.agent_identity_digest, UNSPECIFIED),
        evaluator_identity_digest: or_unspecified(&record.evaluator_identity_digest, UNSPECIFIED),
        evaluator_skill_digest: or_unspecified(&record.evaluator_skill_digest, UNSPECIFIED),
        evaluator_repo_head: or_unspecified(&record.evaluator_repo_head, UNSPECIFIED),
        blinding: or_unspecified(&record.blinding, UNSPECIFIED),
        tool_definitions_digest: or_unspecified(&record.tool_definitions_digest, UNSPECIFIED),
        ahu_version: or_unspecified(&record.ahu_version, UNSPECIFIED),
        ahu_build_digest: or_unspecified(&record.ahu_build_digest, UNSPECIFIED),
        target_repo_head: or_unspecified(&record.target_repo_head, UNSPECIFIED),
        fingerprint_completeness: or_unspecified(&record.fingerprint_completeness, UNSPECIFIED),
        input_fingerprint: or_unspecified(&record.input_fingerprint, UNSPECIFIED),
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
            let scores: Vec<f64> = items.iter().filter_map(|item| item.score).collect();
            let mut coverage = Coverage::default();
            let mut token_fields = BTreeSet::new();
            let mut token_amounts: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            let mut reported_cost_fields = BTreeSet::new();
            let mut reported_cost_amounts: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            let mut elapsed = Vec::new();
            let mut calls = Vec::new();
            let mut decision_service_durations = Vec::new();
            let mut decision_request_bytes = Vec::new();
            let mut decision_request_observations = 0u64;
            let mut mcp_requests = Vec::new();
            let mut mcp_tool_lists = Vec::new();
            let mut mcp_tool_calls = Vec::new();
            let mut mcp_tool_errors = Vec::new();
            let mut typed_decision_errors = Vec::new();
            let mut mcp_tools: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            let mut mcp_tool_errors_by_name: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            let mut answer_passes = 0usize;
            let mut answer_observations = 0usize;
            let mut answer_quality_passes = 0usize;
            let mut answer_scores = Vec::new();
            let mut launch_elapsed = Vec::new();
            let mut total_elapsed = Vec::new();
            let mut selection_elapsed = Vec::new();
            let mut selection_inputs = Vec::new();
            let mut selection_outputs = Vec::new();
            let mut selection_partial_inputs = Vec::new();
            let mut selection_partial_outputs = Vec::new();
            let mut selection_reported_models = BTreeMap::new();
            let mut candidate_selection_values: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            let mut candidate_selection_runs = 0;
            let mut selection_fallbacks = 0;
            let mut selection_telemetry_observations = 0;
            let mut judge_scores = Vec::new();
            let mut judge_passes = 0usize;
            let mut judge_failed = 0usize;
            let mut criterion_scores: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            let mut reason_codes: BTreeMap<String, usize> = BTreeMap::new();
            let mut attempts = Vec::new();
            let mut tool_pass = 0usize;
            let mut tool_fail = 0usize;
            let mut tool_unknown = 0usize;
            let mut tool_not_applicable = 0usize;
            let mut terminal_statuses: BTreeMap<String, usize> = BTreeMap::new();
            let mut telemetry_receiver = crate::eval_otel::ReceiverStats::default();
            for item in &items {
                if let Some(value) = item.total_elapsed_ms {
                    total_elapsed.push(value);
                }
                if item.selection_telemetry_observed == Some(true) {
                    selection_telemetry_observations += 1;
                }
                if !item.candidate_selection.is_empty() {
                    candidate_selection_runs += 1;
                    for (key, value) in &item.candidate_selection {
                        candidate_selection_values
                            .entry(key.clone())
                            .or_default()
                            .push(*value);
                    }
                }
                if let Some(selection) = &item.skill_selection {
                    selection_elapsed.push(selection.elapsed_ms);
                    selection_fallbacks += usize::from(selection.status == "fallback");
                    if let Some(service) = &selection.service {
                        if service
                            .get("model_reported")
                            .and_then(serde_json::Value::as_bool)
                            == Some(true)
                            && let Some(model) =
                                service.get("model").and_then(serde_json::Value::as_str)
                        {
                            *selection_reported_models
                                .entry(model.to_owned())
                                .or_insert(0) += 1;
                        }

                        if let Some(v) = service
                            .get("prompt_tokens")
                            .and_then(serde_json::Value::as_f64)
                        {
                            if service["prompt_tokens_complete"] == true {
                                selection_inputs.push(v);
                            } else {
                                selection_partial_inputs.push(v);
                            }
                        }
                        if let Some(v) = service
                            .get("generated_tokens")
                            .and_then(serde_json::Value::as_f64)
                        {
                            if service["generated_tokens_complete"] == true {
                                selection_outputs.push(v);
                            } else {
                                selection_partial_outputs.push(v);
                            }
                        }
                    }
                }

                let status = item.terminal_status.as_deref().unwrap_or(UNSPECIFIED);
                *terminal_statuses.entry(status.to_owned()).or_default() += 1;
                let scored = item.answer_scored();
                if scored {
                    answer_observations += 1;
                }
                if item.answer_pass() {
                    answer_passes += 1;
                    if scored {
                        answer_quality_passes += 1;
                    }
                }
                if let Some(score) = item.answer_score {
                    answer_scores.push(score);
                }
                if let Some(score) = item.judge_score {
                    judge_scores.push(score);
                }
                if item.judge_passed == Some(true) {
                    judge_passes += 1;
                }
                // A judge that failed is recorded as a judge failure, never as a
                // candidate failure and never as a score of zero.
                if item.judge_status.as_deref() == Some("failed") {
                    judge_failed += 1;
                }
                for (criterion, score) in item.judge_criterion_scores.iter().flatten() {
                    criterion_scores
                        .entry(criterion.clone())
                        .or_default()
                        .push(*score);
                }
                for code in item.judge_reason_codes.iter().flatten() {
                    *reason_codes.entry(code.clone()).or_default() += 1;
                }
                if let Some(count) = item.attempts {
                    attempts.push(count);
                }
                match item.tool_status() {
                    ToolExpectationStatus::Pass => tool_pass += 1,
                    ToolExpectationStatus::Fail => tool_fail += 1,
                    ToolExpectationStatus::Unknown => tool_unknown += 1,
                    ToolExpectationStatus::NotApplicable => tool_not_applicable += 1,
                }
                if let Some(bytes) = item.decision_request_bytes {
                    decision_request_bytes.push(bytes as f64);
                    decision_request_observations = decision_request_observations
                        .saturating_add(item.decision_request_observations);
                }
                match item.telemetry_coverage.as_deref() {
                    Some("complete_session") => coverage.telemetry_complete += 1,
                    Some("partial_spans") => coverage.telemetry_partial += 1,
                    Some("none") => coverage.telemetry_none += 1,
                    // A record that reported no coverage at all: the MCP flag is
                    // the only evidence there is.
                    _ => {
                        if item.mcp_observed == Some(true) {
                            coverage.telemetry_complete += 1;
                        } else {
                            coverage.telemetry_none += 1;
                        }
                    }
                }
                for (name, count) in item.mcp_tool_errors_by_name.iter().flatten() {
                    mcp_tool_errors_by_name
                        .entry(name.clone())
                        .or_default()
                        .push(*count);
                }
                if let Some(tokens) = &item.reported_tokens {
                    token_fields.extend(
                        tokens
                            .keys()
                            .filter(|name| !name.starts_with("ahu.cost."))
                            .cloned(),
                    );
                    reported_cost_fields.extend(
                        tokens
                            .keys()
                            .filter(|name| name.starts_with("ahu.cost."))
                            .cloned(),
                    );
                    let observed: Vec<_> = tokens
                        .iter()
                        .filter(|(name, _)| !name.starts_with("ahu.cost."))
                        .filter_map(|(name, value)| observed_token(value).then_some(name.clone()))
                        .collect();
                    if !observed.is_empty() {
                        coverage.tokens += 1;
                    }
                    for (field, value) in tokens {
                        // A field named but not measured is not an amount of
                        // zero, so only a reported number joins the mean.
                        if let Some(amount) = token_amount(value) {
                            if field.starts_with("ahu.cost.") {
                                reported_cost_amounts
                                    .entry(field.clone())
                                    .or_default()
                                    .push(amount);
                            } else {
                                token_amounts.entry(field.clone()).or_default().push(amount);
                            }
                        }
                    }
                }
                // A reported zero is an observation; only an absent field is not.
                if let Some(value) = item.elapsed_ms {
                    coverage.timing += 1;
                    elapsed.push(value);
                }
                if let Some(value) = item.launch_elapsed_ms {
                    coverage.launch_timing += 1;
                    launch_elapsed.push(value);
                }
                if let Some(value) = item.decision_call_count {
                    coverage.decision_calls += 1;
                    calls.push(value);
                }
                if let Some(value) = item.decision_service_duration_ms {
                    decision_service_durations.push(value);
                }
                if item.mcp_observed == Some(true) {
                    coverage.mcp += 1;
                    if let Some(value) = item.mcp_request_count {
                        mcp_requests.push(value);
                    }
                    if let Some(value) = item.mcp_tool_list_count {
                        mcp_tool_lists.push(value);
                    }
                    if let Some(value) = item.mcp_tool_call_count {
                        mcp_tool_calls.push(value);
                    }
                    if let Some(value) = item.mcp_tool_error_count {
                        mcp_tool_errors.push(value);
                    }
                    if let Some(value) = item.typed_decision_error_count {
                        typed_decision_errors.push(value);
                    }
                    if let Some(tools) = &item.mcp_tools {
                        let named_calls: f64 = tools.values().sum();
                        let all_calls_named = item
                            .mcp_tool_call_count
                            .is_some_and(|total| total == named_calls);
                        for name in crate::mcp::TOOL_NAMES {
                            if let Some(count) = tools.get(name) {
                                mcp_tools.entry(name.to_owned()).or_default().push(*count);
                            } else if all_calls_named {
                                mcp_tools.entry(name.to_owned()).or_default().push(0.0);
                            }
                        }
                    }
                }
                if let Some(stats) = item.telemetry_receiver {
                    coverage.telemetry_receiver += 1;
                    telemetry_receiver.add(stats);
                }
            }
            let tool_decided = tool_pass + tool_fail;
            let token_field_observations = token_amounts
                .iter()
                .map(|(field, amounts)| (field.clone(), amounts.len()))
                .collect();
            Group {
                trajectory: trajectory::Summary::collect(
                    items
                        .iter()
                        .map(|r| (r.trajectory.as_ref(), r.trajectory_budget_status)),
                ),
                key,
                runs,
                passes,
                mean_score: mean(&scores),
                score_observations: scores.len(),
                pass_rate: round4(passes as f64 / runs as f64),
                coverage,
                telemetry_receiver: (coverage.telemetry_receiver > 0).then_some(telemetry_receiver),
                mean_elapsed_ms: mean(&elapsed),
                mean_launch_elapsed_ms: mean(&launch_elapsed),
                mean_total_elapsed_ms: mean(&total_elapsed),
                evaluator_metrics: decision::Summary::collect(
                    items
                        .iter()
                        .map(|r| (r.evaluator_metrics.as_ref(), r.evaluation_elapsed_ms)),
                ),
                mean_selection_elapsed_ms: mean(&selection_elapsed),
                selection_fallbacks,
                selection_telemetry_observations,
                mean_selection_input_tokens: mean(&selection_inputs),
                mean_selection_output_tokens: mean(&selection_outputs),
                selection_input_complete_runs: selection_inputs.len(),
                selection_output_complete_runs: selection_outputs.len(),
                selection_input_partial_runs: selection_partial_inputs.len(),
                selection_output_partial_runs: selection_partial_outputs.len(),
                mean_selection_partial_input_tokens: mean(&selection_partial_inputs),
                mean_selection_partial_output_tokens: mean(&selection_partial_outputs),
                selection_reported_models,
                mean_candidate_selection: candidate_selection_values
                    .into_iter()
                    .filter_map(|(k, v)| mean(&v).map(|m| (k, m)))
                    .collect(),
                candidate_selection_runs,
                mean_decision_calls: mean(&calls),
                mean_decision_service_duration_ms: mean(&decision_service_durations),
                mean_decision_request_bytes: mean(&decision_request_bytes),
                decision_request_runs: decision_request_bytes.len(),
                decision_request_observations,
                token_fields,
                mean_tokens: token_amounts
                    .into_iter()
                    .filter_map(|(field, amounts)| mean(&amounts).map(|value| (field, value)))
                    .collect(),
                token_field_observations,
                reported_cost_observations: reported_cost_amounts
                    .iter()
                    .map(|(field, amounts)| (field.clone(), amounts.len()))
                    .collect(),
                reported_cost_fields,
                mean_reported_cost_usd: reported_cost_amounts
                    .into_iter()
                    .filter_map(|(field, amounts)| mean(&amounts).map(|value| (field, value)))
                    .collect(),
                mean_mcp_requests: mean(&mcp_requests),
                mean_mcp_tool_lists: mean(&mcp_tool_lists),
                mean_mcp_tool_calls: mean(&mcp_tool_calls),
                mean_mcp_tool_errors: mean(&mcp_tool_errors),
                mean_typed_decision_errors: mean(&typed_decision_errors),
                mcp_tools: mcp_tools
                    .into_iter()
                    .filter_map(|(name, values)| mean(&values).map(|value| (name, value)))
                    .collect(),
                mcp_tool_errors: mcp_tool_errors_by_name
                    .into_iter()
                    .filter_map(|(name, values)| mean(&values).map(|value| (name, value)))
                    .collect(),
                answer_passes,
                answer_pass_rate: round4(answer_passes as f64 / runs as f64),
                answer_pass_interval: stats::Interval::wilson(answer_passes, runs),
                answer_observations,
                answer_quality_passes,
                // Over the answered attempts only, and absent when nothing was
                // answered: no answer is not a wrong answer.
                answer_quality_rate: (answer_observations > 0)
                    .then(|| round4(answer_quality_passes as f64 / answer_observations as f64)),
                answer_quality_interval: stats::Interval::wilson(
                    answer_quality_passes,
                    answer_observations,
                ),
                attempts_without_answer: runs - answer_observations,
                tool_pass,
                tool_fail,
                tool_unknown,
                tool_not_applicable,
                // Taken over the decided runs, so an undecidable run neither
                // lifts nor lowers the rate.
                tool_pass_rate: (tool_decided > 0)
                    .then(|| round4(tool_pass as f64 / tool_decided as f64)),
                tool_pass_interval: stats::Interval::wilson(tool_pass, tool_decided),
                judge_scored: judge_scores.len(),
                judge_failed,
                judge_passes,
                mean_judge_score: mean(&judge_scores),
                mean_answer_score: mean(&answer_scores),
                judge_criterion_means: criterion_scores
                    .into_iter()
                    .filter_map(|(name, values)| mean(&values).map(|value| (name, value)))
                    .collect(),
                judge_reason_codes: reason_codes,
                mean_attempts: mean(&attempts),
                terminal_statuses,
            }
        })
        .collect()
}

/// The terminal view: a comparison table, then one block per configuration.
pub fn render(report: &Report) -> String {
    render_at(report, table::columns())
}

/// The terminal view, laid out for an explicit width.
pub fn render_at(report: &Report, width: usize) -> String {
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
            "\nNo run records. Append one with `ahu eval run`.\n",
        ));
        return out;
    }
    if report.groups.iter().any(|group| {
        matches!(
            group.key.evaluator_kind.as_str(),
            "agent" | "typed_decision"
        )
    }) {
        out.push_str("  TOKENS: candidate native usage; grading usage below.\n");
    }
    out.push('\n');
    out.push_str(&table::render(
        style,
        width,
        SUMMARY_COLUMNS,
        &summary_rows(&report.groups),
    ));
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
            "  agent      {} {}\n  evaluator  {} {} ({}, {} {})\n  model      {}\n  harness    {} {}\n  ahu        {}  skill digest {}\n",
            style.paint(Role::Agent, &display_safe(&key.agent)),
            display_safe(&key.agent_version),
            display_safe(&key.evaluator),
            display_safe(&key.evaluator_version),
            display_safe(&key.evaluator_model),
            display_safe(&key.evaluator_harness),
            display_safe(&key.evaluator_harness_version),
            style.paint(Role::Runtime, &display_safe(&key.model)),
            display_safe(&key.harness),
            display_safe(&key.harness_version),
            display_safe(&key.ahu_version),
            display_safe(&key.skill_digest)
        ));
        let trajectory = &group.trajectory;
        out.push_str(&format!(
            "  trajectory missing {}/{} runs; coverage {}; sources {}; tracking limited {}\n",
            trajectory.missing_runs,
            group.runs,
            serde_json::to_string(&trajectory.coverage).unwrap_or_default(),
            serde_json::to_string(&trajectory.sources).unwrap_or_default(),
            trajectory.limited_runs
        ));
        for name in [
            "steps",
            "tool_calls",
            "tool_errors",
            "repeated_tool_calls",
            "repeated_tool_errors",
            "tool_recoveries",
        ] {
            out.push_str(&format!(
                "             {name} mean {} ({}/{} observations)\n",
                measurement(trajectory.means.get(name).copied()),
                trajectory.observations.get(name).copied().unwrap_or(0),
                group.runs
            ));
        }
        out.push_str(&format!(
            "             budgets {}\n",
            serde_json::to_string(&trajectory.budget_statuses).unwrap_or_default()
        ));
        if matches!(key.evaluator_kind.as_str(), "agent" | "typed_decision") {
            let metrics = &group.evaluator_metrics;
            out.push_str(&format!(
                "  grading    {} policy {} configured {}\n             status {}; OTel {}; provider calls {}\n             provider complete input/output {}/{} of {} runs; reported services {}\n",
                display_safe(&key.evaluator_kind),
                display_safe(&key.decision_evaluator_policy_digest),
                display_safe(&key.decision_evaluator_policy),
                display_safe(&serde_json::to_string(&metrics.statuses).unwrap_or_default()),
                display_safe(&serde_json::to_string(&metrics.telemetry_coverage).unwrap_or_default()),
                metrics.calls_attempted,
                metrics.provider_input_complete_runs, metrics.provider_output_complete_runs, group.runs,
                display_safe(&serde_json::to_string(&metrics.reported_service_identities).unwrap_or_default()),
            ));
            for (metric, mean) in &metrics.means {
                out.push_str(&format!(
                    "             {} mean {:.2} ({}/{} observations)\n",
                    display_safe(metric),
                    mean,
                    metrics.observations[metric],
                    group.runs
                ));
            }
        }
        if key.selection_mode != "none" {
            out.push_str(&format!(
                "  selection  {}  {:.1} ms; total preparation + launch {:.1} ms; fallbacks {}; OTel {}/{}\n",
                display_safe(&key.selection_mode),
                group.mean_selection_elapsed_ms.unwrap_or_default(),
                group.mean_total_elapsed_ms.unwrap_or_default(),
                group.selection_fallbacks, group.selection_telemetry_observations, group.runs
            ));
            out.push_str(&format!(
                "             service input/output {:?}/{:?}; complete runs {}/{}; partial known {:?}/{:?} over {}/{} runs\n",
                group.mean_selection_input_tokens, group.mean_selection_output_tokens,
                group.selection_input_complete_runs, group.selection_output_complete_runs,
                group.mean_selection_partial_input_tokens, group.mean_selection_partial_output_tokens,
                group.selection_input_partial_runs, group.selection_output_partial_runs,
            ));
            out.push_str(&format!(
                "             reported models {}\n",
                display_safe(
                    &serde_json::to_string(&group.selection_reported_models).unwrap_or_default()
                )
            ));
        }
        if group.candidate_selection_runs > 0 {
            out.push_str(&format!("  MCP advice known observations {} ({}/{} runs); partial usage remains a subtotal\n",
                display_safe(&serde_json::to_string(&group.mean_candidate_selection).unwrap_or_default()),
                group.candidate_selection_runs, group.runs));
        }
        out.push_str(&format!(
            "  inputs     case {}  prompt {} v{}  scoring v{}  suite {} {}\n                          agent id {}  evaluator id {}  blinding {}\n                          tools {}  ahu {} build {}  target HEAD {}  fingerprint {} ({})\n",
            display_safe(&key.case_digest),
            display_safe(&key.prompt_profile),
            display_safe(&key.prompt_version),
            display_safe(&key.scoring_version),
            display_safe(&key.suite_id),
            display_safe(&key.suite_version),
            display_safe(&key.agent_identity_digest),
            display_safe(&key.evaluator_identity_digest),
            display_safe(&key.blinding),
            display_safe(&key.tool_definitions_digest),
            display_safe(&key.ahu_version),
            display_safe(&key.ahu_build_digest),
            display_safe(&key.target_repo_head),
            display_safe(&key.input_fingerprint),
            if key.fingerprint_completeness == "complete" {
                display_safe(&key.fingerprint_completeness)
            } else {
                style.paint(Role::Gap, &display_safe(&key.fingerprint_completeness))
            }
        ));
        out.push_str(&format!(
            "  runs {}  mean score {}  pass rate {:.4} ({}/{})\n",
            group.runs,
            group
                .mean_score
                .map_or_else(|| "unscored".to_owned(), |score| format!("{score:.4}")),
            group.pass_rate,
            group.passes,
            group.runs
        ));
        // Two different questions, printed as two lines rather than one rate:
        // how often this configuration produced a correct answer at all, and how
        // often the answers it did produce were correct. Collapsing them would
        // report a failed launch as a wrong answer.
        out.push_str(&format!(
            "  answer     reliability {:.4} ({}/{} attempts)  95% CI {}\n",
            group.answer_pass_rate,
            group.answer_passes,
            group.runs,
            interval(group.answer_pass_interval)
        ));
        out.push_str(&format!(
            "             quality {} ({}/{} valid answers)  95% CI {}  no answer {}\n",
            group.answer_quality_rate.map_or_else(
                || style.paint(Role::Gap, "no answer"),
                |rate| format!("{rate:.4}")
            ),
            group.answer_quality_passes,
            group.answer_observations,
            interval(group.answer_quality_interval),
            group.attempts_without_answer
        ));
        // How the attempts ended, by name. A group with failures is visibly a
        // group with failures rather than a lower mean.
        out.push_str(&format!(
            "  terminal   {}\n",
            group
                .terminal_statuses
                .iter()
                .map(|(status, count)| {
                    let text = format!("{} {count}", display_safe(status));
                    if is_no_answer_status(status) {
                        style.paint(Role::Gap, &text)
                    } else {
                        text
                    }
                })
                .collect::<Vec<_>>()
                .join("  ")
        ));
        out.push_str(&format!(
            "  tools      pass {}  fail {}  unknown {}  n/a {}  pass rate {}  95% CI {}\n",
            group.tool_pass,
            group.tool_fail,
            group.tool_unknown,
            group.tool_not_applicable,
            measurement(group.tool_pass_rate),
            interval(group.tool_pass_interval)
        ));
        out.push_str(&format!(
            "  judge      scored {}  passed {}  failed {}  mean {}  (single judge, uncalibrated)\n",
            group.judge_scored,
            group.judge_passes,
            group.judge_failed,
            measurement(group.mean_judge_score)
        ));
        out.push_str(&format!(
            "  telemetry  complete {}  partial {}  absent {}\n",
            group.coverage.telemetry_complete,
            group.coverage.telemetry_partial,
            group.coverage.telemetry_none
        ));
        if let Some(receiver) = group.telemetry_receiver {
            out.push_str(&format!(
                "  OTLP recv  observed {}  rejected spans {}  requests {}  connections {}  accept errors {}\n",
                group.coverage.telemetry_receiver,
                receiver.rejected_spans,
                receiver.rejected_requests,
                receiver.rejected_connections,
                receiver.accept_errors,
            ));
        } else {
            out.push_str("  OTLP recv  none observed\n");
        }
        // The coverage the means below were taken over, and the attempts that
        // produced no answer at all, on the same line as the means they qualify:
        // a mean over the survivors is not a mean over the attempts.
        out.push_str(&format!(
            "  coverage   tokens {}/{}  timing {}/{}  launch timing {}/{}  decision calls {}/{}  MCP {}/{}  failed attempts {}/{}\n",
            group.coverage.tokens,
            group.runs,
            group.coverage.timing,
            group.runs,
            group.coverage.launch_timing,
            group.runs,
            group.coverage.decision_calls,
            group.runs,
            group.coverage.mcp,
            group.runs,
            group.attempts_without_answer,
            group.runs
        ));
        out.push_str(&format!(
            "  observed   elapsed ms {} ({}/{} runs, {} unanswered)  launch ms {} ({}/{})  decision calls {}  decision service ms {}  MCP requests {}  tool calls {} (errors {})  typed-decision errors {}\n",
            measurement(group.mean_elapsed_ms),
            group.coverage.timing,
            group.runs,
            group.attempts_without_answer,
            measurement(group.mean_launch_elapsed_ms),
            group.coverage.launch_timing,
            group.runs,
            measurement(group.mean_decision_calls),
            measurement(group.mean_decision_service_duration_ms),
            measurement(group.mean_mcp_requests),
            measurement(group.mean_mcp_tool_calls),
            measurement(group.mean_mcp_tool_errors),
            measurement(group.mean_typed_decision_errors),
        ));
        if group.decision_request_runs > 0 {
            out.push_str(&format!(
                "  decisions  MCP argument bytes {} mean/run ({}/{} runs; {} measured calls)\n",
                measurement(group.mean_decision_request_bytes),
                group.decision_request_runs,
                group.runs,
                group.decision_request_observations,
            ));
        }
        // Mean amount per field, with the sample it was taken over. A field the
        // recorder named but could not measure keeps its name and shows no
        // amount: nothing here sums or derives one.
        out.push_str(&format!(
            "  tokens     {}\n",
            if group.token_fields.is_empty() {
                style.paint(Role::Gap, "none observed")
            } else {
                group
                    .token_fields
                    .iter()
                    .map(|field| {
                        format!(
                            "{} {} ({}/{})",
                            display_safe(field),
                            match group.mean_tokens.get(field) {
                                Some(mean) => format!("{mean}"),
                                None => style.paint(Role::Gap, MISSING),
                            },
                            group
                                .token_field_observations
                                .get(field)
                                .copied()
                                .unwrap_or(0),
                            group.runs
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("  ")
            }
        ));
        if !group.reported_cost_fields.is_empty() {
            out.push_str(&format!(
                "  cost       {}\n",
                group
                    .reported_cost_fields
                    .iter()
                    .map(|field| {
                        format!(
                            "{} {} ({}/{})",
                            display_safe(field),
                            group.mean_reported_cost_usd.get(field).map_or_else(
                                || style.paint(Role::Gap, MISSING),
                                |mean| format!("${mean:.6}"),
                            ),
                            group
                                .reported_cost_observations
                                .get(field)
                                .copied()
                                .unwrap_or(0),
                            group.runs
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("  ")
            ));
        }
    }
    out.push_str(&style.paint(
        Role::Hint,
        "\nA dash in the table is a metric no run reported, never a measured zero;\n\
         the blocks above give each mean the coverage it was taken over.\n\
         TIME marked total includes candidate preparation and launch; unmarked time is harness elapsed.\n\
         TOKENS shows a reported native total, or input/output marked I/O; service usage stays separate.\n\
         ANSWER shows correct answers over valid answers; reliability below includes every attempt.\n\
         Means cover only the runs that reported the measurement.\n\
         Coverage below the run count is missing observation, not a measured zero.\n\
         Answer reliability covers every attempt, so a failed launch counts\n\
         against it; answer quality covers only the attempts that produced a\n\
         valid answer, and has no interval when none did. A failed attempt is\n\
         never a wrong answer, and the terminal line names how each one ended.\n\
         Latency and token means cover the runs that reported them, so read them\n\
         against the coverage and failed-attempt counts beside them rather than\n\
         as the cost of the whole matrix.\n\
         Intervals are 95% Wilson over the runs shown; the tool rate covers only\n\
         the runs telemetry could decide, and `unknown` is neither a pass nor a fail.\n\
         A judge score is one uncalibrated observation; no interval is claimed for\n\
         the weighted mean score.\n\
         All records use evaluation schema 2.\n",
    ));
    out
}

/// What any cell shows when no run reported the metric it holds.
///
/// Never a zero. The whole point of the coverage counts below the table is
/// that a configuration which measured nothing must not read as one that
/// measured none, and the table would undo that in a single character.
const MISSING: &str = "\u{2014}";

/// The comparison table the report opens with.
///
/// A reader comes to this report asking which configuration did better, and
/// the detailed blocks answer that only by being read end to end. So the
/// columns naming *which* configuration keep their width before the ones
/// saying *how well*: the agent and the runtime are what a comparison is
/// between, while the case id is usually the same the whole way down and is
/// the first to be cut. The measurements leave in reverse priority -- tokens,
/// then time, then the tool rate, then the runtime -- and the answer rate
/// never leaves, because a table without it compares nothing. The intervals
/// stay in the blocks: a table is for spotting a difference, not for
/// concluding one.
const SUMMARY_COLUMNS: &[table::Column] = &[
    table::Column {
        header: "CASE",
        min: 14,
        shrink: Some(2),
        drop: None,
    },
    table::Column {
        header: "AGENT",
        min: 10,
        shrink: Some(0),
        drop: None,
    },
    table::Column {
        header: "RUNTIME",
        min: 12,
        shrink: Some(1),
        drop: Some(3),
    },
    table::Column {
        header: "ANSWER",
        min: 0,
        shrink: None,
        drop: None,
    },
    table::Column {
        header: "TOOLS",
        min: 0,
        shrink: None,
        drop: Some(2),
    },
    table::Column {
        header: "TIME",
        min: 0,
        shrink: None,
        drop: Some(1),
    },
    table::Column {
        header: "TOKENS",
        min: 0,
        shrink: None,
        drop: Some(0),
    },
];

/// One table row per comparable configuration.
///
/// Every value is external record content, so all of it is escaped before it
/// reaches a cell.
fn summary_rows(groups: &[Group]) -> Vec<Vec<table::Cell>> {
    use crate::style::Role;
    groups
        .iter()
        .map(|group| {
            let key = &group.key;
            // The version is what distinguishes two rows for the same agent, so
            // it travels with the name rather than in a column of its own.
            let mut agent = if key.agent_version == UNSPECIFIED {
                key.agent.clone()
            } else {
                format!("{}@{}", key.agent, key.agent_version)
            };
            if key.selection_mode != "none" {
                agent.push_str(&format!(" [{}]", key.selection_mode));
            }
            match key.evaluator_kind.as_str() {
                "typed_decision" => agent = format!("typed: {agent}"),
                "agent" => agent = format!("@{}: {agent}", key.evaluator),
                _ => {}
            }
            vec![
                table::Cell::plain(display_safe(&key.case_id)),
                table::Cell::painted(Role::Agent, display_safe(&agent)),
                table::Cell::painted(
                    Role::Runtime,
                    display_safe(&format!("{} / {}", key.harness, key.model)),
                ),
                if group.answer_observations == 0 {
                    table::Cell::painted(Role::Gap, format!("no answer ({})", group.runs))
                } else {
                    fraction(group.answer_quality_passes, group.answer_observations)
                },
                // Over the decided runs only, as the block below reports it.
                fraction(group.tool_pass, group.tool_pass + group.tool_fail),
                measured(
                    group
                        .evaluator_metrics
                        .means
                        .get("evaluation_elapsed_ms")
                        .filter(|_| {
                            matches!(key.evaluator_kind.as_str(), "agent" | "typed_decision")
                        })
                        .map(|ms| format!("{} eval", human_duration(*ms)))
                        .or_else(|| {
                            group
                                .mean_total_elapsed_ms
                                .map(|ms| format!("{} total", human_duration(ms)))
                        })
                        .or_else(|| group.mean_elapsed_ms.map(human_duration)),
                ),
                measured(summary_tokens(group)),
            ]
        })
        .collect()
}

fn summary_tokens(group: &Group) -> Option<String> {
    if let Some(total) = group.mean_total_tokens() {
        return Some(human_tokens(total));
    }
    let input = group.mean_tokens.get("ahu.tokens.input");
    let output = group.mean_tokens.get("ahu.tokens.output");
    if input.is_none() && output.is_none() {
        return None;
    }
    let amount = |value: Option<&f64>| {
        value.map_or_else(|| MISSING.to_string(), |value| human_tokens(*value))
    };
    Some(format!("{}/{} I/O", amount(input), amount(output)))
}

/// A pass count over the runs it was taken over, or a dash when no run was
/// decidable: an undecided sample is not a rate of zero.
fn fraction(passes: usize, decided: usize) -> table::Cell {
    if decided == 0 {
        missing()
    } else {
        table::Cell::plain(format!("{passes}/{decided}"))
    }
}

/// A measurement, or the dash that says no run reported one.
fn measured(value: Option<String>) -> table::Cell {
    value.map_or_else(missing, table::Cell::plain)
}

fn missing() -> table::Cell {
    table::Cell::painted(crate::style::Role::Gap, MISSING)
}

/// A mean elapsed time in the unit that makes it comparable at a glance.
///
/// One significant decimal is all a mean over a handful of runs supports; the
/// millisecond figure stays in the block below for anyone who wants it.
fn human_duration(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{}ms", ms.round())
    } else if ms < 60_000.0 {
        format!("{:.1}s", ms / 1000.0)
    } else if ms < 3_600_000.0 {
        format!("{:.1}m", ms / 60_000.0)
    } else {
        format!("{:.1}h", ms / 3_600_000.0)
    }
}

/// A mean token count, abbreviated once it stops being worth reading in full.
fn human_tokens(tokens: f64) -> String {
    if tokens < 1000.0 {
        format!("{}", tokens.round())
    } else if tokens < 1_000_000.0 {
        format!("{:.1}k", tokens / 1000.0)
    } else {
        format!("{:.1}M", tokens / 1_000_000.0)
    }
}

/// A Wilson interval that may have had nothing to measure.
fn interval(interval: Option<stats::Interval>) -> String {
    match interval {
        Some(interval) => format!("[{:.4}, {:.4}]", interval.lower, interval.upper),
        None => crate::style::stdout().paint(crate::style::Role::Gap, "none"),
    }
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
        // Said out loud in the contract rather than left to a reader to infer:
        // a single judge's score is one uncalibrated observation, and no
        // inferential interval is offered for the weighted continuous mean.
        "caveats": {
            "judge_calibration": "single_judge_uncalibrated",
            "judge_repeats": "not_implemented",
            "conditional_efficiency": "latency and token means cover only the attempts that reported them; read them against coverage and attempts_without_answer rather than as the cost of the whole matrix",
            "mean_score_interval": "not_reported: a weighted continuous mean is not a binomial proportion, so no inferential interval is claimed for it in this report version",
        },
        "groups": report.groups.iter().map(group_json).collect::<Vec<_>>(),
    });
    Ok(serde_json::to_string(&value)?)
}

/// Most candidate trials one `ahu eval run` will perform.
///
/// The matrix is cases times agents times runs, and every trial launches at
/// least one model. The cap is checked in preflight, before anything launches,
/// so an accidental large suite is a refusal rather than a long bill.
pub const MAX_TRIALS: usize = 200;

/// Most agent launches one `ahu eval run` will perform.
pub const MAX_LAUNCHES: usize = 400;

/// What to evaluate, and how.
#[derive(Debug, Clone)]
pub struct RunRequest<'a> {
    /// A single case. Mutually exclusive with `suite`.
    pub case: Option<&'a Path>,
    /// A suite of cases with fixed weights. Mutually exclusive with `case`.
    pub suite: Option<&'a Path>,
    /// Candidate agents, in the order named on the command line. That order is
    /// the execution order, so a rerun repeats it.
    pub agents: &'a [String],
    pub evaluator: Option<&'a str>,
    /// A separately prepared checkout to run the evaluator from. When given, the
    /// evaluator's environment is not the candidate's, and the record says
    /// `isolated` rather than `prompt_only`.
    pub evaluator_repo: Option<&'a Path>,
    pub decision_evaluator: bool,
    pub skill_selection: crate::skill_selection::Mode,
    pub records: &'a Path,
    pub runs: u32,
    pub timeout_seconds: u64,
    pub allow_widened_approvals: bool,
    pub json_output: bool,
}

/// One planned trial: which case, which agent, which repetition.
struct Trial {
    index: usize,
    case_index: usize,
    agent_index: usize,
    run_index: u32,
}

/// Run the requested matrix, appending one record per terminal candidate trial.
pub fn run(
    console: &mut crate::launcher::Console<'_>,
    repo: &crate::git::Repo,
    request: &RunRequest<'_>,
) -> Result<i32> {
    if request.decision_evaluator
        && (request.evaluator.is_some() || request.evaluator_repo.is_some())
    {
        bail!(kind: ErrorKind::Usage, "--decision-evaluator conflicts with --evaluator and --evaluator-repo");
    }
    let records = writable_records_path(repo, request.records)?;

    // ---- preflight: everything that can be refused is refused before a launch.
    let (suite_identity, cases) = load_cases(request)?;
    let decision_policy = if request.decision_evaluator {
        for entry in &cases {
            decision::preflight(&entry.case).map_err(|e| e.with_kind(ErrorKind::Usage))?;
        }
        Some(decision::policy(crate::mcp::decision_configuration()?))
    } else {
        None
    };
    if request.agents.is_empty() {
        bail!(kind: ErrorKind::Usage, "`ahu eval run` needs at least one --agent @name");
    }
    let mut seen_agents = BTreeSet::new();
    for label in request.agents {
        let name = label.strip_prefix('@').unwrap_or(label);
        if !seen_agents.insert(name) {
            bail!(kind: ErrorKind::Usage, "candidate {label:?} is named more than once; each --agent must name a distinct registered agent");
        }
    }
    let agents = crate::agent::load_all(&repo.root)?;
    let candidates = request
        .agents
        .iter()
        .map(|label| {
            let name = label.strip_prefix('@').unwrap_or(label);
            agents
                .iter()
                .find(|agent| agent.manifest.name == name)
                .ok_or_else(|| {
                    Error::new(format!(
                        "evaluation candidate {label:?} is not a registered ahu agent"
                    ))
                    .with_kind(ErrorKind::Usage)
                })
        })
        .collect::<Result<Vec<_>>>()?;
    for candidate in &candidates {
        if candidate.manifest.permissions.widens_defaults() && !request.allow_widened_approvals {
            bail!(kind: ErrorKind::Usage, "candidate manifest widens harness approvals; pass --allow-widened-approvals to authorize that launch");
        }
    }

    // The evaluator comes from its own checkout when one was named, so a request
    // for isolation is a request for a prepared repository rather than a flag.
    let evaluator_repo = match request.evaluator_repo {
        None => None,
        Some(path) => {
            if request.evaluator.is_none() {
                bail!(kind: ErrorKind::Usage, "--evaluator-repo needs --evaluator @name: it names where that evaluator runs from");
            }
            Some(evaluator_checkout(repo, path)?)
        }
    };
    let evaluator_root = evaluator_repo.as_ref().unwrap_or(repo);
    let evaluator_agents = match &evaluator_repo {
        None => None,
        Some(other) => Some(crate::agent::load_all(&other.root)?),
    };
    let evaluator = request
        .evaluator
        .map(|label| {
            let name = label.strip_prefix('@').unwrap_or(label);
            evaluator_agents
                .as_ref()
                .unwrap_or(&agents)
                .iter()
                .find(|agent| agent.manifest.name == name)
                .ok_or_else(|| {
                    Error::new(format!(
                        "evaluation agent {label:?} is not a registered ahu agent in the checkout it runs from"
                    ))
                    .with_kind(ErrorKind::Usage)
                })
                .cloned()
        })
        .transpose()?;
    if evaluator
        .as_ref()
        .is_some_and(|agent| agent.manifest.permissions.widens_defaults())
        && !request.allow_widened_approvals
    {
        bail!(kind: ErrorKind::Usage, "evaluator manifest widens harness approvals; pass --allow-widened-approvals to authorize that launch");
    }
    if evaluator.is_some()
        && let Some(missing) = cases.iter().find(|entry| entry.case.rubric.is_none())
    {
        bail!(kind: ErrorKind::Usage, "--evaluator requires a case `rubric` object with one criterion per scored answer field; case {:?} has none", missing.case.id);
    }
    let blinding = if request.decision_evaluator {
        fingerprint::Blinding::TypedRequest
    } else if evaluator_repo.is_some() {
        fingerprint::Blinding::Isolated
    } else {
        fingerprint::Blinding::PromptOnly
    };

    // The whole matrix, in the order it will run.
    let plan: Vec<Trial> = (0..cases.len())
        .flat_map(|case_index| {
            (0..candidates.len()).flat_map(move |agent_index| {
                (1..=request.runs).map(move |run_index| (case_index, agent_index, run_index))
            })
        })
        .enumerate()
        .map(|(index, (case_index, agent_index, run_index))| Trial {
            index: index + 1,
            case_index,
            agent_index,
            run_index,
        })
        .collect();
    let launches = plan.len() * if evaluator.is_some() { 2 } else { 1 };
    if plan.is_empty() {
        bail!(kind: ErrorKind::Usage, "the evaluation matrix is empty");
    }
    if plan.len() > MAX_TRIALS || launches > MAX_LAUNCHES {
        bail!(kind: ErrorKind::Usage,
            "the evaluation matrix is {} case(s) x {} agent(s) x {} run(s) = {} trial(s) and {launches} launch(es), beyond the {MAX_TRIALS} trial and {MAX_LAUNCHES} launch caps. Split the suite or lower --runs.",
            cases.len(), candidates.len(), request.runs, plan.len());
    }

    // ---- execution: sequential, in the planned order, one fresh task per trial.
    let run_id = make_run_id(&cases[0].case.id);
    let receiver = crate::eval_otel::Receiver::start_for(Some(&run_id), None)
        .map_err(|error| Error::new(format!("cannot start local OTLP receiver: {error}")))?;
    let artifact_root =
        create_private_run_dir(records.parent().unwrap_or_else(|| Path::new(".")), &run_id)?;
    let build = fingerprint::BuildIdentity::detect();
    let mut outputs = Vec::new();
    for trial in &plan {
        let receiver_stats_before = receiver.receiver_stats();
        let entry = &cases[trial.case_index];
        let case = &entry.case;
        let candidate = candidates[trial.agent_index];
        let label = request.agents[trial.agent_index].as_str();
        let run_dir = artifact_root.join(format!("run-{:03}", trial.index));
        create_private_dir(&run_dir)?;

        let mut print = fingerprint::InputFingerprint {
            case_id: case.id.clone(),
            corpus_version: case.corpus_version.clone(),
            case_schema_version: case.schema_version,
            case_digest: case.digest.clone(),
            prompt_profile: case.prompt_profile(),
            prompt_version: case::PROMPT_VERSION,
            scoring_version: case::SCORING_VERSION,
            suite: suite_identity.clone(),
            case_weight: entry.weight,
            candidate: fingerprint::AgentFingerprint::of(candidate),
            evaluator: evaluator.as_ref().map(fingerprint::AgentFingerprint::of),
            decision_evaluator: decision_policy.clone(),
            blinding,
            skill_digest: None,
            selection_policy_digest: None,
            evaluator_skill_digest: None,
            build: build.clone(),
            target_repo_head: repo.head.clone(),
            evaluator_repo_head: None,
        };

        // The selector receives only candidate-visible material, never expected
        // answers, scoring weights or the hidden evaluator rubric.
        let trial_started = std::time::Instant::now();
        let mut evaluator_observation = decision::Observation::new(if request.decision_evaluator {
            "typed_decision"
        } else if evaluator.is_some() {
            "agent"
        } else {
            "none"
        });
        let visible_task = format!("{}\n{}", case.purpose, serde_json::to_string(&case.state)?);
        let selection =
            crate::skill_selection::prepare(repo, &visible_task, request.skill_selection)?;
        print.selection_policy_digest = selection_policy_digest(&selection);
        write_private_file(
            &run_dir.join("skill-selection.json"),
            &serde_json::to_vec(&selection)?,
        )?;
        let selection_id = format!("selection-{}-{}", run_id, trial.index);
        let selection_exported = crate::telemetry::export_eval_selection(
            receiver.endpoint(),
            &run_id,
            &selection_id,
            &selection,
        );
        let selection_observed = selection_exported
            && receiver
                .task(&selection_id, 1)
                .is_some_and(|t| t.selection_observations == 1);
        let mut candidate_prompt = case.candidate_prompt();
        let advice = selection.prompt_block();
        if !advice.is_empty() {
            candidate_prompt.push_str("\n\n");
            candidate_prompt.push_str(&advice);
        }
        let launch = launch_eval_agent(
            repo,
            label,
            &candidate_prompt,
            &run_dir.join("candidate"),
            &run_id,
            case,
            "candidate",
            request.timeout_seconds,
            request.allow_widened_approvals,
            receiver.endpoint(),
        );
        let total_elapsed_ms = trial_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let candidate_result = match launch.result {
            Ok(result) => result,
            Err(error) => {
                // The candidate never produced an answer. The row says so, with
                // the inputs that were known, rather than scoring a zero — and
                // with whatever the failed attempt did measure, so it is not
                // mistaken for an attempt that cost nothing.
                let envelope = launch.envelope.as_ref();
                // An attempt that failed still ran on a harness and still had a
                // skill bundle in front of it, and the envelope states both. Take
                // them here as the success path does, so the fingerprint is not
                // marked `partial` over fields that were actually observed.
                if let Some(envelope) = envelope {
                    print.candidate = print
                        .candidate
                        .clone()
                        .with_harness_version(harness_version(envelope));
                    print.skill_digest = skill_digest(envelope);
                }
                let harness_outcome = envelope
                    .and_then(|value| value.get("outcome"))
                    .and_then(serde_json::Value::as_str);
                let status = match harness_outcome {
                    Some("timed_out") => "candidate_timed_out",
                    Some("cancelled") => "candidate_cancelled",
                    _ => "candidate_launch_failed",
                };
                let mut record = print.record_fields();
                record.extend(base_fields(&run_id, trial, "candidate"));
                record.extend(outcome_fields(status, Some("candidate_run_failed")));
                record.insert("launch_elapsed_ms".into(), launch.elapsed_ms.into());
                insert_selection_fields(
                    &mut record,
                    &selection,
                    selection_observed,
                    total_elapsed_ms,
                )?;
                record.insert("failure_reason".into(), error.to_string().into());
                if let Some(harness_outcome) = harness_outcome {
                    record.insert("outcome".into(), harness_outcome.into());
                }
                // A launch that reached a task still has a task id, an attempt,
                // and possibly exported telemetry. Collected only from what the
                // envelope actually reported: nothing here invents a zero.
                let task = envelope.and_then(|value| {
                    let task_id = value.get("task_id").and_then(serde_json::Value::as_str)?;
                    let attempt = value
                        .get("attempt")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(1) as u32;
                    Some((task_id.to_owned(), attempt))
                });
                let telemetry = match &task {
                    Some((task_id, attempt)) => {
                        record.insert("task_id".into(), task_id.clone().into());
                        record.insert("candidate_task_id".into(), task_id.clone().into());
                        record.insert("attempt".into(), (*attempt).into());
                        record.insert("attempts".into(), (*attempt).into());
                        receiver.task(task_id, *attempt)
                    }
                    None => None,
                };
                insert_telemetry_fields(&mut record, telemetry.as_ref())?;
                if let Some(tokens) = reported_tokens(envelope, telemetry.as_ref()) {
                    record.insert("reported_tokens".into(), tokens);
                }
                if let Some(envelope) = envelope {
                    // The envelope is real evidence about this attempt, so it is
                    // kept beside the ones that succeeded.
                    write_private_file(
                        &run_dir.join("candidate-result.json"),
                        &serde_json::to_vec(envelope)?,
                    )?;
                }
                record.insert(
                    "telemetry_receiver".into(),
                    serde_json::to_value(receiver.receiver_stats().since(receiver_stats_before))?,
                );
                insert_trajectory_fields(&mut record, envelope, case.trajectory_budgets.as_ref())?;
                insert_evaluation_fields(&mut record, &evaluator_observation, trial_started)?;
                let trajectory_budget_status = record.get("trajectory_budget_status").cloned();
                append_jsonl(&records, &serde_json::Value::Object(record))?;
                outputs.push(serde_json::json!({
                    "trajectory_budget_status": trajectory_budget_status,
                    "trial": trial.index,
                    "case_id": case.id,
                    "agent": candidate.manifest.name,
                    "run_index": trial.run_index,
                    "terminal_status": status,
                    "launch_elapsed_ms": launch.elapsed_ms,
                    "failure": error.to_string(),
                }));
                continue;
            }
        };
        let launch_elapsed_ms = launch.elapsed_ms;
        let candidate_task = candidate_result
            .get("task_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::new("candidate task result did not contain a task ID"))?;
        let attempt = candidate_result
            .get("attempt")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as u32;
        let worktree = candidate_result
            .get("worktree")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::new("candidate task result did not contain its worktree"))?;
        print.candidate = print
            .candidate
            .clone()
            .with_harness_version(harness_version(&candidate_result));
        print.skill_digest = skill_digest(&candidate_result);
        write_private_file(
            &run_dir.join("candidate-result.json"),
            &serde_json::to_vec(&candidate_result)?,
        )?;
        let answer_artifact = (|| -> Result<(Vec<u8>, serde_json::Value)> {
            let path = safe_artifact(Path::new(worktree), "answer.json")?;
            let bytes = std::fs::read(path)?;
            if bytes.len() > 64 * 1024 {
                bail!("answer too large");
            }
            let answer: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| Error::new("invalid answer JSON"))?;
            case::validate_answer(case, &answer)?;
            Ok((bytes, answer))
        })();
        let (answer_bytes, answer) = match answer_artifact {
            Ok(answer) => answer,
            Err(error) => {
                let status = if error.to_string().contains("too large") {
                    "candidate_answer_too_large"
                } else if error.to_string().contains("JSON") {
                    "candidate_answer_invalid_json"
                } else if error.to_string().contains("answer.json") {
                    "candidate_answer_missing"
                } else {
                    "candidate_answer_invalid"
                };
                let mut record = print.record_fields();
                record.extend(base_fields(&run_id, trial, "candidate"));
                record.extend(outcome_fields(status, Some("candidate_answer_invalid")));
                record.insert("task_id".into(), candidate_task.into());
                record.insert("attempt".into(), attempt.into());
                record.insert("attempts".into(), attempt.into());
                record.insert("launch_elapsed_ms".into(), launch_elapsed_ms.into());
                insert_selection_fields(
                    &mut record,
                    &selection,
                    selection_observed,
                    total_elapsed_ms,
                )?;
                record.insert("failure_reason".into(), error.to_string().into());
                let telemetry = receiver.task(candidate_task, attempt);
                insert_telemetry_fields(&mut record, telemetry.as_ref())?;
                if let Some(tokens) = reported_tokens(Some(&candidate_result), telemetry.as_ref()) {
                    record.insert("reported_tokens".into(), tokens);
                }
                record.insert(
                    "telemetry_receiver".into(),
                    serde_json::to_value(receiver.receiver_stats().since(receiver_stats_before))?,
                );
                insert_trajectory_fields(
                    &mut record,
                    Some(&candidate_result),
                    case.trajectory_budgets.as_ref(),
                )?;
                insert_evaluation_fields(&mut record, &evaluator_observation, trial_started)?;
                let trajectory_budget_status = record.get("trajectory_budget_status").cloned();
                append_jsonl(&records, &serde_json::Value::Object(record))?;
                outputs.push(serde_json::json!({
                    "trajectory_budget_status": trajectory_budget_status,
                    "trial": trial.index, "case_id": case.id,
                    "agent": candidate.manifest.name, "run_index": trial.run_index,
                    "task_id": candidate_task, "terminal_status": status,
                }));
                continue;
            }
        };
        write_private_file(&run_dir.join("answer.json"), &answer_bytes)?;

        let (answer_score, answer_passed) = case::deterministic_score(case, &answer)?;

        // The judge is a separate concern from the answer check. Its failure is
        // recorded as its own failure; it never becomes the candidate's.
        let mut judge_status = "not_used";
        let mut judgement: Option<case::Judgement> = None;
        let mut judge_failure: Option<String> = None;
        let mut evaluator_task = None;
        if request.decision_evaluator {
            match decision::judge_with(case, &answer, &mut evaluator_observation, |arguments| {
                crate::mcp::typed_decide(repo, arguments)
            }) {
                Ok(judged) => {
                    judge_status = "scored";
                    judgement = Some(judged);
                }
                Err(_) => {
                    judge_status = "failed";
                    judge_failure = Some("typed_decision_grading_failed".into());
                }
            }
            let observation_id = format!("evaluator-{}-{}", run_id, trial.index);
            evaluator_observation.observation_id = Some(observation_id.clone());
            if evaluator_observation.calls_attempted > 0
                && crate::telemetry::export_eval_decision(
                    receiver.endpoint(),
                    &run_id,
                    &observation_id,
                    &evaluator_observation,
                )
                && receiver
                    .task(&observation_id, 1)
                    .is_some_and(|t| t.evaluator_decision_observations == 1)
            {
                evaluator_observation.telemetry_coverage = "typed_decision_span".into();
            }
        }
        if let Some(agent) = evaluator.as_ref() {
            let evaluator_started = std::time::Instant::now();
            let evaluator_label = request.evaluator.unwrap_or("@evaluator");
            let prompt = case.evaluator_prompt(&answer)?;
            let evaluator_launch = launch_eval_agent(
                evaluator_root,
                evaluator_label,
                &prompt,
                &run_dir.join("evaluator"),
                &run_id,
                case,
                "evaluator",
                request.timeout_seconds,
                request.allow_widened_approvals,
                receiver.endpoint(),
            );
            let envelope = evaluator_launch
                .result
                .as_ref()
                .ok()
                .or(evaluator_launch.envelope.as_ref());
            if let Some(envelope) = envelope {
                write_private_file(
                    &run_dir.join("evaluator-result.json"),
                    &serde_json::to_vec(envelope)?,
                )?;
                print.evaluator = Some(
                    fingerprint::AgentFingerprint::of(agent)
                        .with_harness_version(harness_version(envelope)),
                );
                print.evaluator_skill_digest = skill_digest(envelope);
                print.evaluator_repo_head = evaluator_root.head.clone();
                observe_evaluator_agent(&mut evaluator_observation, envelope, &receiver)?;
                evaluator_task = evaluator_observation.task_id.clone();
            }
            match evaluator_launch.result.and_then(|result| {
                let worktree = result
                    .get("worktree")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        Error::new("evaluator task result did not contain its worktree")
                    })?;
                let score_path = safe_artifact(Path::new(worktree), "score.json")?;
                let score_bytes = std::fs::read(&score_path).map_err(|error| {
                    Error::new(format!("evaluator did not produce score.json: {error}"))
                })?;
                if score_bytes.len() > 32 * 1024 {
                    bail!("evaluator score.json exceeds the 32 KiB artifact limit");
                }
                let score_json: serde_json::Value = serde_json::from_slice(&score_bytes)
                    .map_err(|_| Error::new("evaluator score.json is not valid JSON"))?;
                let judged = case::validate_judgement(case, &score_json)?;
                Ok((score_bytes, judged))
            }) {
                Ok((score_bytes, judged)) => {
                    write_private_file(&run_dir.join("score.json"), &score_bytes)?;
                    judge_status = "scored";
                    judgement = Some(judged);
                }
                Err(error) => {
                    judge_status = "failed";
                    judge_failure = Some(error.to_string());
                }
            }
            evaluator_observation.elapsed_ms =
                Some(evaluator_started.elapsed().as_secs_f64() * 1000.0);
        }

        if evaluator.is_some() {
            evaluator_observation.status = judge_status.into();
        }
        let telemetry = receiver.task(candidate_task, attempt);
        let telemetry_coverage = telemetry
            .as_ref()
            .map_or(crate::eval_otel::Coverage::None, |t| t.coverage());
        let tool_status =
            case::score_tool_expectations(case.tool_expectations.as_ref(), telemetry.as_ref());
        let mcp_observed = telemetry.as_ref().is_some_and(|t| t.mcp_observed);

        // Failed judging has no headline score. Deterministic answer checks
        // remain available separately, and cannot turn a judge failure into a pass.
        let (score, passed, score_source) = match &judgement {
            Some(judged) => (Some(judged.score), judged.passed, "judge"),
            None if judge_status == "failed" => (None, false, "judge_failed"),
            None => (Some(answer_score), answer_passed, "deterministic"),
        };

        let mut record = print.record_fields();
        record.extend(base_fields(&run_id, trial, "candidate"));
        record.extend(outcome_fields(
            if judge_status == "failed" {
                "candidate_scored_judge_failed"
            } else {
                "candidate_scored"
            },
            judge_failure.as_ref().map(|_| "evaluator_run_failed"),
        ));
        let mut put = |key: &str, value: serde_json::Value| {
            record.insert(key.to_owned(), value);
        };
        put("task_id", candidate_task.into());
        put("candidate_task_id", candidate_task.into());
        put(
            "evaluator_task_id",
            evaluator_task.map_or(serde_json::Value::Null, Into::into),
        );
        put("attempt", attempt.into());
        put("attempts", attempt.into());
        put("launch_elapsed_ms", launch_elapsed_ms.into());
        put("answer_status", "scored".into());
        put(
            "outcome",
            candidate_result
                .get("outcome")
                .cloned()
                .unwrap_or_else(|| "unknown".into()),
        );
        put("score", score.map_or(serde_json::Value::Null, number));
        put("passed", passed.into());
        put("score_source", score_source.into());
        put("answer_score", number(answer_score));
        put("answer_passed", answer_passed.into());
        // Kept under its version 1 name as well, so an existing reader of the
        // deterministic figure keeps reading it.
        put("deterministic_score", number(answer_score));
        put("judge_status", judge_status.into());
        put("judge_calibration", "single_judge_uncalibrated".into());
        put(
            "judge_score",
            judgement
                .as_ref()
                .map_or(serde_json::Value::Null, |j| number(j.score)),
        );
        put(
            "judge_passed",
            judgement
                .as_ref()
                .map_or(serde_json::Value::Null, |j| j.passed.into()),
        );
        put(
            "judge_criterion_scores",
            judgement.as_ref().map_or(serde_json::Value::Null, |j| {
                serde_json::to_value(&j.criterion_scores).unwrap_or(serde_json::Value::Null)
            }),
        );
        put(
            "judge_reason_codes",
            judgement.as_ref().map_or(serde_json::Value::Null, |j| {
                serde_json::to_value(&j.reason_codes).unwrap_or(serde_json::Value::Null)
            }),
        );
        put("tool_expectation_status", tool_status.as_str().into());
        put(
            "tool_expectation_required",
            serde_json::to_value(
                case.tool_expectations
                    .as_ref()
                    .map(|expect| &expect.required),
            )
            .unwrap_or(serde_json::Value::Null),
        );
        put(
            "tool_expectation_required_successful",
            serde_json::to_value(
                case.tool_expectations
                    .as_ref()
                    .map(|expect| &expect.required_successful),
            )
            .unwrap_or(serde_json::Value::Null),
        );
        put(
            "tool_expectation_forbidden",
            serde_json::to_value(
                case.tool_expectations
                    .as_ref()
                    .map(|expect| &expect.forbidden),
            )
            .unwrap_or(serde_json::Value::Null),
        );
        put("telemetry_coverage", telemetry_coverage.as_str().into());
        put(
            "telemetry_duplicate_spans",
            telemetry
                .as_ref()
                .map_or(serde_json::Value::Null, |t| t.duplicate_spans.into()),
        );
        put(
            "trace_id",
            telemetry
                .as_ref()
                .and_then(|t| t.trace_id.clone())
                .map_or(serde_json::Value::Null, Into::into),
        );
        put(
            "elapsed_ms",
            telemetry
                .as_ref()
                .and_then(|t| t.elapsed_ms)
                .map_or(serde_json::Value::Null, Into::into),
        );
        put(
            "decision_service_duration_ms",
            telemetry
                .as_ref()
                .and_then(|t| t.decision_duration_ms)
                .map_or(serde_json::Value::Null, Into::into),
        );
        put(
            "decision_request_bytes",
            telemetry
                .as_ref()
                .and_then(|t| t.decision_request_bytes)
                .map_or(serde_json::Value::Null, Into::into),
        );
        put(
            "decision_request_observations",
            telemetry
                .as_ref()
                .map_or(0, |t| t.decision_request_observations)
                .into(),
        );
        put(
            "decision_request_formats",
            telemetry
                .as_ref()
                .map(|t| serde_json::to_value(&t.decision_request_formats).unwrap_or_default())
                .unwrap_or(serde_json::Value::Null),
        );
        put("mcp_observed", mcp_observed.into());
        put(
            "telemetry_receiver",
            serde_json::to_value(receiver.receiver_stats().since(receiver_stats_before))?,
        );
        for (key, pick) in [
            ("decision_call_count", 0usize),
            ("mcp_request_count", 1),
            ("mcp_tool_list_count", 2),
            ("mcp_tool_call_count", 3),
            ("mcp_tool_error_count", 4),
            ("typed_decision_error_count", 5),
        ] {
            let value = telemetry
                .as_ref()
                .filter(|t| t.mcp_observed)
                .and_then(|t| match pick {
                    0 => Some(t.typed_decision_calls),
                    1 => Some(t.mcp_requests),
                    2 => Some(t.tool_list_calls),
                    3 => Some(t.tool_calls),
                    4 => Some(t.tool_errors),
                    _ if t.tool_errors_fully_named() => Some(t.typed_decision_errors),
                    _ => None,
                });
            put(key, value.map_or(serde_json::Value::Null, Into::into));
        }
        put(
            "mcp_tools",
            telemetry
                .as_ref()
                .filter(|t| t.mcp_observed)
                .map(|t| serde_json::to_value(&t.tool_calls_by_name).unwrap_or_default())
                .unwrap_or(serde_json::Value::Null),
        );
        put(
            "mcp_tool_errors_by_name",
            telemetry
                .as_ref()
                .filter(|t| t.mcp_observed)
                .map(|t| serde_json::to_value(&t.tool_errors_by_name).unwrap_or_default())
                .unwrap_or(serde_json::Value::Null),
        );
        put(
            "reported_tokens",
            reported_tokens(Some(&candidate_result), telemetry.as_ref())
                .unwrap_or(serde_json::Value::Null),
        );
        insert_selection_fields(
            &mut record,
            &selection,
            selection_observed,
            total_elapsed_ms,
        )?;
        insert_candidate_selection_fields(&mut record, telemetry.as_ref());
        record.retain(|_, value| !value.is_null());
        insert_trajectory_fields(
            &mut record,
            Some(&candidate_result),
            case.trajectory_budgets.as_ref(),
        )?;
        insert_evaluation_fields(&mut record, &evaluator_observation, trial_started)?;
        let trajectory_budget_status = record.get("trajectory_budget_status").cloned();
        append_jsonl(&records, &serde_json::Value::Object(record))?;
        outputs.push(serde_json::json!({
                    "trajectory_budget_status": trajectory_budget_status,
            "trial": trial.index,
            "case_id": case.id,
            "agent": candidate.manifest.name,
            "run_index": trial.run_index,
            "task_id": candidate_task,
            "score": score,
            "passed": passed,
            "score_source": score_source,
            "answer_passed": answer_passed,
            "judge_status": judge_status,
            "tool_expectation_status": tool_status.as_str(),
            "telemetry_coverage": telemetry_coverage.as_str(),
            "fingerprint_completeness": print.completeness().as_str(),
        }));
    }

    let result = serde_json::json!({
        "schema_version": RECORD_SCHEMA_VERSION,
        "run_id": run_id,
        "suite_id": suite_identity.as_ref().map(|suite| suite.id.clone()),
        "cases": cases.iter().map(|entry| entry.case.id.clone()).collect::<Vec<_>>(),
        "candidates": candidates.iter().map(|agent| agent.label()).collect::<Vec<_>>(),
        "evaluator": request.evaluator,
        "decision_evaluator": decision_policy,
        "blinding": blinding.as_str(),
        "blinding_caveat": blinding.caveat(),
        "planned_trials": plan.len(),
        "planned_launches": launches,
        "trials": outputs,
        "telemetry_receiver": "local_otlp_http",
    });
    if request.json_output {
        println!("{}", serde_json::to_string(&result)?);
    }
    console.say(&format!(
        "ahu eval run {}  {} trial(s) over {} case(s) and {} agent(s)\n",
        run_id,
        plan.len(),
        cases.len(),
        candidates.len()
    ))?;
    for row in result["trials"].as_array().into_iter().flatten() {
        console.say(&format!(
            "  trial {}  {} {}  run {}  score {}  answer {}  judge {}  tools {}  telemetry {}  trajectory budget {}\n",
            row["trial"],
            display_safe(row["case_id"].as_str().unwrap_or("?")),
            display_safe(row["agent"].as_str().unwrap_or("?")),
            row["run_index"],
            row["score"],
            row["answer_passed"],
            display_safe(row["judge_status"].as_str().unwrap_or("?")),
            display_safe(row["tool_expectation_status"].as_str().unwrap_or("?")),
            display_safe(row["telemetry_coverage"].as_str().unwrap_or("?")),
            display_safe(row["trajectory_budget_status"].as_str().unwrap_or("not_applicable")),
        ))?;
    }
    console.say(&format!(
        "  blinding {}\n  records {}\n  artifacts {}\n",
        blinding.as_str(),
        display_path(&records),
        display_path(&artifact_root)
    ))?;
    Ok(0)
}

/// A finite f64 as JSON, or null.
fn number(value: f64) -> serde_json::Value {
    serde_json::Number::from_f64(value).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

/// The fields every row of one run carries.
fn base_fields(
    run_id: &str,
    trial: &Trial,
    stage: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let mut fields = serde_json::Map::new();
    fields.insert("schema_version".into(), RECORD_SCHEMA_VERSION.into());
    fields.insert("recorded_at".into(), crate::task::now_rfc3339().into());
    fields.insert("run_id".into(), run_id.into());
    fields.insert("stage".into(), stage.into());
    fields.insert("trial_index".into(), trial.index.into());
    fields.insert("case_index".into(), (trial.case_index + 1).into());
    fields.insert("agent_index".into(), (trial.agent_index + 1).into());
    fields.insert("run_index".into(), trial.run_index.into());
    fields
}

/// How a trial ended, and why, when it did not end cleanly.
fn outcome_fields(
    terminal_status: &str,
    failure_category: Option<&str>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut fields = serde_json::Map::new();
    fields.insert("terminal_status".into(), terminal_status.into());
    if let Some(category) = failure_category {
        fields.insert("failure_category".into(), category.into());
    }
    if is_no_answer_status(terminal_status) {
        // No valid answer means no answer score. Recording 0 here would be a
        // claim the candidate answered wrongly, which is not what happened.
        // `answer_passed: false` stays, because the all-attempt reliability rate
        // is meant to count this attempt against the configuration; the
        // `answer_status` below is what keeps it out of the quality rate.
        fields.insert("answer_status".into(), "no_answer".into());
        fields.insert("outcome".into(), "failed".into());
        fields.insert("judge_status".into(), "not_reached".into());
        fields.insert(
            "tool_expectation_status".into(),
            case::ToolExpectationStatus::Unknown.as_str().into(),
        );
        fields.insert(
            "telemetry_coverage".into(),
            crate::eval_otel::Coverage::None.as_str().into(),
        );
        fields.insert("score".into(), serde_json::Value::Null);
        fields.insert("passed".into(), false.into());
        fields.insert("answer_passed".into(), false.into());
    }
    fields
}

/// Project whatever telemetry arrived for one attempt onto its record fields.
///
/// Shared by every terminal path so a failed or timed-out attempt reports the
/// same observations a scored one would. Every field is written only from a
/// count the session actually reported: an absent observation stays absent, and
/// the coverage field says so rather than a zero standing in for it.
fn insert_telemetry_fields(
    record: &mut serde_json::Map<String, serde_json::Value>,
    telemetry: Option<&crate::eval_otel::TaskTelemetry>,
) -> Result<()> {
    insert_candidate_selection_fields(record, telemetry);
    let coverage = telemetry.map_or(crate::eval_otel::Coverage::None, |item| item.coverage());
    record.insert("telemetry_coverage".into(), coverage.as_str().into());
    record.insert(
        "mcp_observed".into(),
        telemetry.is_some_and(|item| item.mcp_observed).into(),
    );
    if let Some(item) = telemetry {
        if let Some(trace_id) = &item.trace_id {
            record.insert("trace_id".into(), trace_id.clone().into());
        }
        if let Some(bytes) = item.decision_request_bytes {
            record.insert("decision_request_bytes".into(), bytes.into());
        }
        record.insert(
            "decision_request_observations".into(),
            item.decision_request_observations.into(),
        );
        record.insert(
            "decision_request_formats".into(),
            serde_json::to_value(&item.decision_request_formats)?,
        );
        // Timing and duration arrive with the spans, so they survive an attempt
        // that failed after exporting them.
        if let Some(elapsed) = item.elapsed_ms {
            record.insert("elapsed_ms".into(), elapsed.into());
        }
        if let Some(duration) = item.decision_duration_ms {
            record.insert("decision_service_duration_ms".into(), duration.into());
        }
        record.insert(
            "telemetry_duplicate_spans".into(),
            item.duplicate_spans.into(),
        );
    }
    if let Some(item) = telemetry.filter(|item| item.mcp_observed) {
        for (name, count) in [
            ("decision_call_count", item.typed_decision_calls),
            ("mcp_request_count", item.mcp_requests),
            ("mcp_tool_list_count", item.tool_list_calls),
            ("mcp_tool_call_count", item.tool_calls),
            ("mcp_tool_error_count", item.tool_errors),
        ] {
            record.insert(name.into(), count.into());
        }
        record.insert(
            "mcp_tools".into(),
            serde_json::to_value(&item.tool_calls_by_name)?,
        );
        record.insert(
            "mcp_tool_errors_by_name".into(),
            serde_json::to_value(&item.tool_errors_by_name)?,
        );
        // Only meaningful once every error has a name to belong to.
        if item.tool_errors_fully_named() {
            record.insert(
                "typed_decision_error_count".into(),
                item.typed_decision_errors.into(),
            );
        }
    }
    Ok(())
}

/// One case of the matrix, with the weight the suite gave it.
struct PlannedCase {
    case: case::EvalCase,
    weight: Option<f64>,
}

/// Load either the single case or the whole suite, validating everything first.
fn load_cases(
    request: &RunRequest<'_>,
) -> Result<(Option<fingerprint::SuiteIdentity>, Vec<PlannedCase>)> {
    match (request.case, request.suite) {
        (Some(_), Some(_)) => {
            bail!(kind: ErrorKind::Usage, "--case and --suite are alternatives: pass one")
        }
        (None, None) => {
            bail!(kind: ErrorKind::Usage, "`ahu eval run` needs --case <path> or --suite <path>")
        }
        (Some(path), None) => Ok((
            None,
            vec![PlannedCase {
                case: case::load(path)?,
                weight: None,
            }],
        )),
        (None, Some(path)) => {
            let suite = suite::load(path)?;
            let identity = fingerprint::SuiteIdentity {
                id: suite.id.clone(),
                suite_version: suite.suite_version.clone(),
                digest: suite.digest.clone(),
            };
            let cases = suite
                .cases
                .into_iter()
                .map(|entry| PlannedCase {
                    case: entry.case,
                    weight: Some(entry.weight),
                })
                .collect();
            Ok((Some(identity), cases))
        }
    }
}

/// Open the separately prepared checkout an evaluator runs from.
///
/// It has to be a real repository, and it has to be a different one: pointing
/// `--evaluator-repo` back at the candidate's checkout would claim isolation
/// that is not there.
fn evaluator_checkout(repo: &crate::git::Repo, path: &Path) -> Result<crate::git::Repo> {
    let other = crate::git::discover(path).map_err(|error| {
        Error::new(format!(
            "--evaluator-repo {} is not a Git checkout: {error}",
            display_path(path)
        ))
        .with_kind(ErrorKind::Usage)
    })?;
    if other.common_dir == repo.common_dir {
        bail!(kind: ErrorKind::Usage, "--evaluator-repo must be a separately prepared checkout; {} belongs to the same repository as the candidate, which is not isolation", display_path(path));
    }
    Ok(other)
}

fn writable_records_path(repo: &crate::git::Repo, path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent.canonicalize().map_err(|error| {
        Error::new(format!(
            "records directory {} is unavailable: {error}",
            display_path(parent)
        ))
        .with_kind(ErrorKind::Usage)
    })?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::new("records path must name a file").with_kind(ErrorKind::Usage))?;
    let target = parent.join(name);
    let mut roots = vec![repo.root.clone()];
    if let Ok(primary) = repo.primary_root() {
        roots.push(primary);
    }
    for root in roots {
        if root
            .canonicalize()
            .is_ok_and(|root| target.starts_with(root))
        {
            bail!(kind: ErrorKind::Usage, "evaluation records and run artifacts must live outside this repository");
        }
    }
    if let Ok(metadata) = std::fs::symlink_metadata(&target)
        && (!metadata.file_type().is_file() || metadata.file_type().is_symlink())
    {
        bail!(kind: ErrorKind::Usage, "evaluation records path must be a regular file, not a symlink or special file");
    }
    Ok(target)
}

fn make_run_id(case_id: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "{}-{nanos:x}-{:x}-{sequence:x}",
        &case_id[..case_id.len().min(20)],
        std::process::id()
    )
}

fn create_private_run_dir(parent: &Path, run_id: &str) -> Result<PathBuf> {
    let path = parent.join(format!("ahu-eval-{run_id}"));
    create_private_dir(&path)?;
    Ok(path)
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder
        .recursive(false)
        .mode(0o700)
        .create(path)
        .map_err(Into::into)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir(path).map_err(Into::into)
}

#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn append_jsonl(path: &Path, value: &serde_json::Value) -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    Ok(())
}

/// What one eval launch produced, whether or not it can be used.
///
/// The envelope is kept even for a launch that failed: a timed-out attempt still
/// reports its task id, its outcome, and whatever token metrics the harness
/// measured, and those are real observations. Discarding them with the error
/// would leave the row looking like a run that cost nothing.
struct LaunchOutcome {
    /// Wall-clock milliseconds the runner spent on the launch. Measured here
    /// rather than in the harness, so it exists even when nothing was exported.
    elapsed_ms: u64,
    /// The harness result envelope, whenever the launch emitted valid JSON.
    envelope: Option<serde_json::Value>,
    /// The usable result, or why there is none.
    result: Result<serde_json::Value>,
}

/// Launch one eval agent, timing it and keeping whatever evidence it produced.
#[allow(clippy::too_many_arguments)]
fn launch_eval_agent(
    repo: &crate::git::Repo,
    agent: &str,
    prompt: &str,
    output_dir: &Path,
    run_id: &str,
    case: &case::EvalCase,
    stage: &str,
    timeout_seconds: u64,
    allow_widened: bool,
    otel_endpoint: &str,
) -> LaunchOutcome {
    let started = std::time::Instant::now();
    let mut envelope = None;
    let result = launch_eval_agent_inner(
        repo,
        agent,
        prompt,
        output_dir,
        run_id,
        case,
        stage,
        timeout_seconds,
        allow_widened,
        otel_endpoint,
        &mut envelope,
    );
    LaunchOutcome {
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        envelope,
        result,
    }
}

#[allow(clippy::too_many_arguments)]
fn launch_eval_agent_inner(
    repo: &crate::git::Repo,
    agent: &str,
    prompt: &str,
    output_dir: &Path,
    run_id: &str,
    case: &case::EvalCase,
    stage: &str,
    timeout_seconds: u64,
    allow_widened: bool,
    otel_endpoint: &str,
    envelope: &mut Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    create_private_dir(output_dir)?;
    let prompt_path = output_dir.join("prompt.txt");
    write_private_file(&prompt_path, prompt.as_bytes())?;
    let case_id = &case.id;
    let corpus = &case.corpus_version;
    let resource_attrs = format!(
        "ahu.eval.run_id={run_id},ahu.eval.case_id={case_id},ahu.eval.corpus_version={corpus},ahu.eval.stage={stage}"
    );
    if resource_attrs.len() > 512 {
        bail!("evaluation telemetry identity exceeds its bound");
    }
    // Do not inherit unrelated OTel resource values into an evaluation run.
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("--repo")
        .arg(&repo.root)
        .arg(agent)
        .arg("--headless")
        .arg("--timeout")
        .arg(timeout_seconds.to_string())
        .arg("--prompt-file")
        .arg(&prompt_path)
        .arg("--output")
        .arg("json")
        .env("AHU_EVAL_OTEL_ENDPOINT", otel_endpoint)
        .env("AHU_EVAL_LOCAL_METRICS", "1")
        .env("OTEL_RESOURCE_ATTRIBUTES", &resource_attrs)
        .env_remove("OTEL_EXPORTER_OTLP_HEADERS")
        .env_remove("OTEL_EXPORTER_OTLP_TRACES_HEADERS")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if allow_widened {
        command.arg("--allow-widened-approvals");
    }
    let mut child = command
        .spawn()
        .map_err(|error| Error::new(format!("could not start eval agent {agent}: {error}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::new("eval agent stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::new("eval agent stderr unavailable"))?;
    let out_reader = std::thread::spawn(move || drain_bounded(stdout, 2 * 1024 * 1024));
    let err_reader = std::thread::spawn(move || drain_bounded(stderr, 256 * 1024));
    let status = child.wait()?;
    let (stdout, out_exceeded) = out_reader
        .join()
        .map_err(|_| Error::new("eval stdout capture stopped unexpectedly"))??;
    let (stderr, err_exceeded) = err_reader
        .join()
        .map_err(|_| Error::new("eval stderr capture stopped unexpectedly"))??;
    if out_exceeded || err_exceeded {
        bail!("eval agent output exceeded the capture limit");
    }
    write_private_file(&output_dir.join("agent-stderr.log"), &stderr)?;
    let result: serde_json::Value = serde_json::from_slice(&stdout).map_err(|_| {
        let _ = write_private_file(&output_dir.join("agent-stdout.log"), &stdout);
        Error::new(format!(
            "eval agent {agent} returned no valid JSON result (exit {}); bounded stderr and stdout were saved under the external run artifacts at {}",
            status.code().unwrap_or(128),
            display_path(output_dir)
        ))
    })?;
    // Kept before the success check: the caller records what this launch
    // observed even when the launch is unusable.
    *envelope = Some(result.clone());
    if !status.success()
        || result.get("outcome").and_then(serde_json::Value::as_str) != Some("succeeded")
    {
        bail!(
            "eval agent {agent} did not complete successfully ({}); inspect the external run result for its task outcome",
            display_safe(
                result
                    .get("outcome")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("no outcome reported")
            )
        );
    }
    Ok(result)
}

fn drain_bounded<R: Read>(mut reader: R, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::with_capacity(limit.min(64 * 1024));
    let mut exceeded = false;
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let room = limit.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..count.min(room)]);
        exceeded |= count > room;
    }
    Ok((retained, exceeded))
}

fn safe_artifact(worktree: &Path, name: &str) -> Result<PathBuf> {
    let root = worktree
        .canonicalize()
        .map_err(|error| Error::new(format!("cannot locate task worktree: {error}")))?;
    let path = root.join(name);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| Error::new(format!("missing {name}: {error}")))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("{name} must be a regular file at the task worktree root");
    }
    let actual = path.canonicalize()?;
    if !actual.starts_with(&root) {
        bail!("{name} resolves outside the task worktree");
    }
    Ok(actual)
}

/// The harness version the launch reported, or `None` when it reported none.
///
/// Read from the launch envelope, which carries it under two names for the same
/// value. Either will do, and taking both means a launch that failed still
/// identifies the harness it ran on: without this the fingerprint is `partial`
/// and names `harness_version` missing although the envelope stated it.
fn harness_version(result: &serde_json::Value) -> Option<String> {
    [
        "/native_reference/harness_version",
        "/capabilities/harness_version",
    ]
    .into_iter()
    .filter_map(|pointer| result.pointer(pointer))
    .filter_map(serde_json::Value::as_str)
    .map(str::trim)
    .find(|version| !version.is_empty())
    .map(str::to_owned)
}

fn insert_trajectory_fields(
    record: &mut serde_json::Map<String, serde_json::Value>,
    envelope: Option<&serde_json::Value>,
    budgets: Option<&trajectory::Budgets>,
) -> Result<()> {
    // Typed projection: never copy unrecognized payload fields into eval records.
    let observation = envelope
        .and_then(|value| value.pointer("/harness/trajectory"))
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::from_value::<trajectory::Observation>(value.clone()).ok());
    if let Some(observation) = &observation {
        record.insert("trajectory".into(), serde_json::to_value(observation)?);
    }
    if let Some(budgets) = budgets {
        record.insert("trajectory_budgets".into(), serde_json::to_value(budgets)?);
        record.insert(
            "trajectory_budget_status".into(),
            serde_json::to_value(budgets.score(observation.as_ref()))?,
        );
    }
    Ok(())
}

/// Everything one attempt reported about token usage, from both of its sources.
///
/// The harness's own counters come from the launch envelope; the decision
/// service's come from the telemetry the receiver collected. Both are kept for
/// every terminal path, because an attempt can call the decision service and
/// then fail: dropping the service usage with the failure would make the attempt
/// look like it never spent anything there. `None` when neither source reported
/// a field, so an absent observation stays absent.
fn reported_tokens(
    envelope: Option<&serde_json::Value>,
    telemetry: Option<&crate::eval_otel::TaskTelemetry>,
) -> Option<serde_json::Value> {
    let mut tokens = envelope
        .and_then(|value| value.pointer("/metrics/values"))
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(amount) = envelope
        .and_then(|value| value.pointer("/harness/cost/usd"))
        .and_then(serde_json::Value::as_f64)
        .filter(|amount| amount.is_finite() && *amount >= 0.0)
    {
        tokens.insert(
            "ahu.cost.harness_reported_usd".into(),
            serde_json::json!({"kind":"observed_float","value":amount}),
        );
    }
    if let Some(telemetry) = telemetry {
        for (name, value) in [
            ("decision_service.input", telemetry.decision_input_tokens),
            ("decision_service.output", telemetry.decision_output_tokens),
            (
                "skill_selection_service.input_known",
                telemetry.selection_input_tokens,
            ),
            (
                "skill_selection_service.output_known",
                telemetry.selection_output_tokens,
            ),
        ] {
            if let Some(value) = value {
                tokens.insert(name.to_owned(), value.into());
            }
        }
    }
    (!tokens.is_empty()).then_some(serde_json::Value::Object(tokens))
}

/// Digest over the skills the launch reported, or `None` when it reported none.
///
/// Absent rather than `"unspecified"`: a fingerprint that could not be completed
/// has to be visibly incomplete, and a placeholder string would group with other
/// placeholder strings as though the inputs matched.
fn skill_digest(result: &serde_json::Value) -> Option<String> {
    let entries = result
        .get("skill_catalog")
        .and_then(serde_json::Value::as_array)?;
    let mut entries = entries
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("source")?.as_str()?,
                entry.get("digest")?.as_str()?,
            ))
        })
        .collect::<Vec<_>>();
    entries.sort_unstable();
    Some(crate::util::digest_bytes(
        &serde_json::to_vec(&entries).unwrap_or_default(),
    ))
}

/// One group as JSON.
///
/// Built in named parts rather than one literal: the object is wide enough that
/// a single `json!` hits the macro's recursion limit, and the parts also say
/// which fields belong together.
fn group_json(group: &Group) -> serde_json::Value {
    let key = &group.key;
    let mut object = serde_json::Map::new();
    let mut merge = |value: serde_json::Value| {
        if let serde_json::Value::Object(fields) = value {
            object.extend(fields);
        }
    };
    merge(serde_json::json!({
        "trajectory": group.trajectory,
        "case_id": key.case_id,
        "corpus_version": key.corpus_version,
        "stage": key.stage,
        "agent": key.agent,
        "agent_version": key.agent_version,
        "evaluator": key.evaluator,
        "evaluator_kind": key.evaluator_kind,
        "decision_evaluator_policy_digest": key.decision_evaluator_policy_digest,
        "decision_evaluator_policy": serde_json::from_str::<serde_json::Value>(&key.decision_evaluator_policy).unwrap_or_default(),
        "evaluator_version": key.evaluator_version,
        "evaluator_model": key.evaluator_model,
        "evaluator_harness": key.evaluator_harness,
        "evaluator_harness_version": key.evaluator_harness_version,
        "model": key.model,
        "harness": key.harness,
        "harness_version": key.harness_version,
    }));
    merge(serde_json::json!({
        "skill_digest": key.skill_digest,
        "selection_policy_digest": key.selection_policy_digest,
        "selection_mode": key.selection_mode,
        "case_schema_version": key.case_schema_version,
        "case_digest": key.case_digest,
        "prompt_profile": key.prompt_profile,
        "prompt_version": key.prompt_version,
        "scoring_version": key.scoring_version,
        "suite_id": key.suite_id,
        "suite_version": key.suite_version,
        "suite_digest": key.suite_digest,
        "agent_identity_digest": key.agent_identity_digest,
        "evaluator_identity_digest": key.evaluator_identity_digest,
        "evaluator_skill_digest": key.evaluator_skill_digest,
        "evaluator_repo_head": key.evaluator_repo_head,
        "blinding": key.blinding,
        "tool_definitions_digest": key.tool_definitions_digest,
        "ahu_version": key.ahu_version,
        "ahu_build_digest": key.ahu_build_digest,
        "target_repo_head": key.target_repo_head,
        "fingerprint_completeness": key.fingerprint_completeness,
        "input_fingerprint": key.input_fingerprint,
    }));
    merge(serde_json::json!({
        "runs": group.runs,
        "passed": group.passes,
        "mean_score": group.mean_score,
        "score_observations": group.score_observations,
        "pass_rate": group.pass_rate,
        "answer": {
            // Reliability: over every attempt, failed launches included.
            "passed": group.answer_passes,
            "pass_rate": group.answer_pass_rate,
            "pass_interval": stats::interval_json(group.answer_pass_interval),
            "mean_score": group.mean_answer_score,
            // Quality: over the attempts that produced a valid answer. Null
            // rather than zero when nothing was answered.
            "observations": group.answer_observations,
            "quality_passed": group.answer_quality_passes,
            "quality_rate": group.answer_quality_rate,
            "quality_interval": stats::interval_json(group.answer_quality_interval),
            "no_answer": group.attempts_without_answer,
        },
        "tool_expectations": {
            "pass": group.tool_pass,
            "fail": group.tool_fail,
            "unknown": group.tool_unknown,
            "not_applicable": group.tool_not_applicable,
            "pass_rate": group.tool_pass_rate,
            "pass_interval": stats::interval_json(group.tool_pass_interval),
        },
        "judge": {
            "scored": group.judge_scored,
            "passed": group.judge_passes,
            "failed": group.judge_failed,
            "mean_score": group.mean_judge_score,
            "criterion_means": group.judge_criterion_means,
            "reason_codes": group.judge_reason_codes,
            "calibration": "single_judge_uncalibrated",
        },
        "terminal_statuses": group.terminal_statuses,
        "attempts_without_answer": group.attempts_without_answer,
    }));
    merge(serde_json::json!({
        "coverage": {
            "token_observations": group.coverage.tokens,
            "timing_observations": group.coverage.timing,
            "launch_timing_observations": group.coverage.launch_timing,
            "decision_call_observations": group.coverage.decision_calls,
            "mcp_observations": group.coverage.mcp,
            "telemetry_complete_session": group.coverage.telemetry_complete,
            "telemetry_partial_spans": group.coverage.telemetry_partial,
            "telemetry_absent": group.coverage.telemetry_none,
            "telemetry_receiver": group.coverage.telemetry_receiver,
        },
        "telemetry_receiver": group.telemetry_receiver,
        "observed": {
            "mean_elapsed_ms": group.mean_elapsed_ms,
            "mean_launch_elapsed_ms": group.mean_launch_elapsed_ms,
            "mean_total_elapsed_ms": group.mean_total_elapsed_ms,
            "evaluator_metrics": group.evaluator_metrics,
            "mean_selection_elapsed_ms": group.mean_selection_elapsed_ms,
            "mean_selection_input_tokens": group.mean_selection_input_tokens,
            "mean_selection_output_tokens": group.mean_selection_output_tokens,
            "selection_fallbacks": group.selection_fallbacks,
            "selection_input_complete_runs": group.selection_input_complete_runs,
            "selection_output_complete_runs": group.selection_output_complete_runs,
            "selection_input_partial_runs": group.selection_input_partial_runs,
            "selection_output_partial_runs": group.selection_output_partial_runs,
            "mean_selection_partial_input_tokens": group.mean_selection_partial_input_tokens,
            "mean_selection_partial_output_tokens": group.mean_selection_partial_output_tokens,
            "selection_reported_models": group.selection_reported_models,
            "mean_candidate_selection": group.mean_candidate_selection,
            "candidate_selection_runs": group.candidate_selection_runs,
            "selection_telemetry_observations": group.selection_telemetry_observations,
            "mean_decision_calls": group.mean_decision_calls,
            "mean_decision_service_duration_ms": group.mean_decision_service_duration_ms,
            "mean_decision_request_bytes": group.mean_decision_request_bytes,
            "decision_request_runs": group.decision_request_runs,
            "decision_request_observations": group.decision_request_observations,
            "mean_mcp_requests": group.mean_mcp_requests,
            "mean_mcp_tool_lists": group.mean_mcp_tool_lists,
            "mean_mcp_tool_calls": group.mean_mcp_tool_calls,
            "mean_mcp_tool_errors": group.mean_mcp_tool_errors,
            "mean_typed_decision_errors": group.mean_typed_decision_errors,
            "mcp_tools": group.mcp_tools,
            "mcp_tool_errors": group.mcp_tool_errors,
            "mean_attempts": group.mean_attempts,
            "token_fields": group.token_fields,
            // Added alongside the field names, never in place of them: a
            // reader of the older contract keeps what it read, and one that
            // wants amounts gets them only for the fields a run measured.
            "mean_tokens": group.mean_tokens,
            "token_field_observations": group.token_field_observations,
            "reported_cost_fields": group.reported_cost_fields,
            "mean_reported_cost_usd": group.mean_reported_cost_usd,
            "reported_cost_observations": group.reported_cost_observations,
            "mean_total_tokens": group.mean_total_tokens(),
        },
    }));
    serde_json::Value::Object(object)
}

#[cfg(test)]
#[path = "eval_tests.rs"]
mod tests;

/// Selection output is evidence, not part of the policy identity: repetitions
/// with different suggestions must remain comparable.
fn selection_policy_digest(selection: &crate::skill_selection::Selection) -> Option<String> {
    if selection.mode == crate::skill_selection::Mode::None {
        return None;
    }
    Some(crate::util::digest_bytes(
        &serde_json::to_vec(&serde_json::json!({
            "mode":selection.mode, "version":selection.policy_version,
            "catalog":selection.catalog_digest,
            "configuration":selection.configuration,
        }))
        .expect("selection identity serializes"),
    ))
}

fn insert_selection_fields(
    record: &mut serde_json::Map<String, serde_json::Value>,
    selection: &crate::skill_selection::Selection,
    observed: bool,
    total_elapsed_ms: u64,
) -> Result<()> {
    record.insert("skill_selection".into(), serde_json::to_value(selection)?);
    record.insert("selection_telemetry_observed".into(), observed.into());
    record.insert("total_elapsed_ms".into(), total_elapsed_ms.into());
    Ok(())
}

fn insert_candidate_selection_fields(
    record: &mut serde_json::Map<String, serde_json::Value>,
    telemetry: Option<&crate::eval_otel::TaskTelemetry>,
) {
    let Some(t) = telemetry.filter(|t| t.selection_observations > 0) else {
        return;
    };
    let mut value = serde_json::json!({
        "observations":t.selection_observations,
        "duration_ms_known":t.selection_duration_ms,
        "input_tokens_known":t.selection_input_tokens,
        "output_tokens_known":t.selection_output_tokens,
        "input_complete_observations":t.selection_input_complete_observations,
        "output_complete_observations":t.selection_output_complete_observations,
    });
    value.as_object_mut().unwrap().retain(|_, v| !v.is_null());
    record.insert("candidate_selection".into(), value);
}

fn insert_evaluation_fields(
    record: &mut serde_json::Map<String, serde_json::Value>,
    evaluator: &decision::Observation,
    started: std::time::Instant,
) -> Result<()> {
    record.insert(
        "evaluation_elapsed_ms".into(),
        number(started.elapsed().as_secs_f64() * 1000.0),
    );
    record.insert("evaluator_metrics".into(), serde_json::to_value(evaluator)?);
    Ok(())
}

fn observe_evaluator_agent(
    observation: &mut decision::Observation,
    envelope: &serde_json::Value,
    receiver: &crate::eval_otel::Receiver,
) -> Result<()> {
    observation.task_id = envelope["task_id"].as_str().map(str::to_owned);
    observation.attempt = observation
        .task_id
        .as_ref()
        .map(|_| envelope["attempt"].as_u64().unwrap_or(1) as u32);
    let telemetry = observation
        .task_id
        .as_ref()
        .and_then(|id| receiver.task(id, observation.attempt.unwrap_or(1)));
    observation.telemetry_coverage = telemetry
        .as_ref()
        .map_or("none", |t| t.coverage().as_str())
        .into();
    observation.telemetry = telemetry.as_ref().map(serde_json::to_value).transpose()?;
    observation.reported_tokens = reported_tokens(Some(envelope), telemetry.as_ref())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
        .into_iter()
        .collect();
    Ok(())
}
