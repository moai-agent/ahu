//! Local, per-repository harness account bindings used to guard session resume.
//!
//! The binding contains only a digest of provider identity metadata. It never
//! stores credentials. A provider without a verifiable principal is refused.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::git::Repo;
use crate::util::{Error, Result, digest_bytes};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BindingFile {
    schema_version: u32,
    repo_identity: String,
    active_profile: String,
    profiles: BTreeMap<String, BTreeMap<String, StoredBinding>>,
    #[serde(default)]
    content_digest: String,
}

impl Default for BindingFile {
    fn default() -> Self {
        Self {
            schema_version: 2,
            repo_identity: String::new(),
            active_profile: "default".into(),
            profiles: BTreeMap::new(),
            content_digest: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyBindingFile {
    schema_version: u32,
    #[serde(default)]
    repo_identity: String,
    #[serde(default)]
    bindings: BTreeMap<String, StoredBinding>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredBinding {
    fingerprint: String,
    identity_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Identity {
    pub harness: String,
    pub identity_kind: String,
    pub principal: String,
    pub organization: Option<String>,
    pub workspace: Option<String>,
}

impl Identity {
    fn fingerprint(&self) -> Result<String> {
        let bytes = serde_json::to_vec(&json!({
            "harness": self.harness,
            "identity_kind": self.identity_kind,
            "principal": self.principal,
            "organization": self.organization,
            "workspace": self.workspace,
        }))?;
        Ok(digest_bytes(&bytes))
    }

    pub fn display(&self) -> String {
        let mut parts = vec![format!("{} ({})", self.harness, self.identity_kind)];
        if self.harness == "ollama" {
            parts.push(format!("account {}", self.principal));
            return parts.join("; ");
        }
        if let Some(org) = &self.organization {
            parts.push(format!("organization {org}"));
        }
        if let Some(workspace) = &self.workspace {
            parts.push(format!("workspace {workspace}"));
        }
        parts.push(format!("account {}", self.principal));
        parts.join("; ")
    }
}

fn binding_path(repo: &Repo) -> Result<PathBuf> {
    let root = repo.primary_root()?;
    crate::state::ensure_checkout_state(&root)?;
    let path = binding_path_without_creation(repo)?;
    if let Some(parent) = path.parent() {
        crate::state::create_private_dir_all(parent)?;
    }
    crate::state::confine_file(&path)?;
    Ok(path)
}

fn binding_path_without_creation(repo: &Repo) -> Result<PathBuf> {
    Ok(repo
        .primary_root()?
        .join(".ahu/state/repos")
        .join(repo.identity())
        .join("auth-bindings.json"))
}

fn load(repo: &Repo, path: &Path) -> Result<BindingFile> {
    let value: Value = crate::state::read_json(path)?;
    let migrate_legacy = value.get("schema_version").and_then(Value::as_u64) == Some(1);
    let mut bindings = if value.is_null() {
        BindingFile::default()
    } else if value.get("schema_version").and_then(Value::as_u64) == Some(1) {
        let legacy: LegacyBindingFile = serde_json::from_value(value)?;
        if legacy.schema_version != 1 {
            return Err(Error::new("unsupported local auth binding schema"));
        }
        let profiles = if legacy.bindings.is_empty() {
            BTreeMap::new()
        } else {
            BTreeMap::from([("default".into(), legacy.bindings)])
        };
        BindingFile {
            schema_version: 2,
            repo_identity: legacy.repo_identity,
            active_profile: "default".into(),
            profiles,
            content_digest: String::new(),
        }
    } else {
        let bindings: BindingFile = serde_json::from_value(value)?;
        if bindings.schema_version != 2 {
            return Err(Error::new("unsupported local auth binding schema"));
        }
        if bindings.content_digest != content_digest(&bindings)? {
            return Err(Error::new(
                "local auth profiles changed outside `ahu auth`; refuse to use them until they are rebound through the CLI",
            ));
        }
        bindings
    };
    let expected = repo.identity();
    if !bindings.repo_identity.is_empty() && bindings.repo_identity != expected {
        return Err(Error::new(
            "local auth bindings belong to a different repository",
        ));
    }
    bindings.repo_identity = expected;
    if migrate_legacy {
        save(path, &bindings)?;
    }
    Ok(bindings)
}

fn save(path: &Path, bindings: &BindingFile) -> Result<()> {
    crate::state::confine_file(path)?;
    let mut bindings = bindings.clone();
    bindings.content_digest = content_digest(&bindings)?;
    let bytes = serde_json::to_vec_pretty(&bindings)?;
    crate::private_io::atomic_write(path, &bytes, crate::private_io::Durability::Durable)
}

fn content_digest(bindings: &BindingFile) -> Result<String> {
    let bytes = serde_json::to_vec(&json!({
        "schema_version": bindings.schema_version,
        "repo_identity": bindings.repo_identity,
        "active_profile": bindings.active_profile,
        "profiles": bindings.profiles,
    }))?;
    Ok(digest_bytes(&bytes))
}

pub fn bind(repo: &Repo, harness: &str, replace: bool) -> Result<String> {
    let path = binding_path(repo)?;
    let bindings = load(repo, &path)?;
    bind_profile(repo, harness, &bindings.active_profile, replace)
}

pub fn bind_profile(repo: &Repo, harness: &str, profile: &str, replace: bool) -> Result<String> {
    validate_profile_name(profile)?;
    let identity = probe(harness, &repo.root)?;
    let path = binding_path(repo)?;
    let mut bindings = load(repo, &path)?;
    let fingerprint = identity.fingerprint()?;
    let profile_bindings = bindings.profiles.entry(profile.to_owned()).or_default();
    if let Some(existing) = profile_bindings.get(harness) {
        if existing.fingerprint == fingerprint {
            return Ok(format!(
                "{} is already bound to profile {profile:?}.",
                identity.display()
            ));
        }
        if !replace {
            return Err(Error::new(format!(
                "{harness} is already bound to a different account in profile {profile:?}; use `ahu auth bind --harness {harness} --profile {profile} --replace` only after confirming the intended account"
            )));
        }
    }
    profile_bindings.insert(
        harness.to_owned(),
        StoredBinding {
            fingerprint,
            identity_kind: identity.identity_kind.clone(),
            profile: None,
        },
    );
    save(&path, &bindings)?;
    Ok(format!(
        "Bound {} to local profile {profile:?}.",
        identity.display()
    ))
}

pub fn status(repo: &Repo, harness: &str) -> Result<String> {
    let path = binding_path(repo)?;
    let bindings = load(repo, &path)?;
    check_identity(repo, harness, &bindings.active_profile, &bindings)
}

/// Return a secret-free account readiness projection for scripts and evals.
/// Provider diagnostics and principal values are deliberately omitted: this
/// reports whether identity can be checked and whether the active project
/// binding matches, not the account details themselves.
pub fn readiness(repo: &Repo, harness: &str, model: Option<&str>) -> Value {
    let binding_harness = if harness == "opencode" {
        model.and_then(|model| task_binding_harness(harness, model))
    } else {
        task_binding_harness(harness, "")
    };
    let Some(binding_harness) = binding_harness else {
        return json!({
            "schema_version": 1,
            "harness": harness,
            "model": model,
            "identity": "unsupported",
            "binding": "unsupported",
            "ready": false,
        });
    };
    let path = match binding_path_without_creation(repo) {
        Ok(path) => path,
        Err(_) => return readiness_error(harness, model, "unavailable", "unavailable"),
    };
    if crate::state::confine_file(&path).is_err() {
        return readiness_error(harness, model, "unavailable", "unavailable");
    }
    let bindings = if path.exists() {
        match load(repo, &path) {
            Ok(bindings) => Some(bindings),
            Err(_) => return readiness_error(harness, model, "unavailable", "unavailable"),
        }
    } else {
        None
    };
    let identity = match probe(binding_harness, &repo.root) {
        Ok(identity) => identity,
        Err(_) => return readiness_error(harness, model, "unavailable", "unavailable"),
    };
    let Some(bindings) = bindings.filter(|bindings| !bindings.profiles.is_empty()) else {
        return json!({
            "schema_version": 1,
            "harness": harness,
            "model": model,
            "identity": "verified",
            "binding": "not_configured",
            "ready": false,
        });
    };
    let binding = bindings
        .profiles
        .get(&bindings.active_profile)
        .and_then(|items| items.get(binding_harness));
    let binding_status = match binding {
        None => "not_bound",
        Some(expected) if identity.fingerprint().ok().as_deref() == Some(&expected.fingerprint) => {
            "matched"
        }
        Some(_) => "mismatch",
    };
    json!({
        "schema_version": 1,
        "harness": harness,
        "model": model,
        "identity": "verified",
        "binding": binding_status,
        "ready": binding_status == "matched",
    })
}

fn readiness_error(harness: &str, model: Option<&str>, identity: &str, binding: &str) -> Value {
    json!({
        "schema_version": 1,
        "harness": harness,
        "model": model,
        "identity": identity,
        "binding": binding,
        "ready": false,
    })
}

pub fn status_profile(repo: &Repo, harness: &str, profile: &str) -> Result<String> {
    validate_profile_name(profile)?;
    let path = binding_path(repo)?;
    let bindings = load(repo, &path)?;
    check_identity(repo, harness, profile, &bindings)
}

fn check_identity(
    repo: &Repo,
    harness: &str,
    profile: &str,
    bindings: &BindingFile,
) -> Result<String> {
    let identity = probe(harness, &repo.root)?;
    let current = identity.fingerprint()?;
    match bindings
        .profiles
        .get(profile)
        .and_then(|items| items.get(harness))
    {
        Some(binding) if binding.fingerprint == current => Ok(format!(
            "{}; matches local profile {profile:?}.",
            identity.display()
        )),
        Some(_) => Err(Error::new(format!(
            "{}; does not match local profile {profile:?}",
            identity.display()
        ))),
        None => Err(Error::new(format!(
            "{}; no account is bound for profile {profile:?}. Run `ahu auth bind --harness {harness} --profile {profile}` after confirming the intended account.",
            identity.display()
        ))),
    }
}

pub fn select_profile(repo: &Repo, profile: &str) -> Result<String> {
    validate_profile_name(profile)?;
    let path = binding_path(repo)?;
    let mut bindings = load(repo, &path)?;
    if !bindings.profiles.contains_key(profile) {
        return Err(Error::new(format!(
            "auth profile {profile:?} does not exist; bind at least one harness with `ahu auth bind --harness ID --profile {profile}` first"
        )));
    }
    bindings.active_profile = profile.to_owned();
    save(&path, &bindings)?;
    Ok(format!(
        "Selected local auth profile {profile:?} for this project."
    ))
}

pub fn list_profiles(repo: &Repo) -> Result<String> {
    let path = binding_path(repo)?;
    let bindings = load(repo, &path)?;
    if bindings.profiles.is_empty() {
        return Ok("No local auth profiles are configured. Bind an account with `ahu auth bind --harness ID`.".into());
    }
    let mut rows = Vec::new();
    for (name, providers) in &bindings.profiles {
        let active = if name == &bindings.active_profile {
            " (active)"
        } else {
            ""
        };
        rows.push(format!(
            "{name}{active} — {} provider binding(s)",
            providers.len()
        ));
    }
    Ok(rows.join("\n"))
}

fn validate_profile_name(profile: &str) -> Result<()> {
    if !crate::util::is_safe_name(profile) {
        return Err(Error::new(
            "auth profile names must use lowercase letters, numbers, hyphens, or underscores",
        ));
    }
    Ok(())
}

pub fn verify_resume(repo: &Repo, harness: &str) -> Result<()> {
    status(repo, harness).map(|_| ())
}

/// Check the active project profile before planning creates task state or a
/// worktree. The headless runner repeats this immediately before process start.
pub fn verify_launch(repo: &Repo, harness: &str, model: &str) -> Result<()> {
    let path = binding_path_without_creation(repo)?;
    crate::state::confine_file(&path)?;
    if !path.exists() {
        return Ok(());
    }
    let bindings = load(repo, &path)?;
    // Before a project enrolls any profiles, preserve the existing launch
    // behavior. Once a profile exists, every provider launch must be verifiable.
    if bindings.profiles.is_empty() {
        return Ok(());
    }
    let binding_harness = task_binding_harness(harness, model).ok_or_else(|| {
        Error::new(format!(
            "ahu cannot verify the account used by {harness} model {model}; launch refused"
        ))
    })?;
    check_identity(repo, binding_harness, &bindings.active_profile, &bindings).map(|_| ())
}

/// Freeze the verified account used by the first attempt. This is called after
/// ahu has validated its task record and immediately before it starts the
/// harness, before any project prompt is sent to the provider.
pub fn capture_task(repo: &Repo, harness: &str, task_dir: &Path) -> Result<()> {
    let binding_harness =
        task_binding_harness(harness, "").or_else(|| (harness == "opencode").then_some("opencode"));
    capture_task_for(repo, binding_harness, task_dir)
}

pub fn capture_task_for_model(
    repo: &Repo,
    harness: &str,
    model: &str,
    task_dir: &Path,
) -> Result<()> {
    let path = binding_path_without_creation(repo)?;
    crate::state::confine_file(&path)?;
    if !path.exists() {
        return Ok(());
    }
    let bindings = load(repo, &path)?;
    if bindings.profiles.is_empty() {
        return Ok(());
    }
    let binding_harness = task_binding_harness(harness, model);
    capture_task_for(repo, binding_harness, task_dir)
}

/// Pin interactive task startup to the selected project profile when the
/// project has opted into auth bindings. Projects without profiles retain the
/// legacy interactive behavior; headless startup always requires a binding.
pub fn capture_interactive_task_for_model(
    repo: &Repo,
    harness: &str,
    model: &str,
    task_dir: &Path,
) -> Result<()> {
    capture_task_for_model(repo, harness, model, task_dir)
}

fn capture_task_for(repo: &Repo, binding_harness: Option<&str>, task_dir: &Path) -> Result<()> {
    let Some(binding_harness) = binding_harness else {
        // These CLIs do not expose an identity signal that ahu can verify. Keep
        // allowing fresh attempts, but persist an explicit unknown marker so a
        // later resume fails closed rather than assuming the account stayed put.
        let pin_path = task_dir.join("auth-identity.json");
        crate::state::confine_file(&pin_path)?;
        let pin = StoredBinding {
            fingerprint: "unverified".into(),
            identity_kind: "unverified-provider-identity".into(),
            profile: None,
        };
        let bytes = serde_json::to_vec(&pin)?;
        let _ = crate::private_io::atomic_create(
            &pin_path,
            &bytes,
            crate::private_io::Durability::Durable,
        )?;
        return Ok(());
    };
    let identity = probe(binding_harness, &repo.root)?;
    let path = binding_path(repo)?;
    let bindings = load(repo, &path)?;
    let fingerprint = identity.fingerprint()?;
    let expected = bindings
        .profiles
        .get(&bindings.active_profile)
        .and_then(|profile| profile.get(binding_harness))
        .ok_or_else(|| {
        Error::new(format!(
            "no {binding_harness} account is bound to active profile {:?}; confirm the intended account, then run `ahu auth bind --harness {binding_harness} --profile {}`",
            bindings.active_profile, bindings.active_profile
        ))
    })?;
    if expected.fingerprint != fingerprint {
        return Err(Error::new(format!(
            "current {binding_harness} account does not match this repository's local binding; no prompt was sent"
        )));
    }
    let pin_path = task_dir.join("auth-identity.json");
    crate::state::confine_file(&pin_path)?;
    let pin = StoredBinding {
        fingerprint,
        identity_kind: identity.identity_kind,
        profile: Some(bindings.active_profile.clone()),
    };
    let bytes = serde_json::to_vec(&pin)?;
    if !crate::private_io::atomic_create(&pin_path, &bytes, crate::private_io::Durability::Durable)?
    {
        let existing: StoredBinding = crate::state::read_json(&pin_path)?;
        if existing.fingerprint != pin.fingerprint || existing.profile != pin.profile {
            return Err(Error::new(
                "this task is already pinned to a different auth profile or account; submit a new task",
            ));
        }
    }
    Ok(())
}

/// Check both the project's current account policy and the account frozen for
/// this exact task. Rebinding the project can never migrate an old task.
pub fn verify_task_resume(repo: &Repo, harness: &str, task_dir: &Path) -> Result<()> {
    verify_task_resume_for_model(repo, harness, "", task_dir)
}

pub fn verify_task_resume_for_model(
    repo: &Repo,
    harness: &str,
    model: &str,
    task_dir: &Path,
) -> Result<()> {
    let path = binding_path_without_creation(repo)?;
    crate::state::confine_file(&path)?;
    if !path.exists() {
        return Ok(());
    }
    let bindings = load(repo, &path)?;
    if bindings.profiles.is_empty() {
        return Ok(());
    }
    let Some(binding_harness) = task_binding_harness(harness, model) else {
        return Err(Error::new(format!(
            "ahu cannot verify the signed-in principal for {harness}; task resume refused before starting the harness"
        )));
    };
    let identity = probe(binding_harness, &repo.root)?;
    verify_task_resume_identity(harness, binding_harness, task_dir, &bindings, &identity)
}

fn verify_task_resume_identity(
    harness: &str,
    binding_harness: &str,
    task_dir: &Path,
    bindings: &BindingFile,
    identity: &Identity,
) -> Result<()> {
    let fingerprint = identity.fingerprint()?;
    let expected = bindings
        .profiles
        .get(&bindings.active_profile)
        .and_then(|profile| profile.get(binding_harness))
        .ok_or_else(|| {
            Error::new(format!(
                "no {binding_harness} account is bound to active profile {:?}; resume refused",
                bindings.active_profile
            ))
        })?;
    if expected.fingerprint != fingerprint {
        return Err(Error::new(format!(
            "current {binding_harness} account does not match this repository's local binding; resume refused before starting the harness"
        )));
    }
    let pin_path = task_dir.join("auth-identity.json");
    let pin: StoredBinding = crate::state::read_json(&pin_path).map_err(|_| {
        Error::new("task has no valid account pin; resume refused, submit a new task")
    })?;
    if pin.fingerprint != fingerprint {
        return Err(Error::new(format!(
            "current {harness} account differs from the account that started this task; resume refused before starting the harness"
        )));
    }
    if pin.profile.as_deref() != Some(bindings.active_profile.as_str()) {
        return Err(Error::new(
            "the project's active auth profile changed since this task started; resume refused before starting the harness",
        ));
    }
    Ok(())
}

/// Ollama's `:cloud` and `-cloud` tags execute remotely and use the account
/// managed by the Ollama daemon. Other OpenCode providers remain unverified.
fn task_binding_harness(harness: &str, model: &str) -> Option<&'static str> {
    if harness != "opencode" {
        return Some(match harness {
            "codex" => "codex",
            "claude-code" => "claude-code",
            "antigravity" => "antigravity",
            _ => return None,
        });
    }
    let model = model.to_ascii_lowercase();
    let (provider, name) = model.split_once('/')?;
    (provider == "ollama" && (name.contains(":cloud") || name.ends_with("-cloud")))
        .then_some("ollama")
}

pub fn probe(harness: &str, cwd: &Path) -> Result<Identity> {
    match harness {
        "codex" => probe_codex(cwd),
        "claude-code" => probe_claude(cwd),
        "antigravity" => probe_antigravity(cwd),
        "ollama" => probe_ollama(),
        "opencode" => Err(Error::new(format!(
            "ahu cannot verify the signed-in principal for {harness}; resume is unsafe until that harness exposes a reliable read-only identity check"
        ))),
        _ => Err(Error::new(format!(
            "unsupported auth binding harness {harness:?}"
        ))),
    }
}

fn probe_ollama() -> Result<Identity> {
    let configured = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "127.0.0.1:11434".into());
    let endpoint = if configured.contains("://") {
        configured
    } else {
        format!("http://{configured}")
    };
    let mut base = reqwest::Url::parse(&endpoint)
        .map_err(|_| Error::new("OLLAMA_HOST is not a valid local server address"))?;
    let local = base
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|address| address.is_loopback())
        || matches!(base.host_str(), Some("localhost"));
    if !local || !matches!(base.scheme(), "http" | "https") {
        return Err(Error::new(
            "Ollama account binding only supports a loopback Ollama server",
        ));
    }
    base.set_path("/api/me");
    base.set_query(None);
    base.set_fragment(None);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(4))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| Error::new("cannot initialize the Ollama identity check"))?;
    let response = client
        .post(base)
        .json(&json!({}))
        .send()
        .map_err(|_| Error::new("cannot reach the local Ollama account identity endpoint"))?;
    if !response.status().is_success() {
        return Err(Error::new(
            "local Ollama did not provide a signed-in account identity",
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > 64 * 1024)
    {
        return Err(Error::new("Ollama account identity response is too large"));
    }
    let mut bytes = Vec::new();
    response
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::new("cannot read the Ollama account identity response"))?;
    if bytes.len() > 64 * 1024 {
        return Err(Error::new("Ollama account identity response is too large"));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::new("Ollama account identity response is invalid"))?;
    let principal = value
        .get("email")
        .and_then(Value::as_str)
        .filter(|email| email.contains('@') && !email.chars().any(char::is_control))
        .ok_or_else(|| Error::new("Ollama is not signed in or returned no account email"))?;
    let account_id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::new("Ollama returned no stable account identifier"))?;
    Ok(Identity {
        harness: "ollama".into(),
        identity_kind: "ollama-account".into(),
        principal: principal.into(),
        organization: None,
        workspace: Some(account_id.into()),
    })
}

