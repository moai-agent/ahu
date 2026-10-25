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
