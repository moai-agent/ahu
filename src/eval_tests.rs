use super::*;

/// A valid record, with `overrides` replacing or adding raw JSON fields.
///
/// Built from a map so an override never emits a duplicate key: the parser
/// refuses those, which is the point of building the fixture this way.
fn record(overrides: &[(&str, &str)]) -> String {
    let mut fields: BTreeMap<&str, &str> = BTreeMap::from([
        ("schema_version", "2"),
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
fn unscored_failed_trials_remain_in_pass_rate_without_a_fake_zero_score() {
    let text = record(&[("score", "null"), ("passed", "false")]);
    let records = parse_records(Path::new("runs.jsonl"), &text).expect("failure row parses");
    let groups = group(&records);
    assert_eq!(groups[0].runs, 1);
    assert_eq!(groups[0].passes, 0);
    assert_eq!(groups[0].pass_rate, 0.0);
    assert_eq!(groups[0].mean_score, None);
    assert_eq!(groups[0].score_observations, 0);
}

#[test]
fn unavailable_token_entries_do_not_claim_coverage() {
    let text = format!(
        "{}\n{}\n",
        record(&[("reported_tokens", r#"{"input":{"kind":"unavailable"}}"#)]),
        record(&[(
            "reported_tokens",
            r#"{"output":{"kind":"observed","value":0}}"#
        )])
    );
    let rows = parse_records(Path::new("runs.jsonl"), &text).expect("token rows parse");
    let groups = group(&rows);
    assert_eq!(groups[0].coverage.tokens, 1);
    assert_eq!(
        groups[0].token_fields,
        BTreeSet::from(["input".to_owned(), "output".to_owned()])
    );
    assert!(!groups[0].mean_tokens.contains_key("input"));
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
        ("evaluator_model", "\"judge-model\""),
        ("evaluator_harness", "\"judge-harness\""),
        ("evaluator_harness_version", "\"2.0.0\""),
        ("harness_version", "\"9\""),
        ("target_repo_head", "\"deadbee\""),
        ("skill_digest", "\"sha256:1\""),
        ("evaluator_skill_digest", "\"sha256:2\""),
        ("evaluator_repo_head", "\"feedbee\""),
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
    assert_eq!(only.mean_score, Some(0.6667));
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
                    "{good}\n{{\"schema_version\":2,\"model\":\"m\",\"harness\":\"h\",\"score\":1,\"passed\":true}}\n"
                ),
                2,
                "missing field `case_id`",
            ),
            (
                "{\"schema_version\":2,\"case_id\":\"c\",\"model\":\"m\",\"harness\":\"h\",\"score\":\"high\",\"passed\":true}\n"
                    .to_string(),
                1,
                "expected f64 at column",
            ),
            (
                format!(
                    "{good}\n{{\"schema_version\":2,\"case_id\":\"a\",\"case_id\":\"b\",\"model\":\"m\",\"harness\":\"h\",\"score\":1,\"passed\":true}}\n"
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
    assert_eq!(json["schema_version"], REPORT_SCHEMA_VERSION);
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
    let text = "{\"schema_version\":2,\"case_id\":\"c\\u001b[31m1\",\"model\":\"m\",\"harness\":\"h\",\
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

/// The report for `lines`, as `render_at` and `render_json` see it.
fn report_of(lines: &[String]) -> Report {
    let text = lines.join("\n") + "\n";
    let records = parse_records(Path::new("runs.jsonl"), &text).expect("parses");
    Report {
        records: PathBuf::from("runs.jsonl"),
        record_count: records.len(),
        groups: group(&records),
    }
}

/// Terminal columns a rendered line occupies. Every fixture here is ASCII
/// apart from the ellipsis and the em dash, which are one column each.
fn columns_of(line: &str) -> usize {
    line.chars().count()
}

/// Two candidates on one case: the comparison the table exists for. One
/// reported tokens and decidable tool expectations; the other reported
/// neither.
fn two_candidates() -> Vec<String> {
    let measured = |elapsed: u32, input: u32, total: u32, tools: &str| {
        record(&[
            ("case_id", "\"synthetic-ticket-routing-001\""),
            ("agent", "\"@triage\""),
            ("agent_version", "\"1.0.0\""),
            ("model", "\"ollama/fixture-model\""),
            ("harness", "\"opencode\""),
            ("elapsed_ms", &elapsed.to_string()),
            (
                "reported_tokens",
                &format!(
                    "{{\"ahu.tokens.input\":{{\"kind\":\"observed\",\"value\":{input}}},\
                          \"ahu.tokens.total\":{{\"kind\":\"observed\",\"value\":{total}}},\
                          \"ahu.tokens.cached\":{{\"kind\":\"unavailable\"}}}}"
                ),
            ),
            ("tool_expectation_status", &format!("\"{tools}\"")),
        ])
    };
    let bare = |passed: bool| {
        record(&[
            ("case_id", "\"synthetic-ticket-routing-001\""),
            ("agent", "\"@sorter\""),
            ("agent_version", "\"1.0.0\""),
            ("model", "\"ollama/other-model\""),
            ("harness", "\"opencode\""),
            ("score", if passed { "1.0" } else { "0.0" }),
            ("passed", if passed { "true" } else { "false" }),
            ("tool_expectation_status", "\"unknown\""),
        ])
    };
    vec![
        measured(41_000, 14_000, 18_000, "pass"),
        measured(46_000, 15_000, 19_400, "pass"),
        measured(43_000, 14_500, 18_700, "fail"),
        bare(true),
        bare(false),
    ]
}

#[test]
fn the_report_opens_with_a_comparison_row_per_configuration() {
    let report = report_of(&two_candidates());
    let rendered = render_at(&report, 120);
    let mut lines = rendered
        .lines()
        .skip_while(|line| !line.starts_with("CASE"));
    let header = lines.next().expect("a header line");
    assert_eq!(
        header.split_whitespace().collect::<Vec<_>>(),
        [
            "CASE", "AGENT", "RUNTIME", "ANSWER", "TOOLS", "TIME", "TOKENS"
        ]
    );
    // Sorted by group key, so @sorter precedes @triage.
    let sorter = lines.next().expect("the @sorter row");
    let triage = lines.next().expect("the @triage row");
    assert_eq!(
        sorter.split_whitespace().collect::<Vec<_>>(),
        [
            "synthetic-ticket-routing-001",
            "@sorter@1.0.0",
            "opencode",
            "/",
            "ollama/other-model",
            "1/2",
            // Nothing timed, nothing decided, nothing counted.
            MISSING,
            MISSING,
            MISSING,
        ]
    );
    assert_eq!(
        triage.split_whitespace().collect::<Vec<_>>(),
        [
            "synthetic-ticket-routing-001",
            "@triage@1.0.0",
            "opencode",
            "/",
            "ollama/fixture-model",
            // The answer rate and the tool rate are separate metrics: all
            // three answers were right, one tool expectation was not.
            "3/3",
            "2/3",
            "43.3s",
            "18.7k",
        ]
    );
    // The table leads: the first detail block comes after it.
    let table_end = rendered.find(triage).expect("the row is in the report");
    let first_block = rendered.find("  agent      ").expect("a detail block");
    assert!(table_end < first_block, "{rendered}");
}

#[test]
fn the_table_fits_its_width_and_never_wraps() {
    let report = report_of(&two_candidates());
    for width in [88, 104, 120] {
        let rendered = render_at(&report, width);
        let table: Vec<&str> = rendered
            .lines()
            .skip_while(|line| !line.starts_with("CASE"))
            .take(3)
            .collect();
        assert_eq!(table.len(), 3, "{rendered}");
        for line in &table {
            assert!(
                columns_of(line) <= width,
                "{line:?} is wider than {width} columns"
            );
        }
    }
    // Wide enough for every column at its natural width, so nothing is cut.
    let wide = render_at(&report, 120);
    let row = wide
        .lines()
        .find(|line| line.contains("@triage"))
        .expect("the @triage row");
    assert!(!row.contains('\u{2026}'), "{row}");
    assert!(row.contains("synthetic-ticket-routing-001"), "{row}");

    // At 88 the case id gives up width first: it is the same on every row,
    // while the agent and the runtime are what the rows differ by.
    let narrow = render_at(&report, 88);
    let row = narrow
        .lines()
        .find(|line| line.contains("@triage"))
        .expect("the @triage row");
    assert!(row.starts_with("synthetic-tic\u{2026}"), "{row}");
    assert!(row.contains("@triage@1.0.0"), "{row}");
    assert!(row.contains("2/3"), "{row}");
    assert!(row.contains("43.3s"), "{row}");
    assert!(row.contains("18.7k"), "{row}");
}

#[test]
fn a_metric_no_run_reported_is_a_dash_in_the_table_rather_than_a_zero() {
    // One run that measured nothing at all.
    let report = report_of(&[record(&[("agent", "\"@quiet\"")])]);
    let row = render_at(&report, 120)
        .lines()
        .find(|line| line.contains("@quiet"))
        .expect("the row")
        .to_string();
    assert_eq!(row.matches(MISSING).count(), 3, "{row}");
    assert!(
        !row.contains(" 0 "),
        "{row} must not read as a measured zero"
    );
    // The answer rate was measured, so it is a fraction rather than a dash.
    assert!(row.contains("1/1"), "{row}");
}

#[test]
fn token_amounts_are_averaged_per_field_and_a_named_field_without_one_is_absent() {
    let report = report_of(&[
        record(&[(
            "reported_tokens",
            "{\"ahu.tokens.input\":{\"kind\":\"observed\",\"value\":100},\
                  \"ahu.tokens.total\":{\"kind\":\"observed\",\"value\":150},\
                  \"ahu.tokens.cached\":{\"kind\":\"unavailable\"}}",
        )]),
        // The bare-number shape a projecting recorder writes.
        record(&[(
            "reported_tokens",
            "{\"ahu.tokens.input\":200,\"ahu.tokens.total\":250}",
        )]),
        // A run that reported no tokens at all.
        record(&[]),
    ]);
    let only = &report.groups[0];
    assert_eq!(only.runs, 3);
    assert_eq!(only.coverage.tokens, 2);
    assert_eq!(only.mean_tokens.get("ahu.tokens.input"), Some(&150.0));
    assert_eq!(only.mean_tokens.get("ahu.tokens.total"), Some(&200.0));
    // Named by one recorder, measured by neither: no mean is invented, and
    // the field keeps its place in the names the group observed.
    assert_eq!(only.mean_tokens.get("ahu.tokens.cached"), None);
    assert!(only.token_fields.contains("ahu.tokens.cached"));
    assert_eq!(
        only.token_field_observations.get("ahu.tokens.input"),
        Some(&2)
    );
    assert_eq!(only.mean_total_tokens(), Some(200.0));

    // The block prints the amounts with the sample each was taken over.
    let rendered = render_at(&report, 120);
    assert!(
        rendered.contains("tokens     ahu.tokens.cached \u{2014} (0/3)"),
        "{rendered}"
    );
    assert!(
        rendered.contains("ahu.tokens.input 150 (2/3)"),
        "{rendered}"
    );
    assert!(
        rendered.contains("ahu.tokens.total 200 (2/3)"),
        "{rendered}"
    );

    // The JSON contract keeps the field names and adds the amounts.
    let json: serde_json::Value =
        serde_json::from_str(&render_json(&report).expect("renders")).expect("valid JSON");
    let observed = &json["groups"][0]["observed"];
    assert_eq!(
        observed["token_fields"],
        serde_json::json!(["ahu.tokens.cached", "ahu.tokens.input", "ahu.tokens.total"])
    );
    assert_eq!(observed["mean_tokens"]["ahu.tokens.input"], 150.0);
    assert_eq!(observed["token_field_observations"]["ahu.tokens.total"], 2);
    assert_eq!(observed["mean_total_tokens"], 200.0);
    assert!(
        observed["mean_tokens"].get("ahu.tokens.cached").is_none(),
        "{observed}"
    );
}

#[test]
fn decision_service_usage_is_separate_from_agent_time_and_tokens() {
    let report = report_of(&[
        record(&[
            ("elapsed_ms", "1000"),
            ("decision_service_duration_ms", "250.0"),
            (
                "reported_tokens",
                r#"{"ahu.tokens.total":800,"decision_service.input":120,"decision_service.output":20}"#,
            ),
        ]),
        record(&[
            ("elapsed_ms", "1500"),
            ("decision_service_duration_ms", "350.0"),
            (
                "reported_tokens",
                r#"{"ahu.tokens.total":1000,"decision_service.input":140,"decision_service.output":30}"#,
            ),
        ]),
    ]);
    let group = &report.groups[0];
    assert_eq!(group.mean_elapsed_ms, Some(1250.0));
    assert_eq!(group.mean_decision_service_duration_ms, Some(300.0));
    assert_eq!(group.mean_tokens.get("ahu.tokens.total"), Some(&900.0));
    assert_eq!(
        group.mean_tokens.get("decision_service.input"),
        Some(&130.0)
    );
    assert_eq!(
        group.mean_tokens.get("decision_service.output"),
        Some(&25.0)
    );
    assert_eq!(group.mean_total_tokens(), Some(900.0));
    let json: serde_json::Value = serde_json::from_str(&render_json(&report).unwrap()).unwrap();
    let observed = &json["groups"][0]["observed"];
    assert_eq!(observed["mean_decision_service_duration_ms"], 300.0);
    assert_eq!(observed["mean_total_tokens"], 900.0);
    let terminal = render_at(&report, 200);
    assert!(terminal.contains("decision service ms 300"), "{terminal}");
    assert!(
        terminal.contains("decision_service.input 130 (2/2)"),
        "{terminal}"
    );
}

#[test]
fn a_total_is_never_derived_from_the_fields_that_were_reported() {
    let report = report_of(&[record(&[(
        "reported_tokens",
        "{\"ahu.tokens.input\":100,\"ahu.tokens.output\":40}",
    )])]);
    let only = &report.groups[0];
    assert_eq!(only.mean_tokens.get("ahu.tokens.input"), Some(&100.0));
    assert_eq!(only.mean_total_tokens(), None);
    let row = render_at(&report, 120)
        .lines()
        .find(|line| line.starts_with("c1"))
        .expect("the row")
        .to_string();
    assert!(row.ends_with("100/40 I/O"), "{row}");
    assert!(!row.ends_with("140"), "{row} must not invent a total");
}

#[test]
fn a_token_amount_that_is_not_a_count_is_refused_by_line() {
    for tokens in [
        r#"{"ahu.tokens.input":-1}"#,
        r#"{"ahu.tokens.input":{"kind":"observed","value":-1}}"#,
    ] {
        let text = format!("{}\n", record(&[("reported_tokens", tokens)]));
        let error = parse_records(Path::new("runs.jsonl"), &text).expect_err("refused");
        assert_eq!(error.kind(), ErrorKind::Usage);
        assert!(error.to_string().starts_with("runs.jsonl:1: "), "{error}");
    }
    // A value that is not a number at all stays tolerated: it is simply not
    // an amount, and the field is still a name the recorder reported.
    let text = format!("{}\n", record(&[("reported_tokens", r#"{"input":"n/a"}"#)]));
    let records = parse_records(Path::new("runs.jsonl"), &text).expect("parses");
    let only = &group(&records)[0];
    assert!(only.token_fields.contains("input"));
    assert!(only.mean_tokens.is_empty());
}

#[test]
fn measurements_are_shown_in_units_a_reader_can_compare() {
    assert_eq!(human_duration(0.0), "0ms");
    assert_eq!(human_duration(845.4), "845ms");
    assert_eq!(human_duration(43_500.0), "43.5s");
    assert_eq!(human_duration(128_833.3), "2.1m");
    assert_eq!(human_duration(7_200_000.0), "2.0h");
    assert_eq!(human_tokens(0.0), "0");
    assert_eq!(human_tokens(999.4), "999");
    assert_eq!(human_tokens(18_200.0), "18.2k");
    assert_eq!(human_tokens(2_500_000.0), "2.5M");
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
fn fixture_repo(root: &Path) -> crate::git::Repo {
    crate::git::Repo {
        root: root.to_path_buf(),
        common_dir: root.join(".git"),
        head: None,
    }
}

fn fixture_case(path: &Path, id: &str) {
    std::fs::write(
        path,
        format!(
            r#"---
okf_version: "0.2"
type: ahu:eval-case
schema_version: 2
id: {id}
corpus_version: "1.0.0"
state:
  subject: invoice
questions:
  route:
    type: choice
    instructions: Select a team
    options:
      billing: Payments
      technical: Product
expected:
  route: billing
scoring:
  route: 1.0
  exact_match_pass_threshold: 1.0
---
Synthetic routing case.
"#
        ),
    )
    .unwrap();
}

fn fixture_request<'a>(case: &'a Path, records: &'a Path) -> RunRequest<'a> {
    RunRequest {
        case: Some(case),
        suite: None,
        agents: &[],
        evaluator: None,
        evaluator_repo: None,
        decision_evaluator: false,
        skill_selection: crate::skill_selection::Mode::None,
        records,
        runs: 1,
        timeout_seconds: 1,
        allow_widened_approvals: false,
        json_output: false,
    }
}

#[test]
fn report_rejects_non_files_oversized_files_and_invalid_utf8() {
    let checkout = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let repo = fixture_repo(checkout.path());
    let error = report(&repo, external.path()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Usage);
    assert!(error.to_string().contains("not a regular file"));

    let path = external.path().join("records.jsonl");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_RECORDS_BYTES + 1).unwrap();
    let error = report(&repo, &path).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Usage);
    assert!(error.to_string().contains("beyond the 67108864 byte limit"));
    drop(file);

    std::fs::write(&path, [0xff]).unwrap();
    assert!(
        report(&repo, &path)
            .unwrap_err()
            .to_string()
            .contains("cannot read eval records")
    );
    std::fs::write(&path, format!("{}\n", record(&[]))).unwrap();
    let report = report(&repo, &path).unwrap();
    assert_eq!(report.record_count, 1);
    assert_eq!(report.groups[0].mean_score, Some(1.0));
}

#[test]
fn writable_records_require_an_existing_external_directory_and_regular_file() {
    let checkout = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let repo = fixture_repo(checkout.path());
    for (path, message) in [
        (
            external.path().join("missing/records.jsonl"),
            "is unavailable",
        ),
        (PathBuf::from("/"), "records path must name a file"),
        (checkout.path().join("records.jsonl"), "must live outside"),
        (external.path().to_path_buf(), "must be a regular file"),
    ] {
        let error = writable_records_path(&repo, &path).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Usage);
        assert!(error.to_string().contains(message), "{error}");
    }
    let path = external.path().join("records.jsonl");
    let expected = external
        .path()
        .canonicalize()
        .unwrap()
        .join("records.jsonl");
    assert_eq!(writable_records_path(&repo, &path).unwrap(), expected);
    std::fs::write(&path, "").unwrap();
    assert_eq!(writable_records_path(&repo, &path).unwrap(), expected);
}

#[cfg(unix)]
#[test]
fn records_symlinks_cannot_bypass_repository_or_append_checks() {
    use std::os::unix::fs::symlink;
    let checkout = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let repo = fixture_repo(checkout.path());
    let inside = checkout.path().join("records.jsonl");
    std::fs::write(&inside, record(&[])).unwrap();
    let link = external.path().join("link.jsonl");
    symlink(&inside, &link).unwrap();
    let error = records_path_outside_repository(&repo, &link).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Usage);
    assert!(error.to_string().contains("resolves to"));
    let error = writable_records_path(&repo, &link).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Usage);
    assert!(error.to_string().contains("not a symlink"));
}

#[test]
fn case_selection_requires_one_source_and_preserves_suite_order_and_weights() {
    let external = tempfile::tempdir().unwrap();
    let first = external.path().join("first.md");
    let second = external.path().join("second.md");
    let suite = external.path().join("suite.md");
    let records = external.path().join("records.jsonl");
    fixture_case(&first, "first");
    fixture_case(&second, "second");
    std::fs::write(&suite, "---\nokf_version: '0.2'\ntype: ahu:eval-suite\nschema_version: 1\nid: synthetic-suite\nsuite_version: '1.0.0'\ncases:\n  - path: second.md\n    weight: 3.0\n  - path: first.md\n    weight: 1.0\n---\nSynthetic weighted suite.\n").unwrap();
    let mut request = fixture_request(&first, &records);
    let (identity, cases) = load_cases(&request).unwrap();
    assert!(identity.is_none());
    assert_eq!(cases[0].case.id, "first");
    assert_eq!(cases[0].weight, None);
    request.suite = Some(&suite);
    let error = load_cases(&request).err().unwrap();
    assert_eq!(error.kind(), ErrorKind::Usage);
    assert!(
        error
            .to_string()
            .contains("--case and --suite are alternatives")
    );
    request.case = None;
    let (identity, cases) = load_cases(&request).unwrap();
    let identity = identity.unwrap();
    assert_eq!(identity.id, "synthetic-suite");
    assert_eq!(identity.suite_version, "1.0.0");
    assert_eq!(
        identity.digest,
        crate::util::digest_bytes(&std::fs::read(&suite).unwrap())
    );
    assert_eq!(
        cases
            .iter()
            .map(|entry| (entry.case.id.as_str(), entry.weight))
            .collect::<Vec<_>>(),
        vec![("second", Some(3.0)), ("first", Some(1.0))]
    );
    request.suite = None;
    let error = load_cases(&request).err().unwrap();
    assert_eq!(error.kind(), ErrorKind::Usage);
    assert!(error.to_string().contains("needs --case"));
}

#[test]
fn run_preflight_refuses_invalid_requests_without_creating_run_artifacts() {
    let checkout = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let repo = fixture_repo(checkout.path());
    let agents_dir = checkout.path().join(".agents/ahu/agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    for (name, permissions) in [("candidate", "prompt"), ("wide", "auto")] {
        std::fs::write(agents_dir.join(format!("{name}.md")), format!("---\nokf_version: '0.2'\ntype: ahu:agent\ntitle: {name}\ndescription: Synthetic agent\nstatus: stable\ntags: [agents]\nharness: codex\nmodel: gpt-6-astra\npermissions: {permissions}\nversion: 1.0.0\n---\nSynthetic instructions.\n")).unwrap();
    }
    let case = external.path().join("case.md");
    fixture_case(&case, "synthetic");
    let records = external.path().join("records.jsonl");
    let candidate = vec!["@candidate".into()];
    let duplicate = vec!["candidate".into(), "@candidate".into()];
    let wide = vec!["@wide".into()];
    let missing = vec!["@missing".into()];
    for (agents, evaluator, evaluator_repo, runs, message) in [
        (&[][..], None, None, 1, "needs at least one --agent"),
        (duplicate.as_slice(), None, None, 1, "named more than once"),
        (
            missing.as_slice(),
            None,
            None,
            1,
            "not a registered ahu agent",
        ),
        (wide.as_slice(), None, None, 1, "candidate manifest widens"),
        (
            candidate.as_slice(),
            None,
            Some(external.path()),
            1,
            "--evaluator-repo needs --evaluator",
        ),
        (
            candidate.as_slice(),
            Some("@candidate"),
            Some(external.path()),
            1,
            "is not a Git checkout",
        ),
        (
            candidate.as_slice(),
            Some("@missing"),
            None,
            1,
            "evaluation agent",
        ),
        (
            candidate.as_slice(),
            Some("@wide"),
            None,
            1,
            "evaluator manifest widens",
        ),
        (
            candidate.as_slice(),
            Some("@candidate"),
            None,
            1,
            "requires a case `rubric`",
        ),
        (candidate.as_slice(), None, None, 0, "matrix is empty"),
        (
            candidate.as_slice(),
            None,
            None,
            MAX_TRIALS as u32 + 1,
            "beyond the 200 trial",
        ),
    ] {
        let mut request = fixture_request(&case, &records);
        request.agents = agents;
        request.evaluator = evaluator;
        request.evaluator_repo = evaluator_repo;
        request.runs = runs;
        let mut input = std::io::Cursor::new(Vec::new());
        let mut output = Vec::new();
        let mut console = crate::launcher::Console {
            input: &mut input,
            output: &mut output,
            interactive: false,
        };
        let error = run(&mut console, &repo, &request).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Usage);
        assert!(
            error.to_string().contains(message),
            "expected {message}: {error}"
        );
        assert!(output.is_empty());
        assert!(!records.exists());
        assert_eq!(std::fs::read_dir(external.path()).unwrap().count(), 1);
    }
}

#[test]
fn report_counts_judge_reasons_and_does_not_invent_unnamed_mcp_calls() {
    let report = report_of(&[
        record(&[
            ("judge_reason_codes", r#"["correct","correct"]"#),
            ("mcp_observed", "true"),
            ("mcp_tool_call_count", "3"),
            ("mcp_tools", r#"{"ahu_agents_list":1}"#),
        ]),
        record(&[
            ("judge_reason_codes", r#"["correct","grounded"]"#),
            ("mcp_observed", "true"),
            ("mcp_tool_call_count", "2"),
            ("mcp_tools", r#"{"ahu_typed_decide":2}"#),
        ]),
    ]);
    let group = &report.groups[0];
    assert_eq!(group.judge_reason_codes["correct"], 3);
    assert_eq!(group.judge_reason_codes["grounded"], 1);
    assert_eq!(group.mcp_tools["ahu_agents_list"], 0.5);
    // Only the second run observes typed decisions. The first run
    // leaves calls unnamed, so it must not dilute that mean with zero.
    assert_eq!(group.mcp_tools["ahu_typed_decide"], 2.0);
    assert_eq!(group.mean_mcp_tool_calls, Some(2.5));
    let json: serde_json::Value = serde_json::from_str(&render_json(&report).unwrap()).unwrap();
    assert_eq!(json["groups"][0]["judge"]["reason_codes"]["correct"], 3);
}

#[test]
fn bounded_capture_drains_excess_and_propagates_read_failures() {
    let bytes = vec![b'x'; 20_000];
    let mut reader = std::io::Cursor::new(&bytes);
    let (retained, exceeded) = drain_bounded(&mut reader, 8192).unwrap();
    assert_eq!(retained, bytes[..8192]);
    assert!(exceeded);
    assert_eq!(reader.position(), bytes.len() as u64);
    assert_eq!(
        drain_bounded(&b"abc"[..], 3).unwrap(),
        (b"abc".to_vec(), false)
    );
    assert_eq!(drain_bounded(&b"a"[..], 0).unwrap(), (Vec::new(), true));
    struct BrokenReader;
    impl Read for BrokenReader {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("synthetic read failure"))
        }
    }
    assert_eq!(
        drain_bounded(BrokenReader, 3).unwrap_err().to_string(),
        "synthetic read failure"
    );
}

#[test]
fn safe_artifact_reports_missing_roots_and_requires_regular_files() {
    let external = tempfile::tempdir().unwrap();
    let root = external.path().join("task");
    assert!(
        safe_artifact(&root, "answer.json")
            .unwrap_err()
            .to_string()
            .contains("cannot locate task worktree")
    );
    std::fs::create_dir(&root).unwrap();
    assert!(
        safe_artifact(&root, "answer.json")
            .unwrap_err()
            .to_string()
            .contains("missing answer.json")
    );
    std::fs::create_dir(root.join("answer.json")).unwrap();
    assert!(
        safe_artifact(&root, "answer.json")
            .unwrap_err()
            .to_string()
            .contains("must be a regular file")
    );
    std::fs::remove_dir(root.join("answer.json")).unwrap();
    std::fs::write(root.join("answer.json"), "{}").unwrap();
    assert_eq!(
        safe_artifact(&root, "answer.json").unwrap(),
        root.canonicalize().unwrap().join("answer.json")
    );
}
#[test]
fn judge_metadata_limits_reject_bad_records_at_the_original_line() {
    let criteria = |count| {
        serde_json::Value::Object(
            (0..count)
                .map(|index| (format!("criterion_{index}"), serde_json::json!(0.5)))
                .collect(),
        )
        .to_string()
    };
    let codes = |count| serde_json::to_string(&vec!["valid_code"; count]).unwrap();
    for (field, value) in [
        ("judge_criterion_scores", criteria(65)),
        ("judge_criterion_scores", r#"{"route":1.01}"#.into()),
        ("judge_reason_codes", codes(33)),
        ("judge_reason_codes", r#"[""]"#.into()),
        ("judge_reason_codes", r#"["bad-code"]"#.into()),
        (
            "judge_reason_codes",
            serde_json::to_string(&vec!["x".repeat(65)]).unwrap(),
        ),
        ("answer_score", "-0.01".into()),
        ("judge_score", "1.01".into()),
        ("attempts", "-1".into()),
    ] {
        let text = format!("{}\n\n{}", record(&[]), record(&[(field, &value)]));
        let error = parse_records(Path::new("runs.jsonl"), &text).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Usage);
        assert!(
            error
                .to_string()
                .starts_with("runs.jsonl:3: scores and observed metrics"),
            "{field}: {error}"
        );
    }
    // The inclusive bounds must remain valid, including an observed zero.
    let records = parse_records(
        Path::new("runs.jsonl"),
        &record(&[
            ("judge_criterion_scores", &criteria(64)),
            ("judge_reason_codes", &codes(32)),
            ("answer_score", "0"),
            ("judge_score", "1"),
            ("attempts", "0"),
        ]),
    )
    .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].judge_criterion_scores.as_ref().unwrap().len(),
        64
    );
    assert_eq!(records[0].judge_reason_codes.as_ref().unwrap().len(), 32);
    assert_eq!(records[0].attempts, Some(0.0));
    let longest = serde_json::to_string(&vec!["x".repeat(64)]).unwrap();
    assert!(
        parse_records(
            Path::new("runs.jsonl"),
            &record(&[("judge_reason_codes", &longest)])
        )
        .is_ok()
    );
}

/// Telemetry that observed the decision service's own token usage.
fn decision_telemetry(input: Option<u64>, output: Option<u64>) -> crate::eval_otel::TaskTelemetry {
    crate::eval_otel::TaskTelemetry {
        task_id: "t".into(),
        attempt: 1,
        decision_input_tokens: input,
        decision_output_tokens: output,
        ..crate::eval_otel::TaskTelemetry::default()
    }
}

#[test]
fn decision_service_usage_survives_a_launch_that_called_it_and_then_failed() {
    let envelope = serde_json::json!({
        "metrics": {"values": {"ahu.tokens.input": {"kind": "observed", "value": 10}}}
    });
    // Both sources, as every terminal path now collects them. An attempt can
    // reach the decision service and fail afterwards; the service usage is a
    // real observation and is not discarded with the failure.
    let merged = reported_tokens(Some(&envelope), Some(&decision_telemetry(Some(7), Some(9))))
        .expect("both sources reported");
    assert_eq!(merged["ahu.tokens.input"]["value"], 10);
    assert_eq!(merged["decision_service.input"], 7);
    assert_eq!(merged["decision_service.output"], 9);

    // Telemetry alone is enough: a launch whose envelope carried no harness
    // counters still reports what the service spent.
    let service_only = reported_tokens(None, Some(&decision_telemetry(Some(7), Some(9))))
        .expect("the service reported");
    assert_eq!(service_only["decision_service.input"], 7);
    assert!(service_only.get("ahu.tokens.input").is_none());

    // A field neither source measured stays absent rather than becoming zero.
    let partial = reported_tokens(None, Some(&decision_telemetry(Some(7), None)))
        .expect("one field reported");
    assert_eq!(partial["decision_service.input"], 7);
    assert!(partial.get("decision_service.output").is_none());

    // Nothing reported at all is no observation, not a map of zeros.
    assert!(reported_tokens(None, None).is_none());
    assert!(reported_tokens(Some(&serde_json::json!({})), None).is_none());
    assert!(
        reported_tokens(
            Some(&serde_json::json!({"metrics": {"values": {}}})),
            Some(&decision_telemetry(None, None))
        )
        .is_none()
    );
}

#[test]
fn a_failed_launch_still_identifies_the_harness_the_envelope_named() {
    // Either name for the same value will do, so a launch that reported only one
    // of them is not recorded as a run on an unknown harness.
    for envelope in [
        serde_json::json!({"native_reference": {"harness_version": "2.1.283 (Claude Code)"}}),
        serde_json::json!({"capabilities": {"harness_version": "2.1.283 (Claude Code)"}}),
    ] {
        assert_eq!(
            harness_version(&envelope).as_deref(),
            Some("2.1.283 (Claude Code)"),
            "{envelope}"
        );
    }
    // native_reference wins when both are present and they agree or differ:
    // one source is picked rather than the two being reconciled here.
    let both = serde_json::json!({
        "native_reference": {"harness_version": "2.1.283"},
        "capabilities": {"harness_version": "2.1.284"},
    });
    assert_eq!(harness_version(&both).as_deref(), Some("2.1.283"));
    // A blank or absent value is not a version, and never a placeholder string.
    for envelope in [
        serde_json::json!({}),
        serde_json::json!({"native_reference": {"harness_version": "   "}}),
        serde_json::json!({"native_reference": {"harness_version": null}}),
        serde_json::json!({"capabilities": {"harness_version": 2}}),
    ] {
        assert_eq!(harness_version(&envelope), None, "{envelope}");
    }
    // A blank in the first source falls through to the second.
    let fallback = serde_json::json!({
        "native_reference": {"harness_version": ""},
        "capabilities": {"harness_version": "1.18.32"},
    });
    assert_eq!(harness_version(&fallback).as_deref(), Some("1.18.32"));
}

#[test]
fn selection_policy_identity_excludes_outcomes_but_tracks_configuration() {
    let mut selection: crate::skill_selection::Selection =
        serde_json::from_value(serde_json::json!({
            "mode":"decision","policy_version":1,"catalog_digest":"a".repeat(64),
            "candidate_count":1,"selected":[],"status":"abstained","elapsed_ms":2.0,
            "service":{"backend":"typesafe","model":"jev-1.13.0","prompt_tokens":100},
            "configuration":{"backend":"typesafe","requested_model":"jev-1.13.0"},
            "error_code":null
        }))
        .unwrap();
    let base = selection_policy_digest(&selection);
    selection
        .selected
        .push(".agents/skills/fixture/SKILL.md".into());
    selection.elapsed_ms = 999.0;
    selection.status = "suggested".into();
    selection.service.as_mut().unwrap()["prompt_tokens"] = 900.into();
    assert_eq!(selection_policy_digest(&selection), base);
    selection.service.as_mut().unwrap()["model"] = "different-model".into();
    assert_eq!(selection_policy_digest(&selection), base);
    selection.service = None;
    selection.status = "fallback".into();
    assert_eq!(selection_policy_digest(&selection), base);
    selection.configuration.as_mut().unwrap()["requested_model"] = "different-model".into();
    assert_ne!(selection_policy_digest(&selection), base);
    selection.mode = crate::skill_selection::Mode::None;
    assert!(selection_policy_digest(&selection).is_none());
}

#[test]
fn candidate_mcp_selection_cost_survives_failure_projection_and_native_tokens_stay_separate() {
    let telemetry = crate::eval_otel::TaskTelemetry {
        selection_observations: 2,
        selection_duration_ms: Some(12.0),
        selection_input_tokens: Some(55),
        selection_output_tokens: Some(4),
        selection_input_complete_observations: 1,
        selection_output_complete_observations: 1,
        ..Default::default()
    };
    let mut failed = outcome_fields("candidate_answer_missing", Some("candidate_answer_invalid"));
    insert_telemetry_fields(&mut failed, Some(&telemetry)).unwrap();
    assert_eq!(failed["candidate_selection"]["input_tokens_known"], 55);
    assert_eq!(
        failed["candidate_selection"]["input_complete_observations"],
        1
    );
    assert_eq!(failed["candidate_selection"]["observations"], 2);
    let usage = reported_tokens(None, Some(&telemetry)).unwrap();
    assert_eq!(usage["skill_selection_service.input_known"], 55);
    assert!(usage.get("ahu.tokens.input").is_none());
    assert!(usage.get("decision_service.input").is_none());
}

#[test]
fn decision_evaluator_report_groups_configured_policy_and_retains_failure_costs() {
    let mut success = decision::Observation::new("typed_decision");
    success.status = "scored".into();
    success.elapsed_ms = Some(10.0);
    success.calls_attempted = 1;
    success.telemetry_coverage = "typed_decision_span".into();
    success.service = Some(
        serde_json::json!({"backend":"mock","model":"reported-v2","model_reported":true,"prompt_tokens":12}),
    );
    success.provider_input_complete = true;
    let mut failure = decision::Observation::new("typed_decision");
    failure.status = "failed".into();
    failure.elapsed_ms = Some(30.0);
    failure.calls_attempted = 1;
    let succeeded = serde_json::to_string(&success).unwrap();
    let failed = serde_json::to_string(&failure).unwrap();
    let report = report_of(&[
        record(&[
            ("evaluator_kind", "\"typed_decision\""),
            ("decision_evaluator_policy_digest", "\"configured-v1\""),
            ("evaluator_metrics", &succeeded),
            ("evaluation_elapsed_ms", "110"),
        ]),
        record(&[
            ("evaluator_kind", "\"typed_decision\""),
            ("decision_evaluator_policy_digest", "\"configured-v1\""),
            ("evaluator_metrics", &failed),
            ("evaluation_elapsed_ms", "150"),
            ("score", "null"),
            ("passed", "false"),
        ]),
    ]);
    assert_eq!(report.groups.len(), 1);
    let summary = &report.groups[0].evaluator_metrics;
    assert_eq!(summary.means["evaluation_elapsed_ms"], 130.0);
    assert_eq!(summary.means["evaluator_elapsed_ms"], 20.0);
    assert_eq!(summary.statuses["failed"], 1);
    assert_eq!(summary.provider_input_complete_runs, 1);
    assert_eq!(summary.provider_output_complete_runs, 0);
    assert_eq!(summary.observations["evaluator_provider.prompt_tokens"], 1);
    assert_eq!(summary.means["evaluator_provider.prompt_tokens"], 12.0);
    assert_eq!(summary.telemetry_coverage["typed_decision_span"], 1);
    assert_eq!(summary.telemetry_coverage["none"], 1);
    assert!(report.groups[0].mean_tokens.is_empty());
    let rendered = render(&report);
    assert!(rendered.contains("typed_decision"));
    assert!(rendered.contains("evaluation_elapsed_ms mean 130.00 (2/2 observations)"));
    assert!(rendered.contains("provider complete input/output 1/0 of 2 runs"));
    let json = group_json(&report.groups[0]);
    assert_eq!(json["observed"]["evaluator_metrics"]["calls_attempted"], 2);
    for field in ["evaluator_kind", "decision_evaluator_policy_digest"] {
        let records = parse_records(
            Path::new("runs.jsonl"),
            &format!("{}\n{}", record(&[]), record(&[(field, "\"different\"")])),
        )
        .unwrap();
        assert_eq!(group(&records).len(), 2);
    }
}

#[test]
fn evaluator_agent_joins_only_its_owned_task_and_attempt_including_failures() {
    let receiver = crate::eval_otel::Receiver::start_for(Some("run"), None).unwrap();
    let mut decision = decision::Observation::new("typed_decision");
    decision.calls_attempted = 1;
    decision.status = "failed".into();
    assert!(crate::telemetry::export_eval_decision(
        receiver.endpoint(),
        "run",
        "judge-task",
        &decision
    ));
    let mut observation = decision::Observation::new("agent");
    let mut envelope = serde_json::json!({"task_id":"judge-task", "attempt":2, "outcome":"failed", "metrics":{"values":{"input":{"kind":"observed", "value":42}, "output":{"kind":"unavailable"}}}});
    observe_evaluator_agent(&mut observation, &envelope, &receiver).unwrap();
    assert_eq!(observation.task_id.as_deref(), Some("judge-task"));
    assert_eq!(observation.attempt, Some(2));
    assert_eq!(observation.telemetry_coverage, "none");
    envelope["attempt"] = serde_json::json!(1);
    observe_evaluator_agent(&mut observation, &envelope, &receiver).unwrap();
    assert_eq!(observation.telemetry_coverage, "partial_spans");
    assert_eq!(
        observation.telemetry.as_ref().unwrap()["evaluator_decision_errors"],
        1
    );
    assert_eq!(observation.reported_tokens["input"]["value"], 42);
    let summary = decision::Summary::collect([(Some(&observation), Some(100.0))].into_iter());
    assert_eq!(summary.means["evaluator_agent.input"], 42.0);
    assert!(!summary.means.contains_key("evaluator_agent.output"));
    assert!(observation.validate());
}

#[test]
fn decision_evaluator_preflights_later_suite_cases_before_agent_lookup_or_artifacts() {
    let checkout = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let repo = fixture_repo(checkout.path());
    let first = external.path().join("first.md");
    let second = external.path().join("second.md");
    fixture_case(&first, "first");
    fixture_case(&second, "second");
    let valid = std::fs::read_to_string(&first).unwrap().replace(
        "scoring:\n",
        "rubric: {route: Choose payments for invoices}\nscoring:\n",
    );
    std::fs::write(&first, valid).unwrap();
    let suite = external.path().join("suite.md");
    std::fs::write(&suite, "---\nokf_version: '0.2'\ntype: ahu:eval-suite\nschema_version: 1\nid: fixture\nsuite_version: '1'\ncases:\n  - {path: first.md, weight: 1}\n  - {path: second.md, weight: 1}\n---\nSynthetic suite.\n").unwrap();
    let records = external.path().join("records.jsonl");
    let mut request = fixture_request(&first, &records);
    request.case = None;
    request.suite = Some(&suite);
    request.decision_evaluator = true;
    let mut input = std::io::Cursor::new(Vec::new());
    let mut output = Vec::new();
    let mut console = crate::launcher::Console {
        input: &mut input,
        output: &mut output,
        interactive: false,
    };
    let error = run(&mut console, &repo, &request).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Usage);
    assert!(error.to_string().contains("requires a rubric"), "{error}");
    assert!(output.is_empty());
    assert!(!records.exists());
    assert_eq!(std::fs::read_dir(external.path()).unwrap().count(), 3);
    assert_eq!(std::fs::read_dir(checkout.path()).unwrap().count(), 0);
}
