//! Eval runner exercises only synthetic files and stub harnesses. No provider
//! endpoint or real model is contacted by these tests.

mod common;

use std::path::Path;
use std::process::Stdio;

use common::TestRepo;
use serde_json::Value;
use tempfile::TempDir;

fn fixture_case(path: &Path) {
    fixture_case_named(path, "eval-smoke-001", "");
}

/// A schema 2 case, with `extra` front-matter lines appended.
fn fixture_case_named(path: &Path, id: &str, extra: &str) {
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
  subject: duplicate charge
questions:
  route:
    type: choice
    instructions: Select the team
    options:
      billing: Payments
      technical: Product issues
expected:
  route: billing
rubric:
  route: Routes the duplicate charge to payments
scoring:
  route: 1.0
  exact_match_pass_threshold: 1.0
{extra}---

Synthetic routing smoke test.
"#
        ),
    )
    .unwrap();
}

#[test]
fn eval_run_launches_candidate_captures_local_otel_and_appends_external_record() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent_on("triage", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
    repo.add_agent_on("judge", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
    repo.write(".agents/skills/billing/SKILL.md", "---\nname: billing\ndescription: Resolve duplicate charge disputes\n---\nReview billing facts.\n");
    repo.commit("evaluation agents");

    let external = TempDir::new().unwrap();
    let bin = external.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let harness = bin.join("opencode");
    std::fs::write(
        &harness,
        r#"#!/usr/bin/env python3
import json, os, subprocess, sys
if '--version' in sys.argv:
    print('1.18.32')
    raise SystemExit(0)
prompt = sys.argv[-1]
if 'Score this candidate output' in prompt:
    with open('score.json', 'w', encoding='utf-8') as score:
        json.dump({'schema_version':1,'criterion_scores':{'route':1.0},'reason_codes':['routed_to_billing']}, score)
else:
    with open('answer.json', 'w', encoding='utf-8') as answer:
        json.dump({'route':'billing'}, answer)
requests = [
    {'jsonrpc':'2.0','id':1,'method':'initialize','params':{}},
    {'jsonrpc':'2.0','id':2,'method':'tools/list','params':{}},
    {'jsonrpc':'2.0','id':3,'method':'tools/call','params':{'name':'ahu_agents_list','arguments':{}}},
]
server = subprocess.Popen([os.environ['AHU_BIN'], 'mcp', 'serve'], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, text=True)
server.stdin.write(''.join(json.dumps(row)+'\n' for row in requests))
server.stdin.flush()
# Keep MCP stdin open past harness exit. The headless supervisor must allow the
# server to see EOF and flush its session summary before cleaning descendants.
subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(0.2)'], stdin=server.stdin, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
server.stdin.close()
session = os.environ['AHU_PARENT_TASK']
def emit(kind, part):
    print(json.dumps({'type':kind,'timestamp':1,'sessionID':session,'part':part}), flush=True)
emit('text', {'type':'text','text':'synthetic answer saved'})
emit('step_finish', {'type':'step-finish','reason':'stop','usage':{'input_tokens':12,'output_tokens':4,'total_tokens':16},'model':'fixture-model'})
"#,
    )
    .unwrap();
    std::fs::set_permissions(&harness, std::fs::Permissions::from_mode(0o755)).unwrap();
    let case = external.path().join("case.md");
    fixture_case(&case);
    let records = external.path().join("runs.jsonl");
    let home = external.path().canonicalize().unwrap().join("home");
    std::fs::create_dir(&home).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = common::ahu()
        .current_dir(repo.path())
        .args([
            "eval",
            "run",
            "--case",
            case.to_str().unwrap(),
            "--agent",
            "@triage",
            "--skill-selection",
            "lexical",
            "--evaluator",
            "@judge",
            "--records",
            records.to_str().unwrap(),
            "--runs",
            "1",
            "--output",
            "json",
        ])
        .env("PATH", path)
        .env("HOME", &home)
        .env("AHU_CMUX_BIN", external.path().join("missing-cmux"))
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    if !output.status.success() {
        let kept = external.keep();
        panic!(
            "eval smoke failed; external diagnostic artifacts kept at {}\n{}",
            kept.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    let trial = &summary["trials"][0];
    assert_eq!(trial["score"], 1.0);
    assert_eq!(trial["passed"], true);
    assert_eq!(trial["score_source"], "judge");
    assert_eq!(trial["answer_passed"], true);
    assert_eq!(trial["judge_status"], "scored");
    assert_eq!(trial["telemetry_coverage"], "complete_session");
    // The case states no tool expectation, so its tool score is not_applicable
    // rather than a pass by absence.
    assert_eq!(trial["tool_expectation_status"], "not_applicable");
    assert_eq!(summary["evaluator"], "@judge");
    assert_eq!(summary["planned_trials"], 1);
    assert_eq!(summary["planned_launches"], 2);
    assert_eq!(summary["blinding"], "prompt_only");
    assert!(
        summary["blinding_caveat"]
            .as_str()
            .unwrap()
            .contains("not environmental isolation"),
        "the run output says what prompt-level blinding does not claim"
    );

    let rows: Vec<Value> = std::fs::read_to_string(&records)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["schema_version"], 2);
    assert_eq!(rows[0]["mcp_tool_call_count"], 1);
    assert_eq!(rows[0]["mcp_tools"]["ahu_agents_list"], 1);
    assert_eq!(rows[0]["evaluator"], "judge");
    assert_eq!(rows[0]["score"], 1.0);
    assert_eq!(rows[0]["deterministic_score"], 1.0);
    assert_eq!(rows[0]["answer_score"], 1.0);
    assert_eq!(rows[0]["answer_passed"], true);
    assert_eq!(rows[0]["score_source"], "judge");
    // The judge's own evidence is retained, not collapsed into its number.
    assert_eq!(rows[0]["judge_status"], "scored");
    assert_eq!(rows[0]["judge_score"], 1.0);
    assert_eq!(rows[0]["judge_criterion_scores"]["route"], 1.0);
    assert_eq!(
        rows[0]["judge_reason_codes"],
        serde_json::json!(["routed_to_billing"])
    );
    assert_eq!(rows[0]["judge_calibration"], "single_judge_uncalibrated");
    // Exact input identity, with the build identity kept apart from the HEAD of
    // the checkout under evaluation.
    assert_eq!(rows[0]["case_schema_version"], 2);
    assert_eq!(rows[0]["prompt_profile"], "tool_neutral_v3");
    for field in [
        "case_digest",
        "agent_manifest_digest",
        "agent_source_digest",
        "agent_instructions_digest",
        "agent_identity_digest",
        "evaluator_manifest_digest",
        "evaluator_identity_digest",
        "tool_definitions_digest",
        "ahu_build_digest",
        "skill_digest",
        "target_repo_head",
        "input_fingerprint",
    ] {
        assert_eq!(
            rows[0][field].as_str().map(str::len),
            Some(if field == "target_repo_head" { 40 } else { 64 }),
            "{field} must be recorded as a digest"
        );
    }
    assert_eq!(rows[0]["skill_selection"]["mode"], "lexical");
    assert_eq!(rows[0]["skill_selection"]["status"], "suggested");
    assert_eq!(rows[0]["selection_telemetry_observed"], true);
    assert_eq!(
        rows[0]["selection_policy_digest"].as_str().map(str::len),
        Some(64)
    );
    assert!(
        rows[0]["total_elapsed_ms"].as_f64().unwrap()
            >= rows[0]["launch_elapsed_ms"].as_f64().unwrap()
    );
    assert_eq!(rows[0]["ahu_version"], env!("CARGO_PKG_VERSION"));
    assert!(
        rows[0].get("ahu_revision").is_none(),
        "the ambiguous v1 field is not written"
    );
    assert_eq!(rows[0]["fingerprint_completeness"], "complete");
    assert_eq!(rows[0]["blinding"], "prompt_only");
    assert_eq!(rows[0]["telemetry_coverage"], "complete_session");
    assert_eq!(rows[0]["tool_expectation_status"], "not_applicable");
    assert_eq!(rows[0]["terminal_status"], "candidate_scored");
    assert_eq!(rows[0]["attempts"], 1);
    assert_eq!(rows[0]["trial_index"], 1);
    assert_eq!(rows[0]["evaluator_model"], "ollama/glm-5.3:cloud");
    assert_eq!(rows[0]["evaluator_harness"], "opencode");
    assert_eq!(rows[0]["evaluator_harness_version"], "1.18.32");
    assert!(rows[0]["evaluator_task_id"].as_str().is_some());
    assert!(rows[0]["elapsed_ms"].as_u64().is_some());
    assert!(rows[0].get("trace_id").and_then(Value::as_str).is_some());
    assert!(
        rows[0].get("answer").is_none(),
        "record rows omit answer data"
    );
    let artifact_dir = external
        .path()
        .join(format!("ahu-eval-{}", summary["run_id"].as_str().unwrap()));
    let candidate_prompt =
        std::fs::read_to_string(artifact_dir.join("run-001/candidate/prompt.txt")).unwrap();
    let evaluator_prompt =
        std::fs::read_to_string(artifact_dir.join("run-001/evaluator/prompt.txt")).unwrap();
    assert!(!candidate_prompt.contains("\"expected\""));
    assert!(!candidate_prompt.contains("Rubric:"));
    assert!(!evaluator_prompt.contains("@triage"));
    assert!(!evaluator_prompt.contains("ollama/"));
    assert!(!evaluator_prompt.contains("\"expected\""));

    let report = common::ahu()
        .current_dir(repo.path())
        .args([
            "eval",
            "report",
            "--records",
            records.to_str().unwrap(),
            "--output",
            "json",
        ])
        .env("HOME", &home)
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    let comparison: Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(comparison["schema_version"], 2);
    assert_eq!(comparison["groups"][0]["case_schema_version"], "2");
    assert_eq!(
        comparison["groups"][0]["fingerprint_completeness"],
        "complete"
    );
    assert_eq!(comparison["groups"][0]["answer"]["pass_rate"], 1.0);
    assert_eq!(
        comparison["groups"][0]["answer"]["pass_interval"]["samples"],
        1
    );
    assert_eq!(comparison["groups"][0]["judge"]["scored"], 1);
    assert_eq!(
        comparison["groups"][0]["judge"]["criterion_means"]["route"],
        1.0
    );
    assert_eq!(comparison["groups"][0]["coverage"]["mcp_observations"], 1);
    assert_eq!(
        comparison["groups"][0]["observed"]["mean_mcp_tool_calls"],
        1.0
    );
    assert_eq!(
        comparison["groups"][0]["observed"]["mcp_tools"]["ahu_agents_list"],
        1.0
    );
    assert_eq!(
        comparison["groups"][0]["evaluator_model"],
        "ollama/glm-5.3:cloud"
    );
    assert_eq!(comparison["groups"][0]["evaluator_harness"], "opencode");
}

#[test]
fn eval_run_records_invalid_candidate_answers_without_scoring_them_as_zero() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent_on("triage", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
    repo.add_agent_on("judge", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
    repo.commit("evaluation agent");

    let external = TempDir::new().unwrap();
    let bin = external.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let harness = bin.join("opencode");
    std::fs::write(
        &harness,
        r##"#!/usr/bin/env python3
import json, os, subprocess, sys, time
if '--version' in sys.argv:
    print('1.18.32')
    raise SystemExit(0)
mode = os.environ['AHU_TEST_ANSWER']
if mode == 'launch_failed':
    raise SystemExit(1)
elif mode == 'invalid_json':
    with open('answer.json', 'w', encoding='utf-8') as answer:
        answer.write('{ malformed')
elif mode == 'too_large':
    with open('answer.json', 'w', encoding='utf-8') as answer:
        answer.write('x' * 65537)
elif mode == 'invalid_value':
    with open('answer.json', 'w', encoding='utf-8') as answer:
        json.dump({'route':'unknown'}, answer)
elif mode == 'timed_out':
    session = os.environ['AHU_PARENT_TASK']
    print(json.dumps({'type':'step_finish','timestamp':2,'sessionID':session,'part':{'type':'step-finish','reason':'stop','usage':{'input_tokens':3,'output_tokens':4,'total_tokens':7},'model':'fixture'}}), flush=True)
    time.sleep(60)
    raise SystemExit(0)
requests = [
    {'jsonrpc':'2.0','id':1,'method':'initialize','params':{}},
    {'jsonrpc':'2.0','id':2,'method':'tools/list','params':{}},
    {'jsonrpc':'2.0','id':3,'method':'tools/call','params':{'name':'ahu_agents_list','arguments':{}}},
]
server = subprocess.Popen([os.environ['AHU_BIN'], 'mcp', 'serve'], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, text=True)
server.stdin.write(''.join(json.dumps(row)+'\n' for row in requests))
server.stdin.close()
server.wait(timeout=5)
session = os.environ['AHU_PARENT_TASK']
print(json.dumps({'type':'text','timestamp':1,'sessionID':session,'part':{'type':'text','text':'saved invalid answer'}}), flush=True)
print(json.dumps({'type':'step_finish','timestamp':2,'sessionID':session,'part':{'type':'step-finish','reason':'stop','usage':{'input_tokens':1,'output_tokens':1,'total_tokens':2},'model':'fixture'}}), flush=True)
"##,
    )
    .unwrap();
    std::fs::set_permissions(&harness, std::fs::Permissions::from_mode(0o755)).unwrap();
    let case = external.path().join("case.md");
    fixture_case(&case);
    let home = external.path().canonicalize().unwrap().join("home");
    std::fs::create_dir(&home).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    for (mode, status, category) in [
        (
            "launch_failed",
            "candidate_launch_failed",
            "candidate_run_failed",
        ),
        ("timed_out", "candidate_timed_out", "candidate_run_failed"),
        (
            "invalid_json",
            "candidate_answer_invalid_json",
            "candidate_answer_invalid",
        ),
        (
            "too_large",
            "candidate_answer_too_large",
            "candidate_answer_invalid",
        ),
        (
            "invalid_value",
            "candidate_answer_invalid",
            "candidate_answer_invalid",
        ),
        (
            "missing",
            "candidate_answer_missing",
            "candidate_answer_invalid",
        ),
    ] {
        let records = external.path().join(format!("{mode}.jsonl"));
        let output = common::ahu()
            .current_dir(repo.path())
            .args([
                "eval",
                "run",
                "--case",
                case.to_str().unwrap(),
                "--agent",
                "@triage",
                "--records",
                records.to_str().unwrap(),
                "--runs",
                "1",
                "--timeout",
                if mode == "timed_out" { "1" } else { "120" },
                "--output",
                "json",
            ])
            .env("PATH", &path)
            .env("HOME", &home)
            .env("AHU_CMUX_BIN", external.path().join("missing-cmux"))
            .env("AHU_TEST_ANSWER", mode)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(summary["trials"][0]["terminal_status"], status);
        let row: Value = std::fs::read_to_string(records)
            .unwrap()
            .lines()
            .next()
            .map(|line| serde_json::from_str(line).unwrap())
            .unwrap();
        assert_eq!(row["terminal_status"], status);
        assert_eq!(row["failure_category"], category);
        // The harness's own outcome survives when it reported one, so a run that
        // ran out of time is not filed as a generic failure.
        assert_eq!(
            row["outcome"],
            if mode == "timed_out" {
                "timed_out"
            } else {
                "failed"
            }
        );
        assert_eq!(row["judge_status"], "not_reached");
        assert_eq!(row["score"], Value::Null);
        assert_eq!(row["passed"], false);
        assert_eq!(row["answer_passed"], false);
        // An attempt with no valid answer is marked as such, so the report can
        // keep it out of the answer-quality rate without turning it into a
        // wrong answer. The all-attempt reliability rate still counts it.
        assert_eq!(row["answer_status"], "no_answer", "{mode}");
        // Launch timing is the runner's own measurement, so a failed or
        // timed-out attempt is never an attempt that appears to cost nothing.
        let launch_ms = row["launch_elapsed_ms"]
            .as_f64()
            .unwrap_or_else(|| panic!("{mode} kept its launch timing: {row}"));
        assert!(launch_ms >= 0.0, "{mode}: {row}");
        // Whatever the launch did reach is kept beside the failure.
        assert!(
            row["telemetry_coverage"].is_string(),
            "{mode} states its telemetry coverage: {row}"
        );
        if mode == "timed_out" {
            // The harness reported the timeout, and the row says so rather than
            // calling it a generic launch failure.
            assert_eq!(row["outcome"], "timed_out");
            assert!(row["task_id"].is_string(), "{row}");
            // The attempt ran to its one-second bound: the timing is the real
            // elapsed launch, not a placeholder.
            assert!(launch_ms >= 1000.0, "{mode}: {launch_ms} ms");
        }
        if mode != "launch_failed" && mode != "timed_out" {
            assert_eq!(row["mcp_observed"], true);
            assert_eq!(row["mcp_tool_call_count"], 1);
            assert_eq!(row["mcp_tools"]["ahu_agents_list"], 1);
        }
        // A failed attempt still ran on a harness with a skill bundle in front
        // of it, and the launch envelope states both. The fingerprint must not
        // be marked partial over fields that were actually observed.
        let missing: Vec<&str> = row["fingerprint_missing"]
            .as_array()
            .map(|names| names.iter().filter_map(|name| name.as_str()).collect())
            .unwrap_or_default();
        for field in ["harness_version", "skill_digest"] {
            assert!(
                !missing.contains(&field),
                "{mode} reported {field} but the fingerprint calls it missing: {row}"
            );
            assert!(
                row[field].is_string(),
                "{mode} kept its observed {field}: {row}"
            );
        }
        assert_eq!(row["harness_version"], "1.18.32", "{mode}");
        assert_eq!(
            row["skill_digest"].as_str().map(str::len),
            Some(64),
            "{mode}: {row}"
        );
        // The harness counters it did report are kept beside the failure.
        if mode != "launch_failed" {
            assert!(
                row["reported_tokens"].is_object(),
                "{mode} kept the usage it reported: {row}"
            );
        }
    }
}

#[test]
fn eval_run_refuses_invalid_matrix_and_checkout_combinations_before_launch() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent_on("triage", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
    repo.add_agent_on("judge", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
    repo.add_agent_on("writer", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
    repo.commit("evaluation agent");
    let external = TempDir::new().unwrap();
    let case = external.path().join("case.md");
    fixture_case(&case);
    let case_without_rubric = external.path().join("case-without-rubric.md");
    fixture_case_named(&case_without_rubric, "without-rubric", "");
    let case_without_rubric_text = std::fs::read_to_string(&case_without_rubric).unwrap();
    std::fs::write(
        &case_without_rubric,
        case_without_rubric_text.replace(
            "rubric:\n  route: Routes the duplicate charge to payments\n",
            "",
        ),
    )
    .unwrap();
    let case_arg = case.to_str().unwrap();
    let no_rubric_arg = case_without_rubric.to_str().unwrap();
    let external_arg = external.path().to_str().unwrap();
    let records_arg = external.path().join("preflight-runs.jsonl");
    let records_arg = records_arg.to_str().unwrap();
    let cases = vec![
        (vec!["eval", "run", "--agent", "@triage"], "--case"),
        (vec!["eval", "run", "--case", case_arg], "--agent @name"),
        (
            vec![
                "eval", "run", "--case", case_arg, "--agent", "@triage", "--agent", "@triage",
            ],
            "named more than once",
        ),
        (
            vec!["eval", "run", "--case", case_arg, "--agent", "@missing"],
            "not a registered ahu agent",
        ),
        (
            vec![
                "eval",
                "run",
                "--case",
                case_arg,
                "--agent",
                "@triage",
                "--evaluator",
                "@missing",
            ],
            "not a registered ahu agent",
        ),
        (
            vec![
                "eval",
                "run",
                "--case",
                no_rubric_arg,
                "--agent",
                "@triage",
                "--evaluator",
                "@judge",
            ],
            "requires a case `rubric` object",
        ),
        (
            vec![
                "eval",
                "run",
                "--case",
                case_arg,
                "--agent",
                "@triage",
                "--evaluator",
                "@judge",
                "--evaluator-repo",
                external_arg,
            ],
            "not a Git checkout",
        ),
        (
            vec![
                "eval", "run", "--case", case_arg, "--agent", "@triage", "--agent", "@judge",
                "--agent", "@writer", "--runs", "100",
            ],
            "beyond the",
        ),
        (
            vec![
                "eval",
                "run",
                "--case",
                case_arg,
                "--suite",
                "also-a-suite.md",
                "--agent",
                "@triage",
            ],
            "alternatives",
        ),
        (
            vec![
                "eval",
                "run",
                "--case",
                case_arg,
                "--agent",
                "@triage",
                "--evaluator-repo",
                external_arg,
            ],
            "needs --evaluator",
        ),
    ];
    for (args, expected) in cases {
        let output = common::ahu()
            .current_dir(repo.path())
            .args(args.iter().copied())
            .args(["--records", records_arg])
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted args: {args:?}");
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(diagnostic.contains(expected), "{args:?}: {diagnostic}");
    }
}

#[test]
fn candidate_mcp_selection_usage_survives_success_and_missing_answer() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    for answer in [true, false] {
        let repo = TestRepo::new();
        repo.init_config();
        repo.add_agent_on("triage", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
        repo.write(".agents/skills/billing/SKILL.md", "---\nname: billing\ndescription: Resolve duplicate charge disputes\n---\nReview billing facts.\n");
        repo.commit("selection cost fixture");
        let external = TempDir::new().unwrap();
        let bin = external.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let harness = bin.join("opencode");
        std::fs::write(&harness, r#"#!/usr/bin/env python3
import json, os, subprocess, sys
if '--version' in sys.argv:
    print('1.18.32')
    raise SystemExit(0)
rows = [
 {'jsonrpc':'2.0','id':1,'method':'initialize','params':{}},
 {'jsonrpc':'2.0','method':'notifications/initialized'},
 {'jsonrpc':'2.0','id':2,'method':'tools/call','params':{'name':'ahu_skills_suggest','arguments':{'task':'Resolve duplicate charge','mode':'decision'}}}
]
p = subprocess.run([os.environ['AHU_BIN'], 'mcp', 'serve'], input=''.join(json.dumps(r)+'\n' for r in rows), text=True, capture_output=True, timeout=15)
assert p.returncode == 0, p.stderr
responses = [json.loads(line) for line in p.stdout.splitlines()]
assert responses[-1]['result']['structuredContent']['status'] == 'suggested', responses
if os.environ['FIXTURE_WRITE_ANSWER'] == 'yes':
    with open('answer.json','w') as f:
        json.dump({'route':'billing'}, f)
print(json.dumps({'type':'text','sessionID':os.environ['AHU_PARENT_TASK'],'part':{'type':'text','text':'done'}}), flush=True)
print(json.dumps({'type':'step_finish','sessionID':os.environ['AHU_PARENT_TASK'],'part':{'type':'step-finish','reason':'stop'}}), flush=True)
"#).unwrap();
        std::fs::set_permissions(&harness, std::fs::Permissions::from_mode(0o755)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1/decisions", listener.local_addr().unwrap());
        let service = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "provider never called"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = None;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some((key, value)) = line.split_once(':')
                    && key.eq_ignore_ascii_case("content-length")
                {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
            let mut body = vec![0; length.unwrap()];
            reader.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            let answers: serde_json::Map<String, Value> = request["questions"]
                .as_object()
                .unwrap()
                .keys()
                .map(|key| (key.clone(), serde_json::json!({"value":0.99})))
                .collect();
            let response = serde_json::json!({"answers":answers,"service":{"backend":"fixture","model":"fixture","prompt_tokens":31,"generated_tokens":2,"duration_ms":4}}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
        });
        let case = external.path().join("case.md");
        fixture_case(&case);
        let records = external.path().join("runs.jsonl");
        let home = external.path().canonicalize().unwrap().join("home");
        std::fs::create_dir(&home).unwrap();
        let output = common::ahu()
            .current_dir(repo.path())
            .args([
                "eval",
                "run",
                "--case",
                case.to_str().unwrap(),
                "--agent",
                "@triage",
                "--records",
                records.to_str().unwrap(),
                "--output",
                "json",
            ])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("HOME", home)
            .env("AHU_DECISION_URL", endpoint)
            .env("FIXTURE_WRITE_ANSWER", if answer { "yes" } else { "no" })
            .env("AHU_CMUX_BIN", external.path().join("missing-cmux"))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        if service.join().is_err() {
            let kept = external.keep();
            panic!(
                "provider missing; diagnostics {} stdout {} stderr {}",
                kept.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let row: Value =
            serde_json::from_str(std::fs::read_to_string(&records).unwrap().trim()).unwrap();
        assert_eq!(row["skill_selection"]["mode"], "none");
        assert_eq!(row["mcp_tools"]["ahu_skills_suggest"], 1);
        assert_eq!(row["candidate_selection"]["input_tokens_known"], 31);
        assert_eq!(row["candidate_selection"]["output_tokens_known"], 2);
        assert_eq!(row["candidate_selection"]["input_complete_observations"], 1);
        assert_eq!(row["candidate_selection"]["observations"], 1);
        if row["answer_status"] != if answer { "scored" } else { "no_answer" } {
            let kept = external.keep();
            panic!(
                "unexpected answer status; {} record {}",
                kept.display(),
                row
            );
        }
        let discovered = ahu::git::discover(repo.path()).unwrap();
        let report = ahu::eval::report(&discovered, &records).unwrap();
        assert_eq!(
            report.groups[0].mean_candidate_selection["input_tokens_known"],
            31.0
        );
    }
}
