//! Version-specific headless controls. Interactive adapters never use these.
//! Claude's disableAllHooks preserves managed hooks, skills and direct MCP:
//! https://code.claude.com/docs/en/hooks#disable-or-remove-hooks
//! Verified on 2.1.283 with synthetic SessionStart/UserPromptSubmit/Stop markers.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub id: &'static str,
    pub args: &'static [&'static str],
}

pub fn profile(harness: &str, version: &str) -> Option<Profile> {
    let version = version
        .split_whitespace()
        .find(|v| crate::util::is_semver(v))?;
    match (harness, version) {
        ("claude-code", "2.1.283") => Some(Profile {
            id: "claude-2.1.283-nonmanaged-hooks-disabled-v1",
            args: &["--settings", "{\"disableAllHooks\":true}"],
        }),
        ("codex", "0.157.1") => Some(Profile {
            id: "codex-0.157.1-effective-guarded-hooks-v1",
            // Never disable hooks or bypass trust. Native strict configuration
            // refuses conflicts with mandatory feature requirements. Admission
            // additionally requires an explicit null requirements response.
            args: &[
                "--strict-config",
                "-c",
                "features.hooks=true",
                "-c",
                "features.plugins=false",
                "-c",
                "features.remote_plugin=false",
                "-c",
                "notify=[]",
            ],
        }),
        _ => None,
    }
}
