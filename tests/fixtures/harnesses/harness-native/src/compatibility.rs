/// Static capability and isolation contract, separate from local evidence.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IsolationProfile {
    pub harness: &'static str,
    pub headless_verified_versions: &'static [&'static str],
    pub version_policy: &'static str,
    pub evidence: &'static str,
    pub unknown_integration_opt_in: bool,
    pub interactive_supported: bool,
    pub limitations: &'static str,
}

pub fn isolation_profile(id: &str) -> Option<IsolationProfile> {
    let (id, versions) = evidence(id)?;
    let evidence = match id {
        "codex" => {
            "Absent native sources or exact reviewed guarded hook commands. Codex 0.157.1 and 0.160.0 require fresh native hook and requirements inspection with their frozen invocation profiles; credentials remain native and unread, optional plugins and notify are disabled, and mandatory policy must be absent."
        }
        "opencode" => {
            "Absent native sources or the exact reviewed guarded Session plugin; Feed is unsafe, and authentication/account stores, substitutions and declared modules are unresolved."
        }
        "claude-code" => {
            "2.1.283, 2.1.288, 2.1.289 and 2.1.302 headless invocations disable non-managed hooks with frozen profiles. The 2.1.302 profile is based on installed CLI help, vendor documentation and synthetic admission/argv tests, not authenticated headless verification. Enabled plugins and managed settings must still be resolved."
        }
        "antigravity" => {
            "Absent inspected hook configuration; custom hooks, extensions and configuration overrides are unverified."
        }
        "kiro" => {
            "V3 2.28.0/2.29.0 local configuration inspection admits a bounded declarative subset of native JSON/Markdown agents and CLI preferences. Nonempty hooks, extensions, Powers, workflows, unknown fields/formats, cloud configuration and environment indirection are refused. Native helper isolation and arbitrary MCP subprocess behavior remain unverified."
        }
        _ => return None,
    };
    Some(IsolationProfile {
        harness: id,
        headless_verified_versions: versions,
        version_policy: "installed CLI version floats by default; an exact project pin is optional; specialized native controls remain version-specific",
        evidence,
        unknown_integration_opt_in: false,
        interactive_supported: true,
        limitations: "Bounded local inspection is not live conformance, authentication validation or a sandbox. Interactive support still requires normal launch prerequisites and approval checks.",
    })
}

/// The actual CLI version string, with common harness decoration removed.
pub fn version_token(version: &str) -> Option<&str> {
    version.split_whitespace().find_map(|token| {
        let token = token.strip_prefix('v').unwrap_or(token);
        crate::util::is_semver(token).then_some(token)
    })
}

pub(crate) fn evidence(id: &str) -> Option<(&'static str, &'static [&'static str])> {
    Some(match id {
        "codex" => ("codex", &["0.154.0", "0.155.1", "0.157.1", "0.160.0"]),
        "claude-code" => ("claude-code", &["2.1.269", "2.1.270", "2.1.283"]),
        "antigravity" => ("antigravity", &["1.2.2", "1.2.16"]),
        "opencode" => ("opencode", &["1.18.29", "1.18.30", "1.18.31", "1.18.32"]),
        "kiro" => ("kiro", &["2.28.0", "2.29.0"]),
        _ => return None,
    })
}
pub(crate) enum Feature {
    HeadlessLaunch,
}
pub(crate) fn supports(id: &str, _: Feature) -> bool {
    evidence(id).is_some()
}
pub(crate) fn check_headless_version(id: &str, version: &str) -> crate::Result<()> {
    if evidence(id).is_none() || version_token(version).is_none() {
        return Err(crate::Error::invalid());
    }
    Ok(())
}
