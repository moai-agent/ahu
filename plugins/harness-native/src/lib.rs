//! Provider implementation support for installed native harness runtime binaries.
//! Core must not link this crate. See README for authority and operation contracts.
mod admission;
mod ambient;
mod auth;
mod codex_metadata;
mod compatibility;
/// Parse the native CLI version without claiming live compatibility.
pub use compatibility::version_token;
mod files;
mod helpers;
mod hook_inventory;
mod isolation;
mod process;
mod profile;
mod recovery;
mod setup;
mod source_actions;
mod sources;
mod trust;
mod util;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
type Environment = std::collections::BTreeMap<String, String>;

type Result<T> = std::result::Result<T, Error>;
/// Fixed, nonsensitive failure. Unknown effects forbid automatic replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub effects_unknown: bool,
}
impl Error {
    fn invalid() -> Self {
        Self {
            effects_unknown: false,
        }
    }
    fn uncertain() -> Self {
        Self {
            effects_unknown: true,
        }
    }
    // Extracted implementations supply context, deliberately discarded at this boundary.
    fn new(_: impl std::fmt::Display) -> Self {
        Self::invalid()
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("native operation refused or unavailable")
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::invalid()
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::invalid()
    }
}
macro_rules! bail { ($($arg:tt)*) => {return Err(crate::Error::new(format_args!($($arg)*)))}; }
use bail;

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Permissions {
    Prompt,
    AcceptEdits,
    Auto,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeParams {
    project: PathBuf,
    home: PathBuf,
    native_executable: PathBuf,
    environment: Environment,
}
impl ProbeParams {
    fn validate(&self) -> Result<()> {
        if self.environment.len() > 128
            || self.environment.iter().any(|(k, v)| {
                k.is_empty()
                    || k == "HOME"
                    || k.contains('=')
                    || k.contains('\0')
                    || v.contains('\0')
            })
        {
            return Err(Error::invalid());
        }
        directory(&self.project)?;
        directory(&self.home)?;
        if !self.native_executable.is_absolute() || !self.native_executable.is_file() {
            return Err(Error::invalid());
        }
        Ok(())
    }
}
fn directory(path: &Path) -> Result<()> {
    files::check_components(path)?;
    if !path.is_dir() || path.canonicalize()? != path {
        return Err(Error::invalid());
    }
    Ok(())
}
fn decode<T: serde::de::DeserializeOwned>(params: &Value) -> Result<T> {
    if serde_json::to_vec(params)?.len() > files::LIMIT {
        return Err(Error::invalid());
    }
    Ok(serde_json::from_value(params.clone())?)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthParams {
    #[serde(default)]
    environment_names: Vec<String>,
    project: PathBuf,
    home: PathBuf,
    native_executable: PathBuf,
    environment: Environment,
    model: Option<String>,
    ollama_host: Option<String>,
    requested_identity: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustParams {
    overrides: Vec<String>,
    task_dir: PathBuf,
    worktree: PathBuf,
    home: PathBuf,
    permissions: Permissions,
    mode: Mode,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Interactive,
    Headless,
    Direct,
}

pub(crate) fn valid_environment_names(names: &[String]) -> bool {
    names.len() <= 512
        && names
            .iter()
            .all(|name| !name.is_empty() && name.len() <= 256 && !name.contains(['\0', '=']))
}

/// Dispatch a typed native operation. Never resolves executables through PATH.
/// Trust calls require a host-verified task/worktree ownership grant. Setup only
/// proposes changes. Account calls never return credentials or raw native output.
pub fn call(harness: &str, operation: &str, params: &Value) -> Result<Value> {
    if operation == "admission" && params.get("action").is_some() {
        return bound_output(profile::call(harness, params), false);
    }
    // Validate operation-specific inputs before any native dispatch. A recognized
    // project snapshot surrounds all operations that may execute native code.
    if matches!(
        operation,
        "probe" | "models" | "auth" | "budget" | "admission"
    ) {
        let project = params
            .get("project")
            .and_then(Value::as_str)
            .map(Path::new)
            .ok_or_else(Error::invalid)?;
        directory(project)?;
        let before = files::snapshot(project)?;
        let result = dispatch(harness, operation, params);
        if files::snapshot(project).map_err(|_| Error::uncertain())? != before {
            return Err(Error::uncertain());
        }
        return bound_output(result, true);
    }
    bound_output(
        dispatch(harness, operation, params),
        operation.starts_with("trust_"),
    )
}
fn bound_output(result: Result<Value>, effects_possible: bool) -> Result<Value> {
    let value = result?;
    if serde_json::to_vec(&value)?.len() > 4 * files::LIMIT {
        return Err(Error {
            effects_unknown: effects_possible,
        });
    }
    Ok(value)
}
fn dispatch(harness: &str, operation: &str, params: &Value) -> Result<Value> {
    if compatibility::evidence(harness).is_none() {
        return Err(Error::invalid());
    }
    match operation {
        "probe" | "models" => {
            let p: ProbeParams = decode(params)?;
            p.validate()?;
            if operation == "probe" {
                probe(harness, &p)
            } else {
                models(harness, &p)
            }
        }
        "auth" | "budget" => {
            let a: AuthParams = decode(params)?;
            if !valid_environment_names(&a.environment_names)
                || a.environment_names.iter().any(|name| {
                    ambient::auth_override(harness, name) && !a.environment.contains_key(name)
                })
            {
                return Err(Error::invalid());
            }
            let p = ProbeParams {
                project: a.project,
                home: a.home,
                native_executable: a.native_executable,
                environment: a.environment,
            };
            p.validate()?;
            auth::call(
                harness,
                &p,
                operation == "budget",
                a.model.as_deref(),
                a.ollama_host.as_deref(),
                a.requested_identity.as_deref(),
            )
            .map_err(|_| Error::uncertain())
        }
        "setup" => setup::call(harness, params),
        "native_sources" => sources::call(harness, params),
        "recover" => recovery::call(harness, params),
        "admission" => {
            if params.get("settings").is_some() || params.get("managed_claude").is_none() {
                ambient::admission(harness, params)
            } else {
                admit(harness, params)
            }
        }
        "trust_prepare" | "trust_cleanup" => {
            let p: TrustParams = decode(params)?;
            if operation == "trust_prepare"
                && (p.permissions != Permissions::Auto
                    || p.mode != Mode::Interactive
                    || !matches!(harness, "codex" | "claude-code" | "antigravity"))
            {
                if std::fs::symlink_metadata(p.task_dir.join("native-trust.json")).is_ok() {
                    return Err(Error::invalid());
                }
                return Ok(json!({"status":"not_applicable","receipt_path":null}));
            }
            let relocated: &[&str] = match harness {
                "codex" => &["CODEX_HOME"],
                "claude-code" => &["CLAUDE_CONFIG_DIR"],
                _ => &["GEMINI_CLI_HOME", "AGY_CONFIG_DIR"],
            };
            if operation == "trust_prepare"
                && (p.overrides.len() > 512
                    || p.overrides
                        .iter()
                        .any(|key| relocated.contains(&key.as_str())))
            {
                return Err(Error::invalid());
            }
            directory(&p.home)?;
            directory(&p.task_dir)?;
            files::check_components(&p.worktree)?;
            if operation == "trust_prepare" {
                directory(&p.worktree)?;
            }
            trust::update(
                &p.home,
                &p.task_dir,
                harness,
                &p.worktree,
                operation == "trust_cleanup",
            )
            .map_err(|_| Error::uncertain())?;
            Ok(json!({"status":"complete","receipt_path":p.task_dir.join("native-trust.json")}))
        }
        _ => Err(Error::invalid()),
    }
}
fn probe(harness: &str, p: &ProbeParams) -> Result<Value> {
    let output = process::run(
        &p.native_executable,
        &["--version"],
        &p.project,
        &p.home,
        &p.environment,
        std::time::Duration::from_secs(8),
    )?;
    let text = std::str::from_utf8(&output).map_err(|_| Error::uncertain())?;
    let version = compatibility::version_token(text)
        .filter(|v| v.len() <= 128)
        .ok_or_else(Error::uncertain)?;
    Ok(
        json!({"version":version,"compatibility":compatibility::isolation_profile(harness),"reviewed_version":compatibility::evidence(harness).is_some_and(|(_,v)|v.contains(&version)),"enforcement":{"model":"initial_selection_only","identity":"prompt_only","filesystem_confinement":"not_established","live_conformance":"untested"}}),
    )
}
fn models(harness: &str, p: &ProbeParams) -> Result<Value> {
    let suggested = match harness {
        "codex" => vec!["gpt-6-astra", "gpt-6.1-sol"],
        "claude-code" => vec!["claude-opus-5"],
        _ => vec![],
    };
    if !suggested.is_empty() {
        return Ok(json!({"source":"suggested","models":suggested,"entitlement_checked":false}));
    }
    let args = if harness == "kiro" {
        let bytes = process::run(
            &p.native_executable,
            &["whoami", "--format", "json"],
            &p.project,
            &p.home,
            &p.environment,
            std::time::Duration::from_secs(8),
        )?;
        let identity: Value = serde_json::from_slice(&bytes)?;
        if identity
            .get("accountType")
            .and_then(Value::as_str)
            .is_none_or(|s| s.is_empty())
        {
            return Err(Error::invalid());
        }
        vec!["chat", "--v3", "--list-models", "--format", "json"]
    } else {
        vec!["models"]
    };
    let bytes = process::run(
        &p.native_executable,
        &args,
        &p.project,
        &p.home,
        &p.environment,
        std::time::Duration::from_secs(8),
    )?;
    let ids: Vec<String> = if harness == "kiro" {
        let v: Value = serde_json::from_slice(&bytes)?;
        v.get("models")
            .and_then(Value::as_array)
            .ok_or_else(Error::invalid)?
            .iter()
            .map(|m| {
                m.get("model_id")
                    .and_then(Value::as_str)
                    .filter(|s| valid_model(s))
                    .map(str::to_owned)
                    .ok_or_else(Error::invalid)
            })
            .collect::<Result<_>>()?
    } else {
        std::str::from_utf8(&bytes)
            .map_err(|_| Error::invalid())?
            .lines()
            .filter_map(|l| {
                if harness == "opencode" {
                    l.split_whitespace().find(|t| t.contains('/'))
                } else {
                    l.split_whitespace().next()
                }
            })
            .filter(|s| valid_model(s))
            .map(str::to_owned)
            .collect()
    };
    if ids.len() > 4096 {
        return Err(Error::invalid());
    }
    Ok(json!({"source":"native","models":ids,"entitlement_checked":false}))
}
fn valid_model(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/-+".contains(&b))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionParams {
    project: PathBuf,
    home: PathBuf,
    native_executable: PathBuf,
    environment: Environment,
    version: String,
    model: String,
    permissions: Permissions,
    mode: Mode,
    work_mode: bool,
    /// Presence-only names. Values never enter evidence.
    overrides: Vec<String>,
    managed_claude: Option<PathBuf>,
    managed_opencode: Option<PathBuf>,
    managed_preferences: Option<PathBuf>,
    system_codex: Option<PathBuf>,
    codex_managed_preferences: admission::ManagedPreferences,
    trust_opencode_native_state: bool,
    helpers: HelperParams,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelperParams {
    policy: String,
    helper_model: Option<String>,
    helper_role: String,
    max_concurrent: u32,
    max_depth: u32,
    budget_usd: Option<f64>,
    assignment_writes: bool,
}
fn admit(harness: &str, params: &Value) -> Result<Value> {
    let p: AdmissionParams = decode(params)?;
    let probe = ProbeParams {
        project: p.project.clone(),
        home: p.home.clone(),
        native_executable: p.native_executable.clone(),
        environment: p.environment.clone(),
    };
    probe.validate()?;
    if !valid_model(&p.model)
        || compatibility::version_token(&p.version).is_none()
        || (matches!(harness, "kiro" | "opencode") && p.permissions == Permissions::AcceptEdits)
    {
        return Err(Error::invalid());
    }
    for path in [
        &p.managed_claude,
        &p.managed_opencode,
        &p.managed_preferences,
        &p.system_codex,
    ]
    .into_iter()
    .flatten()
    {
        files::check_components(path)?;
    }
    if p.overrides.len() > 512
        || p.overrides.iter().any(|s| {
            s.is_empty()
                || s.len() > 256
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphabetic() || b.is_ascii_digit() || b == b'_')
        })
    {
        return Err(Error::invalid());
    }
    let managed_scope_missing = match harness {
        "codex" => p.system_codex.is_none(),
        "claude-code" => p.managed_claude.is_none(),
        "opencode" => {
            p.managed_opencode.is_none()
                || (cfg!(target_os = "macos") && p.managed_preferences.is_none())
        }
        _ => false,
    };
    let locations = admission::Locations {
        home: Some(p.home.clone()),
        overrides: p
            .overrides
            .into_iter()
            .chain(p.environment.keys().cloned())
            .map(|s| (s, String::new()))
            .collect(),
        managed_claude: p.managed_claude,
        managed_opencode: p.managed_opencode,
        managed_preferences: p.managed_preferences,
        system_codex: p.system_codex,
        codex_managed_preferences: p.codex_managed_preferences,
    };
    if !p.work_mode && matches!(harness, "opencode" | "kiro") {
        let listing = models(harness, &probe)?;
        if !listing["models"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(&p.model)))
        {
            return Err(Error::invalid());
        }
    }
    if harness == "opencode" && p.mode != Mode::Headless {
        let v2 = compatibility::version_token(&p.version).is_some_and(|v| v.starts_with("2."));
        if v2 && p.permissions == Permissions::Auto {
            return Err(Error::invalid());
        }
        let args = if v2 {
            vec!["mini", "--help"]
        } else {
            vec!["--help"]
        };
        let bytes = process::run(
            &p.native_executable,
            &args,
            &p.project,
            &p.home,
            &p.environment,
            std::time::Duration::from_secs(8),
        )?;
        let help = std::str::from_utf8(&bytes).map_err(|_| Error::uncertain())?;
        for flag in ["--model", "--prompt"]
            .into_iter()
            .chain((p.permissions == Permissions::Auto).then_some("--auto"))
        {
            if !help.lines().any(|line| {
                line.split_whitespace()
                    .take_while(|word| word.starts_with('-'))
                    .any(|word| word.trim_end_matches(',') == flag)
            }) {
                return Err(Error::invalid());
            }
        }
    }
    let mut helper = helpers::profile(&helpers::Request {
        harness,
        harness_version: &p.version,
        policy: &p.helpers.policy,
        session_model: &p.model,
        helper_model: p.helpers.helper_model.as_deref(),
        helper_role: &p.helpers.helper_role,
        max_concurrent: p.helpers.max_concurrent,
        max_depth: p.helpers.max_depth,
        budget_usd: p.helpers.budget_usd,
        assignment_writes: p.helpers.assignment_writes,
    })?;
    if harness == "codex" {
        helper
            .args
            .extend(["-c".into(), "agents.enabled=false".into()]);
    }
    let mut status = admission::inspect_in(&p.project, harness, &locations);
    if p.work_mode {
        return Ok(
            json!({"allowed":true,"decision":"work_mode","guarded_checks":"intentionally_skipped","conformance":"untested","evidence":status,"helper_profile":helper,"invocation_profile":null}),
        );
    }
    let mut invocation = Value::Null;
    if p.mode == Mode::Headless {
        status = admission::headless_status(status, &p.version);
        if let Some(profile) = isolation::profile(harness, &p.version) {
            invocation = json!({"id":profile.id,"args":profile.args});
            if harness == "codex" {
                let metadata = codex_metadata::inspect(
                    &p.native_executable,
                    &p.project,
                    &p.version,
                    &p.home,
                    &p.environment,
                )
                .map_err(|_| Error::uncertain())?;
                status =
                    admission::codex_effective_status(status, &p.project, &metadata, &p.version)?
                        .with_version(Some(&p.version));
            }
        }
        if harness == "opencode" && p.trust_opencode_native_state {
            admission::trust_only_opencode_account_state(&mut status, &p.home);
        }
    } else {
        status = status.with_version(Some(&p.version));
    }
    if managed_scope_missing {
        status.headless.allowed = false;
        status
            .headless
            .reasons
            .push("native managed scope not supplied; coverage is unknown".into());
    }
    let allowed = p.mode != Mode::Headless || status.headless.allowed;
    Ok(
        json!({"allowed":allowed,"decision":if allowed{"admitted"}else{"refused"},"guarded_checks":"inspected","conformance":"untested","evidence":status,"helper_profile":helper,"invocation_profile":invocation}),
    )
}

#[cfg(test)]
mod admission_tests;

#[cfg(test)]
mod kiro_tests;

#[cfg(test)]
mod hook_tests;
