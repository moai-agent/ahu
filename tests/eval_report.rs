//! `ahu eval report`: argument parsing, the repository boundary for run
//! evidence, and what the two output surfaces say about coverage.
//!
//! The records are the compact JSONL rows `ahu eval run`
//! appends. Every fixture here is synthetic and lives in a `tempfile::TempDir`
//! outside the checkout under test, which is also what the boundary test
//! inverts deliberately.

mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use ahu::cli::{self, Command};
use common::TestRepo;

/// One synthetic record with the fields the report groups and scores by.
struct Row {
    agent: &'static str,
    score: f64,
    passed: bool,
    elapsed_ms: Option<i64>,
    decision_calls: Option<i64>,
    tokens: Option<&'static str>,
}

impl Row {
    fn candidate(agent: &'static str, score: f64, passed: bool) -> Self {
        Row {
            agent,
            score,
            passed,
            elapsed_ms: None,
            decision_calls: None,
            tokens: None,
        }
    }

    fn json(&self) -> String {
        let mut fields = vec![
            "\"schema_version\":2".to_string(),
            "\"case_id\":\"synthetic-ticket-routing-001\"".to_string(),
            "\"corpus_version\":\"1.0.0\"".to_string(),
            "\"stage\":\"candidate\"".to_string(),
            format!("\"agent\":\"{}\"", self.agent),
            "\"agent_version\":\"1.0.0\"".to_string(),
            "\"evaluator\":\"unspecified\"".to_string(),
            "\"evaluator_version\":\"unspecified\"".to_string(),
            "\"model\":\"ollama/fixture-model\"".to_string(),
            "\"harness\":\"opencode\"".to_string(),
            "\"harness_version\":\"1.0.0\"".to_string(),
            "\"skill_digest\":\"sha256:fixture\"".to_string(),
            format!("\"score\":{}", self.score),
            format!("\"passed\":{}", self.passed),
        ];
        if let Some(value) = self.elapsed_ms {
            fields.push(format!("\"elapsed_ms\":{value}"));
        }
        if let Some(value) = self.decision_calls {
            fields.push(format!("\"decision_call_count\":{value}"));
        }
        if let Some(tokens) = self.tokens {
            fields.push(format!("\"reported_tokens\":{tokens}"));
        }
        format!("{{{}}}", fields.join(","))
    }
}

/// Write records to a file outside every repository, as the recorder requires.
fn records_outside(dir: &Path, rows: &[Row]) -> PathBuf {
    let path = dir.join("runs.jsonl");
    let body: String = rows.iter().map(|row| format!("{}\n", row.json())).collect();
    std::fs::write(&path, body).expect("write records");
    path
}

/// Run `ahu eval report` from `repo` with a clean, harness-free environment.
fn run(repo: &TestRepo, args: &[&str]) -> std::process::Output {
    run_at(repo, args, Some("80"), true)
}

/// The same, laid out for an explicit terminal width and with colour chosen.
///
/// `COLUMNS` is set on every run so a developer's own terminal size cannot
/// change what these tests read, and the table's layout is the test's to pick.
fn run_at(
    repo: &TestRepo,
    args: &[&str],
    width: Option<&str>,
    no_color: bool,
) -> std::process::Output {
    let mut command = common::ahu();
    command
        .current_dir(repo.path())
        .arg("eval")
        .arg("report")
        .args(args)
        .env_remove("COLUMNS")
        .env_remove("NO_COLOR")
        .env("AHU_CMUX_BIN", repo.state.path().join("missing-cmux"))
        .stdin(Stdio::null());
    if let Some(width) = width {
        command.env("COLUMNS", width);
    }
    if no_color {
        command.env("NO_COLOR", "1");
    }
    command.output().expect("ahu runs")
}

/// Terminal columns a rendered line occupies. Every fixture here is ASCII
/// apart from the ellipsis and the em dash, which are one column each.
fn columns_of(line: &str) -> usize {
    line.chars().count()
}

/// The comparison table: the header line and the rows under it.
///
/// The header is found by its own column names rather than by position,
/// because a painted header begins with a style escape rather than with
/// `CASE`.
fn summary_table(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .skip_while(|line| !(line.contains("CASE") && line.contains("TOKENS")))
        .take_while(|line| !line.trim().is_empty())
        .collect()
}

#[test]
fn parsing_requires_a_records_path_and_accepts_only_json_output() {
    assert_eq!(
        cli::parse(["eval", "report", "--records", "/outside/runs.jsonl"]).unwrap(),
        Command::EvalReport {
            records: PathBuf::from("/outside/runs.jsonl"),
            output_json: false,
        }
    );
    assert_eq!(
        cli::parse([
            "eval",
            "report",
            "--records",
            "/outside/runs.jsonl",
            "--output",
            "json",
        ])
        .unwrap(),
        Command::EvalReport {
            records: PathBuf::from("/outside/runs.jsonl"),
            output_json: true,
        }
    );
    for args in [
        vec!["eval"],
        vec!["eval", "report"],
        vec!["eval", "trend", "--records", "/outside/runs.jsonl"],
        vec!["eval", "report", "--records"],
        vec![
            "eval",
            "report",
            "--records",
            "/a.jsonl",
            "--records",
            "/b.jsonl",
        ],
        vec![
            "eval",
            "report",
            "--records",
            "/a.jsonl",
            "--output",
            "yaml",
        ],
        vec!["eval", "report", "--records", "/a.jsonl", "--verbose"],
    ] {
        let error = cli::parse(args.clone()).expect_err("refused");
        assert_eq!(
            error.kind(),
            ahu::util::ErrorKind::Usage,
            "{args:?} is a usage error"
        );
    }
}

