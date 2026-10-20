//! Bounded evidence about native integrations, plus the gated entry points
//! that act on it. Inspection and status evidence collection is read-only: it
//! reads at most LIMIT bytes per file, considers at most MAX_ENTRIES entries,
//! and no hook bodies are persisted, interpreted, installed, or copied by
//! inspection.
//!
//! Mutation is narrow and individually gated: `sanitize` runs only after
//! admission, removing `CMUX_*` routing variables from a child command and
//! restoring reviewed disable controls; `execute_install` runs the native
//! installer only after the installation plan is inspected and verified
//! (`plan.available` and the exact `["hooks", agent, "install"]` argv check).
//! The `enforce*` family are pure gate predicates over inspection results:
//! they read and decide, never mutate.
//!
//! Native command generation reviewed against cmux ddd4a01bc:
//! <https://github.com/manaflow-ai/cmux/blob/ddd4a01bc/CLI/CMUXCLI%2BAgentHookDefinitions.swift>.
//! OpenCode and Claude fingerprints identify inspected native artifacts, not
//! implementations maintained by ahu. A changed fingerprint is unknown.
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use serde_json::Value;

use crate::util::{Error, Result, digest_bytes, display_safe, shell_single_quote};

const LIMIT: u64 = 1024 * 1024;
const MAX_ENTRIES: usize = 256;
const REVIEWED_VERSION: &str = "0.64.22 (102) [ddd4a01bc]";
const FEED_DIGEST: &str = "aae76d0577a008d89e96932abb3b7c61d83b0086ca5e08b7622aac0aa8d76b9c";
const SESSION_DIGEST: &str = "a9664ca1aff28fc5aa194efdfb2ed41b2584ee4569e91503e9388b3035a625b7";
const WRAPPER_DIGEST: &str = "e930666189e326972d4b4ed36ca7bd955260845953a779b3049be44677c175d4";

// Exact native command bytes reviewed in cmux ddd4a01bc. These fingerprints
// identify code; a marker, filename, or matching substring never establishes it.
const CODEX_COMMANDS: &[(&str, &str)] = &[
    (
        "PermissionRequest",
        "563533aafffeb346cf801342b83777c927125ddaa001f6061a4254c955d7c3ca",
    ),
    (
        "PostCompact",
        "debac46f677cacc603c95b1779a466528224ccb369b95e1faca5e812a60a79b8",
    ),
    (
        "PostToolUse",
        "af2cc98aebaaa20784604a798fa84b612470ea79027be6c5bf878f044dad3f6d",
    ),
    (
        "PreCompact",
        "502302f8fbc0df0ba49e1d0f662371bcc8a1135b8783e9fce2a4ddb961ad0b5b",
    ),
    (
        "PreToolUse",
        "0f640048731777c1d4b134f34b3c2a7a4f2db9d2b3e21051d28731bed37f3f1b",
    ),
    (
        "SubagentStart",
        "8f855b07fbe1ace2f17f5c1a863cf2ea8abcfffb00b9711d48f6180a819462b4",
    ),
    (
        "SubagentStop",
        "39ce821db00c1a26ae934a3d033297df519e9260d3f33fb31b2550ad48a32df5",
    ),
    (
        "UserPromptSubmit",
        "9b5ed21ec71b8f3996e912663f3ff10072b3fdcacec15d4bd5999cb2d13955a3",
    ),
    (
        "SessionStart",
        "6d14ad8d3539332107af5c68dcb89edecf2b8972c8848f55a9e075d53077b942",
    ),
    (
        "Stop",
        "ec802d73827f74550c99641ac4cabcd6c7190925160a573d6d5c46c3c740ea47",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Registration {
    Installed,
    Missing,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
    Enabled,
    Disabled,
    Unknown,
    NotApplicable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    Absent,
    VerifiedDisable,
    Unsafe,
    Unknown,
    DirectExecutableOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Conformance {
    Untested,
    SourceInspected,
}

#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub path: PathBuf,
    pub scope: String,
    pub method: String,
    pub digest: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Component {
    pub name: String,
    pub registration: Registration,
    pub activation: Activation,
    pub isolation: Isolation,
    pub conformance: Conformance,
    pub evidence: Vec<Evidence>,
    pub detail: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub harness: String,
    pub native_agent: Option<String>,
    pub profile: Option<crate::catalog::IsolationProfile>,
    pub cli_version: Option<String>,
    pub version_checked: bool,
    pub components: Vec<Component>,
    pub gaps: Vec<String>,
    pub conformance: String,
    pub headless: HeadlessPolicy,
    pub next_action: String,
}
impl Status {
    /// Add a version observation to the same evidence used for interactive
    /// disclosure. This never gates interactive execution or probes a provider.
    pub fn with_version(mut self, version: Option<&str>) -> Self {
        self.version_checked = true;
        self.cli_version = version.map(str::to_owned);
        if self.harness == "codex"
            && crate::harness::isolation::profile("codex", version.unwrap_or("")).is_some()
            && !self
                .components
                .iter()
                .any(|c| c.name == "native invocation profile")
        {
            self.headless.allowed = false;
            self.headless.reasons.push("This Codex version requires fresh native effective hook and requirements inspection using the frozen invocation profile".into());
        }
        if let Err(error) =
            crate::catalog::check_headless_version(&self.harness, version.unwrap_or(""))
        {
            self.headless.allowed = false;
            self.headless.reasons.push(error.to_string());
            if crate::catalog::supports(&self.harness, crate::catalog::Feature::HeadlessLaunch) {
                self.next_action = format!(
                    "Use a CLI that reports a parseable semantic version. Interactive execution remains available subject to normal prerequisites. {}",
                    self.next_action
                );
            } else {
                self.next_action = "Use interactive execution; this harness has no validated ahu headless adapter.".into();
            }
        }
        if self.harness == "kiro" && !kiro_inspected_version(version.unwrap_or("")) {
            self.headless.allowed = false;
            self.headless.reasons.push(
                "Kiro headless configuration inspection requires inspected V3 2.28.0 or 2.29.0"
                    .into(),
            );
        }
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HeadlessPolicy {
    pub allowed: bool,
    pub disable_variable: Option<String>,
    pub reasons: Vec<String>,
    pub evidence_digest: String,
}

/// Result of the native preference presence probe. Values are never decoded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ManagedPreferences {
    #[default]
    NotApplicable,
    Absent,
    Present,
    Unknown,
}

#[cfg(target_os = "macos")]
fn codex_managed_preferences() -> ManagedPreferences {
    use std::ffi::{c_char, c_void};
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            text: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFPreferencesCopyAppValue(
            key: *const c_void,
            application: *const c_void,
        ) -> *const c_void;
        fn CFRelease(object: *const c_void);
    }
    // Match the native loader's application and keys. Inspect only whether the
    // API returns an opaque retained value; do not decode, log or hash values.
    // SAFETY: static NUL-terminated strings use the UTF-8 encoding constant.
    // Every non-null create/copy reference is released exactly once.
    unsafe {
        let domain =
            CFStringCreateWithCString(std::ptr::null(), c"com.openai.codex".as_ptr(), 0x08000100);
        if domain.is_null() {
            return ManagedPreferences::Unknown;
        }
        for name in [c"config_toml_base64", c"requirements_toml_base64"] {
            let key = CFStringCreateWithCString(std::ptr::null(), name.as_ptr(), 0x08000100);
            if key.is_null() {
                CFRelease(domain);
                return ManagedPreferences::Unknown;
            }
            let value = CFPreferencesCopyAppValue(key, domain);
            CFRelease(key);
            if !value.is_null() {
                CFRelease(value);
                CFRelease(domain);
                return ManagedPreferences::Present;
            }
        }
        CFRelease(domain);
    }
    ManagedPreferences::Absent
}
#[cfg(not(target_os = "macos"))]
fn codex_managed_preferences() -> ManagedPreferences {
    ManagedPreferences::NotApplicable
}

fn codex_system_root() -> PathBuf {
    // Only this OS-owned, fixed native anchor has a known macOS alias. User
    // homes, config overrides, artifacts and children still reject all links.
    let root = if cfg!(target_os = "macos")
        && std::fs::read_link("/etc")
            .is_ok_and(|p| p == Path::new("private/etc") || p == Path::new("/private/etc"))
    {
        Path::new("/private/etc")
    } else {
        Path::new("/etc")
    };
    root.join("codex")
}

/// Explicit inputs keep fixture tests independent of the host's configuration.
#[derive(Debug, Clone, Default)]
pub struct Locations {
    pub home: Option<PathBuf>,
    pub overrides: BTreeMap<String, String>,
    pub wrapper: Option<PathBuf>,
    pub managed_claude: Option<PathBuf>,
    pub managed_opencode: Option<PathBuf>,
    pub managed_preferences: Option<PathBuf>,
    pub system_codex: Option<PathBuf>,
    pub codex_managed_preferences: ManagedPreferences,
}
impl Locations {
    pub fn detect() -> Self {
        let names = [
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "XDG_CONFIG_HOME",
            "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_DIR",
            "OPENCODE_CONFIG_CONTENT",
            "OPENCODE_AUTH_CONTENT",
            "OPENCODE_TEST_MANAGED_CONFIG_DIR",
            "OPENCODE_TEST_HOME",
            "OPENCODE_DB",
            "XDG_DATA_HOME",
            "GEMINI_CLI_HOME",
            "AGY_CONFIG_DIR",
            "KIRO_HOME",
            "KAS_BUNDLE_PATH",
            "KIRO_TEST_WORKSPACE_SETTINGS_PATH",
            "CLOUD_CONFIG_ENDPOINT",
            "KIRO_REMOTE_SESSIONS_ENDPOINT",
            "KIRO_ACP_RECORD_PATH",
            "KIRO_CHAT_LOG_FILE",
            "KIRO_DUMP_CONFIGURATION",
            "KIRO_DUMP_REQUESTS",
            "GLOBAL_ENV_FILE",
            "ASBX_KIRO_MANDATORY_MCPS",
        ];
        Self {
            home: std::env::var_os("HOME").map(PathBuf::from),
            overrides: names
                .into_iter()
                .filter_map(|key| std::env::var_os(key).map(|_| (key.to_string(), String::new())))
                .collect(),
            wrapper: Some(PathBuf::from(
                "/Applications/cmux.app/Contents/Resources/bin/cmux-claude-wrapper",
            )),
            managed_claude: Some(PathBuf::from(
                "/Library/Application Support/ClaudeCode/managed-settings.json",
            )),
            managed_opencode: Some(PathBuf::from(if cfg!(target_os = "macos") {
                "/Library/Application Support/opencode"
            } else {
                "/etc/opencode"
            })),
            managed_preferences: cfg!(target_os = "macos")
                .then(|| PathBuf::from("/Library/Managed Preferences")),
            system_codex: Some(codex_system_root()),
            codex_managed_preferences: codex_managed_preferences(),
        }
    }
}

fn native_agent(harness: &str) -> Option<&'static str> {
    match harness {
        "codex" => Some("codex"),
        "opencode" => Some("opencode"),
        "antigravity" => Some("antigravity"),
        "claude-code" => Some("claude"),
        _ => None,
    }
}
fn disable_variable(harness: &str) -> Option<&'static str> {
    match harness {
        "codex" => Some("CMUX_CODEX_HOOKS_DISABLED"),
        "opencode" => Some("CMUX_OPENCODE_HOOKS_DISABLED"),
        "claude-code" => Some("CMUX_CLAUDE_HOOKS_DISABLED"),
        "antigravity" => Some("CMUX_ANTIGRAVITY_HOOKS_DISABLED"),
        _ => None,
    }
}

// Check all existing ancestors before opening: dangling links, linked parent
// directories and special files are uncertainty, never evidence of absence.
fn check_path(path: &Path) -> std::result::Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("relative or unresolved path".into());
    }
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("symlink evidence is unresolved".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("cannot inspect path metadata".into()),
        }
    }
    Ok(())
}
fn read(path: &Path) -> std::result::Result<Option<Vec<u8>>, String> {
    check_path(path)?;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("cannot inspect file".into()),
    };
    if !meta.is_file() || meta.len() > LIMIT {
        return Err("not a bounded regular file".into());
    }
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "cannot open file")?;
    if !file
        .metadata()
        .map_err(|_| "cannot inspect opened file")?
        .is_file()
    {
        return Err("not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read file")?;
    if bytes.len() as u64 > LIMIT {
        return Err("file exceeds inspection limit".into());
    }
    Ok(Some(bytes))
}
fn component(name: &str, path: &Path, scope: &str) -> Component {
    Component {
        name: name.into(),
        registration: Registration::Unknown,
        activation: Activation::Unknown,
        isolation: Isolation::Unknown,
        conformance: Conformance::Untested,
        evidence: vec![Evidence {
            path: path.into(),
            scope: scope.into(),
            method: "bounded native configuration inspection".into(),
            digest: None,
        }],
        detail: "unverified native component".into(),
    }
}
fn absent(c: &mut Component) {
    c.registration = Registration::Missing;
    c.activation = Activation::NotApplicable;
    c.isolation = Isolation::Absent;
    c.detail = "no registration in inspected scope".into();
}
fn file_bytes(c: &mut Component) -> Option<Vec<u8>> {
    match read(&c.evidence[0].path) {
        Ok(None) => {
            absent(c);
            None
        }
        Ok(Some(bytes)) => {
            c.evidence[0].digest = Some(digest_bytes(&bytes));
            Some(bytes)
        }
        Err(reason) => {
            c.detail = reason;
            None
        }
    }
}