fn probe_claude(cwd: &Path) -> Result<Identity> {
    let executable = crate::selection::resolve_executable("claude")
        .ok_or_else(|| Error::new("Claude Code is not available on PATH"))?;
    let output = bounded_command(&executable, &["auth", "status", "--json"], cwd)?;
    let value: Value = serde_json::from_slice(&output)
        .map_err(|_| Error::new("Claude auth status returned invalid JSON"))?;
    if value.get("loggedIn").and_then(Value::as_bool) != Some(true) {
        return Err(Error::new("Claude Code is not logged in"));
    }
    let principal = value
        .get("email")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Error::new("Claude auth status does not expose a verifiable account identity")
        })?;
    let org = value
        .get("orgId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let auth_method = value
        .get("authMethod")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    Ok(Identity {
        harness: "claude-code".into(),
        identity_kind: format!(
            "{auth_method}/{}",
            value
                .get("apiProvider")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ),
        principal: principal.into(),
        organization: org.map(str::to_owned),
        workspace: None,
    })
}

fn probe_codex(cwd: &Path) -> Result<Identity> {
    let executable = crate::selection::resolve_executable("codex")
        .ok_or_else(|| Error::new("Codex is not available on PATH"))?;
    let account = crate::harness::codex_metadata::read_account(&PathBuf::from(executable), cwd)?;
    // account/read returns an envelope with `account` and routing metadata;
    // tolerate a flattened shape as well for compatible app-server versions.
    let account_details = account
        .get("account")
        .filter(|value| value.is_object())
        .unwrap_or(&account);
    if account_details.get("type").and_then(Value::as_str) != Some("chatgpt") {
        return Err(Error::new(
            "Codex API-key authentication does not expose a verifiable user principal; account binding is unavailable",
        ));
    }
    let routing = account
        .get("workspaceRouting")
        .or_else(|| account_details.get("workspaceRouting"))
        .unwrap_or(&Value::Null);
    let principal = account_details
        .get("email")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| routing.get("chatgptAccountId").and_then(Value::as_str))
        .ok_or_else(|| Error::new("Codex account metadata has no verifiable account identifier"))?;
    Ok(Identity {
        harness: "codex".into(),
        identity_kind: account_details
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .into(),
        principal: principal.into(),
        organization: None,
        workspace: match (
            routing.get("chatgptAccountId").and_then(Value::as_str),
            routing.get("backendOrigin").and_then(Value::as_str),
        ) {
            (Some(id), Some(origin)) => Some(format!("{id}@{origin}")),
            (Some(id), None) => Some(id.to_owned()),
            (None, Some(origin)) => Some(origin.to_owned()),
            (None, None) => None,
        },
    })
}

