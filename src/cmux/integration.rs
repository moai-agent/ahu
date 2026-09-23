//! Read-only, bounded evidence about native integrations. No hook bodies are
//! persisted, interpreted, installed, or copied by inspection.
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
        if let Err(error) =
            crate::catalog::check_headless_version(&self.harness, version.unwrap_or(""))
        {
            self.headless.allowed = false;
            self.headless.reasons.push(error.to_string());
            self.next_action = format!(
                "Use a reviewed CLI version or validate the new CLI profile before headless execution. Interactive execution remains available subject to normal prerequisites. {}",
                self.next_action
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
            "opencode" => "Use interactive execution for unresolved native configuration/authentication sources; credentials and account databases are not read. For unguarded Feed, explicit native removal is `cmux hooks opencode uninstall`, then inspect again. Reinstalling the same Feed does not provide isolation.".into(),
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

// OpenCode v1.18.29-v1.18.31 config/config.ts, config/managed.ts,
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
        if ["notify", "include", "imports", "profile"].contains(&key.as_str())
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

pub fn enforce(repo: &Path, harness: &str) -> Result<Status> {
    let status = inspect(repo, harness);
    if !status.headless.allowed {
        return Err(Error::new(format!(
            "headless cmux isolation is unverified for {harness}:\n{}\n{}",
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
            "compatible with inspected components"
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
            "compatible with inspected components"
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
            "  profile {}: {}; reviewed versions: {}\n  evidence: {}\n  unknown integration opt-in: {}; interactive supported: {}\n  limitations: {}\n",
            profile.harness, profile.version_policy, profile.headless_verified_versions.join(", "),
            profile.evidence, profile.unknown_integration_opt_in, profile.interactive_supported, profile.limitations
        ));
    }
    for reason in &status.headless.reasons {
        text.push_str(&format!("  refusal: {}\n", display_safe(reason)));
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
    let statuses: Vec<_> = ["claude-code", "codex", "opencode", "antigravity"]
        .into_iter()
        .map(|h| {
            let version = crate::catalog::harness(h)
                .and_then(|entry| crate::selection::installed_version(entry.executable));
            inspect(repo, h).with_version(version.as_deref())
        })
        .collect();
    let plans = statuses
        .iter()
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
