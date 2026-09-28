use std::path::Path;

#[test]
fn impact_cases_score_answers_independently_of_tool_usage() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/impact");
    let suite = ahu::eval::suite::load(&root.join("suites/typed-decision-impact.md")).unwrap();
    assert_eq!(suite.cases.len(), 3);
    for name in ["routing-batch", "priority-batch", "extraction-control"] {
        let case = ahu::eval::case::load(&root.join(format!("cases/{name}.md"))).unwrap();
        assert_eq!(case.questions.len(), 6);
        assert!(case.tool_expectations.is_none());
        let answer = serde_json::to_value(&case.expected).unwrap();
        ahu::eval::case::validate_answer(&case, &answer).unwrap();
        assert_eq!(
            ahu::eval::case::deterministic_score(&case, &answer).unwrap(),
            (1.0, true)
        );
        let mut wrong = answer.clone();
        for (key, question) in &case.questions {
            wrong[key] = question["options"]
                .as_object()
                .unwrap()
                .keys()
                .find(|option| answer[key].as_str() != Some(option.as_str()))
                .unwrap()
                .clone()
                .into();
        }
        assert_eq!(
            ahu::eval::case::deterministic_score(&case, &wrong).unwrap(),
            (0.0, false)
        );
        let prompt = case.candidate_prompt();
        assert!(!prompt.contains("\"expected\""));
        assert!(!prompt.contains("ahu_typed_decide"));
    }
}
