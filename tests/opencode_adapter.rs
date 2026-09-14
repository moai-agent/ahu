//! What the OpenCode adapter builds, and what it refuses to build.
//!
//! OpenCode's CLI is permissive in exactly the places an adapter can be tempted
//! to over-claim: `--agent <name>` accepts a name that does not exist and only
//! warns, `--model` takes a `provider/model` string whose provider half is
//! resolved by the user's own configuration, and the only permission flag is
//! `--auto`, which widens. So the assertions here are mostly about absence —
//! the flags ahu must never pass — and about refusals that must stay refusals
//! rather than becoming approximations.

mod common;

use ahu::agent::Permissions;
use ahu::harness::{self, LaunchCommand, LaunchRequest};

const MODEL: &str = "ollama/glm-5.3:cloud";
const PROMPT: &str = "<<<contract>>>\nreview the launcher\n<<</contract>>>";

fn command_for(permissions: Permissions) -> ahu::util::Result<LaunchCommand> {
    harness::adapter_for("opencode")
        .unwrap()
        .launch_command(&LaunchRequest {
            model: MODEL,
            prompt: PROMPT,
            cwd: std::path::Path::new("/tmp"),
            permissions,
        })
}

fn error_for(model: &str) -> String {
    harness::adapter_for("opencode")
        .unwrap()
        .launch_command(&LaunchRequest {
            model,
            prompt: PROMPT,
            cwd: std::path::Path::new("/tmp"),
            permissions: Permissions::Prompt,
        })
        .expect_err("the model must be refused")
        .to_string()
}

/// The whole argv, not a subset of it.
///
/// An exact vector is the only assertion that catches a flag being *added*.
/// Verified against OpenCode 1.18.29 and 1.18.30: `--prompt` submits the prompt and leaves
/// the session interactive, and there is no `--` because the default command's
/// positional argument is a project directory.
#[test]
fn the_argv_for_each_supported_permission_value_is_exact() {
    let prompting = command_for(Permissions::Prompt).unwrap();
    assert_eq!(prompting.program, "opencode");
    assert_eq!(
        prompting.args,
        vec!["--model", MODEL, "--prompt", PROMPT],
        "permissions = prompt must pass no permission flag at all"
    );

    let auto = command_for(Permissions::Auto).unwrap();
    assert_eq!(auto.program, "opencode");
    assert_eq!(
        auto.args,
        vec!["--model", MODEL, "--auto", "--prompt", PROMPT],
        "permissions = auto must pass exactly --auto"
    );
}

/// `accept-edits` has no OpenCode equivalent, so it is refused rather than
/// approximated.
///
/// `--auto` would be a widening of what the manifest asked for; passing nothing
/// would report accept-edits on a session that got the harness's own defaults.
/// Both are worse than a refusal that says what to do instead.
#[test]
fn accept_edits_is_refused_with_an_actionable_message() {
    let error = command_for(Permissions::AcceptEdits)
        .expect_err("accept-edits has no OpenCode equivalent")
        .to_string();
    assert!(
        error.contains("no accept-edits"),
        "the refusal must say the mode does not exist: {error}"
    );
    assert!(
        error.contains("--auto") && error.contains("will not widen"),
        "the refusal must say why --auto is not the answer: {error}"
    );
    assert!(
        error.contains("permissions = \"prompt\"") && error.contains("permissions = \"auto\""),
        "the refusal must name the manifest values that do work: {error}"
    );
    assert!(
        error.contains("permission"),
        "the refusal must point at OpenCode's own permission configuration: {error}"
    );
}

/// Three different bad models, three different reasons.
///
/// The unqualified case is the one specific to OpenCode: `glm-5.3:cloud` is a
/// real Ollama tag, and which provider it would reach depends on configuration
/// ahu does not own. Guessing `ollama/` would be a silent misroute.
#[test]
fn an_empty_leading_dash_or_unqualified_model_is_refused_with_its_own_message() {
    let empty = error_for("");
    assert!(
        empty.contains("requires an exact model identifier"),
        "{empty}"
    );

    let dash = error_for("--model");
    assert!(dash.contains("would be read as an option"), "{dash}");

    let unqualified = error_for("glm-5.3:cloud");
    assert!(
        unqualified.contains("not provider-qualified"),
        "{unqualified}"
    );
    assert!(
        unqualified.contains("<provider>/<model>"),
        "the refusal must name the required form: {unqualified}"
    );
    assert!(
        unqualified.contains("will not guess"),
        "the refusal must not offer to guess a provider: {unqualified}"
    );
    assert!(
        !unqualified.contains("ollama/glm-5.3:cloud\"."),
        "the refusal must not resolve the caller's own identifier for them: {unqualified}"
    );

    // The three messages are distinguishable from one another.
    assert_ne!(empty, dash);
    assert_ne!(dash, unqualified);
    assert_ne!(empty, unqualified);

    // A half-qualified or over-qualified identifier is unqualified too.
    for bad in ["/glm-5.3:cloud", "ollama/", "a/b/c"] {
        assert!(
            error_for(bad).contains("not provider-qualified"),
            "{bad} must be refused as unqualified"
        );
    }
}

