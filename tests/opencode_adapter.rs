//! Core launch policy and validation through an independent installed plugin.
//! Provider-specific flags are exercised in provider packages, not a core catalog.
mod common;
use ahu::agent::Permissions;
use ahu::harness::LaunchRequest;
use serde_json::{Value, json};
const MODEL: &str = "sixth/exact";
const PROMPT: &str = "review literal $(touch /tmp/ahu-fixture-must-not-run)";
fn plan(permissions: Permissions, policy: Value) -> ahu::util::Result<ahu::launch::LaunchPlan> {
    common::runtime_plan_with_policy(
        "sixth",
        &LaunchRequest {
            model: MODEL,
            prompt: PROMPT,
            cwd: std::path::Path::new("/tmp"),
            permissions,
        },
        policy,
    )
}
#[test]
fn the_argv_for_each_supported_permission_value_is_exact() {
    if !common::in_harness_fixture("the_argv_for_each_supported_permission_value_is_exact") {
        return;
    }
    for (permission, flag) in [
        (Permissions::Prompt, None),
        (Permissions::Auto, Some("--auto")),
        (Permissions::AcceptEdits, Some("--accept-edits")),
    ] {
        let plan = plan(permission, json!({})).unwrap();
        let index = plan.command.prompt_arg.unwrap();
        let mut expected = vec!["--model", MODEL];
        if let Some(flag) = flag {
            expected.push(flag);
        }
        expected.push("--");
        assert_eq!(plan.command.args[..index], expected);
        assert_eq!(plan.command.program, "sixth-native");
        assert_eq!(
            ahu::orchestration::fence_body(
                &plan.command.args[index],
                "request",
                &plan.delivery.nonce
            ),
            Some(PROMPT)
        );
    }
}
#[test]
fn unsupported_permission_is_refused_without_approximating_it() {
    if !common::in_harness_fixture("unsupported_permission_is_refused_without_approximating_it") {
        return;
    }
    assert!(
        plan(
            Permissions::AcceptEdits,
            json!({"refuse_permissions":["accept-edits"]})
        )
        .is_err()
    );
    plan(
        Permissions::Prompt,
        json!({"refuse_permissions":["accept-edits"]}),
    )
    .unwrap();
}
#[test]
fn exact_model_refusal_is_not_replaced_by_a_suggested_model() {
    if !common::in_harness_fixture("exact_model_refusal_is_not_replaced_by_a_suggested_model") {
        return;
    }
    assert!(plan(Permissions::Prompt, json!({"refuse_models":[MODEL]})).is_err());
    for model in ["", "--model", "bad model"] {
        assert!(
            common::runtime_plan(
                "sixth",
                &LaunchRequest {
                    model,
                    prompt: PROMPT,
                    cwd: std::path::Path::new("/tmp"),
                    permissions: Permissions::Prompt
                }
            )
            .is_err()
        );
    }
}
#[test]
fn the_prompt_is_one_argv_element_that_redaction_replaces() {
    if !common::in_harness_fixture("the_prompt_is_one_argv_element_that_redaction_replaces") {
        return;
    }
    let plan = plan(Permissions::Prompt, json!({})).unwrap();
    let index = plan.command.prompt_arg.unwrap();
    assert_eq!(
        plan.command
            .args
            .iter()
            .filter(|arg| arg.contains(PROMPT))
            .count(),
        1
    );
    let mut expected = plan.command.args.clone();
    expected[index] = ahu::harness::REDACTED_PROMPT.into();
    assert_eq!(plan.command.redacted().args, expected);
}
#[test]
fn a_plugin_cannot_redirect_the_native_executable() {
    if !common::in_harness_fixture("a_plugin_cannot_redirect_the_native_executable") {
        return;
    }
    for program in ["different-native", "/bin/sh", "../sixth-native"] {
        assert!(
            plan(
                Permissions::Prompt,
                json!({"launch_overrides":{"program":program}})
            )
            .is_err()
        );
    }
}
#[test]
fn malformed_prompt_positions_are_refused_before_native_execution() {
    if !common::in_harness_fixture("malformed_prompt_positions_are_refused_before_native_execution")
    {
        return;
    }
    for overrides in [
        json!({"prompt_arg":999}),
        json!({"prompt_arg":null}),
        json!({"args":["<prompt: see prompt.txt>","trailing"],"prompt_arg":0}),
        json!({"args":["<prompt: see prompt.txt>","<prompt: see prompt.txt>"],"prompt_arg":1}),
    ] {
        assert!(plan(Permissions::Prompt, json!({"launch_overrides":overrides})).is_err());
    }
}
#[test]
fn unknown_or_missing_launch_fields_do_not_use_compatibility_defaults() {
    if !common::in_harness_fixture(
        "unknown_or_missing_launch_fields_do_not_use_compatibility_defaults",
    ) {
        return;
    }
    assert!(
        plan(
            Permissions::Prompt,
            json!({"launch_overrides":{"untrusted_extra":true}})
        )
        .is_err()
    );
    assert!(
        plan(
            Permissions::Prompt,
            json!({"launch_omit":["native_eval_otlp"]})
        )
        .is_err()
    );
}
#[test]
fn the_enforcement_report_preserves_its_limits_and_installed_identity() {
    if !common::in_harness_fixture(
        "the_enforcement_report_preserves_its_limits_and_installed_identity",
    ) {
        return;
    }
    let plan = plan(Permissions::Prompt, json!({})).unwrap();
    assert_eq!(plan.enforcement.harness, "sixth");
    assert_eq!(plan.enforcement.harness_version.as_deref(), Some("1.2.3"));
    assert!(!plan.enforcement.model_fixed_for_session);
    assert!(!plan.enforcement.gaps.is_empty());
    assert_eq!(
        plan.command.planner_binding_digest.as_deref(),
        Some(plan.pair.runtime_binding_digest.as_str())
    );
}
#[test]
fn metadata_suggestions_are_not_a_core_model_allowlist() {
    if !common::in_harness_fixture("metadata_suggestions_are_not_a_core_model_allowlist") {
        return;
    }
    let model = "invented/unlisted-exact";
    let plan = common::runtime_plan(
        "sixth",
        &LaunchRequest {
            model,
            prompt: PROMPT,
            cwd: std::path::Path::new("/tmp"),
            permissions: Permissions::Prompt,
        },
    )
    .unwrap();
    assert_eq!(plan.pair.model, model);
    assert_eq!(plan.command.args[1], model);
}
#[test]
fn untrusted_argument_limits_are_enforced_before_execution() {
    if !common::in_harness_fixture("untrusted_argument_limits_are_enforced_before_execution") {
        return;
    }
    for args in [
        vec!["x"; 257],
        vec!["bad\0argument", "<prompt: see prompt.txt>"],
    ] {
        assert!(
            plan(
                Permissions::Prompt,
                json!({"launch_overrides":{"args":args,"prompt_arg":args.len()-1}})
            )
            .is_err()
        );
    }
}