/// Antigravity has no standalone account-status command, but its startup TUI
/// renders the signed-in account before a prompt can be entered. Run it in a
/// bounded pseudo-terminal, capture only that startup text, and terminate it
/// as soon as an account email appears. No prompt is sent and no model request
/// is made. If the UI stops exposing a recognizable identity, fail closed.
fn probe_antigravity(cwd: &Path) -> Result<Identity> {
    let executable = crate::selection::resolve_executable("agy")
        .ok_or_else(|| Error::new("Antigravity CLI is not available on PATH"))?;
    let output = antigravity_startup(&executable, cwd)?;
    let principal = extract_email(&output).ok_or_else(|| {
        Error::new("Antigravity startup did not expose a signed-in account email")
    })?;
    let visible = String::from_utf8_lossy(&output);
    let identity_kind = if visible.contains("Agent Platform") {
        "agent-platform"
    } else if visible.contains("Gemini API key") {
        return Err(Error::new(
            "Antigravity is using API-key authentication and exposes no user principal",
        ));
    } else {
        "antigravity-account"
    };
    Ok(Identity {
        harness: "antigravity".into(),
        identity_kind: identity_kind.into(),
        principal,
        organization: None,
        workspace: extract_labeled_value(&visible, "GCP Project:"),
    })
}

