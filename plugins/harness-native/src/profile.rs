use crate::{Error, Result, helpers};
use serde::Deserialize;
use serde_json::Value;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    action: String,
    project: Option<std::path::PathBuf>,
    request: Request,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    harness: String,
    harness_version: String,
    policy: String,
    session_model: String,
    helper_model: Option<String>,
    helper_role: String,
    max_concurrent: u32,
    max_depth: u32,
    budget_usd: Option<f64>,
    assignment_writes: bool,
}
pub(crate) fn call(harness: &str, value: &Value) -> Result<Value> {
    if value.get("action").and_then(Value::as_str) == Some("execution_capabilities") {
        return execution_capabilities(harness, value);
    }
    let p: Params = crate::decode(value)?;
    let r = p.request;
    if p.action != "native_profile"
        || r.harness != harness
        || !crate::valid_model(&r.session_model)
        || r.helper_model
            .as_deref()
            .is_some_and(|m| !crate::valid_model(m))
    {
        return Err(Error::invalid());
    }
    if let Some(project) = p.project {
        crate::directory(&project)?;
    }
    let mut profile = helpers::profile(&helpers::Request {
        harness: &r.harness,
        harness_version: &r.harness_version,
        policy: &r.policy,
        session_model: &r.session_model,
        helper_model: r.helper_model.as_deref(),
        helper_role: &r.helper_role,
        max_concurrent: r.max_concurrent,
        max_depth: r.max_depth,
        budget_usd: r.budget_usd,
        assignment_writes: r.assignment_writes,
    })?;
    if harness == "codex" {
        profile
            .args
            .extend(["-c".into(), "agents.enabled=false".into()]);
    }
    if let Some(isolation) = crate::isolation::profile(harness, &r.harness_version) {
        profile
            .args
            .extend(isolation.args.iter().map(|s| s.to_string()));
        profile.enforced.push(helpers::Control{id:format!("native.isolation.{}",isolation.id),mechanism:helpers::Mechanism::Flag,detail:"Version-specific native invocation isolation; effective configuration still requires admission".into()});
    }
    Ok(serde_json::to_value(profile)?)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityParams {
    action: String,
    harness: String,
    native_version: String,
    permissions: crate::Permissions,
    work_mode: bool,
    #[serde(default = "disabled_helpers")]
    native_helpers: String,
}

fn disabled_helpers() -> String {
    "disabled".into()
}

fn execution_capabilities(harness: &str, value: &Value) -> Result<Value> {
    let p: CapabilityParams = crate::decode(value)?;
    if p.action != "execution_capabilities"
        || p.harness != harness
        || !matches!(
            harness,
            "codex" | "claude-code" | "antigravity" | "opencode" | "kiro"
        )
        || p.native_version.is_empty()
        || p.native_version.len() > 256
        || p.native_version.chars().any(char::is_control)
        || !matches!(p.native_helpers.as_str(), "disabled" | "bounded")
    {
        return Err(Error::invalid());
    }
    // These are invocation permission limits, not evidence of guarded admission
    // or version certification. Admission still checks effective native settings.
    let read_only =
        !p.work_mode && harness == "codex" && p.permissions == crate::Permissions::Prompt;
    let helpers_read_only = !p.work_mode && p.native_helpers == "bounded";
    Ok(serde_json::json!({
        "structured_output": !read_only && !helpers_read_only,
        "broker_transport": !read_only,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn execution_capabilities_preserve_permission_limits() {
        for harness in ["codex", "claude-code", "antigravity", "opencode", "kiro"] {
            for permission in ["prompt", "accept-edits", "auto"] {
                for work_mode in [false, true] {
                    for helpers in ["disabled", "bounded"] {
                        let value = crate::call(
                            harness,
                            "admission",
                            &json!({
                                "action":"execution_capabilities", "harness":harness,
                                "native_version":"unknown", "permissions":permission,
                                "work_mode":work_mode, "native_helpers":helpers,
                            }),
                        )
                        .unwrap();
                        let writable = work_mode || harness != "codex" || permission != "prompt";
                        assert_eq!(
                            value,
                            json!({
                                "structured_output": writable && (work_mode || helpers != "bounded"),
                                "broker_transport": writable,
                            })
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn execution_capabilities_validate_without_native_execution() {
        let params = json!({"action":"execution_capabilities", "harness":"codex",
            "native_version":"0.163.0", "permissions":"prompt", "work_mode":false});
        assert_eq!(
            crate::call("codex", "admission", &params).unwrap(),
            json!({
            "structured_output":false, "broker_transport":false})
        );
        assert!(crate::call("kiro", "admission", &params).is_err());
        for (key, value) in [
            ("native_helpers", json!("unrestricted")),
            ("permissions", json!("yes")),
            ("extra", json!(true)),
        ] {
            let mut invalid = params.clone();
            invalid[key] = value;
            assert!(crate::call("codex", "admission", &invalid).is_err());
        }
    }
}