/// `prompt_arg` indexes the prompt, and `redacted()` replaces exactly it.
#[test]
fn the_prompt_is_one_argv_element_that_redaction_replaces() {
    for permissions in [Permissions::Prompt, Permissions::Auto] {
        let command = command_for(permissions).unwrap();
        let index = command.prompt_arg.expect("the prompt slot is recorded");
        assert_eq!(command.args[index], PROMPT);
        assert_eq!(
            command
                .args
                .iter()
                .filter(|arg| arg.contains(PROMPT))
                .count(),
            1,
            "the prompt must appear exactly once: {:?}",
            command.args
        );

        let redacted = command.redacted();
        assert_eq!(redacted.args[index], harness::REDACTED_PROMPT);
        assert!(
            !redacted.args.iter().any(|arg| arg.contains(PROMPT)),
            "redaction must remove every trace of the prompt: {:?}",
            redacted.args
        );
        // Nothing else moves.
        assert_eq!(redacted.program, command.program);
        assert_eq!(redacted.args.len(), command.args.len());
    }
}

/// A prompt that begins with `-` is refused, because OpenCode would not read it
/// as the `--prompt` value and `--` cannot be used to stop it.
///
/// Observed on 1.18.29 and re-checked on 1.18.30: `opencode --prompt --version` printed the version, and
/// `opencode --prompt -h` printed help — the token after `--prompt` was parsed
/// as an option in both cases. `opencode /definitely/not/a/dir-xyz` failed with
/// "Failed to change directory", which is why `--` is not an option here: the
/// default command's positional argument is a project path.
#[test]
fn a_prompt_beginning_with_a_dash_is_refused_rather_than_silently_reparsed() {
    let error = harness::adapter_for("opencode")
        .unwrap()
        .launch_command(&LaunchRequest {
            model: MODEL,
            prompt: "--version is not a prompt",
            cwd: std::path::Path::new("/tmp"),
            permissions: Permissions::Prompt,
        })
        .expect_err("a prompt ahu cannot deliver intact must not launch")
        .to_string();
    assert!(error.contains("begins with '-'"), "{error}");
    assert!(
        error.contains("project directory"),
        "the refusal must explain why `--` is not the fix: {error}"
    );
}

/// `--agent` is never in the argv, on any permission value.
///
/// A literal assertion, so an edit that adds the flag back fails here rather
/// than in a launch. `--agent <missing>` only warns and falls back to the
/// default agent, so the flag cannot confirm an identity; it would also
/// override the named agent's own model, contradicting the model ahu pins.
#[test]
fn no_agent_selection_or_session_flag_is_ever_passed() {
    for permissions in [Permissions::Prompt, Permissions::Auto] {
        let command = command_for(permissions).unwrap();
        for forbidden in [
            "--agent",
            "--pure",
            "--port",
            "--hostname",
            "--mdns",
            "--cors",
            "--session",
            "-s",
            "--continue",
            "-c",
            "--fork",
            "--share",
            "--mini",
            "--",
        ] {
            assert!(
                !command.args.iter().any(|arg| arg == forbidden),
                "the OpenCode adapter must not pass {forbidden}: {:?}",
                command.args
            );
        }
    }
}