fn antigravity_startup(executable: &str, cwd: &Path) -> Result<Vec<u8>> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut master_fd = -1;
    let mut slave_fd = -1;
    let mut size = libc::winsize {
        ws_row: 30,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty initializes two owned descriptors, which are wrapped
    // immediately below. No name or termios override is requested.
    if unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    } != 0
    {
        return Err(Error::new(
            "cannot create bounded Antigravity status terminal",
        ));
    }
    // SAFETY: each descriptor was freshly returned by openpty and has one owner.
    let mut master = unsafe { std::fs::File::from_raw_fd(master_fd) };
    // SAFETY: each descriptor was freshly returned by openpty and has one owner.
    let slave = unsafe { std::fs::File::from_raw_fd(slave_fd) };
    let stdin = slave.try_clone()?;
    let stdout = slave.try_clone()?;
    let mut command = Command::new(executable);
    command
        .current_dir(cwd)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(slave));
    crate::cmux::integration::sanitize(&mut command, None);
    command.env("TERM", "xterm-256color").process_group(0);
    let mut child = command
        .spawn()
        .map_err(|_| Error::new("cannot start Antigravity identity probe"))?;
    let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        crate::headless::signal_group(child.id(), libc::SIGKILL);
        let _ = child.wait();
        return Err(Error::new("cannot bound Antigravity startup reads"));
    }
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut output = Vec::new();
    let mut buffer = [0u8; 4096];
    let outcome = loop {
        if extract_email(&output).is_some() {
            break Ok(output);
        }
        if output.len() >= 64 * 1024 {
            break Err(Error::new(
                "Antigravity startup exceeded the identity output limit",
            ));
        }
        if Instant::now() >= deadline {
            break Err(Error::new("Antigravity startup identity check timed out"));
        }
        let mut descriptor = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut descriptor, 1, 150) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break Err(Error::new("cannot wait for Antigravity startup identity"));
        }
        if ready > 0 {
            match master.read(&mut buffer) {
                Ok(0) => break Err(Error::new("Antigravity closed its startup terminal")),
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => (),
                Err(_) => break Err(Error::new("cannot read Antigravity startup identity")),
            }
        }
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                break Err(Error::new(
                    "Antigravity exited before exposing its account identity",
                ));
            }
            Err(_) => break Err(Error::new("cannot inspect Antigravity probe process")),
            _ => (),
        }
    };
    crate::headless::signal_group(child.id(), libc::SIGKILL);
    let _ = child.wait();
    outcome
}

