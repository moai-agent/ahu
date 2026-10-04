//! Version-specific headless controls. Interactive adapters never use these.
//! Claude's disableAllHooks preserves managed hooks, skills and direct MCP:
//! https://code.claude.com/docs/en/hooks#disable-or-remove-hooks
//! Verified on 2.1.283 and 2.1.288 with synthetic
//! SessionStart/UserPromptSubmit/Stop markers. Homes and authentication stay native.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub id: &'static str,
    pub args: &'static [&'static str],
}

const CLAUDE_ARGS: &[&str] = &["--settings", "{\"disableAllHooks\":true}"];
// Never disable hooks or bypass trust. Native strict configuration refuses
// conflicts with mandatory feature requirements. Admission additionally
// requires an explicit null requirements response.
const CODEX_ARGS: &[&str] = &[
    "--strict-config",
    "-c",
    "features.hooks=true",
    "-c",
    "features.plugins=false",
    "-c",
    "features.remote_plugin=false",
    "-c",
    "notify=[]",
];

pub fn profile(harness: &str, version: &str) -> Option<Profile> {
    let version = version
        .split_whitespace()
        .find(|v| crate::util::is_semver(v))?;
    match (harness, version) {
        ("claude-code", "2.1.283") => Some(Profile {
            id: "claude-2.1.283-nonmanaged-hooks-disabled-v1",
            args: CLAUDE_ARGS,
        }),
        ("claude-code", "2.1.288") => Some(Profile {
            id: "claude-2.1.288-nonmanaged-hooks-disabled-v1",
            args: CLAUDE_ARGS,
        }),
        ("codex", "0.157.1") => Some(Profile {
            id: "codex-0.157.1-effective-guarded-hooks-v1",
            args: CODEX_ARGS,
        }),
        ("codex", "0.160.0") => Some(Profile {
            id: "codex-0.160.0-effective-guarded-hooks-v1",
            args: CODEX_ARGS,
        }),
        _ => None,
    }
}
