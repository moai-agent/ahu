//! Local, per-repository harness account bindings used to guard session resume.
//!
//! The binding contains only a digest of provider identity metadata. It never
//! stores credentials. A provider without a verifiable principal is refused.

use std::collections::BTreeMap;
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

/// Provider identity observation. Raw principals remain inside the plugin.
#[derive(Debug, Clone)]
pub struct Identity {
    pub harness: String,
    pub identity_kind: String,
    fingerprint: String,
}
impl Identity {
    fn fingerprint(&self) -> Result<String> {
        if !valid_fingerprint(&self.fingerprint) {
            return Err(invalid_observation());
        }
        Ok(self.fingerprint.clone())
    }
    pub fn display(&self) -> String {
        format!(
            "{} ({}; account fingerprint verified)",
            crate::util::display_safe(&self.harness),
            crate::util::display_safe(&self.identity_kind)
        )
    }
}
fn valid_fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn invalid_observation() -> Error {
    Error::new("native identity or capacity observation is unavailable or invalid")
}

pub fn budget(repo: &Repo) -> Result<Value> {
    let descriptors = crate::harness_runtime::discover(repo)?;
    let harnesses = descriptors.into_iter().map(|d| d.id).collect::<Vec<_>>();
    budget_with_probe(repo, &harnesses, |harness| {
        let runtime = crate::harness_runtime::load(repo, harness)?;
        let mut params = crate::harness_runtime::native_params(repo, runtime.descriptor())?;
        params["environment_names"] = json!(
            std::env::vars_os()
                .filter_map(|(name, _)| name.into_string().ok())
                .collect::<Vec<_>>()
        );
        params["requested_identity"] = json!(harness);
        runtime.call("budget", params)
    })
}
fn budget_with_probe(
    repo: &Repo,
    harnesses: &[String],
    mut probe_capacity: impl FnMut(&str) -> Result<Value>,
) -> Result<Value> {
    let path = binding_path_without_creation(repo)?;
    crate::state::confine_file(&path)?;
    let bindings = match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BindingFile::default(),
        Err(_) => return Err(invalid_observation()),
        Ok(_) => load_with_migration(repo, &path, false)?,
    };
    let profile = bindings.profiles.get(&bindings.active_profile);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| invalid_observation())?
        .as_secs();
    let mut providers = Vec::new();
    let mut selected = harnesses
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    if let Some(profile) = profile {
        selected.extend(profile.keys().cloned());
    }
    for harness in &selected {
        let binding = profile.and_then(|p| p.get(harness));
        let mut signal = json!({"harness":harness,"status":if profile.is_none(){"not_configured"}else{"not_bound"},"signal_type":"unknown","source":"none","scope":"shared_account","project_identity":repo.identity(),"observed_at_unix":now,"freshness":"unavailable","data":null,"windows":[]});
        if let Some(binding) = binding {
            let value = probe_capacity(harness)?;
            let observed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| invalid_observation())?
                .as_secs();
            let observation = validate_capacity(value, observed)?;
            signal["observed_at_unix"] = json!(observed);
            if observation
                .binding_harness
                .as_ref()
                .is_some_and(|key| key != harness)
            {
                return Err(invalid_observation());
            }
            if (observation.status == "available" || observation.status == "unknown")
                && observation.fingerprint.as_deref() != Some(&binding.fingerprint)
            {
                return Err(Error::new(
                    "current native account does not match the active auth profile; signal omitted",
                ));
            }
            signal["status"] = json!(observation.status);
            signal["source"] = json!(observation.source.unwrap_or_else(|| "none".into()));
            if observation.status == "available" {
                signal["signal_type"] = json!("rate_windows");
                signal["freshness"] = json!("queried_now; do_not_reuse_for_admission");
                signal["windows"] = json!(observation.windows);
                signal["data"] =
                    json!({"windows":signal["windows"],"semantics":"time_scoped_percentage"});
            }
        }
        providers.push(signal);
    }
    Ok(
        json!({"schema_version":1,"capacity_schema_version":1,"signal":"provider_rate_limit_windows","token_counts":"not_provided","shared_account_capacity":true,"profile":profile.map(|_|bindings.active_profile.clone()),"project":repo.identity(),"observed_at_unix":now,"providers":providers}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capacity {
    status: String,
    signal_type: String,
    windows: Vec<Value>,
    fingerprint: Option<String>,
    binding_harness: Option<String>,
    source: Option<String>,
    scope: Option<String>,
    observed_at_unix: Option<u64>,
}
fn validate_capacity(value: Value, now: u64) -> Result<Capacity> {
    if serde_json::to_vec(&value)?.len() > 65536 {
        return Err(invalid_observation());
    }
    let capacity: Capacity = serde_json::from_value(value).map_err(|_| invalid_observation())?;
    if !["available", "unknown", "unsupported"].contains(&capacity.status.as_str())
        || capacity.windows.len() > 64
        || capacity
            .fingerprint
            .as_ref()
            .is_some_and(|s| !valid_fingerprint(s))
        || capacity
            .binding_harness
            .as_ref()
            .is_some_and(|s| !crate::util::is_safe_name(s))
        || capacity
            .source
            .as_ref()
            .is_some_and(|s| s.len() > 128 || s.chars().any(char::is_control))
        || capacity
            .scope
            .as_ref()
            .is_some_and(|s| s != "shared_account")
        || capacity
            .observed_at_unix
            .is_some_and(|t| t > now.saturating_add(60))
    {
        return Err(invalid_observation());
    }
    if capacity.status != "available" {
        if !capacity.windows.is_empty() || capacity.signal_type != "unknown" {
            return Err(invalid_observation());
        }
        return Ok(capacity);
    }
    if capacity.signal_type != "rate_windows"
        || capacity.windows.is_empty()
        || capacity.fingerprint.is_none()
    {
        return Err(invalid_observation());
    }
    let mut names = std::collections::BTreeSet::new();
    for window in &capacity.windows {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Window {
            name: String,
            used_percent: f64,
            remaining_percent: f64,
            window_duration_minutes: u64,
            resets_at_unix: Option<u64>,
        }
        let w: Window =
            serde_json::from_value(window.clone()).map_err(|_| invalid_observation())?;
        if !crate::util::is_safe_name(&w.name)
            || !names.insert(w.name)
            || !w.used_percent.is_finite()
            || !w.remaining_percent.is_finite()
            || !(0.0..=100.0).contains(&w.used_percent)
            || !(0.0..=100.0).contains(&w.remaining_percent)
            || (w.used_percent + w.remaining_percent - 100.0).abs() > 0.000001
            || !(1..=525600).contains(&w.window_duration_minutes)
            || w.resets_at_unix.is_some_and(|t| t <= now)
        {
            return Err(invalid_observation());
        }
    }
    Ok(capacity)
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
    load_with_migration(repo, path, true)
}

fn load_with_migration(repo: &Repo, path: &Path, persist_migration: bool) -> Result<BindingFile> {
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
    if migrate_legacy && persist_migration {
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
    crate::util::require_host_operator("ahu auth bind and ahu auth select")?;
    let path = binding_path(repo)?;
    let bindings = load(repo, &path)?;
    bind_profile(repo, harness, &bindings.active_profile, replace)
}

pub fn bind_profile(repo: &Repo, harness: &str, profile: &str, replace: bool) -> Result<String> {
    crate::util::require_host_operator("ahu auth bind and ahu auth select")?;
    validate_profile_name(profile)?;
    let identity = probe(harness, &repo.root)?;
    let path = binding_path(repo)?;
    let mut bindings = load(repo, &path)?;
    let fingerprint = identity.fingerprint()?;
    let profile_bindings = bindings.profiles.entry(profile.to_owned()).or_default();
    if let Some(existing) = profile_bindings.get(&identity.harness) {
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
        identity.harness.clone(),
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
    readiness_with_probe(repo, harness, model, |h, cwd| probe_model(h, cwd, model))
}

fn readiness_with_probe(
    repo: &Repo,
    harness: &str,
    model: Option<&str>,
    probe_identity: impl FnOnce(&str, &Path) -> Result<Identity>,
) -> Value {
    let path = match binding_path_without_creation(repo) {
        Ok(path) => path,
        Err(_) => return readiness_error(harness, model, "unavailable", "unavailable"),
    };
    if crate::state::confine_file(&path).is_err() {
        return readiness_error(harness, model, "unavailable", "unavailable");
    }
    let bindings = if path.exists() {
        match load_with_migration(repo, &path, false) {
            Ok(bindings) => Some(bindings),
            Err(_) => return readiness_error(harness, model, "unavailable", "unavailable"),
        }
    } else {
        None
    };
    let identity = match probe_identity(harness, &repo.root) {
        Ok(identity) => identity,
        Err(error) => {
            let mut value = readiness_error(harness, model, "unavailable", "unavailable");
            // Only a closed vocabulary is ever added; provider text never is.
            if let Some(cause) = crate::harness_runtime::HealthCause::of(&error) {
                value["cause"] = json!(cause.as_str());
            }
            return value;
        }
    };
    let binding_harness = identity.harness.as_str();
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
        .and_then(|items| items.get(&identity.harness))
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
    crate::util::require_host_operator("ahu auth bind and ahu auth select")?;
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
    let identity = probe_model(harness, &repo.root, Some(model))?;
    let expected = bindings
        .profiles
        .get(&bindings.active_profile)
        .and_then(|p| p.get(&identity.harness))
        .ok_or_else(invalid_observation)?;
    if expected.fingerprint != identity.fingerprint()? {
        return Err(Error::new(
            "current native account does not match the active profile",
        ));
    }
    Ok(())
}

/// Freeze the verified account used by the first attempt. This is called after
/// ahu has validated its task record and immediately before it starts the
/// harness, before any project prompt is sent to the provider.
pub fn capture_task(repo: &Repo, harness: &str, task_dir: &Path) -> Result<()> {
    capture_task_for_model(repo, harness, "", task_dir)
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
    let identity = probe_model(harness, &repo.root, Some(model))?;
    capture_task_for(repo, &identity, task_dir)
}

/// Work mode deliberately leaves account selection to the native harness.
/// Persist that uncertainty without probing, copying, or exposing credentials.
pub fn capture_unverified_task(task_dir: &Path) -> Result<()> {
    let pin_path = task_dir.join("auth-identity.json");
    crate::state::confine_file(&pin_path)?;
    let pin = StoredBinding {
        fingerprint: "unverified".into(),
        identity_kind: "work-mode-unverified-provider-identity".into(),
        profile: None,
    };
    let bytes = serde_json::to_vec(&pin)?;
    let _ = crate::private_io::atomic_create(
        &pin_path,
        &bytes,
        crate::private_io::Durability::Durable,
    )?;
    Ok(())
}

/// Pin interactive task startup to the selected project profile when the
/// project has opted into auth bindings. Projects without profiles retain the
/// existing interactive behavior. Headless startup also checks bindings only
/// after the project opts into profiles.
pub fn capture_interactive_task_for_model(
    repo: &Repo,
    harness: &str,
    model: &str,
    task_dir: &Path,
) -> Result<()> {
    capture_task_for_model(repo, harness, model, task_dir)
}

fn capture_task_for(repo: &Repo, identity: &Identity, task_dir: &Path) -> Result<()> {
    let binding_harness = identity.harness.as_str();
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
        identity_kind: identity.identity_kind.clone(),
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
    let identity = probe_model(harness, &repo.root, Some(model))?;
    verify_task_resume_identity(harness, &identity.harness, task_dir, &bindings, &identity)
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

pub fn probe(harness: &str, cwd: &Path) -> Result<Identity> {
    probe_model(harness, cwd, None)
}
fn probe_model(harness: &str, cwd: &Path, model: Option<&str>) -> Result<Identity> {
    let repo = crate::git::discover(cwd)?;
    let runtime = crate::harness_runtime::load(&repo, harness)?;
    let mut params = crate::harness_runtime::native_params(&repo, runtime.descriptor())?;
    params["environment_names"] = json!(
        std::env::vars_os()
            .filter_map(|(name, _)| name.into_string().ok())
            .collect::<Vec<_>>()
    );
    params["requested_identity"] = json!(harness);
    if let Some(model) = model {
        params["model"] = json!(model);
    }
    validate_identity(runtime.call("auth", params)?)
}
fn validate_identity(value: Value) -> Result<Identity> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Observation {
        status: String,
        binding_harness: Option<String>,
        fingerprint: Option<String>,
        identity_kind: Option<String>,
        effective_provider_address_verified: Option<bool>,
    }
    if serde_json::to_vec(&value)?.len() > 4096 {
        return Err(invalid_observation());
    }
    let observation: Observation =
        serde_json::from_value(value).map_err(|_| invalid_observation())?;
    if observation.status != "available" {
        return Err(invalid_observation());
    }
    let harness = observation
        .binding_harness
        .ok_or_else(invalid_observation)?;
    let fingerprint = observation.fingerprint.ok_or_else(invalid_observation)?;
    let identity_kind = observation
        .identity_kind
        .unwrap_or_else(|| "native-account".into());
    if !crate::util::is_safe_name(&harness)
        || !valid_fingerprint(&fingerprint)
        || identity_kind.is_empty()
        || identity_kind.len() > 128
        || identity_kind.chars().any(char::is_control)
    {
        return Err(invalid_observation());
    }
    // A provider can disclose that address routing remains unverified; this is
    // not a claim that identity probing locks or reserves the native account.
    let _routing_evidence = observation.effective_provider_address_verified;
    Ok(Identity {
        harness,
        identity_kind,
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repo_fixture() -> (tempfile::TempDir, Repo) {
        let temp = tempfile::tempdir().unwrap();
        crate::git::run_ok(temp.path(), &["init", "-q"]).unwrap();
        let repo = crate::git::discover(temp.path()).unwrap();
        (temp, repo)
    }

    #[test]
    fn plugin_observations_reject_raw_identity_fields_and_invalid_capacity() {
        let valid = json!({"status":"available","binding_harness":"invented","fingerprint":"a".repeat(64),"identity_kind":"synthetic"});
        assert!(validate_identity(valid.clone()).is_ok());
        let mut raw = valid;
        raw["principal"] = json!("never disclose");
        assert!(validate_identity(raw).is_err());
        assert!(validate_identity(json!({"status":"available","binding_harness":"invented","fingerprint":"not-a-hash"})).is_err());
        let window = json!({"name":"primary","used_percent":20.0,"remaining_percent":80.0,"window_duration_minutes":60,"resets_at_unix":200});
        let capacity = json!({"status":"available","signal_type":"rate_windows","fingerprint":"a".repeat(64),"windows":[window]});
        assert!(validate_capacity(capacity.clone(), 100).is_ok());
        assert!(validate_capacity(capacity.clone(), 200).is_err());
        let mut duplicate = capacity;
        let window = duplicate["windows"][0].clone();
        duplicate["windows"].as_array_mut().unwrap().push(window);
        assert!(validate_capacity(duplicate, 100).is_err());
    }

    #[test]
    fn schema_one_bindings_migrate_into_the_default_profile() {
        let (_temp, repo) = repo_fixture();
        let path = binding_path(&repo).unwrap();
        let legacy = json!({
            "schema_version": 1,
            "repo_identity": repo.identity(),
            "bindings": {
                "invented": {"fingerprint": "a".repeat(64), "identity_kind": "chatgpt"}
            }
        });
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

        let migrated = load(&repo, &path).unwrap();
        assert_eq!(migrated.schema_version, 2);
        assert_eq!(migrated.active_profile, "default");
        assert_eq!(
            migrated.profiles["default"]["invented"].fingerprint,
            "a".repeat(64)
        );
        let saved: Value = crate::state::read_json(&path).unwrap();
        assert_eq!(saved["schema_version"], 2);
        assert!(saved["content_digest"].as_str().is_some());
    }

    #[test]
    fn profile_changes_require_auth_cli_to_refresh_the_digest() {
        // select_profile refuses hosted sessions by design; the assertions
        // below need a host operator context.
        if [
            "AHU_WORKER_SESSION",
            "AHU_TASK_ID",
            "AHU_TASK_DIR",
            "AHU_PARENT_TASK",
            "AHU_BROKER_TOKEN",
        ]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
        {
            eprintln!(
                "skipping profile digest refresh test: ahu worker environment present; \
                 select_profile requires a host operator context"
            );
            return;
        }
        let (_temp, repo) = repo_fixture();
        let path = binding_path(&repo).unwrap();
        let state = BindingFile {
            repo_identity: repo.identity(),
            profiles: BTreeMap::from([(
                "personal".into(),
                BTreeMap::from([(
                    "invented".into(),
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
        let status = readiness_with_probe(&repo, "invented", Some("exact-model"), |_, _| {
            Err(Error::new("private-diagnostic"))
        });
        assert_eq!(status["identity"], "unavailable");
        assert_eq!(status["binding"], "unavailable");
        assert_eq!(status["ready"], false);
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains("@"));
        assert!(!serialized.contains("token"));
        assert!(!serialized.contains("auth.json"));
    }

    #[test]
    fn readiness_preserves_legacy_binding_bytes() {
        let (_temp, repo) = repo_fixture();
        let path = binding_path(&repo).unwrap();
        let legacy = br#"{"schema_version":1,"bindings":{}}"#;
        std::fs::write(&path, legacy).unwrap();
        let status = readiness_with_probe(&repo, "invented", None, |_, _| {
            Err(Error::new("synthetic unavailable identity"))
        });
        assert_eq!(status["ready"], false);
        assert_eq!(std::fs::read(&path).unwrap(), legacy);
    }

    #[test]
    fn readiness_error_projection_contains_only_status_fields() {
        let status = readiness_error("invented", None, "unavailable", "unavailable");
        assert_eq!(
            status,
            json!({
                "schema_version": 1,
                "harness": "invented",
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
            harness: "invented".into(),
            identity_kind: "oauth/first-party".into(),
            fingerprint: "a".repeat(64),
        };
        let changed = Identity {
            fingerprint: "b".repeat(64),
            ..original.clone()
        };
        let bindings = BindingFile {
            repo_identity: repo.identity(),
            profiles: BTreeMap::from([(
                "default".into(),
                BTreeMap::from([(
                    "invented".into(),
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
            "invented",
            "invented",
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
    fn interactive_startup_keeps_legacy_behavior_without_profiles() {
        let (_temp, repo) = repo_fixture();
        let task_dir = tempfile::tempdir().unwrap();

        capture_task_for_model(&repo, "invented", "exact-model", task_dir.path()).unwrap();
        verify_task_resume_for_model(&repo, "invented", "exact-model", task_dir.path()).unwrap();

        assert!(!task_dir.path().join("auth-identity.json").exists());
    }
}
