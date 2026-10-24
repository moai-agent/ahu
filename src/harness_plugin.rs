//! Experimental external launch planning. Core still owns native admission,
//! event decoding, supervision and task persistence; this is not full extraction.
use crate::harness::{LaunchCommand, LaunchRequest, REDACTED_PROMPT};
use crate::util::{Error, Result};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub harness: String,
    pub path: String,
    pub protocol_version: u32,
    pub timeout_ms: u64,
}
impl Declaration {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.protocol_version != 1
            || !(100..=30_000).contains(&self.timeout_ms)
            || !crate::plugin_binding::plain_relative(&self.path)
            || self.harness.is_empty()
            || self.harness.len() > 64
            || !self
                .harness
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(Error::new("invalid harness launch declaration"));
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct Request<'a> {
    protocol_version: u32,
    harness: &'a str,
    mode: &'a str,
    model: &'a str,
    permissions: crate::agent::Permissions,
    work_mode: bool,
    session: Option<&'a str>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    protocol_version: u32,
    harness: String,
    program: String,
    args: Vec<String>,
    prompt_arg: usize,
}

/// No prompts or native configuration are sent to the planner. The response
/// names one placeholder, replaced only after validation by the coordinator.
pub fn plan(
    repo: &crate::git::Repo,
    harness: &str,
    request: &LaunchRequest<'_>,
    batch: Option<&crate::headless::Spec>,
) -> Result<LaunchCommand> {
    if request.model.is_empty() || request.model.starts_with('-') {
        return Err(Error::new("an exact model is required"));
    }
    if batch.is_some_and(|spec| {
        !spec.options.work_mode || request.permissions != crate::agent::Permissions::Auto
    }) {
        return Err(Error::new(
            "harness plugin pilot supports headless work mode only; guarded profiles are not extracted",
        ));
    }
    let plugin_id = format!("harness-{harness}");
    let binding = crate::interactive::plugins::project::inventory(&repo.root)?
        .bindings.into_iter().find(|entry| entry.binding.id == plugin_id)
        .ok_or_else(|| Error::new(format!("install and enable the {plugin_id} plugin with a harness-launch grant; no built-in fallback")))?.binding;
    if !binding.enabled
        || !binding
            .grants
            .contains(&crate::plugin_binding::Grant::HarnessLaunch)
        || binding.compatibility.ahu != env!("CARGO_PKG_VERSION")
    {
        return Err(Error::new(
            "harness plugin is disabled, incompatible or missing its harness-launch grant; no built-in fallback",
        ));
    }
    let store = crate::state::checkout_root(&repo.primary_root()?)?.join("plugins");
    let installation = crate::plugin_install::installed(&store, &binding)?;
    crate::plugin_install::verify_installed(&installation, &binding)?;
    let package = crate::interactive::plugins::local::read_package(&installation.join("package"))?;
    let declaration: Declaration = serde_json::from_value(
        package
            .metadata
            .get("harness_launch")
            .ok_or_else(|| Error::new("plugin has no harness launch declaration"))?
            .clone(),
    )?;
    declaration.validate()?;
    if declaration.harness != harness {
        return Err(Error::new("harness plugin identity mismatch"));
    }
    let input = serde_json::to_vec(&Request {
        protocol_version: 1,
        harness,
        mode: if batch.is_some() {
            "headless"
        } else {
            "interactive"
        },
        model: request.model,
        permissions: request.permissions,
        work_mode: batch.is_some(),
        session: batch.and_then(|s| s.session.as_deref()),
    })?;
    let (snapshot, executable, timeout) = crate::plugin_exec::prepare_executable(
        repo,
        &binding,
        &declaration.path,
        declaration.timeout_ms,
    )?;
    let bytes = crate::plugin_exec::run_bounded(
        &snapshot,
        &executable,
        "--ahu-harness-launch-protocol=1",
        &input,
        timeout,
        &AtomicBool::new(false),
    )
    .map_err(|error| {
        Error::new(format!(
            "harness launch planner failed ({error}); no harness launched"
        ))
    })?;
    let mut command = decode(&bytes, harness, request.prompt)?;
    command.planner_binding_digest =
        Some(crate::util::digest_bytes(&serde_json::to_vec(&binding)?));
    Ok(command)
}

fn decode(bytes: &[u8], harness: &str, prompt: &str) -> Result<LaunchCommand> {
    let mut response: Response = ahu_host_api::protocol::from_slice(bytes)
        .map_err(|_| Error::new("invalid harness launch response"))?;
    // Pilot program identity is deliberately narrower than a general launch API.
    // Do not let plugin output select an arbitrary path or shell executable.
    if response.protocol_version != 1
        || response.harness != harness
        || response.program != harness
        || response.args.is_empty()
        || response.args.len() > 256
        || response.prompt_arg != response.args.len() - 1
        || response.args[response.prompt_arg] != REDACTED_PROMPT
        || response
            .args
            .iter()
            .filter(|s| s.as_str() == REDACTED_PROMPT)
            .count()
            != 1
        || response.args.iter().any(|s| s.contains('\0'))
    {
        return Err(Error::new(
            "harness launch response violates its identity or prompt contract",
        ));
    }
    response.args[response.prompt_arg] = prompt.into();
    Ok(LaunchCommand {
        planner_binding_digest: None,
        program: response.program,
        args: response.args,
        prompt_arg: Some(response.prompt_arg),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn refuses_identity_prompt_and_schema_confusion() {
        let good = json!({"protocol_version":1,"harness":"codex","program":"codex", "args":["--", REDACTED_PROMPT],"prompt_arg":1});
        let command = decode(
            &serde_json::to_vec(&good).unwrap(),
            "codex",
            "--literal prompt",
        )
        .unwrap();
        assert_eq!(command.args, ["--", "--literal prompt"]);
        for (field, value) in [
            ("program", json!("/bin/sh")),
            ("harness", json!("other")),
            ("protocol_version", json!(2)),
            ("prompt_arg", json!(0)),
            ("extra", json!(true)),
            ("args", json!([REDACTED_PROMPT, REDACTED_PROMPT])),
        ] {
            let mut bad = good.clone();
            bad[field] = value;
            assert!(
                decode(&serde_json::to_vec(&bad).unwrap(), "codex", "private").is_err(),
                "{field}"
            );
        }
        assert!(
            decode(
                br#"{"protocol_version":1,"protocol_version":1}"#,
                "codex",
                "private"
            )
            .is_err()
        );
    }
}