pub fn inspect(repo: &Path, harness: &str) -> Status {
    inspect_in(repo, harness, &Locations::detect())
}
pub fn inspect_in(repo: &Path, harness: &str, locations: &Locations) -> Status {
    let mut components = Vec::new();
    let mut gaps = vec!["Configuration evidence only; native trust, future surface activation and live delivery are unverified.".into(),
        "Arbitrary hooks, remote plugins and configuration indirection are not interpreted. Same-user concurrent changes are not isolated.".into()];
    let Some(home) = &locations.home else {
        components.push(component("home", Path::new("HOME"), "user"));
        return finish(harness, components, gaps);
    };
    let relevant: &[&str] = match harness {
        "codex" => &["CODEX_HOME"],
        "claude-code" => &["CLAUDE_CONFIG_DIR"],
        "opencode" => &[
            "XDG_CONFIG_HOME",
            "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_DIR",
            "OPENCODE_CONFIG_CONTENT",
            "OPENCODE_AUTH_CONTENT",
            "OPENCODE_TEST_MANAGED_CONFIG_DIR",
            "OPENCODE_TEST_HOME",
            "OPENCODE_DB",
            "XDG_DATA_HOME",
        ],
        "antigravity" => &["GEMINI_CLI_HOME", "AGY_CONFIG_DIR"],
        "kiro" => &[
            "KIRO_HOME",
            "KAS_BUNDLE_PATH",
            "KIRO_TEST_WORKSPACE_SETTINGS_PATH",
            "CLOUD_CONFIG_ENDPOINT",
            "KIRO_REMOTE_SESSIONS_ENDPOINT",
            "KIRO_ACP_RECORD_PATH",
            "KIRO_CHAT_LOG_FILE",
            "KIRO_DUMP_CONFIGURATION",
            "KIRO_DUMP_REQUESTS",
            "GLOBAL_ENV_FILE",
            "ASBX_KIRO_MANDATORY_MCPS",
        ],
        _ => &[],
    };
    for key in relevant {
        if locations.overrides.contains_key(*key) {
            let mut c = component(key, Path::new(key), "environment override");
            c.detail = "custom native configuration scope is not verified; inspect it explicitly or use the default scope".into();
            components.push(c);
        }
    }
    match harness {
        "codex" => {
            scan_codex_sources(repo, locations, &mut components);
            let config = home.join(".codex/config.toml");
            let activation = codex_activation(&config, &mut components);
            scan_hooks(
                &home.join(".codex/hooks.json"),
                "user",
                harness,
                activation,
                &mut components,
            );
            scan_hooks(
                &repo.join(".codex/hooks.json"),
                "project",
                harness,
                Activation::Unknown,
                &mut components,
            );
            // Project features and notify hooks may override user settings.
            codex_activation(&repo.join(".codex/config.toml"), &mut components);
        }
        "claude-code" => {
            if let Some(path) = &locations.wrapper {
                let mut c = component("wrapper", path, "application bundle");
                if let Some(bytes) = file_bytes(&mut c) {
                    if digest_bytes(&bytes) == WRAPPER_DIGEST {
                        c.registration = Registration::Installed;
                        c.conformance = Conformance::SourceInspected;
                        c.isolation = Isolation::DirectExecutableOnly;
                        c.detail = "reviewed bundled wrapper available; invocation/injection not observed; headless requires a direct harness executable".into();
                    } else {
                        c.detail = "unrecognized wrapper bytes".into();
                    }
                }
                components.push(c);
            }
            for (path, scope) in [
                (home.join(".claude/settings.json"), "user"),
                (repo.join(".claude/settings.json"), "project"),
                (repo.join(".claude/settings.local.json"), "project local"),
            ] {
                scan_hooks(&path, scope, harness, Activation::Unknown, &mut components);
            }
            if let Some(path) = &locations.managed_claude {
                scan_hooks(
                    path,
                    "managed",
                    harness,
                    Activation::Unknown,
                    &mut components,
                );
                scan_directory(
                    &path.with_file_name("managed-settings.d"),
                    "managed",
                    false,
                    &mut components,
                );
            }
            gaps.push("Claude integration is wrapper-managed. Separately configured hooks and plugins have independent behavior.".into());
        }
        "opencode" => {
            scan_opencode_sources(locations, &mut components);
            for (base, scope) in [
                (home.join(".config/opencode"), "user"),
                (home.join(".opencode"), "user legacy"),
                (repo.join(".opencode"), "project"),
            ] {
                scan_directory(&base.join("plugins"), scope, true, &mut components);
                scan_directory(&base.join("plugin"), scope, true, &mut components);
                for name in ["opencode.json", "opencode.jsonc"] {
                    scan_plugin_config(&base.join(name), scope, &mut components);
                }
            }
            for name in ["opencode.json", "opencode.jsonc"] {
                scan_plugin_config(&repo.join(name), "project", &mut components);
            }
        }
        "antigravity" => {
            scan_hooks(
                &home.join(".gemini/config/hooks.json"),
                "user",
                harness,
                Activation::Unknown,
                &mut components,
            );
            gaps.push("Antigravity custom hook formats and extension scopes are not verified; native version eligibility is checked separately.".into());
        }
        "kiro" => {
            scan_kiro(&home.join(".kiro"), "user", &mut components);
            scan_kiro(&repo.join(".kiro"), "project", &mut components);
            for name in [
                "web-session/config.json",
                "cloud-cache/user/config/state.json",
            ] {
                let mut c = component(
                    "Kiro cloud configuration",
                    &home.join(".kiro").join(name),
                    "user",
                );
                file_bytes(&mut c);
                components.push(c);
            }
            scan_kiro(
                &home.join(".kiro/cloud-cache/user/config"),
                "cloud cache",
                &mut components,
            );
            gaps.push("Kiro V3 2.28.0/2.29.0 admission inspects a bounded subset of JSON and Markdown native agents and CLI settings. Nonempty hooks, extensions, Powers, workflows, unknown fields/formats, cloud configuration and environment indirection are refused. Native permissions, skills and MCP remain active; arbitrary MCP subprocess behavior is not inspected. Agent availability is not proof of selection or loading. This is not a sandbox or proof of native helper isolation.".into());
        }
        _ => components.push(component("unsupported harness", repo, "unknown")),
    }
    // Native project discovery may walk upward. Bound that search and include
    // parent settings rather than treating a clean leaf as a clean scope.
    for (depth, parent) in repo.ancestors().skip(1).enumerate() {
        if depth >= 32 || components.len() > MAX_ENTRIES {
            let mut c = component("ancestor scope", parent, "project ancestor");
            c.detail = "ancestor inspection limit exceeded".into();
            components.push(c);
            break;
        }
        match harness {
            "kiro" => scan_kiro(&parent.join(".kiro"), "project ancestor", &mut components),
            "codex" => {
                codex_activation(&parent.join(".codex/config.toml"), &mut components);
                scan_hooks(
                    &parent.join(".codex/hooks.json"),
                    "project ancestor",
                    harness,
                    Activation::Unknown,
                    &mut components,
                );
            }
            "claude-code" => {
                for name in ["settings.json", "settings.local.json"] {
                    scan_hooks(
                        &parent.join(".claude").join(name),
                        "project ancestor",
                        harness,
                        Activation::Unknown,
                        &mut components,
                    );
                }
            }
            "opencode" => {
                for name in ["plugin", "plugins"] {
                    scan_directory(
                        &parent.join(".opencode").join(name),
                        "project ancestor",
                        true,
                        &mut components,
                    );
                }
                for name in ["opencode.json", "opencode.jsonc"] {
                    scan_plugin_config(&parent.join(name), "project ancestor", &mut components);
                    scan_plugin_config(
                        &parent.join(".opencode").join(name),
                        "project ancestor",
                        &mut components,
                    );
                }
            }
            _ => {}
        }
    }
    for file in [".mcp.json", ".agents/settings.json"] {
        let path = repo.join(file);
        let mut c = component("additional integration scope", &path, "project");
        if let Some(bytes) = file_bytes(&mut c) {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(value) if !json_mentions_cmux(&value) => {
                    absent(&mut c);
                    c.detail = "no cmux reference observed; arbitrary subprocess behavior is outside inspection".into();
                },
                Ok(_) => c.detail = "cmux reference in an unsupported integration scope; not proof of registration or isolation".into(),
                Err(_) => c.detail = "malformed integration configuration".into(),
            }
        }
        components.push(c);
    }
    finish(harness, components, gaps)
}

