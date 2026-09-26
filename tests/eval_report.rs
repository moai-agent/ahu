//! `ahu eval report`: argument parsing, the repository boundary for run
//! evidence, and what the two output surfaces say about coverage.
//!
//! The records are the compact JSONL rows `scripts/local_eval.py record`
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
            "\"schema_version\":1".to_string(),
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
            "\"ahu_revision\":\"abc1234\"".to_string(),
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
    common::ahu()
        .current_dir(repo.path())
        .arg("eval")
        .arg("report")
        .args(args)
        .env("NO_COLOR", "1")
        .env("AHU_CMUX_BIN", repo.state.path().join("missing-cmux"))
        .stdin(Stdio::null())
        .output()
        .expect("ahu runs")
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
            case: PathBuf::from("/outside/case.md"),
            agent: "@triage".into(),
            evaluator: Some("@judge".into()),
            records: PathBuf::from("/outside/runs.jsonl"),
            runs: 3,
            timeout_seconds: 120,
            allow_widened_approvals: false,
            output_json: true,
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
    assert_eq!(report["schema_version"], 1);
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
        "{\"model\":\"m\",\"harness\":\"h\",\"score\":1,\"passed\":true}\n",
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
