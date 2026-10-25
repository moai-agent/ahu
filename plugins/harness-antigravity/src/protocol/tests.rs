use super::*;
use crate::planner::{HARNESS, NATIVE_VERSION, PROGRAM};
use serde_json::{Value, json};

fn fixture() -> Value {
    json!({"protocol_version":2,"harness":HARNESS,"mode":"headless",
        "model":"provider/model:tag","permissions":"auto","work_mode":true,
        "session":null,"native_version":NATIVE_VERSION,"timeout_seconds":60})
}
fn accepted(value: &Value) -> bool {
    response(&serde_json::to_vec(value).unwrap()).is_ok()
}

#[test]
fn every_field_is_required_and_duplicate_fields_are_rejected() {
    let value = fixture();
    for key in value.as_object().unwrap().keys() {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(!accepted(&missing), "missing {key}");
        let original = value.to_string();
        let duplicate = format!(
            "{{{}:{},{}",
            serde_json::to_string(key).unwrap(),
            value[key],
            &original[1..]
        );
        assert!(response(duplicate.as_bytes()).is_err(), "duplicate {key}");
    }
}

#[test]
fn unknown_fields_and_invalid_types_are_rejected() {
    let mut value = fixture();
    value["prompt"] = json!("must not be accepted");
    assert!(!accepted(&value));
    for (key, replacements) in [
        (
            "protocol_version",
            vec![json!(1), json!("2"), json!(2.0), Value::Null],
        ),
        (
            "harness",
            vec![json!("other"), json!(""), json!("codex\u{0}")],
        ),
        (
            "mode",
            vec![json!("other"), json!("interactive\n"), Value::Null],
        ),
        (
            "permissions",
            vec![json!("bypass"), json!("auto\u{0}"), Value::Null],
        ),
        ("work_mode", vec![json!(1), json!("true"), Value::Null]),
        (
            "timeout_seconds",
            vec![
                json!(0),
                json!(604801),
                json!(-1),
                json!(1.5),
                json!("60"),
                Value::Null,
            ],
        ),
        ("session", vec![json!({}), json!([]), json!(1), json!(true)]),
    ] {
        for replacement in replacements {
            let mut value = fixture();
            value[key] = replacement;
            assert!(!accepted(&value), "{key}: {}", value[key]);
        }
    }
}

#[test]
fn exact_version_and_timeout_bounds() {
    assert!(accepted(&fixture()));
    for version in [
        "",
        "9.9.9",
        "v0.163.0",
        "2.0.32+build",
        "2.1.310-rc.1",
        "01.4.7",
        "native 1.4.7",
        "1.4.7\n",
    ] {
        let mut value = fixture();
        value["native_version"] = json!(version);
        assert!(!accepted(&value));
    }
    for timeout in [1, 604800] {
        let mut value = fixture();
        value["timeout_seconds"] = json!(timeout);
        assert!(accepted(&value));
    }
}

#[test]
fn models_and_sessions_cannot_inject_options_paths_or_placeholder() {
    for model in [
        "", "-model", " model", "a b", "a\n", "a\u{0}", "a\\b", "$(id)", PROMPT, "é",
    ] {
        let mut value = fixture();
        value["model"] = json!(model);
        assert!(!accepted(&value), "model {model:?}");
    }
    let mut value = fixture();
    value["model"] = json!(format!("p/{}", "a".repeat(255)));
    assert!(!accepted(&value));
    value["model"] = json!(format!("p/{}", "a".repeat(254)));
    assert!(accepted(&value));
    for session in [
        "",
        "--help",
        "-s",
        "../session",
        "a/b",
        "a\\b",
        "a b",
        "a\n",
        "a\u{0}",
        PROMPT,
        "é",
    ] {
        let mut value = fixture();
        value["session"] = json!(session);
        assert!(!accepted(&value), "session {session:?}");
    }
    for session in ["abc-123_DEF".to_owned(), "a".repeat(128)] {
        let mut value = fixture();
        value["session"] = json!(session);
        assert!(accepted(&value));
    }
    let mut value = fixture();
    value["session"] = json!("a".repeat(129));
    assert!(!accepted(&value));
}

#[test]
fn permission_mode_matrix_refuses_guarded_and_interactive_resume() {
    for mode in ["interactive", "headless"] {
        for permission in ["prompt", "accept-edits", "auto"] {
            for work_mode in [false, true] {
                for session in [Value::Null, json!("session_123")] {
                    let mut value = fixture();
                    value["mode"] = json!(mode);
                    value["permissions"] = json!(permission);
                    value["work_mode"] = json!(work_mode);
                    value["session"] = session.clone();
                    let expected = match mode {
                        "interactive" => {
                            !work_mode
                                && session.is_null()
                                && (HARNESS != "opencode" || permission == "prompt")
                        }
                        _ => work_mode && permission == "auto",
                    };
                    assert_eq!(accepted(&value), expected, "{value}");
                }
            }
        }
    }
}

#[test]
fn strict_single_document_framing_and_input_bound() {
    let raw = fixture().to_string();
    for invalid in [
        format!("{raw}{raw}"),
        format!("{raw}garbage"),
        raw[..raw.len() - 1].into(),
        "[]".into(),
        "null".into(),
        String::new(),
    ] {
        assert!(response(invalid.as_bytes()).is_err());
    }
    assert!(response(&[0xff]).is_err());
    let mut exact = raw.into_bytes();
    exact.resize(LIMIT, b' ');
    assert!(response(&exact).is_ok());
    exact.push(b' ');
    assert!(response(&exact).is_err());
}

#[test]
fn response_is_one_bounded_document_with_one_final_literal_placeholder() {
    let output = response(fixture().to_string().as_bytes()).unwrap();
    assert_eq!(output.last(), Some(&b'\n'));
    assert_eq!(output.iter().filter(|&&b| b == b'\n').count(), 1);
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 5);
    assert_eq!(value["protocol_version"], 2);
    assert_eq!(value["harness"], HARNESS);
    assert_eq!(value["program"], PROGRAM);
    let args = value["args"].as_array().unwrap();
    assert_eq!(args.last().unwrap(), PROMPT);
    assert_eq!(args.iter().filter(|v| *v == PROMPT).count(), 1);
    assert_eq!(
        value["prompt_arg"].as_u64().unwrap() as usize,
        args.len() - 1
    );
    let mut plan = Plan {
        protocol_version: 2,
        harness: HARNESS,
        program: PROGRAM,
        args: vec![String::new()],
        prompt_arg: 0,
    };
    let overhead = encode(&plan).unwrap().len();
    plan.args[0] = "x".repeat(LIMIT - overhead);
    assert_eq!(encode(&plan).unwrap().len(), LIMIT);
    plan.args[0].push('x');
    assert!(encode(&plan).is_err());
}

#[test]
fn manifest_and_code_version_evidence_agree() {
    let manifest = include_str!("../../ahu-plugin.md");
    let lines: Vec<_> = manifest.lines().collect();
    for expected in [
        format!("id: harness-{HARNESS}"),
        format!("package: ahu-plugin-{HARNESS}"),
        format!("  harness: {HARNESS}"),
        format!("  program: {PROGRAM}"),
        format!("  path: bin/ahu-plugin-{HARNESS}"),
        "  protocol_version: 2".into(),
        "  timeout_ms: 5000".into(),
        format!("  tested_versions: [\"{NATIVE_VERSION}\"]"),
    ] {
        assert_eq!(
            lines.iter().filter(|line| **line == expected).count(),
            1,
            "{expected}"
        );
    }
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("tested_versions:"))
            .count(),
        1
    );
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.1.0");
}