// Reviewed against the installed 2.28.0 and 2.29.0 V3 KAS profile loaders.
// Only the explicitly inspected declarative subset is admitted.
// No arbitrary executable source is certified by its filename or contents.
fn scan_kiro(base: &Path, scope: &str, out: &mut Vec<Component>) {
    let mut agent_entries = 0;
    scan_kiro_agents(&base.join("agents"), scope, out, &mut agent_entries, 0);
    for name in ["hooks", "extensions", "powers", "workflows"] {
        scan_directory(&base.join(name), scope, false, out);
    }
    let mut c = component("Kiro CLI settings", &base.join("settings/cli.json"), scope);
    if let Some(bytes) = file_bytes(&mut c) {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) if kiro_cli_settings(&value) => {
                absent(&mut c);
                c.conformance = Conformance::SourceInspected;
                c.detail = "only inspected native CLI preferences; effective selection and approval remain native".into();
            }
            _ => c.detail = "Kiro CLI settings contain malformed values or unreviewed keys".into(),
        }
    }
    out.push(c);
    let mut c = component("Kiro MCP settings", &base.join("settings/mcp.json"), scope);
    if let Some(bytes) = file_bytes(&mut c) {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) if value.is_object() && !json_mentions_cmux(&value) => {
                absent(&mut c);
                c.detail = "no cmux reference observed; arbitrary MCP subprocess behavior is outside inspection".into();
            }
            _ => c.detail = "Kiro MCP settings contain unresolved integration configuration".into(),
        }
    }
    out.push(c);
}

fn kiro_inspected_version(version: &str) -> bool {
    matches!(
        crate::catalog::version_token(version),
        Some("2.28.0" | "2.29.0")
    )
}

fn kiro_cli_settings(value: &Value) -> bool {
    value.as_object().is_some_and(|settings| {
        settings.iter().all(|(key, value)| match key.as_str() {
            "chat.defaultAgent" | "chat.defaultModel" => value.is_string(),
            "chat.greeting.enabled"
            | "chat.disableWrap"
            | "chat.preserveScrollback"
            | "chat.startFullscreen" => value.is_boolean(),
            _ => false,
        })
    })
}

fn string_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(Value::is_string))
}

fn kiro_permissions(value: &Value) -> bool {
    value.as_object().is_some_and(|fields| {
        fields.contains_key("rules")
            && fields.iter().all(|(key, value)| match key.as_str() {
                "rules" => value.as_array().is_some_and(|rules| {
                    rules.iter().all(|rule| {
                        rule.as_object().is_some_and(|fields| {
                            fields.get("capability").is_some_and(Value::is_string)
                                && fields
                                    .get("effect")
                                    .and_then(Value::as_str)
                                    .is_some_and(|s| matches!(s, "allow" | "deny" | "ask"))
                                && fields.iter().all(|(key, value)| match key.as_str() {
                                    "capability" | "effect" => value.is_string(),
                                    "match" | "exclude" => string_array(value),
                                    _ => false,
                                })
                        })
                    })
                }),
                "policies" => string_array(value),
                _ => false,
            })
    })
}

fn kiro_mcp_servers(value: &Value) -> bool {
    !json_mentions_cmux(value)
        && value.as_object().is_some_and(|servers| {
            servers.values().all(|server| {
                server.as_object().is_some_and(|fields| {
                    fields.iter().all(|(key, value)| match key.as_str() {
                        "command" | "cwd" | "url" | "type" | "versionNegotiation" => {
                            value.is_string()
                        }
                        "args" | "disabledTools" | "autoApprove" | "oauthScopes" => {
                            string_array(value)
                        }
                        "disabled" | "waitForReady" => value.is_boolean(),
                        "timeout" => value.is_number(),
                        "env" | "headers" => value
                            .as_object()
                            .is_some_and(|fields| fields.values().all(Value::is_string)),
                        "oauth" => value.as_object().is_some_and(|fields| {
                            fields.iter().all(|(key, value)| {
                                matches!(
                                    key.as_str(),
                                    "clientId" | "redirectUri" | "clientMetadataUrl"
                                ) && value.is_string()
                            })
                        }),
                        _ => false,
                    })
                })
            })
        })
}

