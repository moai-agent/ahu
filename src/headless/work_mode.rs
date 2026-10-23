//! Explicit automatic work, without native configuration admission or overrides.
use crate::native_admission::{HeadlessPolicy, Status};

pub(crate) fn hooks(harness: &str) -> crate::hooks::HookInventory {
    crate::hooks::HookInventory {
        unscanned_harness: Some(harness.into()),
        ..Default::default()
    }
}

pub(crate) fn permission_mapping(harness: &str) -> &'static str {
    match harness {
        "codex" => "approval_policy=never, sandbox_mode=workspace-write",
        "claude-code" => {
            "--permission-prompts none --permission-mode auto (native automatic decisions may deny tools)"
        }
        "antigravity" => "--dangerously-skip-permissions (native permission checks bypassed)",
        "opencode" => "--auto (native explicit denies still apply)",
        "kiro" => "--no-interactive --trust-all-tools",
        _ => "unavailable",
    }
}

pub(crate) fn integration(harness: &str, version: Option<&str>, pin: Option<&str>) -> Status {
    let mut gaps = vec![
        format!("Work mode explicitly selects permissions=auto: {}. Harnesses do not provide identical permission controls; native settings may still deny or block work.", permission_mapping(harness)),
        "Work mode does not inspect native hooks, plugins, configuration or isolation, and does not apply Ahu's native integration or helper overrides. Their presence, activation and effects are unknown. Newer/unreviewed CLI behavior and event coverage remain unverified; admission is not a compatibility certification.".into(),
        "A Git worktree is not a sandbox. Native hooks, plugins, tools and same-user processes may access paths outside it; reported write observations are detection with limited coverage, not prevention or proof of isolation.".into(),
        "Account identity is not verified in work mode. The harness uses its native authentication and may fail if it is unavailable or select a different account.".into(),
        "Provider/model availability is not preflighted in work mode; Ahu passes the exact selected model to the adapter and records the harness response.".into(),
    ];
    match (version, pin) {
        (None, _) => gaps.push(
            "Ahu could not parse the installed CLI version; command compatibility is unverified."
                .into(),
        ),
        (Some(version), Some(pin)) if crate::catalog::version_token(version) != Some(pin) => {
            gaps.push(format!(
                "Installed CLI version differs from the project pin ({version} vs {pin}); work mode proceeds without enforcing the pin."
            ));
        }
        _ => {}
    }
    if harness == "codex" || harness == "claude-code" {
        gaps.push("Ahu's native-helper disable controls are not applied in work mode; native delegation follows harness configuration and agent instructions.".into());
    }
    Status {
        harness: harness.into(),
        native_agent: None,
        profile: None,
        cli_version: version.map(str::to_owned),
        version_checked: version.is_some(),
        components: Vec::new(),
        gaps,
        conformance: "unverified; explicit work mode".into(),
        headless: HeadlessPolicy {
            allowed: true,
            reasons: vec!["explicit work mode; native compatibility admission skipped".into()],
            evidence_digest: String::new(),
        },
        next_action:
            "Review task results and actual work; native configuration remains harness-managed."
                .into(),
    }
}

pub(crate) fn check_version(
    harness: &str,
    version: &str,
    pin: Option<&str>,
) -> crate::util::Result<()> {
    // A version pin is advisory in work mode; the adapter still needs a
    // parseable version to choose the harness-specific command shape.
    let _ = (harness, version, pin);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_mode_reports_unverified_differences_and_project_pin_drift() {
        let status = integration("codex", Some("codex-cli 0.163.0"), Some("0.160.0"));
        assert!(status.headless.allowed);
        assert!(
            status
                .gaps
                .iter()
                .any(|gap| gap.contains("permissions=auto"))
        );
        assert!(status.gaps.iter().any(|gap| gap.contains("not a sandbox")));
        assert!(status.gaps.iter().any(|gap| gap.contains("not verified")));
        assert!(
            status
                .gaps
                .iter()
                .any(|gap| gap.contains("0.163.0 vs 0.160.0"))
        );
        assert!(
            status
                .gaps
                .iter()
                .any(|gap| gap.contains("native delegation follows harness configuration"))
        );
        assert!(
            crate::native_admission::render(&status)
                .contains("headless work mode; native compatibility unverified")
        );
        let rendered = crate::native_admission::render(&status);
        assert!(rendered.contains("note: explicit work mode"));
        assert!(!rendered.contains("refusal:"));
        assert!(
            crate::native_admission::render_summary(&status)
                .contains("work mode; native compatibility unverified")
        );
    }

    #[test]
    fn version_pins_do_not_block_work_mode_attempts() {
        assert!(check_version("codex", "0.163.0", Some("0.160.0")).is_ok());
    }
}