#[test]
fn eval_run_requires_named_candidate_case_and_external_records_options() {
    assert_eq!(
        cli::parse([
            "eval",
            "run",
            "--case",
            "/outside/case.md",
            "--agent",
            "@triage",
            "--records",
            "/outside/runs.jsonl",
            "--evaluator",
            "@judge",
            "--runs",
            "3",
            "--timeout",
            "120",
            "--output",
            "json"
        ])
        .unwrap(),
        Command::EvalRun {
            case: Some(PathBuf::from("/outside/case.md")),
            suite: None,
            agents: vec!["@triage".into()],
            evaluator: Some("@judge".into()),
            evaluator_repo: None,
            records: PathBuf::from("/outside/runs.jsonl"),
            runs: 3,
            timeout_seconds: 120,
            allow_widened_approvals: false,
            output_json: true,
        }
    );
    // A suite with several candidates is the new shape, and the agent order is
    // preserved because it is the order the trials run in.
    assert_eq!(
        cli::parse([
            "eval",
            "run",
            "--suite",
            "/outside/suite.md",
            "--agent",
            "@triage",
            "--agent",
            "@sorter",
            "--records",
            "/outside/runs.jsonl",
            "--evaluator",
            "@judge",
            "--evaluator-repo",
            "/outside/judge-checkout",
        ])
        .unwrap(),
        Command::EvalRun {
            case: None,
            suite: Some(PathBuf::from("/outside/suite.md")),
            agents: vec!["@triage".into(), "@sorter".into()],
            evaluator: Some("@judge".into()),
            evaluator_repo: Some(PathBuf::from("/outside/judge-checkout")),
            records: PathBuf::from("/outside/runs.jsonl"),
            runs: 1,
            timeout_seconds: 1800,
            allow_widened_approvals: false,
            output_json: false,
        }
    );
    for args in [
        vec!["eval", "run"],
        vec![
            "eval",
            "run",
            "--case",
            "/case.md",
            "--agent",
            "triage",
            "--records",
            "/runs.jsonl",
        ],
        vec![
            "eval",
            "run",
            "--case",
            "/case.md",
            "--agent",
            "@triage",
            "--records",
            "/runs.jsonl",
            "--runs",
            "101",
        ],
        vec![
            "eval",
            "run",
            "--case",
            "/case.md",
            "--agent",
            "@triage",
            "--records",
            "/runs.jsonl",
            "--timeout",
            "0",
        ],
        vec![
            "eval",
            "run",
            "--case",
            "/case.md",
            "--agent",
            "@triage",
            "--records",
            "/runs.jsonl",
            "--verbose",
        ],
        // --case and --suite are alternatives, not a pair.
        vec![
            "eval",
            "run",
            "--case",
            "/case.md",
            "--suite",
            "/suite.md",
            "--agent",
            "@triage",
            "--records",
            "/runs.jsonl",
        ],
        // Neither one given.
        vec![
            "eval",
            "run",
            "--agent",
            "@triage",
            "--records",
            "/runs.jsonl",
        ],
        // The same candidate twice would run it twice and report it as two.
        vec![
            "eval",
            "run",
            "--suite",
            "/suite.md",
            "--agent",
            "@triage",
            "--agent",
            "triage",
            "--records",
            "/runs.jsonl",
        ],
        // Isolation needs an evaluator to isolate.
        vec![
            "eval",
            "run",
            "--case",
            "/case.md",
            "--agent",
            "@triage",
            "--records",
            "/runs.jsonl",
            "--evaluator-repo",
            "/elsewhere",
        ],
    ] {
        let error = cli::parse(args.clone()).expect_err("invalid eval run options refused");
        assert_eq!(error.kind(), ahu::util::ErrorKind::Usage, "{args:?}");
    }
}

#[test]
fn records_inside_the_checkout_are_refused_including_through_a_symlink() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let rows = [Row::candidate("@triage", 1.0, true)];
    let inside = repo.path().join("evals/runs.jsonl");
    std::fs::create_dir_all(inside.parent().unwrap()).expect("create dir");
    std::fs::write(&inside, format!("{}\n", rows[0].json())).expect("write records");

    let output = run(&repo, &["--records", inside.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("must live outside this repository"),
        "{stderr}"
    );

    // A link that lives outside but resolves inside is the same refusal: the
    // path is canonicalised before it is compared.
    let link = outside.path().join("link.jsonl");
    std::os::unix::fs::symlink(&inside, &link).expect("symlink");
    let output = run(&repo, &["--records", link.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("must live outside this repository"),
        "{stderr}"
    );
}

#[test]
fn a_records_file_outside_the_repository_is_neither_read_nor_written_inside_it() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let records = records_outside(outside.path(), &[Row::candidate("@triage", 1.0, true)]);
    let before = common::git(repo.path(), &["status", "--porcelain"]);
    let output = run(&repo, &["--records", records.to_str().unwrap()]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        before,
        common::git(repo.path(), &["status", "--porcelain"]),
        "eval report must leave no artifact in the checkout"
    );
}