// Deliberately narrower than the native schema: no executable hooks, knowledge
// indexing, Powers activation, or unknown control fields. Permissions and MCP
// are native policy, not a claim that subprocesses are isolated.
fn kiro_agent_fields(value: &Value, markdown: bool) -> bool {
    value.as_object().is_some_and(|fields| {
        !fields.is_empty()
            && fields.iter().all(|(key, value)| match key.as_str() {
                "name" => value.as_str().is_some_and(|s| !s.is_empty()),
                "description" | "model" | "welcomeMessage" => {
                    value.is_string() || (!markdown && value.is_null())
                }
                "prompt" => !markdown && (value.is_string() || value.is_null()),
                "effortLevel" => value.is_string(),
                "tools" => {
                    string_array(value) || value.as_str().is_some_and(|s| markdown || s == "*")
                }
                "excludedTools" => string_array(value),
                "includeMcpJson" => value.is_boolean(),
                "includePowers" => value == &Value::Bool(false),
                "dispatchKind" => value
                    .as_str()
                    .is_some_and(|s| matches!(s, "sub-agent" | "custom-agent" | "spec")),
                "resources" => value.as_array().is_some_and(|items| {
                    items.iter().all(|item| {
                        item.as_str()
                            .is_some_and(|s| s.starts_with("file://") || s.starts_with("skill://"))
                    })
                }),
                "permissions" => kiro_permissions(value),
                "mcpServers" => kiro_mcp_servers(value),
                "hooks" => {
                    value.as_array().is_some_and(Vec::is_empty)
                        || value.as_object().is_some_and(serde_json::Map::is_empty)
                }
                // V3 skips legacy JSON profiles without a permissions block. With
                // one, these legacy fields are ignored by the inspected V3 loader.
                "allowedTools" => fields.contains_key("permissions") && string_array(value),
                "toolsSettings" => {
                    fields.contains_key("permissions")
                        && value.as_object().is_some_and(serde_json::Map::is_empty)
                }
                _ => false,
            })
    })
}

fn kiro_agent_document(bytes: &[u8], markdown: bool) -> Option<Value> {
    if !markdown {
        // Native JSONC extensions remain unknown instead of using a permissive
        // parser whose interpretation could differ from Kiro's loader.
        return serde_json::from_slice(bytes).ok();
    }
    let text = std::str::from_utf8(bytes).ok()?.replace("\r\n", "\n");
    let rest = text.strip_prefix("---\n")?;
    let end = rest
        .find("\n---\n")
        .or_else(|| rest.strip_suffix("\n---").map(str::len))?;
    yaml_serde::from_str(&rest[..end]).ok()
}

fn scan_kiro_agents(
    path: &Path,
    scope: &str,
    out: &mut Vec<Component>,
    count: &mut usize,
    depth: usize,
) {
    if depth > 16 || *count > MAX_ENTRIES {
        let mut c = component("Kiro native agents", path, scope);
        c.detail = "native agent inspection limit exceeded".into();
        out.push(c);
        return;
    }
    // Native discovery is recursive. Every entry consumes the same budget;
    // symlinked directories and files fail closed through check_path/read.
    for path in source_directory(path, "Kiro native agents", out) {
        *count += 1;
        let mut c = component("Kiro native agent", &path, scope);
        if *count > MAX_ENTRIES {
            c.detail = "native agent inspection limit exceeded".into();
            out.push(c);
            break;
        }
        if let Err(reason) = check_path(&path) {
            c.detail = reason;
            out.push(c);
            continue;
        }
        if path.is_dir() {
            scan_kiro_agents(&path, scope, out, count, depth + 1);
            continue;
        }
        if let Some(bytes) = file_bytes(&mut c) {
            let extension = path.extension().and_then(|s| s.to_str());
            let markdown = extension == Some("md");
            if matches!(extension, Some("json" | "md"))
                && kiro_agent_document(&bytes, markdown)
                    .is_some_and(|value| kiro_agent_fields(&value, markdown))
            {
                c.registration = Registration::Installed;
                c.isolation = Isolation::Absent;
                c.conformance = Conformance::SourceInspected;
                c.detail = "inspected native agent fields contain no executable hooks or unreviewed controls; availability does not establish selection, context loading or subprocess isolation".into();
            } else {
                c.detail = "native agent contains nonempty hooks, unreviewed fields/format, or malformed values".into();
            }
        }
        out.push(c);
    }
}

fn json_mentions_cmux(value: &Value) -> bool {
    match value {
        Value::String(text) => text.to_ascii_lowercase().contains("cmux"),
        Value::Array(items) => items.iter().any(json_mentions_cmux),
        Value::Object(items) => items.iter().any(|(key, value)| {
            key.to_ascii_lowercase().contains("cmux") || json_mentions_cmux(value)
        }),
        _ => false,
    }
}

fn finish(harness: &str, mut components: Vec<Component>, mut gaps: Vec<String>) -> Status {
    // A native user path can also be reached by project ancestor discovery.
    // Retain one observation, without collapsing duplicate hook registrations
    // (their numbered names differ).
    let mut seen = std::collections::BTreeSet::new();
    components.retain(|c| seen.insert((c.name.clone(), c.evidence[0].path.clone())));
    if harness == "codex" {
        let missing: Vec<_> = CODEX_COMMANDS
            .iter()
            .filter(|(event, _)| {
                !components.iter().any(|c| {
                    c.registration == Registration::Installed
                        && c.name.starts_with(&format!("{event} registration "))
                })
            })
            .map(|(event, _)| *event)
            .collect();
        if !missing.is_empty() {
            gaps.push(format!(
                "Native event coverage is missing or unverified: {}",
                missing.join(", ")
            ));
        }
    }
    if harness == "opencode" {
        for (label, digest) in [("Session", SESSION_DIGEST), ("Feed", FEED_DIGEST)] {
            if !components.iter().any(|c| {
                c.registration == Registration::Installed
                    && c.evidence
                        .iter()
                        .any(|e| e.digest.as_deref() == Some(digest))
            }) {
                gaps.push(format!(
                    "OpenCode {label} plugin registration is missing or unverified."
                ));
            }
        }
    }
    let reasons: Vec<String> = components
        .iter()
        .filter(|c| matches!(c.isolation, Isolation::Unknown | Isolation::Unsafe))
        .map(|c| {
            format!(
                "{} ({}): {}",
                display_safe(&c.name),
                display_safe(&c.evidence[0].path.to_string_lossy()),
                display_safe(&c.detail)
            )
        })
        .collect();
    let guarded = components
        .iter()
        .any(|c| c.isolation == Isolation::VerifiedDisable);
    let headless = HeadlessPolicy {
        allowed: reasons.is_empty(),
        disable_variable: guarded
            .then(|| disable_variable(harness).map(str::to_string))
            .flatten(),
        reasons,
        evidence_digest: digest_bytes(
            &serde_json::to_vec(&components).expect("component serialization"),
        ),
    };
    Status { profile: crate::catalog::isolation_profile(harness), cli_version: None, version_checked: false, harness: harness.into(), native_agent: native_agent(harness).map(str::to_string),
        components, gaps, conformance: "locally_inspected; live_untested".into(), headless,
        next_action: match harness {
            "claude-code" => "Use interactive execution while independent hooks/plugins or managed settings are unresolved. Review those sources with their native owner, then inspect again. cmux Settings > Automation controls only the Claude wrapper; no native Claude installer is exposed.".into(),
            "opencode" => "Use interactive execution for unresolved native configuration/authentication sources; credentials and account databases are not read. Native --pure disables external plugins but does not resolve authenticated remote configuration. Changing native homes or hiding account stores can change provider/model resolution and is not an isolation repair. For unguarded Feed, explicit native removal is `cmux hooks opencode uninstall`, then inspect again. Reinstalling the same Feed does not provide isolation.".into(),
            _ => "Inspect unknown components and native scope; use interactive execution until isolation is verified. Installation is explicit: ahu cmux install --harness ID --dry-run.".into(),
        } }
}

/// Presence-only inspection for credentials, opaque native stores and legacy
/// formats. Never open these files, hash their contents, or invoke their loader.
fn opaque_source(path: &Path, name: &str, scope: &str, reason: &str, out: &mut Vec<Component>) {
    let mut c = component(name, path, scope);
    c.evidence[0].method = "metadata only; contents not read".into();
    match check_path(path).and_then(|()| match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("cannot inspect native source metadata".into()),
    }) {
        Ok(false) => absent(&mut c),
        Ok(true) => c.detail = reason.into(),
        Err(reason) => c.detail = reason,
    }
    out.push(c);
}

fn source_directory(path: &Path, name: &str, out: &mut Vec<Component>) -> Vec<PathBuf> {
    let mut c = component(name, path, "native source discovery");
    c.evidence[0].method = "bounded directory names only; file contents not read".into();
    if let Err(reason) = check_path(path) {
        c.detail = reason;
        out.push(c);
        return vec![];
    }
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            absent(&mut c);
            out.push(c);
            return vec![];
        }
        Err(_) => {
            c.detail = "cannot enumerate native source scope".into();
            out.push(c);
            return vec![];
        }
    };
    let mut paths = Vec::new();
    for entry in entries.take(MAX_ENTRIES + 1) {
        let Ok(entry) = entry else {
            c.detail = "cannot inspect native source entry".into();
            out.push(c);
            return vec![];
        };
        paths.push(entry.path());
    }
    if paths.len() > MAX_ENTRIES {
        c.detail = "native source enumeration limit exceeded".into();
        out.push(c);
        return vec![];
    }
    paths.sort();
    paths
}

