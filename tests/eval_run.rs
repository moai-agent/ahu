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
    assert_eq!(rows[0]["prompt_profile"], "tool_neutral_v2");
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
