//! Provider-free tests of the in-memory private host boundary.
use ahu::{
    config::TelemetryConfig,
    headless::TokenUsage,
    telemetry::private::{AttemptObservation, PrivateMapping},
};
use serde_json::{Value, json};

const REPO: &str = "0123456789abcdef";
const TASK: &str = "00000000-0000-7000-8000-000000000001";
const CHILD: &str = "00000000-0000-7000-8000-000000000002";

fn input() -> Value {
    json!({"schema_version":1,"record_key":"synthetic-marker","repo_identity":REPO,"tasks":[TASK, CHILD]})
}
fn parse(value: Value) -> Result<PrivateMapping, ahu::util::Error> {
    PrivateMapping::parse(&serde_json::to_vec(&value).unwrap())
}
fn enabled() -> TelemetryConfig {
    TelemetryConfig {
        local_metrics: true,
        ..TelemetryConfig::default()
    }
}
fn sample<'a>(task: &'a str, attempt: u32, usage: &'a TokenUsage) -> AttemptObservation<'a> {
    AttemptObservation {
        repo_identity: REPO,
        task_id: task,
        attempt,
        usage,
    }
}

#[test]
fn schema_is_strict_bounded_and_errors_redact_all_input() {
    let valid = parse(input()).unwrap();
    assert_eq!(valid.record_key(), "synthetic-marker");
    let mut bad = Vec::new();
    for (key, replacement) in [
        ("schema_version", json!(2)),
        ("schema_version", json!(-1)),
        ("record_key", json!("")),
        ("record_key", json!("x".repeat(257))),
        ("record_key", json!("https://example.invalid/private")),
        ("record_key", json!("line\nbreak")),
        ("record_key", json!("秘密")),
        ("repo_identity", json!("../private")),
        ("repo_identity", json!("ABCDEF0123456789")),
        ("tasks", json!([])),
        ("tasks", json!([TASK, TASK])),
        ("tasks", json!(["@name"])),
        ("tasks", json!(["0".repeat(36)])),
        ("tasks", json!(["00000000-0000-7000-8000-00000000000A"])),
        ("tasks", json!(["/worktree"])),
        ("tasks", json!(vec![TASK; 257])),
        ("private-secret-field", json!("private-secret-value")),
    ] {
        let mut value = input();
        value[key] = replacement;
        // Build separately to avoid hiding the field mutation in the fixture.
        bad.push((key, value));
    }
    for (key, value) in bad {
        assert_eq!(
            parse(value).err().unwrap().to_string(),
            "invalid private telemetry input",
            "invalid {key}"
        );
    }
    for key in ["schema_version", "record_key", "repo_identity", "tasks"] {
        let mut value = input();
        value.as_object_mut().unwrap().remove(key);
        assert!(parse(value).is_err());
    }
    for bytes in [
        b"{\"private-secret-field\":".as_slice(),
        &[b'x'; 65537],
        b"null",
    ] {
        let error = PrivateMapping::parse(bytes).err().unwrap().to_string();
        assert_eq!(error, "invalid private telemetry input");
    }
    let duplicate =
        serde_json::to_string(&input())
            .unwrap()
            .replacen("{", "{\"schema_version\":1,", 1);
    assert!(PrivateMapping::parse(duplicate.as_bytes()).is_err());
}

#[test]
fn retries_resumes_and_children_count_once_without_inventing_totals() {
    let mapping = parse(input()).unwrap();
    let first = TokenUsage {
        input: Some(0),
        output: Some(7),
        ..TokenUsage::default()
    };
    let resumed = TokenUsage {
        input: Some(12),
        ..TokenUsage::default()
    };
    let child = TokenUsage {
        input: Some(u64::MAX),
        ..TokenUsage::default()
    };
    let samples = [
        sample(TASK, 1, &first),
        sample(TASK, 1, &first),
        sample(TASK, 2, &resumed),
        sample(CHILD, 1, &child),
    ];
    assert!(
        mapping
            .summarize(&TelemetryConfig::default(), &samples)
            .unwrap()
            .is_none()
    );
    let summary = serde_json::to_value(mapping.summarize(&enabled(), &samples).unwrap()).unwrap();
    assert_eq!(summary["attempts"], 3);
    assert_eq!(
        summary["values"]["ahu.tokens.input"],
        json!({"maximum_observed":u64::MAX,"observed_attempts":3,"unavailable_attempts":0})
    );
    assert_eq!(
        summary["values"]["ahu.tokens.output"],
        json!({"maximum_observed":7,"observed_attempts":1,"unavailable_attempts":2})
    );
    assert_eq!(
        summary["values"]["ahu.tokens.total"],
        json!({"maximum_observed":null,"observed_attempts":0,"unavailable_attempts":3})
    );
    assert_eq!(summary.as_object().unwrap().len(), 4);
    assert_eq!(summary["values"].as_object().unwrap().len(), 6);
    let text = summary.to_string();
    for secret in ["synthetic-marker", REPO, TASK, CHILD] {
        assert!(!text.contains(secret));
    }
    let zero = serde_json::to_value(
        mapping
            .summarize(&enabled(), &[sample(TASK, 1, &first)])
            .unwrap(),
    )
    .unwrap();
    assert_eq!(zero["values"]["ahu.tokens.input"]["maximum_observed"], 0);
    let empty = serde_json::to_value(mapping.summarize(&enabled(), &[]).unwrap()).unwrap();
    assert_eq!(empty["attempts"], 0);
    assert_eq!(
        empty["values"]["ahu.tokens.total"]["maximum_observed"],
        Value::Null
    );
}

#[test]
fn membership_conflicts_and_attempt_bounds_fail_without_partial_results() {
    let mapping = parse(input()).unwrap();
    let usage = TokenUsage::default();
    let changed = TokenUsage {
        input: Some(0),
        ..TokenUsage::default()
    };
    for samples in [
        vec![sample(TASK, 0, &usage)],
        vec![sample("00000000-0000-7000-8000-000000000003", 1, &usage)],
        vec![AttemptObservation {
            repo_identity: "fedcba9876543210",
            ..sample(TASK, 1, &usage)
        }],
        vec![sample(TASK, 1, &usage), sample(TASK, 1, &changed)],
        (0..4097).map(|_| sample(TASK, 1, &usage)).collect(),
    ] {
        assert_eq!(
            mapping
                .summarize(&enabled(), &samples)
                .err()
                .unwrap()
                .to_string(),
            "invalid private telemetry input"
        );
    }
}