fn extract_email(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    for token in
        text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>' | '"' | '\''))
    {
        let token = token.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | ']' | '['));
        let Some((local, domain)) = token.split_once('@') else {
            continue;
        };
        if local.is_empty()
            || domain.is_empty()
            || !domain.contains('.')
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-' | b'@'))
        {
            continue;
        }
        return Some(token.to_owned());
    }
    None
}

fn extract_labeled_value(text: &str, label: &str) -> Option<String> {
    text.split_once(label)?
        .1
        .lines()
        .next()?
        .split_whitespace()
        .next()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn bounded_command(executable: &str, args: &[&str], cwd: &Path) -> Result<Vec<u8>> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::cmux::integration::sanitize(&mut command, None);
    let mut child = command
        .spawn()
        .map_err(|_| Error::new("cannot start read-only harness auth status"))?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::new("harness auth status timed out"));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        return Err(Error::new("harness auth status failed; details omitted"));
    }
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .ok_or_else(|| Error::new("missing auth status output"))?
        .take(64 * 1024 + 1)
        .read_to_end(&mut output)?;
    if output.len() > 64 * 1024 {
        return Err(Error::new("harness auth status exceeded the output limit"));
    }
    Ok(output)
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    fn repo_fixture() -> (tempfile::TempDir, Repo) {
        let temp = tempfile::tempdir().unwrap();
        crate::git::run_ok(temp.path(), &["init", "-q"]).unwrap();
        let repo = crate::git::discover(temp.path()).unwrap();
        (temp, repo)
    }

    #[test]
    fn schema_one_bindings_migrate_into_the_default_profile() {
        let (_temp, repo) = repo_fixture();
        let path = binding_path(&repo).unwrap();
        let legacy = json!({
            "schema_version": 1,
            "repo_identity": repo.identity(),
            "bindings": {
                "codex": {"fingerprint": "a".repeat(64), "identity_kind": "chatgpt"}
            }
        });
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

        let migrated = load(&repo, &path).unwrap();
        assert_eq!(migrated.schema_version, 2);
        assert_eq!(migrated.active_profile, "default");
        assert_eq!(
            migrated.profiles["default"]["codex"].fingerprint,
            "a".repeat(64)
        );
        let saved: Value = crate::state::read_json(&path).unwrap();
        assert_eq!(saved["schema_version"], 2);
        assert!(saved["content_digest"].as_str().is_some());
    }

    #[test]
    fn profile_changes_require_auth_cli_to_refresh_the_digest() {
        let (_temp, repo) = repo_fixture();
        let path = binding_path(&repo).unwrap();
        let state = BindingFile {
            repo_identity: repo.identity(),
            profiles: BTreeMap::from([(
                "personal".into(),
                BTreeMap::from([(
                    "codex".into(),
                    StoredBinding {
                        fingerprint: "b".repeat(64),
                        identity_kind: "chatgpt".into(),
                        profile: None,
                    },
                )]),
            )]),
            ..BindingFile::default()
        };
        save(&path, &state).unwrap();
        select_profile(&repo, "personal").unwrap();
        assert_eq!(load(&repo, &path).unwrap().active_profile, "personal");

        let mut edited: Value = crate::state::read_json(&path).unwrap();
        edited["active_profile"] = Value::String("work".into());
        std::fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();
        assert!(
            load(&repo, &path)
                .unwrap_err()
                .to_string()
                .contains("changed outside `ahu auth`")
        );
    }

    #[test]
    fn readiness_does_not_disclose_principals_or_provider_diagnostics() {
        let (_temp, repo) = repo_fixture();
        let status = readiness(&repo, "opencode", Some("openai/model"));
        assert_eq!(status["identity"], "unsupported");
        assert_eq!(status["binding"], "unsupported");
        assert_eq!(status["ready"], false);
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains("@"));
        assert!(!serialized.contains("token"));
        assert!(!serialized.contains("auth.json"));
    }

    #[test]
    fn readiness_error_projection_contains_only_status_fields() {
        let status = readiness_error("codex", None, "unavailable", "unavailable");
        assert_eq!(
            status,
            json!({
                "schema_version": 1,
                "harness": "codex",
                "model": null,
                "identity": "unavailable",
                "binding": "unavailable",
                "ready": false,
            })
        );
    }

    #[test]
    fn long_paused_resume_refuses_a_changed_account_against_the_original_task_pin() {
        let (_temp, repo) = repo_fixture();
        let task_dir = tempfile::tempdir().unwrap();
        let original = Identity {
            harness: "claude-code".into(),
            identity_kind: "oauth/first-party".into(),
            principal: "personal@example.invalid".into(),
            organization: Some("personal-org".into()),
            workspace: None,
        };
        let changed = Identity {
            principal: "work@example.invalid".into(),
            organization: Some("work-org".into()),
            ..original.clone()
        };
        let bindings = BindingFile {
            repo_identity: repo.identity(),
            profiles: BTreeMap::from([(
                "default".into(),
                BTreeMap::from([(
                    "claude-code".into(),
                    StoredBinding {
                        fingerprint: original.fingerprint().unwrap(),
                        identity_kind: original.identity_kind.clone(),
                        profile: None,
                    },
                )]),
            )]),
            ..BindingFile::default()
        };
        save(&binding_path(&repo).unwrap(), &bindings).unwrap();
        crate::state::write_json(
            &task_dir.path().join("auth-identity.json"),
            &StoredBinding {
                fingerprint: original.fingerprint().unwrap(),
                identity_kind: original.identity_kind.clone(),
                profile: Some("default".into()),
            },
        )
        .unwrap();

        let error = verify_task_resume_identity(
            "claude-code",
            "claude-code",
            task_dir.path(),
            &bindings,
            &changed,
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not match"));
        let pin: StoredBinding =
            crate::state::read_json(&task_dir.path().join("auth-identity.json")).unwrap();
        assert_eq!(pin.fingerprint, original.fingerprint().unwrap());
    }

    #[test]
    fn ollama_cloud_open_code_models_resolve_to_ollama_identity() {
        assert_eq!(
            task_binding_harness("opencode", "ollama/glm-5.3:cloud"),
            Some("ollama")
        );
        assert_eq!(
            task_binding_harness("opencode", "ollama/gpt-oss:120b-cloud"),
            Some("ollama")
        );
        assert_eq!(
            task_binding_harness("opencode", "ollama/qwen3.6:35b-mlx"),
            None
        );
    }

    #[test]
    fn interactive_startup_keeps_legacy_behavior_without_profiles() {
        let (_temp, repo) = repo_fixture();
        let task_dir = tempfile::tempdir().unwrap();

        capture_task_for_model(&repo, "opencode", "ollama/glm-5.3:cloud", task_dir.path()).unwrap();
        verify_task_resume_for_model(&repo, "opencode", "ollama/glm-5.3:cloud", task_dir.path())
            .unwrap();

        assert!(!task_dir.path().join("auth-identity.json").exists());
    }
}
