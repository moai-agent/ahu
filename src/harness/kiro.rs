//! Kiro CLI V3 interactive adapter, verified with 2.28.0 on 2026-10-08.
//!
//! V3 accepts a positional initial prompt, but inserting `--` before it
//! silently dropped that prompt in the live CLI probe. Pass one argv element
//! without a separator, refusing option-shaped input. ahu's composed prompt
//! starts with its delegation fence. Native settings and agents remain native;
//! ahu delivers the registered instructions as text, not via `--agent` lookup.

use super::{Adapter, EnforcementReport, LaunchCommand, LaunchRequest};
use crate::agent::Permissions;
use crate::util::Result;

pub struct Kiro;

/// Bound native inspection and withhold provider diagnostics. Check sign-in
/// first: Kiro's model-list command opens a browser when no login exists.
pub(crate) fn available_models(
    executable: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<Vec<String>> {
    use std::process::Command;
    use std::time::Duration;
    let mut whoami = Command::new(executable);
    whoami.args(["whoami", "--format", "json"]).current_dir(cwd);
    let identity =
        crate::selection::bounded_config_probe(&mut whoami, true, Duration::from_secs(8))
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    if !identity.as_ref().is_some_and(|value| {
        value
            .get("accountType")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| !s.is_empty())
    }) {
        crate::bail!(
            "Kiro is not signed in or its sign-in status could not be verified; run `kiro-cli login` and retry. Native diagnostics are withheld."
        );
    }
    let mut command = Command::new(executable);
    command
        .args(["chat", "--v3", "--list-models", "--format", "json"])
        .current_dir(cwd);
    let output = crate::selection::bounded_config_probe(&mut command, true, Duration::from_secs(8))
        .ok_or_else(|| {
            crate::util::Error::new(
                "Kiro model listing failed or timed out; native diagnostics are withheld",
            )
        })?;
    parse_model_ids(&output)
}

pub(crate) fn parse_model_ids(output: &str) -> Result<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(output).map_err(|_| {
        crate::util::Error::new(
            "Kiro model listing returned invalid JSON; native diagnostics are withheld",
        )
    })?;
    let models = value
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| crate::util::Error::new("Kiro model listing has no models array"))?;
    models
        .iter()
        .map(|model| {
            model
                .get("model_id")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    crate::util::Error::new(
                        "Kiro model listing contains an invalid model identifier",
                    )
                })
        })
        .collect()
}

impl Adapter for Kiro {
    fn id(&self) -> &'static str {
        "kiro"
    }

    fn launch_command(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand> {
        let mut args = vec!["chat".into(), "--v3".into()];
        args.extend(super::model_args(self.id(), request.model)?);
        match request.permissions {
            Permissions::Prompt => {}
            Permissions::Auto => args.push("--trust-all-tools".into()),
            Permissions::AcceptEdits => crate::bail!(
                "Kiro has no validated accept-edits mode. Use permissions = \"prompt\" or \
                 \"auto\"; ahu will not approximate edit approval with --trust-all-tools."
            ),
        }
        if request.prompt.is_empty() || request.prompt.starts_with('-') {
            crate::bail!(
                "Kiro requires a nonempty composed prompt that does not begin with '-'; \
                 its V3 CLI drops the initial prompt when preceded by `--`."
            );
        }
        let prompt_arg = Some(args.len());
        args.push(request.prompt.into());
        Ok(LaunchCommand {
            planner_binding_digest: None,
            program: "kiro-cli".into(),
            args,
            prompt_arg,
        })
    }

    fn enforcement(&self, _model: &str, permissions: Permissions) -> Result<EnforcementReport> {
        let entry = crate::catalog::harness(self.id()).ok_or_else(|| {
            crate::util::Error::new("Kiro is missing from the compatibility catalog")
        })?;
        let approval = match permissions {
            Permissions::Prompt => {
                "ahu passes no trust override; Kiro's native permission configuration decides when it asks"
            }
            Permissions::Auto => {
                "ahu passes --trust-all-tools because the manifest declares permissions = auto; this also auto-approves hook confirmation requests"
            }
            Permissions::AcceptEdits => {
                "ahu refuses accept-edits because no equivalent Kiro mode has been validated"
            }
        };
        Ok(EnforcementReport {
            harness: self.id().into(),
            harness_version: crate::selection::installed_version("kiro-cli"),
            model_fixed_for_session: entry.enforces_model_for_session,
            gaps: entry.enforcement_gaps.iter().map(|s| s.to_string()).collect(),
            applied_controls: vec![
                "chat --v3 selects the verified engine; --model sets the initial exact model".into(),
                approval.into(),
                "registered instructions travel in the prompt; project .kiro configuration is inherited unchanged".into(),
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_literal_prompt_without_the_separator_that_drops_it() {
        let prompt = "<ahu>\n$(touch /tmp/never) `echo literal`\n--color\n</ahu>";
        for permissions in [Permissions::Prompt, Permissions::Auto] {
            let command = Kiro
                .launch_command(&LaunchRequest {
                    model: "claude-sonnet-4.5",
                    prompt,
                    cwd: std::path::Path::new("/tmp"),
                    permissions,
                })
                .unwrap();
            assert_eq!(command.program, "kiro-cli");
            assert_eq!(
                &command.args[..4],
                ["chat", "--v3", "--model", "claude-sonnet-4.5"]
            );
            assert_eq!(command.args.last().unwrap(), prompt);
            assert!(!command.args.iter().any(|s| s == "--" || s == "--agent"));
            assert_eq!(
                command.args.iter().any(|s| s == "--trust-all-tools"),
                permissions == Permissions::Auto
            );
            assert_eq!(
                command.redacted().args.last().unwrap(),
                super::super::REDACTED_PROMPT
            );
        }
    }

    #[test]
    fn refuses_unrepresentable_permissions_and_option_shaped_prompts() {
        for (permissions, prompt) in [
            (Permissions::AcceptEdits, "request"),
            (Permissions::Prompt, "--version"),
            (Permissions::Prompt, ""),
        ] {
            assert!(
                Kiro.launch_command(&LaunchRequest {
                    model: "claude-sonnet-4.5",
                    prompt,
                    cwd: std::path::Path::new("/tmp"),
                    permissions,
                })
                .is_err()
            );
        }
    }

    #[test]
    fn model_list_uses_exact_ids_and_withholds_malformed_diagnostics() {
        assert_eq!(
            parse_model_ids(
                r#"{"models":[{"model_id":"claude-sonnet-4.5","model_name":"display"}]}"#
            )
            .unwrap(),
            ["claude-sonnet-4.5"]
        );
        for bad in [
            "private-marker",
            r#"{"models":[{"model_name":"private-marker"}]}"#,
            "{}",
        ] {
            let error = parse_model_ids(bad).unwrap_err().to_string();
            assert!(!error.contains("private-marker"));
        }
    }
}