// OpenCode v1.18.29-v1.18.32 config/config.ts, config/managed.ts,
// auth/index.ts and account/repo.ts. Native authentication can cause remote
// config to add plugins; authentication values and account databases stay native.
fn scan_opencode_sources(locations: &Locations, out: &mut Vec<Component>) {
    let home = locations.home.as_ref().expect("home checked by caller");
    let config = home.join(".config/opencode");
    scan_plugin_config(&config.join("config.json"), "user defaults", out);
    opaque_source(
        &config.join("config"),
        "legacy config",
        "user",
        "legacy native config may declare plugins; its migration/loader is not executed by inspection",
        out,
    );
    let data = home.join(".local/share/opencode");
    opaque_source(
        &data.join("auth.json"),
        "authenticated remote config",
        "user native authentication",
        "authentication store present; remote plugin configuration is unresolved. Credentials are not read and no remote context is fetched; use interactive execution",
        out,
    );
    for path in source_directory(&data, "native account store discovery", out) {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with("opencode")
            && [".db", ".db-wal", ".db-shm"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
        {
            opaque_source(
                &path,
                "authenticated account config",
                "native account database",
                "account database may select authenticated remote configuration; database and tokens are not read, so headless isolation is unresolved",
                out,
            );
        }
    }
    if let Some(managed) = &locations.managed_opencode {
        for name in ["opencode.json", "opencode.jsonc"] {
            scan_plugin_config(&managed.join(name), "system managed", out);
        }
    }
    if let Some(preferences) = &locations.managed_preferences {
        let domain = "ai.opencode.managed.plist";
        opaque_source(
            &preferences.join(domain),
            "managed preferences",
            "system managed",
            "native managed preferences may declare executable plugins; plist decoding is unsupported",
            out,
        );
        for user in source_directory(preferences, "managed preference domains", out) {
            // The native loader selects a username via the OS, not HOME/USER.
            // Check all bounded candidate domains conservatively, without
            // reading any user's managed preference contents.
            match std::fs::symlink_metadata(&user) {
                Ok(meta) if meta.is_dir() || meta.file_type().is_symlink() => opaque_source(
                    &user.join(domain),
                    "managed user preferences",
                    "system managed",
                    "potential user managed preference source is unresolved; contents are not read",
                    out,
                ),
                Ok(_) => {}
                Err(_) => opaque_source(
                    &user,
                    "managed preference entry",
                    "system managed",
                    "managed source metadata unavailable",
                    out,
                ),
            }
        }
    }
}

// Codex rust-v0.154.0 and rust-v0.155.1 hooks/engine/discovery.rs and
// config/loader: native sources include TOML, plugins, managed and cloud layers.
fn scan_codex_sources(repo: &Path, locations: &Locations, out: &mut Vec<Component>) {
    let home = locations.home.as_ref().expect("home checked by caller");
    let codex = home.join(".codex");
    for (file, name, reason) in [
        (
            "plugins",
            "plugin hook sources",
            "native plugin bundles and remote-installed plugin state are unresolved; plugin code is not loaded by inspection",
        ),
        (
            "auth.json",
            "cloud authentication",
            "native authentication may load remote managed hooks/plugins; credentials are not read and remote config is not fetched",
        ),
        (
            "cloud-config-bundle-cache.json",
            "cloud configuration",
            "native cloud configuration cache may contain managed hooks; cached remote context is not read",
        ),
    ] {
        opaque_source(&codex.join(file), name, "user native state", reason, out);
    }
    codex_activation(&repo.join("config.toml"), out);
    scan_hooks(
        &repo.join("hooks.json"),
        "cwd",
        "codex",
        Activation::Unknown,
        out,
    );
    if let Some(system) = &locations.system_codex {
        codex_activation(&system.join("config.toml"), out);
        scan_hooks(
            &system.join("hooks.json"),
            "system",
            "codex",
            Activation::Unknown,
            out,
        );
        for file in ["requirements.toml", "managed_config.toml"] {
            opaque_source(
                &system.join(file),
                "managed hook sources",
                "system",
                "managed native configuration is present; inline hooks and managed directories are unresolved",
                out,
            );
        }
    }
    if locations.codex_managed_preferences != ManagedPreferences::NotApplicable {
        let mut c = component(
            "native managed preferences",
            Path::new("com.openai.codex"),
            "macOS CFPreferences",
        );
        c.evidence[0].method = "native CFPreferences key presence only; values not decoded".into();
        if locations.codex_managed_preferences == ManagedPreferences::Absent {
            absent(&mut c);
        } else {
            c.detail = "Codex managed preference values are present or the native query is unresolved; their executable hooks have not been verified. Use interactive execution".into();
        }
        out.push(c);
    }
}

fn codex_activation(path: &Path, components: &mut Vec<Component>) -> Activation {
    let mut c = component("configuration", path, "native config");
    if let Some(bytes) = file_bytes(&mut c) {
        match std::str::from_utf8(&bytes)
            .ok()
            .and_then(|s| toml::from_str::<toml::Value>(s).ok())
        {
            Some(value) => {
                let configured_disabled = value
                    .get("features")
                    .and_then(|v| v.get("hooks"))
                    .and_then(toml::Value::as_bool)
                    == Some(false);
                // Enabling hooks is not proof of effective native trust.
                if codex_indirection(&value) {
                    c.detail =
                        "additional command or configuration indirection is unresolved".into();
                } else {
                    absent(&mut c);
                    c.detail = if configured_disabled {
                        "This scope requests hooks disabled; other layers and effective activation remain unverified."
                    } else {
                        "TOML parsed; no additional hook/notify indirection; effective native trust is unverified"
                    }.into();
                }
            }
            None => c.detail = "malformed TOML".into(),
        }
    }
    components.push(c);
    Activation::Unknown
}

fn codex_indirection(value: &toml::Value) -> bool {
    let Some(table) = value.as_table() else {
        return true;
    };
    for (key, entry) in table {
        // An explicit empty argv defines no notification command. Keep every
        // nonempty or malformed value unresolved, including in native profiles.
        if (key == "notify" && entry.as_array().is_none_or(|args| !args.is_empty()))
            || ["include", "imports", "profile"].contains(&key.as_str())
            || (["plugins", "marketplaces"].contains(&key.as_str())
                && entry.as_table().is_none_or(|t| !t.is_empty()))
            || (key == "cli_auth_credentials_store" && entry.as_str() != Some("file"))
        {
            return true;
        }
        if key == "hooks" && entry.is_table() {
            // Native trust hashes are data, not hook command definitions.
            let Some(state) = entry.get("state").and_then(toml::Value::as_table) else {
                return true;
            };
            if entry.as_table().is_none_or(|t| t.len() != 1)
                || state.values().any(|record| {
                    record.as_table().is_none_or(|r| {
                        r.len() != 1
                            || r.get("trusted_hash")
                                .and_then(toml::Value::as_str)
                                .is_none()
                    })
                })
            {
                return true;
            }
        } else if (entry.is_table() && codex_indirection(entry))
            || key.to_ascii_lowercase().contains("cmux")
            || entry.to_string().to_ascii_lowercase().contains("cmux")
        {
            return true;
        }
    }
    false
}

fn scan_hooks(
    path: &Path,
    scope: &str,
    harness: &str,
    activation: Activation,
    out: &mut Vec<Component>,
) {
    let mut c = component("hook registration", path, scope);
    let Some(bytes) = file_bytes(&mut c) else {
        out.push(c);
        return;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        c.detail = "malformed JSON".into();
        out.push(c);
        return;
    };
    if !value.is_object() {
        c.detail = "expected a native settings object".into();
        out.push(c);
        return;
    }
    if value
        .get("enabledPlugins")
        .is_some_and(|v| v.as_object().is_none_or(|o| o.values().any(|v| v != false)))
    {
        let mut plugin = c.clone();
        plugin.name = "native plugins".into();
        plugin.detail = "plugin hook behavior is unresolved".into();
        out.push(plugin);
    }
    let Some(hooks) = value.get("hooks") else {
        absent(&mut c);
        out.push(c);
        return;
    };
    let Some(events) = hooks.as_object() else {
        c.detail = "unsupported hook structure".into();
        out.push(c);
        return;
    };
    if events.is_empty() {
        absent(&mut c);
        out.push(c);
        return;
    }
    let mut count = 0;
    for (event, groups) in events {
        let Some(groups) = groups.as_array() else {
            c.detail = "unsupported hook event structure".into();
            out.push(c);
            return;
        };
        for group in groups {
            let Some(entries) = group.get("hooks").and_then(Value::as_array) else {
                c.detail = "unsupported hook group".into();
                out.push(c);
                return;
            };
            for entry in entries {
                count += 1;
                if count > MAX_ENTRIES {
                    c.detail = "hook inspection entry limit exceeded".into();
                    out.push(c);
                    return;
                }
                let mut item = c.clone();
                item.name = format!("{event} registration {count}");
                item.activation = activation;
                item.detail =
                    "unrecognized native hook command; isolation cannot be established".into();
                if harness == "codex"
                    && entry.get("type").and_then(Value::as_str) == Some("command")
                    && let Some(command) = entry.get("command").and_then(Value::as_str)
                {
                    verify_codex(command, event, &mut item);
                }
                out.push(item);
            }
        }
    }
    if count == 0 {
        absent(&mut c);
        out.push(c);
    }
}
// A deliberately small POSIX literal grammar, not a shell parser. Quoting must
// enclose the whole path; unquoted words allow no expansion or token syntax.
fn literal_artifact_path(command: &str) -> Option<&Path> {
    let path = if let Some(quoted) = command.strip_prefix('\'') {
        let path = quoted.strip_suffix('\'')?;
        if path.contains('\'') || path.chars().any(char::is_control) {
            return None;
        }
        path
    } else {
        if !command
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-.:+".contains(&b))
        {
            return None;
        }
        command
    };
    path.starts_with('/').then(|| Path::new(path))
}

fn verify_codex(command: &str, event: &str, item: &mut Component) {
    let Some((_, expected)) = CODEX_COMMANDS.iter().find(|(name, _)| *name == event) else {
        return;
    };
    let mut digest = digest_bytes(command.as_bytes());
    if digest != *expected {
        let Some(path) = literal_artifact_path(command) else {
            return;
        };
        let Ok(Some(bytes)) = read(path) else {
            item.detail = "referenced hook artifact missing, unreadable or unresolved".into();
            return;
        };
        use std::os::unix::fs::PermissionsExt;
        if !std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0) {
            item.detail = "referenced hook artifact is not executable".into();
            return;
        }
        item.evidence.push(Evidence {
            path: path.into(),
            scope: "referenced native artifact".into(),
            method: "exact reviewed SHA-256".into(),
            digest: Some(digest_bytes(&bytes)),
        });
        let Some(body) = bytes
            .strip_prefix(b"#!/bin/sh\n")
            .and_then(|b| b.strip_suffix(b"\n"))
        else {
            return;
        };
        digest = digest_bytes(body);
    }
    if digest == *expected {
        item.registration = Registration::Installed;
        item.conformance = Conformance::SourceInspected;
        item.isolation = Isolation::VerifiedDisable;
        item.detail = "reviewed native command; disable variable and absent surface guard prevent dispatch; live activation unverified".into();
    }
}
fn scan_directory(path: &Path, scope: &str, plugins: bool, out: &mut Vec<Component>) {
    let mut c = component(
        if plugins {
            "plugin directory"
        } else {
            "managed fragments"
        },
        path,
        scope,
    );
    if let Err(reason) = check_path(path) {
        c.detail = reason;
        out.push(c);
        return;
    }
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            absent(&mut c);
            out.push(c);
            return;
        }
        Err(_) => {
            c.detail = "cannot inspect directory".into();
            out.push(c);
            return;
        }
    };
    let mut paths = Vec::new();
    for entry in entries.take(MAX_ENTRIES + 1) {
        let Ok(entry) = entry else {
            c.detail = "cannot inspect directory entry".into();
            out.push(c);
            return;
        };
        paths.push(entry.path());
    }
    if paths.len() > MAX_ENTRIES {
        c.detail = "directory inspection limit exceeded".into();
        out.push(c);
        return;
    }
    paths.sort();
    if paths.is_empty() {
        absent(&mut c);
        out.push(c);
        return;
    }
    for path in paths {
        let mut item = component("native plugin", &path, scope);
        if let Some(bytes) = file_bytes(&mut item) {
            let plugins = plugins && path.extension().is_some_and(|e| e == "js" || e == "ts");
            match digest_bytes(&bytes).as_str() {
                SESSION_DIGEST if plugins => {
                    item.registration = Registration::Installed; item.isolation = Isolation::VerifiedDisable;
                    item.detail = "reviewed OpenCode session plugin with disable and surface guards; activation unverified".into();
                },
                FEED_DIGEST if plugins => {
                    item.registration = Registration::Installed; item.isolation = Isolation::Unsafe;
                    item.detail = "OpenCode Feed has no disable/surface guard and can connect to a fallback socket".into();
                },
                _ => item.detail = "unrecognized native code; filename does not establish integration or isolation".into(),
            }
        }
        out.push(item);
    }
}
fn scan_plugin_config(path: &Path, scope: &str, out: &mut Vec<Component>) {
    let mut c = component("declared plugins", path, scope);
    if let Some(bytes) = file_bytes(&mut c) {
        // Native substitution runs before JSON parsing. Do not read expansion
        // targets or inspect environment values: even valid literal JSON may
        // expand into a different executable configuration.
        let text = String::from_utf8_lossy(&bytes);
        if text.contains("{env:") || text.contains("{file:") {
            c.detail = "native environment/file substitution is unresolved; expansion targets are not read".into();
            out.push(c);
            return;
        }
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value)
                if value.is_object()
                    && value
                        .get("plugin")
                        .is_none_or(|p| p.as_array().is_some_and(Vec::is_empty))
                    && value.get("plugins").is_none() =>
            {
                absent(&mut c);
            }
            Ok(_) => c.detail =
                "declared plugin or unsupported native configuration; module behavior unresolved"
                    .into(),
            Err(_) => {
                c.detail = "malformed JSON or unsupported JSONC; not evidence of absence".into()
            }
        }
    }
    out.push(c);
}

