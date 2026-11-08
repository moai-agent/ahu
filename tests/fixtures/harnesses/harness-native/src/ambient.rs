//! Native location and guarded profile selection belongs to the provider.
use crate::{Environment, Error, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    project: PathBuf,
    home: PathBuf,
    native_executable: PathBuf,
    environment: Environment,
    #[serde(default)]
    environment_names: Vec<String>,
    version: Option<String>,
    model: Option<String>,
    permissions: Option<String>,
    mode: Option<String>,
    work_mode: Option<bool>,
    #[serde(default)]
    settings: Settings,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    #[serde(default)]
    inspection_only: bool,
    #[serde(default)]
    trust_native_account_state: bool,
    helpers: Option<Value>,
}
pub(crate) fn admission(harness: &str, params: &Value) -> Result<Value> {
    let p: Params = crate::decode(params)?;
    let probe = crate::ProbeParams {
        project: p.project.clone(),
        home: p.home.clone(),
        native_executable: p.native_executable.clone(),
        environment: p.environment.clone(),
    };
    probe.validate()?;
    let mut locations = locations(&p.home, &p.environment);
    if p.environment_names.len() > 512
        || p.environment_names
            .iter()
            .any(|n| n.is_empty() || n.len() > 256 || n.contains(['\0', '=']))
    {
        return Err(Error::invalid());
    }
    locations.overrides.extend(
        p.environment_names
            .into_iter()
            .map(|name| (name, String::new())),
    );
    if p.settings.inspection_only || p.model.is_none() {
        let mut status = crate::admission::inspect_in(&p.project, harness, &locations);
        let mut invocation = Value::Null;
        if p.settings.inspection_only {
            status.headless.allowed = false;
            status
                .headless
                .reasons
                .push("Inventory alone does not admit a native launch".into());
        } else {
            let version = p.version.as_deref().ok_or_else(Error::invalid)?;
            status = crate::admission::headless_status(status, version);
            if let Some(profile) = crate::isolation::profile(harness, version) {
                invocation = json!({"id":profile.id,"args":profile.args});
                if harness == "codex" {
                    let metadata = crate::codex_metadata::inspect(
                        &p.native_executable,
                        &p.project,
                        version,
                        &p.home,
                        &p.environment,
                    )
                    .map_err(|_| Error::uncertain())?;
                    status = crate::admission::codex_effective_status(
                        status, &p.project, &metadata, version,
                    )?
                    .with_version(Some(version));
                }
            }
            if harness == "opencode" && p.settings.trust_native_account_state {
                crate::admission::trust_only_opencode_account_state(&mut status, &p.home);
            }
        }
        return Ok(
            json!({"allowed":status.headless.allowed,"decision":if p.settings.inspection_only{"inspection"}else if status.headless.allowed{"admitted"}else{"refused"},"guarded_checks":if p.settings.inspection_only{"inventory_only"}else{"inspected"},"conformance":"untested","evidence":status,"helper_profile":null,"invocation_profile":invocation}),
        );
    }
    let expanded = json!({"project":p.project,"home":p.home,"native_executable":p.native_executable,"environment":p.environment,"version":p.version.ok_or_else(Error::invalid)?,"model":p.model,"permissions":p.permissions.ok_or_else(Error::invalid)?,"mode":p.mode.ok_or_else(Error::invalid)?,"work_mode":p.work_mode.unwrap_or(false),"overrides":locations.overrides.keys().collect::<Vec<_>>(),"managed_claude":locations.managed_claude,"managed_opencode":locations.managed_opencode,"managed_preferences":locations.managed_preferences,"system_codex":locations.system_codex,"codex_managed_preferences":managed_name(locations.codex_managed_preferences),"trust_opencode_native_state":p.settings.trust_native_account_state,"helpers":p.settings.helpers.unwrap_or_else(|| json!({"policy":"disabled","helper_model":null,"helper_role":"reviewer","max_concurrent":1,"max_depth":1,"budget_usd":null,"assignment_writes":true}))});
    crate::admit(harness, &expanded)
}
fn managed_name(value: crate::admission::ManagedPreferences) -> &'static str {
    use crate::admission::ManagedPreferences::*;
    match value {
        NotApplicable => "not_applicable",
        Absent => "absent",
        Present => "present",
        Unknown => "unknown",
    }
}
fn locations(home: &Path, environment: &Environment) -> crate::admission::Locations {
    crate::admission::Locations {
        home: Some(home.to_path_buf()),
        overrides: environment
            .keys()
            .map(|k| (k.clone(), String::new()))
            .collect(),
        managed_claude: Some(PathBuf::from(if cfg!(target_os = "macos") {
            "/Library/Application Support/ClaudeCode/managed-settings.json"
        } else {
            "/etc/claude-code/managed-settings.json"
        })),
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

use crate::admission::ManagedPreferences;
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

pub(crate) fn auth_override(harness: &str, name: &str) -> bool {
    match harness {
        "codex" => matches!(
            name,
            "CODEX_HOME"
                | "OPENAI_API_KEY"
                | "OPENAI_BASE_URL"
                | "CODEX_API_KEY"
                | "CODEX_API_BASE_URL"
        ),
        "claude-code" => matches!(
            name,
            "CLAUDE_CONFIG_DIR"
                | "ANTHROPIC_API_KEY"
                | "ANTHROPIC_AUTH_TOKEN"
                | "ANTHROPIC_BASE_URL"
                | "CLAUDE_CODE_USE_BEDROCK"
                | "CLAUDE_CODE_USE_VERTEX"
                | "CLAUDE_CODE_USE_FOUNDRY"
        ),
        "kiro" => matches!(name, "KIRO_HOME" | "KIRO_API_KEY"),
        "antigravity" => matches!(
            name,
            "GEMINI_CLI_HOME"
                | "AGY_CONFIG_DIR"
                | "GEMINI_API_KEY"
                | "GOOGLE_API_KEY"
                | "GOOGLE_APPLICATION_CREDENTIALS"
        ),
        "opencode" => matches!(
            name,
            "OLLAMA_HOST"
                | "OPENCODE_CONFIG"
                | "OPENCODE_CONFIG_DIR"
                | "OPENCODE_CONFIG_CONTENT"
                | "OPENCODE_AUTH_CONTENT"
                | "XDG_CONFIG_HOME"
                | "XDG_DATA_HOME"
        ),
        _ => true,
    }
}