/// The enforcement report carries every catalog gap, and its applied controls
/// describe the argv that was actually built.
#[test]
fn the_enforcement_report_matches_the_catalog_and_the_argv() {
    let adapter = harness::adapter_for("opencode").unwrap();
    let entry = ahu::catalog::harness("opencode").expect("catalog entry");
    assert_eq!(entry.enforcement_gaps.len(), 4);

    for permissions in [Permissions::Prompt, Permissions::Auto] {
        let report = adapter.enforcement(MODEL, permissions).unwrap();
        assert_eq!(report.harness, "opencode");
        assert!(!report.model_fixed_for_session);
        for gap in entry.enforcement_gaps {
            assert!(
                report.gaps.iter().any(|reported| reported == gap),
                "the report must carry catalog gap {gap:?}"
            );
        }
        assert_eq!(report.gaps.len(), entry.enforcement_gaps.len());

        // The four gaps say the four things they have to say.
        let all = report.gaps.join("\n");
        assert!(all.contains("--model"), "{all}");
        assert!(all.contains("Falling back to default agent"), "{all}");
        assert!(
            all.contains("defaults most tool permissions to allow"),
            "{all}"
        );
        assert!(
            all.contains("Ollama's cloud service") && all.contains("not local inference"),
            "the report must not let a :cloud tag read as local inference: {all}"
        );

        let controls = report.applied_controls.join("\n");
        let command = command_for(permissions).unwrap();
        let passes_auto = command.args.iter().any(|arg| arg == "--auto");
        assert_eq!(
            passes_auto,
            permissions == Permissions::Auto,
            "the argv and the permission value must agree"
        );
        if passes_auto {
            assert!(
                controls.contains("ahu passes --auto"),
                "the report must own the flag the argv carries: {controls}"
            );
        } else {
            assert!(
                controls.contains("passes no --auto"),
                "the report must say no --auto was passed when none was: {controls}"
            );
        }
        assert!(
            controls.contains("--model pins the exact provider-qualified model"),
            "{controls}"
        );
        assert!(
            controls.contains("AGENTS.md") && controls.contains("task worktree"),
            "the report must say where OpenCode finds its own rules: {controls}"
        );
    }
}

/// The version in a report comes from the same resolution a launch uses.
///
/// More than one OpenCode can be installed — a `~/.opencode/bin` one and a
/// Homebrew one were both on `PATH` at different versions on the machine this
/// adapter was verified on. A report that probed a different binary than the one
/// that will run would describe a session that never happens.
#[test]
fn the_reported_version_is_the_resolved_binary_not_some_other_install() {
    if !common::in_harness_fixture(
        "the_reported_version_is_the_resolved_binary_not_some_other_install",
    ) {
        return;
    }
    let report = harness::adapter_for("opencode")
        .unwrap()
        .enforcement(MODEL, Permissions::Prompt)
        .unwrap();
    let resolved = ahu::selection::resolve_executable("opencode").expect("fixture opencode");
    assert_eq!(
        report.harness_version,
        ahu::selection::probe_version(&resolved),
        "the report must describe the binary the launch would run"
    );
}

/// A catalog entry names the executable the adapter's program string uses.
#[test]
fn the_catalog_entry_and_the_adapter_agree() {
    let entry = ahu::catalog::harness("opencode").expect("catalog entry");
    assert_eq!(entry.executable, "opencode");
    assert_eq!(
        entry.executable,
        command_for(Permissions::Prompt).unwrap().program
    );
    assert!(entry.adapter_available);
    // Both were checked live; the install moved under the probes on the review
    // date, which is why the entry names two.
    assert_eq!(entry.verified_versions, "1.18.29, 1.18.30");

    let model = ahu::catalog::model("opencode", MODEL).expect("catalog model");
    assert_eq!(model.display_name, "GLM 5.3 (Ollama cloud)");
    assert!(
        model.is_moving_alias,
        "a :cloud tag is mutable; the digest behind it can move"
    );
    assert!(
        model.evaluation_basis.contains("8477dab3e25b"),
        "the basis must name the digest that was observed: {}",
        model.evaluation_basis
    );
    assert_eq!(model.reviewed_on, "2026-09-13");

    // The catalog does not offer this model to any other harness.
    for other in ["claude-code", "codex", "antigravity"] {
        assert!(ahu::catalog::model(other, MODEL).is_none());
    }
}

/// A refusal control string must only appear on a permission value that is
/// actually refused.
///
/// The cross-harness control test skips a `(harness, permissions)` pair whose
/// `launch_command` refuses, because a refused pair has no argv for a control
/// to contradict. That leaves the opposite direction unguarded: were
/// `accept-edits` ever made to build a command — widened to `--auto`, say — the
/// enforcement report would go on claiming ahu "refuses to build this launch at
/// all" while a launch was being built.
///
/// The invariant is the pairing: the report describes a refusal exactly when
/// the adapter refuses.
#[test]
fn a_refusal_control_is_only_reported_for_a_permission_value_that_is_refused() {
    let adapter = harness::adapter_for("opencode").unwrap();
    for permissions in [
        Permissions::Prompt,
        Permissions::AcceptEdits,
        Permissions::Auto,
    ] {
        let control = adapter
            .enforcement(MODEL, permissions)
            .expect("the catalog entry must exist")
            .applied_controls
            .join("\n");
        let claims_refusal = control.contains("refuses to build this launch");
        let refused = command_for(permissions).is_err();
        assert_eq!(
            claims_refusal, refused,
            "the enforcement report for {permissions:?} claims refusal={claims_refusal} \
             while launch_command refused={refused}; a report may describe a refusal only \
             when the adapter actually refuses"
        );
    }
}