/// Apply only controls that batch_command also freezes and emits. Inspection
/// without this explicit headless profile retains its interactive evidence.
pub fn headless_status(mut status: Status, version: &str) -> Status {
    if status.harness == "claude-code"
        && let Some(profile) = crate::harness::isolation::profile(&status.harness, version)
    {
        for component in &mut status.components {
            let evidence = &component.evidence[0];
            if status.harness == "claude-code"
                && matches!(
                    evidence.scope.as_str(),
                    "user" | "project" | "project local" | "project ancestor"
                )
                && evidence.digest.is_some()
                && component.name.contains(" registration ")
                && component.isolation == Isolation::Unknown
            {
                component.isolation = Isolation::VerifiedDisable;
                component.activation = Activation::Disabled;
                component.detail = format!(
                    "disabled by invocation profile {}; managed policy is retained",
                    profile.id
                );
            }
        }
        status = finish(&status.harness, status.components, status.gaps);
        status.gaps.push(format!("Headless invocation profile: {}. Managed hooks and unresolved plugins remain admission blockers.", profile.id));
    }
    status.with_version(Some(version))
}

/// Production headless execution supplies its already verified executable.
/// An on-disk metadata report is never accepted as an admission credential.
pub fn enforce_headless_executable(
    repo: &Path,
    harness: &str,
    version: &str,
    executable: &Path,
) -> Result<Status> {
    enforce_headless_executable_with_opencode_state(repo, harness, version, executable, false)
}

/// Inspect headless native configuration, optionally honoring the project's
/// explicit trust decision for OpenCode's opaque account/authentication state.
/// The accepted state remains visible in `gaps`; no credential or database
/// content is read.
pub fn enforce_headless_executable_with_opencode_state(
    repo: &Path,
    harness: &str,
    version: &str,
    executable: &Path,
    trust_opencode_native_state: bool,
) -> Result<Status> {
    if harness != "codex" || crate::harness::isolation::profile(harness, version).is_none() {
        let mut status = headless_status(inspect(repo, harness), version);
        if harness == "opencode"
            && trust_opencode_native_state
            && trust_only_opencode_account_state(
                &mut status,
                &std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default(),
            )
        {
            return Ok(status);
        }
        if !status.headless.allowed {
            return Err(Error::new(format!(
                "headless native integration admission is unverified for {harness}:\n{}\n{}",
                status.headless.reasons.join("\n"),
                status.next_action
            )));
        }
        return Ok(status);
    }
    let status = inspect_codex_effective(repo, executable, version)?.with_version(Some(version));
    if !status.headless.allowed {
        return Err(Error::new(format!(
            "headless native integration admission is unverified for codex:\n{}",
            status.headless.reasons.join("\n")
        )));
    }
    Ok(status)
}

fn trust_only_opencode_account_state(status: &mut Status, home: &Path) -> bool {
    let unresolved = status
        .components
        .iter()
        .filter(|component| matches!(component.isolation, Isolation::Unknown | Isolation::Unsafe))
        .collect::<Vec<_>>();
    if unresolved.is_empty()
        || unresolved.iter().any(|component| {
            !matches!(
                component.name.as_str(),
                "authenticated account config" | "authenticated remote config"
            ) || component.evidence.len() != 1
                || !matches!(
                    component.evidence[0].scope.as_str(),
                    "native account database" | "user native authentication"
                )
                || component.evidence[0].method != "metadata only; contents not read"
                || !opencode_auth_path(home, &component.evidence[0].path)
        })
    {
        return false;
    }
    status.headless.allowed = true;
    status.headless.reasons.clear();
    status.gaps.push(
        "Project policy explicitly trusts OpenCode native account/authentication state for headless execution. It may select authenticated remote configuration and executable plugins; ahu does not inspect, disable, or constrain that state. Credentials and database contents remain unread.".into(),
    );
    status.next_action =
        "OpenCode native account/authentication state is explicitly trusted by project policy; review it with OpenCode's native controls.".into();
    true
}

