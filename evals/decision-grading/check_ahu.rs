//! Standalone parser adapter; check_ahu.py builds it in an external temporary directory.
use ahu::eval::{case, suite};
use serde_json::{Value, json};
use std::{error::Error, path::Path};

fn check(root: &Path) -> Result<usize, Box<dyn Error>> {
    let suite = suite::load(&root.join("suite.md"))?;
    let refs: Value = serde_json::from_slice(&std::fs::read(root.join("reference.json"))?)?;
    for entry in &suite.cases {
        let case = &entry.case;
        let reference = &refs[&case.id];
        let answer = &reference["frozen_answer"];
        case::validate_answer(case, answer)?;
        case::validate_answer(case, &Value::Object(case.expected.clone()))?;
        let scores = &reference["expected_criterion_scores"];
        let verdict = json!({"schema_version":1, "criterion_scores":scores, "reason_codes":["synthetic_reference"]});
        let judged = case::validate_judgement(case, &verdict)?;
        assert_eq!(judged.criterion_scores.len(), 3);
        let prompt = case.evaluator_prompt(answer)?;
        let rubric_text = prompt
            .split_once("\n\nRubric:\n")
            .unwrap()
            .1
            .split_once("\n\nCase context,")
            .unwrap()
            .0;
        let rubric: Value = serde_json::from_str(rubric_text)?;
        assert_eq!(rubric, serde_json::to_value(&case.rubric)?);
        let context_text = prompt
            .split_once("```json\n")
            .unwrap()
            .1
            .split_once("\n```")
            .unwrap()
            .0;
        let context: Value = serde_json::from_str(context_text)?;
        assert_eq!(
            context,
            json!({"case_state":case.state,"questions":case.questions})
        );
        let output_text = prompt
            .split_once("Candidate output:\n```json\n")
            .unwrap()
            .1
            .split_once("\n```")
            .unwrap()
            .0;
        assert_eq!(serde_json::from_str::<Value>(output_text)?, *answer);
        let mut hidden_changed = case.clone();
        hidden_changed.id = "HIDDEN_SENTINEL".into();
        hidden_changed.purpose = "HIDDEN_SENTINEL".into();
        hidden_changed.digest = "HIDDEN_SENTINEL".into();
        hidden_changed.expected.clear();
        hidden_changed.scoring.clear();
        assert_eq!(prompt, hidden_changed.evaluator_prompt(answer)?);
        assert!(!prompt.contains("HIDDEN_SENTINEL"));
    }
    Ok(suite.cases.len())
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args().nth(1).ok_or("missing corpus directory")?;
    let root = Path::new(&path);
    let confirmatory = check(root)?;
    let pilot = check(&root.join("pilot"))?;
    assert_eq!((confirmatory, pilot), (12, 2));
    println!(
        "PASS: ahu suite/case parsers, typed answers, reference verdicts and evaluator prompt boundaries for 12 confirmatory + 2 pilot cases."
    );
    Ok(())
}
