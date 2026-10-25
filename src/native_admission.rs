//! Provider-neutral validation and presentation of native runtime admission.
use crate::util::{Error, Result, digest_bytes, display_safe};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Registration {
    Installed,
    Missing,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
    Enabled,
    Disabled,
    Unknown,
    NotApplicable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    Absent,
    VerifiedDisable,
    Unsafe,
    Unknown,
    DirectExecutableOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Conformance {
    Untested,
    SourceInspected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub path: PathBuf,
    pub scope: String,
    pub method: String,
    pub digest: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub name: String,
    pub registration: Registration,
    pub activation: Activation,
    pub isolation: Isolation,
    pub conformance: Conformance,
    pub evidence: Vec<Evidence>,
    pub detail: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub harness: String,
    pub native_agent: Option<String>,
    pub profile: Option<ProfileEvidence>,
    pub cli_version: Option<String>,
    pub version_checked: bool,
    pub components: Vec<Component>,
    pub gaps: Vec<String>,
    pub conformance: String,
    pub headless: HeadlessPolicy,
    pub next_action: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadlessPolicy {
    pub allowed: bool,
    pub reasons: Vec<String>,
    pub evidence_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileEvidence {
    pub harness: String,
    pub headless_verified_versions: Vec<String>,
    pub version_policy: String,
    pub evidence: String,
    pub unknown_integration_opt_in: bool,
    pub interactive_supported: bool,
    pub limitations: String,
}
impl Status {
    pub fn with_version(mut self, version: Option<&str>) -> Self {
        self.cli_version = version.map(str::to_owned);
        self.version_checked = true;
        self
    }
}

pub fn render_summary(status: &Status) -> String {
    let installed = status
        .components
        .iter()
        .filter(|c| c.registration == Registration::Installed)
        .count();
    let unknown = status
        .components
        .iter()
        .filter(|c| c.registration == Registration::Unknown)
        .count();
    format!(
        "  native     {installed} verified component(s), {unknown} unknown; activation/live delivery unverified; headless {}. Details are included in the launch review.\n",
        if status.headless.allowed {
            if status.conformance == "unverified; explicit work mode" {
                "work mode; native compatibility unverified"
            } else {
                "compatible with inspected components"
            }
        } else {
            "refused"
        }
    )
}

pub fn render(status: &Status) -> String {
    let mut text = format!(
        "native integration {}: headless {}\n",
        display_safe(&status.harness),
        if status.headless.allowed {
            if status.conformance == "unverified; explicit work mode" {
                "work mode; native compatibility unverified"
            } else {
                "compatible with inspected components"
            }
        } else {
            "refused"
        }
    );
    text.push_str(&format!(
        "  harness CLI version: {} ({}); approval, executable and launch checks still apply\n",
        display_safe(status.cli_version.as_deref().unwrap_or("unknown")),
        if status.version_checked {
            "checked"
        } else {
            "not checked; component compatibility only"
        }
    ));
    if let Some(profile) = &status.profile {
        text.push_str(&format!(
            "  profile {}: {}; behavior checked on: {}\n  evidence: {}\n  unknown integration opt-in: {}; interactive supported: {}\n  limitations: {}\n",
            display_safe(&profile.harness), display_safe(&profile.version_policy), display_safe(&profile.headless_verified_versions.join(", ")),
            display_safe(&profile.evidence), profile.unknown_integration_opt_in, profile.interactive_supported, display_safe(&profile.limitations)
        ));
    }
    for reason in &status.headless.reasons {
        text.push_str(&format!(
            "  {}: {}\n",
            if status.headless.allowed {
                "note"
            } else {
                "refusal"
            },
            display_safe(reason)
        ));
    }
    for c in &status.components {
        text.push_str(&format!(
            "  {}: {:?}; activation {:?}; isolation {:?}; conformance {:?}\n    {}\n",
            display_safe(&c.name),
            c.registration,
            c.activation,
            c.isolation,
            c.conformance,
            display_safe(&c.detail)
        ));
        for e in &c.evidence {
            text.push_str(&format!(
                "    {} [{}; {}{}]\n",
                display_safe(&e.path.to_string_lossy()),
                display_safe(&e.scope),
                display_safe(&e.method),
                e.digest
                    .as_ref()
                    .map(|d| format!("; sha256 {}", display_safe(d)))
                    .unwrap_or_default()
            ));
        }
    }
    text.push_str(&format!(
        "  conformance: {}\n  next: {}\n",
        display_safe(&status.conformance),
        display_safe(&status.next_action)
    ));
    for gap in &status.gaps {
        text.push_str(&format!("  gap: {}\n", display_safe(gap)));
    }
    text
}

fn invalid() -> Error {
    Error::new("native admission is unavailable or returned invalid evidence")
}
fn unknown(harness: &str) -> Status {
    Status {
        harness: harness.into(),
        native_agent: None,
        profile: None,
        cli_version: None,
        version_checked: false,
        components: vec![],
        gaps: vec!["Installed provider inspection failed; native configuration is unknown".into()],
        conformance: "untested".into(),
        headless: HeadlessPolicy {
            allowed: false,
            reasons: vec!["native admission unavailable".into()],
            evidence_digest: digest_bytes(b"unknown"),
        },
        next_action: "Review the installed runtime and retry inspection".into(),
    }
}
/// Failure remains explicit unknown evidence for interactive disclosure.
pub fn inspect(repo: &Path, harness: &str) -> Status {
    inspect_result(repo, harness).unwrap_or_else(|_| unknown(harness))
}
pub fn inspect_result(root: &Path, harness: &str) -> Result<Status> {
    let repo = crate::git::discover(root)?;
    let runtime = crate::harness_runtime::load(&repo, harness)?;
    let mut params = crate::harness_runtime::native_params(&repo, runtime.descriptor())?;
    params["environment_names"] = json!(
        std::env::vars_os()
            .filter_map(|(name, _)| name.into_string().ok())
            .collect::<Vec<_>>()
    );
    params["settings"] = json!({"inspection_only":true});
    validate(runtime.call("admission", params)?, harness, false)
}
fn validate(value: Value, harness: &str, require_allowed: bool) -> Result<Status> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        allowed: bool,
        decision: String,
        guarded_checks: String,
        conformance: String,
        evidence: Status,
        helper_profile: Value,
        invocation_profile: Value,
    }
    if serde_json::to_vec(&value)?.len() > 4 * 1024 * 1024 {
        return Err(invalid());
    }
    let response: Response = serde_json::from_value(value).map_err(|_| invalid())?;
    let status = response.evidence;
    if status.harness != harness
        || status.components.len() > 4096
        || status.gaps.len() > 1024
        || status.headless.evidence_digest.len() != 64
        || !status
            .headless
            .evidence_digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
        || response.conformance != "untested"
        || !["admitted", "refused", "work_mode", "inspection"].contains(&response.decision.as_str())
        || !["inspected", "intentionally_skipped", "inventory_only"]
            .contains(&response.guarded_checks.as_str())
        || (!response.helper_profile.is_null() && !response.helper_profile.is_object())
        || (!response.invocation_profile.is_null() && !response.invocation_profile.is_object())
        || (require_allowed
            && (!response.allowed
                || !status.headless.allowed
                || response.guarded_checks != "inspected"))
    {
        return Err(invalid());
    }
    for component in &status.components {
        if component.evidence.len() > 1024
            || component.evidence.iter().any(|item| {
                item.digest
                    .as_ref()
                    .is_some_and(|d| d.len() != 64 || !d.bytes().all(|b| b.is_ascii_hexdigit()))
            })
        {
            return Err(invalid());
        }
    }
    Ok(status)
}
/// Validate the exact interactive launch identity through its installed provider.
pub fn validate_launch(
    root: &Path,
    harness: &str,
    model: &str,
    permissions: crate::agent::Permissions,
    mode: &str,
    version: &str,
    executable: &Path,
) -> Result<Status> {
    if !["interactive", "direct"].contains(&mode) {
        return Err(invalid());
    }
    let repo = crate::git::discover(root)?;
    let runtime = crate::harness_runtime::load(&repo, harness)?;
    let mut params = crate::harness_runtime::native_params(&repo, runtime.descriptor())?;
    params["environment_names"] = json!(
        std::env::vars_os()
            .filter_map(|(name, _)| name.into_string().ok())
            .collect::<Vec<_>>()
    );
    params["model"] = json!(model);
    params["permissions"] = json!(permissions);
    params["mode"] = json!(mode);
    params["version"] = json!(version);
    params["native_executable"] = json!(executable);
    params["settings"] = json!({});
    let result = runtime.call("admission", params)?;
    if result.get("allowed") != Some(&json!(true))
        || result.get("decision") != Some(&json!("admitted"))
        || result.get("guarded_checks") != Some(&json!("inspected"))
    {
        return Err(invalid());
    }
    // Interactive disclosure may report headless refusal without denying an
    // independently validated interactive invocation.
    validate(result, harness, false)
}
/// Provider-specific policy stays opaque; caller authorizes settings before dispatch.
pub fn enforce_with_settings(
    root: &Path,
    harness: &str,
    version: &str,
    executable: &Path,
    settings: Value,
) -> Result<Status> {
    let repo = crate::git::discover(root)?;
    let runtime = crate::harness_runtime::load(&repo, harness)?;
    let mut params = crate::harness_runtime::native_params(&repo, runtime.descriptor())?;
    params["environment_names"] = json!(
        std::env::vars_os()
            .filter_map(|(name, _)| name.into_string().ok())
            .collect::<Vec<_>>()
    );
    params["native_executable"] = json!(executable);
    params["version"] = json!(version);
    params["mode"] = json!("headless");
    params["settings"] = settings;
    validate(runtime.call("admission", params)?, harness, true)
}
pub fn enforce_headless_executable(
    root: &Path,
    harness: &str,
    version: &str,
    executable: &Path,
) -> Result<Status> {
    enforce_with_settings(root, harness, version, executable, json!({}))
}
pub fn enforce_headless(root: &Path, harness: &str, version: &str) -> Result<Status> {
    let repo = crate::git::discover(root)?;
    let runtime = crate::harness_runtime::load(&repo, harness)?;
    let params = crate::harness_runtime::native_params(&repo, runtime.descriptor())?;
    let executable = params
        .get("native_executable")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    enforce_headless_executable(root, harness, version, Path::new(executable))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_and_cross_provider_evidence_cannot_admit() {
        let status = unknown("invented");
        let value = json!({"allowed":true,"decision":"admitted","guarded_checks":"inspected","conformance":"untested","evidence":status,"helper_profile":null,"invocation_profile":null});
        assert!(validate(value.clone(), "other", false).is_err());
        assert!(validate(value, "invented", true).is_err());
    }
}