fn opencode_auth_path(home: &Path, path: &Path) -> bool {
    let root = home.join(".local/share/opencode");
    if !path.starts_with(&root) {
        return false;
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let native_name = name == "auth.json"
        || (name.starts_with("opencode")
            && [".db", ".db-wal", ".db-shm"]
                .iter()
                .any(|suffix| name.ends_with(suffix)));
    native_name
        && std::fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

/// Compatibility probes use the same version-specific invocation controls as
/// launch. A parseable version alone never enables an unreviewed profile.
pub fn inspect_codex_effective(repo: &Path, executable: &Path, version: &str) -> Result<Status> {
    let status = inspect(repo, "codex");
    let metadata = crate::harness::codex_metadata::inspect(executable, repo, version)?;
    codex_effective_status(status, repo, &metadata, version)
}

pub fn codex_effective_status(
    status: Status,
    repo: &Path,
    metadata: &Value,
    version: &str,
) -> Result<Status> {
    let profile = crate::harness::isolation::profile("codex", version).ok_or_else(|| {
        Error::new("no reviewed Codex metadata isolation profile for this version")
    })?;
    if status.harness != "codex" {
        return Err(Error::new(
            "native Codex metadata cannot authorize another harness",
        ));
    }
    let data = metadata
        .get("data")
        .and_then(Value::as_array)
        .filter(|data| data.len() == 1)
        .ok_or_else(|| Error::new("native Codex hook inventory has an invalid scope"))?;
    let entry = &data[0];
    if entry.get("cwd").and_then(Value::as_str) != repo.to_str()
        || !["warnings", "errors"].iter().all(|key| {
            entry
                .get(key)
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        })
    {
        return Err(Error::new(
            "native Codex hook inventory has unresolved scope, warnings or errors",
        ));
    }
    let hooks = entry
        .get("hooks")
        .and_then(Value::as_array)
        .filter(|hooks| hooks.len() <= MAX_ENTRIES)
        .ok_or_else(|| Error::new("native Codex hook inventory is missing or exceeds the bound"))?;
    let mut components = status.components;
    for (index, hook) in hooks.iter().enumerate() {
        let event = match hook.get("eventName").and_then(Value::as_str) {
            Some("preToolUse") => "PreToolUse",
            Some("postToolUse") => "PostToolUse",
            Some("permissionRequest") => "PermissionRequest",
            Some("preCompact") => "PreCompact",
            Some("postCompact") => "PostCompact",
            Some("sessionStart") => "SessionStart",
            Some("userPromptSubmit") => "UserPromptSubmit",
            Some("subagentStart") => "SubagentStart",
            Some("subagentStop") => "SubagentStop",
            Some("stop") => "Stop",
            _ => return Err(Error::new("native Codex hook event is not reviewed")),
        };
        // Disabled or untrusted unknown hooks are not treated as safe: native
        // trust/activation can change independently of their command identity.
        if hook.get("handlerType").and_then(Value::as_str) != Some("command")
            || hook.get("isManaged") != Some(&Value::Bool(false))
            || hook.get("enabled").and_then(Value::as_bool).is_none()
            || !matches!(
                hook.get("source").and_then(Value::as_str),
                Some("user" | "project" | "sessionFlags")
            )
        {
            return Err(Error::new(
                "native Codex hook has an unresolved handler or managed source",
            ));
        }
        let command = hook
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("native Codex hook command is missing"))?;
        let mut item = component(
            &format!("effective {event} registration {index}"),
            repo,
            "native effective metadata",
        );
        item.evidence[0].method =
            "bounded hooks/list command SHA-256; no raw native response retained".into();
        item.evidence[0].digest = Some(digest_bytes(command.as_bytes()));
        verify_codex(command, event, &mut item);
        if item.isolation != Isolation::VerifiedDisable {
            return Err(Error::new(
                "native Codex hook command or artifact is outside the reviewed guarded allowlist",
            ));
        }
        components.push(item);
    }
    for item in &mut components {
        if [
            "plugin hook sources",
            "cloud authentication",
            "cloud configuration",
        ]
        .contains(&item.name.as_str())
            && item.isolation == Isolation::Unknown
            && item.evidence[0].scope == "user native state"
            && item.detail.starts_with("native ")
        {
            item.isolation = Isolation::VerifiedDisable;
            item.detail = "executable consequences covered by the exact Codex invocation profile and fresh native hook metadata; credentials remain native and unread".into();
        }
    }
    let mut invocation = component(
        "native invocation profile",
        repo,
        "native effective metadata",
    );
    invocation.isolation = Isolation::VerifiedDisable;
    invocation.evidence[0].method =
        "exact invocation controls and explicit absent mandatory requirements".into();
    invocation.evidence[0].digest = Some(digest_bytes(&serde_json::to_vec(&(
        profile.id,
        profile.args,
        "requirements:null",
    ))?));
    invocation.detail = format!(
        "{}; fresh native hooks/list and configRequirements/read checked",
        profile.id
    );
    components.push(invocation);
    let mut gaps = status.gaps;
    gaps.push("Codex effective metadata was inspected before this launch with hooks enabled, optional plugins disabled and notify empty. Mandatory requirements must be absent; native strict configuration still rejects newly conflicting requirements. Remote changes between inspection and execution and same-user concurrent changes are not isolated.".into());
    let status = finish("codex", components, gaps);
    if !status.headless.allowed {
        return Err(Error::new(format!(
            "headless native integration admission is unverified for codex:\n{}",
            status.headless.reasons.join("\n")
        )));
    }
    Ok(status)
}

pub fn enforce_headless(repo: &Path, harness: &str, version: &str) -> Result<Status> {
    if harness == "kiro" && !kiro_inspected_version(version) {
        return Err(Error::new(
            "Kiro headless configuration inspection is inspected only on 2.28.0 and 2.29.0 V3; no fallback selected",
        ));
    }
    let status = headless_status(inspect(repo, harness), version);
    if !status.headless.allowed {
        return Err(Error::new(format!(
            "headless native integration admission is unverified for {harness}:\n{}\n{}",
            status.headless.reasons.join("\n"),
            status.next_action
        )));
    }
    Ok(status)
}

pub fn enforce(repo: &Path, harness: &str) -> Result<Status> {
    let status = inspect(repo, harness);
    if !status.headless.allowed {
        return Err(Error::new(format!(
            "headless native integration admission is unverified for {harness}:\n{}\n{}",
            status.headless.reasons.join("\n"),
            status.next_action
        )));
    }
    Ok(status)
}
/// Called only after admission; removes routing and restores reviewed process
/// disable controls. Does not alter homes or native settings.
pub fn sanitize(command: &mut Command, policy: Option<&HeadlessPolicy>) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CMUX_") {
            command.env_remove(key);
        }
    }
    if let Some(policy) = policy.filter(|p| p.allowed)
        && let Some(variable) = &policy.disable_variable
    {
        command.env(variable, "1");
    }
}

/// Compact disclosure for the ordinary launch screen; full provenance remains
/// available in the dry-run and explicit status report.
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
        "  cmux       {installed} verified component(s), {unknown} unknown; activation/live delivery unverified; headless {}. Details: ahu cmux status\n",
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
        "cmux integration {}: headless {}\n",
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
        "  CLI version: {} ({}); approval, executable and launch checks still apply\n",
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
            profile.harness, profile.version_policy, profile.headless_verified_versions.join(", "),
            profile.evidence, profile.unknown_integration_opt_in, profile.interactive_supported, profile.limitations
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

