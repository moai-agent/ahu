//! Provider-independent validation. Installed runtime descriptors own all harness
//! identities, suggestions and compatibility evidence; core has no provider table.
use crate::bail;
use crate::util::{Error, Result};

/// The actual CLI version string, with common harness decoration removed.
pub fn version_token(version: &str) -> Option<&str> {
    version.split_whitespace().find_map(|token| {
        let token = token.strip_prefix('v').unwrap_or(token);
        crate::util::is_semver(token).then_some(token)
    })
}

/// Validate an observed semantic version against an optional exact project pin.
/// Provider compatibility is determined by its runtime, never a core allowlist.
pub fn check_version(version: &str, pin: Option<&str>) -> Result<()> {
    let observed = version_token(version)
        .ok_or_else(|| Error::new("native runtime did not report a semantic version"))?;
    if pin.is_some_and(|pin| observed != pin) {
        bail!("native runtime version does not match the exact project pin");
    }
    Ok(())
}

/// Reject conflicting IDs/aliases and core command names before selection.
pub fn validate_runtime_descriptors<'a>(
    descriptors: impl IntoIterator<Item = &'a ahu_harness_api::Descriptor>,
) -> Result<()> {
    // Native shortcuts are provider-owned, so they are intentionally absent here.
    const RESERVED: &[&str] = &[
        "agents",
        "harness",
        "setup",
        "tasks",
        "doctor",
        "migrate",
        "lock",
        "eval",
        "knowledge",
        "mcp",
        "auth",
        "telemetry",
        "explain",
        "onboard",
        "help",
        "run-task",
        "supervise",
        "plugins",
        "task",
        "wait",
        "result",
        "cancel",
        "approve",
        "reject",
        "resume",
        "cleanup",
        "remove",
        "message",
        "launch",
        "version",
        "none",
        "terminal",
    ];
    let mut names = std::collections::BTreeSet::new();
    for descriptor in descriptors {
        descriptor
            .validate()
            .map_err(|_| crate::util::Error::new("invalid harness runtime descriptor"))?;
        for name in std::iter::once(&descriptor.id).chain(&descriptor.aliases) {
            if RESERVED.contains(&name.as_str()) || !names.insert(name) {
                bail!("duplicate or reserved harness runtime identity/alias");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_names_are_owned_unique_and_do_not_collide_with_core_commands() {
        let descriptor: ahu_harness_api::Descriptor = serde_json::from_value(serde_json::json!({
            "id":"sixth","display_name":"Sixth","program":"sixth-native",
            "aliases":["six"],"operations":["probe"],"modes":["direct"]
        }))
        .unwrap();
        validate_runtime_descriptors([&descriptor]).unwrap();
        assert!(validate_runtime_descriptors([&descriptor, &descriptor]).is_err());
        let mut other = descriptor.clone();
        other.id = "seventh".into();
        assert!(validate_runtime_descriptors([&descriptor, &other]).is_err());
        other.aliases = vec!["setup".into()];
        assert!(validate_runtime_descriptors([&other]).is_err());
    }
    #[test]
    fn semantic_version_pins_are_generic_and_exact() {
        assert_eq!(version_token("sixth-native v6.2.0"), Some("6.2.0"));
        check_version("6.2.0", None).unwrap();
        check_version("6.2.0", Some("6.2.0")).unwrap();
        assert!(check_version("6.2.0", Some("6.1.0")).is_err());
        assert!(check_version("unknown", None).is_err());
    }
}