#[test]
fn candidates_are_grouped_and_coverage_stays_separate_from_measured_zeros() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let records = records_outside(
        outside.path(),
        &[
            Row {
                elapsed_ms: Some(0),
                decision_calls: Some(0),
                tokens: Some("{\"input\":0,\"output\":0}"),
                ..Row::candidate("@triage", 1.0, true)
            },
            // Same configuration, no telemetry at all.
            Row::candidate("@triage", 0.5, false),
            // A different candidate is its own row.
            Row {
                elapsed_ms: Some(200),
                ..Row::candidate("@sorter", 1.0, true)
            },
        ],
    );
    let output = run(
        &repo,
        &["--records", records.to_str().unwrap(), "--output", "json"],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is the JSON contract");
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["record_count"], 3);
    let groups = report["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 2);

    let sorter = groups
        .iter()
        .find(|group| group["agent"] == "@sorter")
        .expect("@sorter row");
    assert_eq!(sorter["runs"], 1);
    assert_eq!(sorter["mean_score"], 1.0);
    assert_eq!(sorter["pass_rate"], 1.0);
    assert_eq!(sorter["coverage"]["timing_observations"], 1);
    assert_eq!(sorter["coverage"]["token_observations"], 0);
    assert!(sorter["observed"]["mean_decision_calls"].is_null());

    let triage = groups
        .iter()
        .find(|group| group["agent"] == "@triage")
        .expect("@triage row");
    assert_eq!(triage["runs"], 2);
    assert_eq!(triage["mean_score"], 0.75);
    assert_eq!(triage["pass_rate"], 0.5);
    // One of the two runs reported; the reported values were zero, and the run
    // that reported nothing is missing coverage rather than another zero.
    assert_eq!(triage["coverage"]["timing_observations"], 1);
    assert_eq!(triage["coverage"]["decision_call_observations"], 1);
    assert_eq!(triage["coverage"]["token_observations"], 1);
    assert_eq!(triage["observed"]["mean_elapsed_ms"], 0.0);
    assert_eq!(triage["observed"]["mean_decision_calls"], 0.0);
    assert_eq!(
        triage["observed"]["token_fields"],
        serde_json::json!(["input", "output"])
    );
    // The machine contract carries no local path.
    let body = String::from_utf8_lossy(&output.stdout);
    assert!(!body.contains(outside.path().to_str().unwrap()), "{body}");

    // Without --output json the readable report owns stdout.
    let output = run(&repo, &["--records", records.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains("synthetic-ticket-routing-001"), "{stdout}");
    assert!(stdout.contains("@triage"), "{stdout}");
    assert!(stdout.contains("coverage   tokens 1/2"), "{stdout}");
    assert!(stdout.contains("decision calls 0/1"), "{stdout}");
    assert!(stdout.contains("none observed"), "{stdout}");
}

#[test]
fn a_malformed_record_names_its_line_and_fails_as_a_usage_error() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let records = outside.path().join("runs.jsonl");
    let good = Row::candidate("@triage", 1.0, true).json();
    std::fs::write(&records, format!("{good}\n\n{good}\nnot json\n")).expect("write records");
    let output = run(&repo, &["--records", records.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    // Blank lines are skipped but still counted, so the number is the file's.
    assert!(stderr.contains("runs.jsonl:4:"), "{stderr}");
    assert!(output.stdout.is_empty(), "nothing is reported on a refusal");

    // A record missing a field the report groups by is refused by name.
    std::fs::write(
        &records,
        "{\"schema_version\":2,\"model\":\"m\",\"harness\":\"h\",\"score\":1,\"passed\":true}\n",
    )
    .expect("write records");
    let output = run(&repo, &["--records", records.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("runs.jsonl:1:"), "{stderr}");
    assert!(stderr.contains("missing field `case_id`"), "{stderr}");
}

#[test]
fn a_missing_records_file_is_a_usage_error_naming_the_path() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let missing = outside.path().join("absent.jsonl");
    let output = run(&repo, &["--records", missing.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("absent.jsonl"), "{stderr}");

    // A directory is not a record file either.
    let output = run(&repo, &["--records", outside.path().to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("not a regular file"), "{stderr}");
}

/// A record schema 2 row, with `extra` raw JSON fields added or replaced.
fn v2_row(extra: &[(&str, &str)]) -> String {
    let mut fields: std::collections::BTreeMap<&str, String> = [
        ("schema_version", "2".to_string()),
        ("case_id", "\"routing-1\"".to_string()),
        ("corpus_version", "\"1.0.0\"".to_string()),
        ("case_schema_version", "2".to_string()),
        ("case_digest", format!("\"{}\"", "a".repeat(64))),
        ("prompt_profile", "\"tool_neutral_v2\"".to_string()),
        ("prompt_version", "2".to_string()),
        ("scoring_version", "2".to_string()),
        ("stage", "\"candidate\"".to_string()),
        ("agent", "\"triage\"".to_string()),
        ("agent_version", "\"1.0.0\"".to_string()),
        ("agent_identity_digest", format!("\"{}\"", "b".repeat(64))),
        ("model", "\"ollama/fixture\"".to_string()),
        ("harness", "\"opencode\"".to_string()),
        ("harness_version", "\"1.18.32\"".to_string()),
        ("skill_digest", format!("\"{}\"", "c".repeat(64))),
        ("tool_definitions_digest", format!("\"{}\"", "d".repeat(64))),
        ("ahu_version", "\"0.5.0\"".to_string()),
        ("ahu_build_digest", format!("\"{}\"", "e".repeat(64))),
        ("target_repo_head", "\"deadbeef\"".to_string()),
        ("input_fingerprint", format!("\"{}\"", "f".repeat(64))),
        ("fingerprint_completeness", "\"complete\"".to_string()),
        ("blinding", "\"prompt_only\"".to_string()),
        ("score", "1.0".to_string()),
        ("passed", "true".to_string()),
        ("answer_score", "1.0".to_string()),
        ("answer_passed", "true".to_string()),
        ("telemetry_coverage", "\"complete_session\"".to_string()),
        ("terminal_status", "\"candidate_scored\"".to_string()),
    ]
    .into_iter()
    .collect();
    for (name, value) in extra {
        fields.insert(name, (*value).to_string());
    }
    let body = fields
        .iter()
        .map(|(name, value)| format!("\"{name}\":{value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{body}}}")
}

fn report_json(repo: &TestRepo, records: &Path) -> serde_json::Value {
    let output = run(
        repo,
        &["--records", records.to_str().unwrap(), "--output", "json"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout is the JSON contract")
}

fn write_lines(dir: &Path, lines: &[String]) -> PathBuf {
    let path = dir.join("runs.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").expect("write records");
    path
}

#[test]
fn records_must_use_schema_two() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    for (row, expected) in [
        (v2_row(&[("schema_version", "1")]), "expected 2"),
        (v2_row(&[("schema_version", "3")]), "expected 2"),
        (
            "{\"case_id\":\"c\",\"model\":\"m\",\"harness\":\"h\",\"score\":1,\"passed\":true}"
                .into(),
            "schema_version",
        ),
    ] {
        let records = write_lines(outside.path(), &[row]);
        let output = run(&repo, &["--records", records.to_str().unwrap()]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{stderr}");
        assert!(stderr.contains(expected), "{stderr}");
    }
}

#[test]
fn any_differing_input_fingerprint_splits_a_group() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    for field in [
        ("case_digest", format!("\"{}\"", "9".repeat(64))),
        ("case_schema_version", "3".to_string()),
        ("prompt_profile", "\"manual_unverified\"".to_string()),
        ("prompt_version", "3".to_string()),
        ("scoring_version", "3".to_string()),
        ("agent_identity_digest", format!("\"{}\"", "9".repeat(64))),
        (
            "evaluator_identity_digest",
            format!("\"{}\"", "9".repeat(64)),
        ),
        ("blinding", "\"isolated\"".to_string()),
        ("tool_definitions_digest", format!("\"{}\"", "9".repeat(64))),
        ("ahu_version", "\"9.9.9\"".to_string()),
        ("ahu_build_digest", format!("\"{}\"", "9".repeat(64))),
        ("target_repo_head", "\"cafebabe\"".to_string()),
        ("skill_digest", format!("\"{}\"", "9".repeat(64))),
        ("suite_id", "\"routing-suite\"".to_string()),
        ("suite_version", "\"2.0.0\"".to_string()),
        ("suite_digest", format!("\"{}\"", "9".repeat(64))),
        ("input_fingerprint", format!("\"{}\"", "9".repeat(64))),
    ] {
        let records = write_lines(
            outside.path(),
            &[v2_row(&[]), v2_row(&[(field.0, field.1.as_str())])],
        );
        let report = report_json(&repo, &records);
        assert_eq!(
            report["groups"].as_array().expect("groups").len(),
            2,
            "{} must split the group",
            field.0
        );
    }
}

#[test]
fn a_partial_fingerprint_is_never_pooled_with_a_complete_one() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    // Identical in every declared field; one simply could not name its build.
    let partial = v2_row(&[
        ("ahu_build_digest", "null"),
        ("fingerprint_completeness", "\"partial\""),
    ]);
    let records = write_lines(outside.path(), &[v2_row(&[]), partial]);
    let report = report_json(&repo, &records);
    let groups = report["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 2);
    let completeness: Vec<&str> = groups
        .iter()
        .map(|group| group["fingerprint_completeness"].as_str().expect("flag"))
        .collect();
    assert!(completeness.contains(&"complete"), "{completeness:?}");
    assert!(completeness.contains(&"partial"), "{completeness:?}");
}

#[test]
fn binary_pass_rates_carry_wilson_intervals_and_an_empty_sample_carries_none() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");

    // Four runs, all answers passing, all tool expectations passing.
    let pass = v2_row(&[("tool_expectation_status", "\"pass\"")]);
    let records = write_lines(outside.path(), &vec![pass; 4]);
    let report = report_json(&repo, &records);
    let group = &report["groups"][0];
    assert_eq!(group["answer"]["pass_rate"], 1.0);
    let interval = &group["answer"]["pass_interval"];
    assert_eq!(interval["method"], "wilson");
    assert_eq!(interval["level"], 0.95);
    assert_eq!(interval["samples"], 4);
    assert_eq!(interval["upper"], 1.0);
    // All-pass keeps a nonzero width: four runs cannot establish certainty.
    let lower = interval["lower"].as_f64().expect("lower");
    assert!(lower > 0.0 && lower < 1.0, "{interval}");
    assert_eq!(group["tool_expectations"]["pass"], 4);
    assert_eq!(group["tool_expectations"]["pass_rate"], 1.0);
    assert_eq!(group["tool_expectations"]["pass_interval"]["samples"], 4);

    // All-fail keeps a nonzero width at the other end.
    let fail = v2_row(&[
        ("answer_passed", "false"),
        ("passed", "false"),
        ("answer_score", "0.0"),
        ("score", "0.0"),
        ("tool_expectation_status", "\"fail\""),
    ]);
    let records = write_lines(outside.path(), &vec![fail; 3]);
    let group = &report_json(&repo, &records)["groups"][0];
    assert_eq!(group["answer"]["pass_rate"], 0.0);
    assert_eq!(group["answer"]["pass_interval"]["lower"], 0.0);
    let upper = group["answer"]["pass_interval"]["upper"]
        .as_f64()
        .expect("upper");
    assert!(upper > 0.0 && upper < 1.0, "{group}");

    // Tool expectations nothing could decide: no rate and no interval, and the
    // unknown runs are visible rather than silently counted as passes.
    let records = write_lines(
        outside.path(),
        &vec![v2_row(&[("tool_expectation_status", "\"unknown\"")]); 2],
    );
    let group = &report_json(&repo, &records)["groups"][0];
    assert_eq!(group["tool_expectations"]["unknown"], 2);
    assert_eq!(group["tool_expectations"]["pass"], 0);
    assert!(group["tool_expectations"]["pass_rate"].is_null(), "{group}");
    assert!(
        group["tool_expectations"]["pass_interval"].is_null(),
        "{group}"
    );

    // A case with no tool expectations at all is not_applicable, not a pass.
    let records = write_lines(outside.path(), &[v2_row(&[])]);
    let group = &report_json(&repo, &records)["groups"][0];
    assert_eq!(group["tool_expectations"]["not_applicable"], 1);
    assert!(group["tool_expectations"]["pass_rate"].is_null(), "{group}");

    // Mixed decided and undecidable: the rate covers only what was decided.
    let records = write_lines(
        outside.path(),
        &[
            v2_row(&[("tool_expectation_status", "\"pass\"")]),
            v2_row(&[("tool_expectation_status", "\"fail\"")]),
            v2_row(&[("tool_expectation_status", "\"unknown\"")]),
        ],
    );
    let group = &report_json(&repo, &records)["groups"][0];
    assert_eq!(group["runs"], 3);
    assert_eq!(group["tool_expectations"]["pass_rate"], 0.5);
    assert_eq!(group["tool_expectations"]["pass_interval"]["samples"], 2);
}

#[test]
fn judge_detail_survives_into_the_report_and_a_judge_failure_is_not_a_candidate_failure() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let scored = v2_row(&[
        ("judge_status", "\"scored\""),
        ("judge_score", "0.75"),
        ("judge_passed", "false"),
        ("judge_criterion_scores", "{\"route\":0.75}"),
        ("judge_reason_codes", "[\"wrong_queue\"]"),
        ("evaluator", "\"judge\""),
        ("evaluator_version", "\"1.0.0\""),
    ]);
    // Same configuration, but the judge itself failed. The answer still passed.
    let judge_failed = v2_row(&[
        ("judge_status", "\"failed\""),
        ("evaluator", "\"judge\""),
        ("evaluator_version", "\"1.0.0\""),
        ("terminal_status", "\"candidate_scored_judge_failed\""),
        ("failure_category", "\"evaluator_run_failed\""),
    ]);
    let records = write_lines(outside.path(), &[scored, judge_failed]);
    let group = &report_json(&repo, &records)["groups"][0];
    assert_eq!(group["runs"], 2);
    assert_eq!(group["judge"]["scored"], 1);
    assert_eq!(group["judge"]["failed"], 1);
    assert_eq!(group["judge"]["passed"], 0);
    assert_eq!(group["judge"]["mean_score"], 0.75);
    // The judge's own evidence, not just its number.
    assert_eq!(group["judge"]["criterion_means"]["route"], 0.75);
    assert_eq!(group["judge"]["reason_codes"]["wrong_queue"], 1);
    assert_eq!(group["judge"]["calibration"], "single_judge_uncalibrated");
    // The candidate answered correctly in both runs; the judge failure did not
    // turn into a candidate failure.
    assert_eq!(group["answer"]["passed"], 2);
    assert_eq!(group["answer"]["pass_rate"], 1.0);
    assert_eq!(group["terminal_statuses"]["candidate_scored"], 1);
    assert_eq!(
        group["terminal_statuses"]["candidate_scored_judge_failed"],
        1
    );
}

#[test]
fn telemetry_coverage_distinguishes_complete_partial_and_absent_observation() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let records = write_lines(
        outside.path(),
        &[
            v2_row(&[("telemetry_coverage", "\"complete_session\"")]),
            v2_row(&[("telemetry_coverage", "\"partial_spans\"")]),
            v2_row(&[("telemetry_coverage", "\"none\"")]),
        ],
    );
    let group = &report_json(&repo, &records)["groups"][0];
    assert_eq!(group["coverage"]["telemetry_complete_session"], 1);
    assert_eq!(group["coverage"]["telemetry_partial_spans"], 1);
    assert_eq!(group["coverage"]["telemetry_absent"], 1);
}

#[test]
fn tool_error_counts_are_retained_by_name_and_separately_from_call_counts() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let records = write_lines(
        outside.path(),
        &[v2_row(&[
            ("mcp_observed", "true"),
            ("mcp_tool_call_count", "4"),
            ("mcp_tool_error_count", "1"),
            ("typed_decision_error_count", "1"),
            ("mcp_tools", "{\"ahu_typed_decide\":4}"),
            ("mcp_tool_errors_by_name", "{\"ahu_typed_decide\":1}"),
        ])],
    );
    let group = &report_json(&repo, &records)["groups"][0];
    assert_eq!(group["observed"]["mean_mcp_tool_calls"], 4.0);
    assert_eq!(group["observed"]["mean_mcp_tool_errors"], 1.0);
    assert_eq!(group["observed"]["mean_typed_decision_errors"], 1.0);
    assert_eq!(group["observed"]["mcp_tools"]["ahu_typed_decide"], 4.0);
    assert_eq!(
        group["observed"]["mcp_tool_errors"]["ahu_typed_decide"],
        1.0
    );

    // An error count attributed to a tool ahu does not serve is refused.
    let records = write_lines(
        outside.path(),
        &[v2_row(&[
            ("mcp_observed", "true"),
            ("mcp_tool_errors_by_name", "{\"not_a_tool\":1}"),
        ])],
    );
    let output = run(&repo, &["--records", records.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn unknown_per_tool_counts_remain_absent_even_with_a_session_summary() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let records = write_lines(
        outside.path(),
        &[v2_row(&[
            ("mcp_observed", "true"),
            ("mcp_tool_call_count", "3"),
            ("mcp_tools", "{\"ahu_typed_decide\":1}"),
        ])],
    );
    let tools = &report_json(&repo, &records)["groups"][0]["observed"]["mcp_tools"];
    assert_eq!(tools["ahu_typed_decide"], 1.0);
    assert!(tools.get("ahu_agents_list").is_none());
    assert!(tools.get("ahu_task_get").is_none());
    assert!(tools.get("ahu_tasks_list").is_none());
}

#[test]
fn receiver_drop_counts_are_preserved_and_summarized() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let records = write_lines(
        outside.path(),
        &[v2_row(&[(
            "telemetry_receiver",
            "{\"rejected_spans\":2,\"rejected_requests\":1,\"rejected_connections\":3,\"accept_errors\":0}",
        )])],
    );
    let group = &report_json(&repo, &records)["groups"][0];
    assert_eq!(group["coverage"]["telemetry_receiver"], 1);
    assert_eq!(group["telemetry_receiver"]["rejected_spans"], 2);
    assert_eq!(group["telemetry_receiver"]["rejected_requests"], 1);
    assert_eq!(group["telemetry_receiver"]["rejected_connections"], 3);
}

#[test]
fn the_report_says_what_it_does_not_claim() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let records = write_lines(outside.path(), &[v2_row(&[])]);
    let report = report_json(&repo, &records);
    assert_eq!(
        report["caveats"]["judge_calibration"],
        "single_judge_uncalibrated"
    );
    assert_eq!(report["caveats"]["judge_repeats"], "not_implemented");
    assert!(
        report["caveats"]["mean_score_interval"]
            .as_str()
            .expect("caveat")
            .starts_with("not_reported"),
        "no inferential interval is claimed for the weighted continuous mean"
    );

    // The readable view says the same things.
    let output = run(&repo, &["--records", records.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("95% Wilson"), "{stdout}");
    assert!(stdout.contains("uncalibrated"), "{stdout}");
    assert!(stdout.contains("neither a pass nor a fail"), "{stdout}");
    assert!(
        stdout.contains("All records use evaluation schema 2."),
        "{stdout}"
    );
}

/// Two candidates on one case, as a reader comparing agents would record them.
fn two_candidate_rows() -> Vec<String> {
    let mut rows = Vec::new();
    for (elapsed, total, tools) in [(41_000, 18_000, "pass"), (46_000, 19_400, "fail")] {
        rows.push(v2_row(&[
            ("case_id", "\"synthetic-ticket-routing-001\""),
            ("agent", "\"@triage\""),
            ("model", "\"ollama/fixture-model\""),
            ("elapsed_ms", &elapsed.to_string()),
            (
                "reported_tokens",
                &format!(
                    "{{\"ahu.tokens.input\":{{\"kind\":\"observed\",\"value\":14000}},\
                      \"ahu.tokens.total\":{{\"kind\":\"observed\",\"value\":{total}}},\
                      \"ahu.tokens.cached\":{{\"kind\":\"unavailable\"}}}}"
                ),
            ),
            ("tool_expectation_status", &format!("\"{tools}\"")),
            ("input_fingerprint", &format!("\"{}\"", "1".repeat(64))),
        ]));
    }
    // A second candidate that reported no timing, no tokens, and no decidable
    // tool expectation: every one of its measurements is missing, not zero.
    for passed in ["true", "false"] {
        rows.push(v2_row(&[
            ("case_id", "\"synthetic-ticket-routing-001\""),
            ("agent", "\"@sorter\""),
            ("model", "\"ollama/other-model\""),
            ("passed", passed),
            ("answer_passed", passed),
            ("score", if passed == "true" { "1.0" } else { "0.0" }),
            ("answer_score", if passed == "true" { "1.0" } else { "0.0" }),
            ("tool_expectation_status", "\"unknown\""),
            ("input_fingerprint", &format!("\"{}\"", "2".repeat(64))),
        ]));
    }
    rows
}

#[test]
fn the_report_opens_with_a_table_that_fits_the_terminal_and_never_wraps() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let records = write_lines(outside.path(), &two_candidate_rows());

    for width in ["88", "120"] {
        let output = run_at(
            &repo,
            &["--records", records.to_str().unwrap()],
            Some(width),
            true,
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let table = summary_table(&stdout);
        assert_eq!(
            table.len(),
            3,
            "a header and one row per candidate: {stdout}"
        );
        let width: usize = width.parse().expect("a width");
        for line in &table {
            assert!(
                columns_of(line) <= width,
                "{line:?} is wider than {width} columns"
            );
        }
        // The table leads the report: the detail blocks come after it.
        let table_end = stdout.find(table[2]).expect("the last row");
        let first_block = stdout.find("  agent      ").expect("a detail block");
        assert!(table_end < first_block, "{stdout}");

        let triage = table
            .iter()
            .find(|line| line.contains("@triage"))
            .expect("the @triage row");
        assert!(triage.contains("@triage@1.0.0"), "{triage}");
        // Mean of 41.0s and 46.0s, and of 18000 and 19400 total tokens.
        assert!(triage.contains("43.5s"), "{triage}");
        assert!(triage.contains("18.7k"), "{triage}");
        let sorter = table
            .iter()
            .find(|line| line.contains("@sorter"))
            .expect("the @sorter row");
        // Nothing timed, nothing counted, nothing decided: three dashes, and
        // the answer rate it did measure.
        assert_eq!(sorter.matches('\u{2014}').count(), 3, "{sorter}");
        assert!(sorter.contains("1/2"), "{sorter}");
    }

    // At 120 every column is shown whole; at 88 the case id is cut first,
    // because it is the same on both rows while the rest is what they differ by.
    let wide = run_at(
        &repo,
        &["--records", records.to_str().unwrap()],
        Some("120"),
        true,
    );
    let wide = String::from_utf8_lossy(&wide.stdout);
    let wide = summary_table(&wide);
    assert!(
        wide.iter().all(|line| !line.contains('\u{2026}')),
        "{wide:?}"
    );
    assert!(
        wide[0].split_whitespace().collect::<Vec<_>>()
            == [
                "CASE", "AGENT", "RUNTIME", "ANSWER", "TOOLS", "TIME", "TOKENS"
            ],
        "{:?}",
        wide[0]
    );
    let narrow = run_at(
        &repo,
        &["--records", records.to_str().unwrap()],
        Some("88"),
        true,
    );
    let narrow = String::from_utf8_lossy(&narrow.stdout);
    let narrow = summary_table(&narrow);
    assert!(narrow[1].starts_with("synthetic-tic\u{2026}"), "{narrow:?}");
    assert!(narrow[1].contains("@sorter@1.0.0"), "{narrow:?}");
}

#[test]
fn the_table_honors_no_color_and_paints_its_roles_when_colour_is_asked_for() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let records = write_lines(outside.path(), &two_candidate_rows());
    let args = ["--records", records.to_str().unwrap()];

    // NO_COLOR is honoured even when stdout would otherwise be painted.
    let plain = run_at(&repo, &args, Some("120"), true);
    let plain = String::from_utf8_lossy(&plain.stdout);
    assert!(!plain.contains('\u{1b}'), "{plain}");
    assert!(summary_table(&plain)[0].starts_with("CASE"), "{plain}");
    assert_eq!(summary_table(&plain).len(), 3, "{plain}");

    // Asked for explicitly, the table carries the same roles as `ahu tasks`:
    // a bold header, the agent and the runtime, and the gap role on a metric
    // no run reported.
    let painted = run_at(
        &repo,
        &["--color", "always", args[0], args[1]],
        Some("120"),
        true,
    );
    let painted = String::from_utf8_lossy(&painted.stdout);
    let table = summary_table(&painted);
    assert!(
        table[0].starts_with("\u{1b}[1mCASE\u{1b}[0m"),
        "{:?}",
        table[0]
    );
    let sorter = table
        .iter()
        .find(|line| line.contains("@sorter"))
        .expect("the @sorter row");
    assert!(
        sorter.contains("\u{1b}[1;36m@sorter@1.0.0\u{1b}[0m"),
        "{sorter:?}"
    );
    assert!(
        sorter.contains("\u{1b}[36mopencode / ollama/other-model\u{1b}[0m"),
        "{sorter:?}"
    );
    assert!(
        sorter.contains("\u{1b}[1;33m\u{2014}\u{1b}[0m"),
        "{sorter:?}"
    );
    // A painted row still ends in content rather than styled padding.
    assert!(!sorter.ends_with(' '), "{sorter:?}");
}

#[test]
fn mean_token_amounts_are_reported_per_field_and_never_derived() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    let records = write_lines(
        outside.path(),
        &[
            v2_row(&[(
                "reported_tokens",
                "{\"ahu.tokens.input\":{\"kind\":\"observed\",\"value\":100},\
                  \"ahu.tokens.output\":{\"kind\":\"observed\",\"value\":40},\
                  \"ahu.tokens.total\":{\"kind\":\"observed\",\"value\":150},\
                  \"ahu.tokens.cached\":{\"kind\":\"unavailable\"}}",
            )]),
            v2_row(&[(
                "reported_tokens",
                "{\"ahu.tokens.input\":300,\"ahu.tokens.output\":60,\"ahu.tokens.total\":370}",
            )]),
            v2_row(&[]),
        ],
    );
    let observed = &report_json(&repo, &records)["groups"][0]["observed"];
    // The field names the older contract carried are still there.
    assert_eq!(
        observed["token_fields"],
        serde_json::json!([
            "ahu.tokens.cached",
            "ahu.tokens.input",
            "ahu.tokens.output",
            "ahu.tokens.total"
        ])
    );
    assert_eq!(observed["mean_tokens"]["ahu.tokens.input"], 200.0);
    assert_eq!(observed["mean_tokens"]["ahu.tokens.total"], 260.0);
    assert_eq!(observed["mean_total_tokens"], 260.0);
    assert_eq!(observed["token_field_observations"]["ahu.tokens.input"], 2);
    // A field one recorder named but neither measured has no mean at all.
    assert!(
        observed["mean_tokens"].get("ahu.tokens.cached").is_none(),
        "{observed}"
    );

    // The readable block prints the amounts against the sample behind each.
    let output = run_at(
        &repo,
        &["--records", records.to_str().unwrap()],
        Some("120"),
        true,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ahu.tokens.input 200 (2/3)"), "{stdout}");
    assert!(stdout.contains("ahu.tokens.total 260 (2/3)"), "{stdout}");
    assert!(
        stdout.contains("ahu.tokens.cached \u{2014} (0/3)"),
        "{stdout}"
    );

    // Without a reported total, no total is derived from input plus output.
    let records = write_lines(
        outside.path(),
        &[v2_row(&[(
            "reported_tokens",
            "{\"ahu.tokens.input\":100,\"ahu.tokens.output\":40}",
        )])],
    );
    let observed = &report_json(&repo, &records)["groups"][0]["observed"];
    assert!(observed["mean_total_tokens"].is_null(), "{observed}");
    let output = run_at(
        &repo,
        &["--records", records.to_str().unwrap()],
        Some("120"),
        true,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let row = summary_table(&stdout)[1];
    assert!(
        row.ends_with('\u{2014}'),
        "{row} must not report 140 tokens"
    );
}

#[test]
fn a_negative_token_amount_is_refused_by_line() {
    let repo = TestRepo::new();
    let outside = tempfile::TempDir::new().expect("temp dir");
    for tokens in [
        r#"{"ahu.tokens.input":-1}"#,
        r#"{"ahu.tokens.input":{"kind":"observed","value":-1}}"#,
    ] {
        let records = write_lines(outside.path(), &[v2_row(&[("reported_tokens", tokens)])]);
        let output = run(&repo, &["--records", records.to_str().unwrap()]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{stderr}");
        assert!(stderr.contains("runs.jsonl:1:"), "{stderr}");
    }
}