#[derive(Debug, Clone, Serialize)]
pub struct NativeCli {
    pub executable: Option<PathBuf>,
    pub version: Option<String>,
    pub installer_supported: bool,
    pub detail: String,
}
impl NativeCli {
    pub fn discover() -> Self {
        let executable = super::resolve_executable().ok();
        let version = executable.as_ref().and_then(|path| {
            if std::env::var_os("AHU_CMUX_BIN").is_some() {
                crate::selection::probe_explicit_utility_version(path)
            } else {
                path.to_str().and_then(crate::selection::probe_version)
            }
        });
        let installer_supported = version
            .as_deref()
            .is_some_and(|v| v.strip_prefix("cmux ").unwrap_or(v) == REVIEWED_VERSION);
        Self { executable, version, installer_supported,
            detail: "CLI probe independent of socket reachability. Installer mapping reviewed only for cmux 0.64.22 (102) [ddd4a01bc]; other builds remain unknown.".into() }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct InstallationPlan {
    pub harness: String,
    pub executable: Option<PathBuf>,
    pub argv: Vec<String>,
    pub available: bool,
    pub scope: String,
    pub affected_paths: Vec<PathBuf>,
    pub unresolved_overrides: Vec<String>,
    pub detail: String,
}
pub fn installation_plan(harness: &str, cli: &NativeCli) -> Result<InstallationPlan> {
    installation_plan_in(harness, cli, &Locations::detect())
}
pub fn installation_plan_in(
    harness: &str,
    cli: &NativeCli,
    locations: &Locations,
) -> Result<InstallationPlan> {
    let agent = native_agent(harness).ok_or_else(|| {
        Error::new("unsupported harness; use claude-code, codex, opencode, or antigravity")
    })?;
    let wrapper = agent == "claude";
    let relevant: &[&str] = match harness {
        "codex" => &["CODEX_HOME"],
        "opencode" => &[
            "XDG_CONFIG_HOME",
            "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_DIR",
            "OPENCODE_CONFIG_CONTENT",
        ],
        "antigravity" => &["GEMINI_CLI_HOME", "AGY_CONFIG_DIR"],
        _ => &[],
    };
    let unresolved_overrides: Vec<String> = relevant
        .iter()
        .filter(|k| locations.overrides.contains_key(**k))
        .map(|k| (*k).into())
        .collect();
    let affected_paths = locations
        .home
        .as_ref()
        .map(|home| {
            let files: &[&str] = match harness {
                "codex" => &[".codex/hooks.json", ".codex/config.toml", ".cmux/hooks"],
                "opencode" => &[
                    ".config/opencode/plugins/cmux-feed.js",
                    ".config/opencode/plugins/cmux-session.js",
                ],
                "antigravity" => &[".gemini/config/hooks.json"],
                _ => &[],
            };
            files.iter().map(|f| home.join(f)).collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let scope_known = unresolved_overrides.is_empty()
        && locations
            .home
            .as_ref()
            .is_some_and(|p| check_path(p).is_ok());
    Ok(InstallationPlan { harness: harness.into(), executable: cli.executable.clone(),
        argv: if wrapper { vec![] } else { vec!["hooks".into(), agent.into(), "install".into()] },
        available: !wrapper && cli.installer_supported && cli.executable.is_some() && scope_known,
        scope: "native default user scope; custom configuration overrides require separate native review; no project or bulk installation requested".into(),
        affected_paths,
        unresolved_overrides,
        detail: if wrapper { "Claude is wrapper-managed: use cmux Settings > Automation. There is no supported `hooks claude install` operation.".into() }
            else { "Native cmux owns hooks/scripts and associated activation/trust settings. Native confirmations and exit status are preserved. Success does not prove activation or live conformance.".into() },
    })
}
/// Inherit terminal streams; never insert --yes or evaluate shell input.
pub fn execute_install(plan: &InstallationPlan) -> Result<i32> {
    if !plan.available {
        return Err(Error::new(
            "native installer unavailable or unverified; inspect the installation plan",
        ));
    }
    let expected = native_agent(&plan.harness)
        .filter(|v| *v != "claude")
        .ok_or_else(|| Error::new("unsupported native installer"))?;
    if plan.argv != ["hooks", expected, "install"] {
        return Err(Error::new("invalid native installation operation"));
    }
    let executable = plan
        .executable
        .as_ref()
        .ok_or_else(|| Error::new("cmux executable unavailable"))?;
    let status = Command::new(executable).args(&plan.argv).status()?;
    use std::os::unix::process::ExitStatusExt;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
}
pub fn status_command(repo: &Path, json: bool) -> Result<i32> {
    let cli = NativeCli::discover();
    let statuses: Vec<_> = ["claude-code", "codex", "opencode", "antigravity", "kiro"]
        .into_iter()
        .map(|h| {
            let version = crate::catalog::harness(h)
                .and_then(|entry| crate::selection::installed_version(entry.executable));
            inspect(repo, h).with_version(version.as_deref())
        })
        .collect();
    let plans = statuses
        .iter()
        .filter(|s| s.harness != "kiro")
        .map(|s| installation_plan(&s.harness, &cli))
        .collect::<Result<Vec<_>>>()?;
    if json {
        println!(
            "{}",
            serde_json::to_string(
                &serde_json::json!({"schema_version":1,"cli":cli,"integrations":statuses,"installation_plans":plans})
            )?
        );
    } else {
        println!(
            "cmux CLI: {} ({})",
            cli.executable
                .as_ref()
                .map(|p| display_safe(&p.display().to_string()))
                .unwrap_or_else(|| "unavailable".into()),
            display_safe(cli.version.as_deref().unwrap_or("version unknown"))
        );
        for status in &statuses {
            print!("{}", render(status));
        }
        for plan in &plans {
            println!(
                "installer {}: {} — {}",
                plan.harness,
                if plan.available {
                    "available"
                } else {
                    "unavailable/unknown"
                },
                plan.detail
            );
        }
    }
    Ok(0)
}
pub fn install_command(repo: &Path, harness: &str, dry_run: bool) -> Result<i32> {
    let cli = NativeCli::discover();
    let plan = installation_plan(harness, &cli)?;
    eprintln!("{}\nScope: {}", plan.detail, plan.scope);
    for path in &plan.affected_paths {
        eprintln!("  {}", display_safe(&path.display().to_string()));
    }
    if !plan.unresolved_overrides.is_empty() {
        eprintln!(
            "Unresolved scope overrides: {}",
            plan.unresolved_overrides.join(", ")
        );
    }

    if !plan.argv.is_empty()
        && let Some(executable) = &plan.executable
    {
        eprintln!(
            "Native operation: {} {}",
            display_safe(&shell_single_quote(&executable.to_string_lossy())),
            plan.argv
                .iter()
                .map(|s| shell_single_quote(s))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    if dry_run {
        println!("{}", serde_json::to_string(&plan)?);
        return Ok(0);
    }
    if harness == "claude-code" {
        return Ok(0);
    }
    let code = execute_install(&plan)?;
    // Rescan even after a failed/partial native operation. No inferred success.
    eprint!("{}", render(&inspect(repo, harness)));
    Ok(code)
}

#[cfg(test)]
mod trusted_opencode_state_tests {
    use super::*;

    fn status(name: &str, scope: &str, path: &Path) -> Status {
        let component = Component {
            name: name.into(),
            registration: Registration::Unknown,
            activation: Activation::Unknown,
            isolation: Isolation::Unknown,
            conformance: Conformance::Untested,
            evidence: vec![Evidence {
                path: path.to_path_buf(),
                scope: scope.into(),
                method: "metadata only; contents not read".into(),
                digest: None,
            }],
            detail: "opaque state".into(),
        };
        Status {
            harness: "opencode".into(),
            native_agent: None,
            profile: None,
            cli_version: Some("1.18.32".into()),
            version_checked: true,
            components: vec![component],
            gaps: vec![],
            conformance: "locally_inspected; live_untested".into(),
            headless: HeadlessPolicy {
                allowed: false,
                disable_variable: None,
                reasons: vec!["opaque state".into()],
                evidence_digest: "fixture".into(),
            },
            next_action: "interactive".into(),
        }
    }

    #[test]
    fn project_trust_accepts_only_native_opencode_authentication_stores() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        for (name, scope, file) in [
            (
                "authenticated account config",
                "native account database",
                "opencode.db-wal",
            ),
            (
                "authenticated remote config",
                "user native authentication",
                "auth.json",
            ),
        ] {
            let path = home.join(".local/share/opencode").join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"opaque fixture").unwrap();
            let mut status = status(name, scope, &path);
            assert!(trust_only_opencode_account_state(&mut status, home));
            assert!(status.headless.allowed);
            assert!(status.headless.reasons.is_empty());
            assert!(
                status
                    .gaps
                    .iter()
                    .any(|gap| gap.contains("executable plugins"))
            );
        }
    }

    #[test]
    fn project_trust_does_not_accept_project_plugins_or_other_unknown_state() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let path = home.join(".config/opencode/config.json");
        let mut status = status("user defaults", "user", &path);
        assert!(!trust_only_opencode_account_state(&mut status, home));
        assert!(!status.headless.allowed);
        assert_eq!(status.headless.reasons, ["opaque state"]);
    }

    #[test]
    fn project_trust_does_not_accept_symlinked_native_state() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let directory = home.join(".local/share/opencode");
        std::fs::create_dir_all(&directory).unwrap();
        let target = home.join("outside.db");
        std::fs::write(&target, b"opaque fixture").unwrap();
        let path = directory.join("opencode.db");
        std::os::unix::fs::symlink(target, &path).unwrap();
        let mut status = status(
            "authenticated account config",
            "native account database",
            &path,
        );
        assert!(!trust_only_opencode_account_state(&mut status, home));
        assert!(!status.headless.allowed);
    }
}
