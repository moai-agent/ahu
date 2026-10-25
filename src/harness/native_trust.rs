//! Task ownership and frozen runtime binding for provider-owned trust receipts.
//! Native trust formats and reconciliation live exclusively in the runtime.
use crate::task::TaskRecord;
use crate::util::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
const BINDING: &str = "native-runtime-trust-binding.json";
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Frozen {
    schema: u8,
    task_id: String,
    task_dir: PathBuf,
    repo_identity: String,
    harness: String,
    worktree: PathBuf,
    runtime_digest: String,
    trust_supported: bool,
    home: PathBuf,
    task_identity_digest: String,
}
fn refused() -> Error {
    Error::new(
        "native task trust requires its original verified task and installed runtime; no rebinding or fallback",
    )
}
fn verify_scope(
    dir: &Path,
    worktree: &Path,
    task_id: &str,
    repo_identity: &str,
) -> Result<crate::git::Repo> {
    let repo = crate::git::discover(worktree)?;
    if repo.identity() != repo_identity
        || repo.root.canonicalize()? != worktree
        || crate::state::worktree_dir(&repo.root, task_id)? != worktree
        || worktree.canonicalize()? != worktree
        || crate::state::enclosing_checkout(dir).as_deref() != Some(worktree)
    {
        return Err(refused());
    }
    crate::state::verify_worktree_inside_repo(&repo.root, worktree)?;
    crate::state::confine_existing_dir(dir)?;
    Ok(repo)
}
pub(crate) fn prepare(dir: &Path, record: &TaskRecord) -> Result<()> {
    let repo = verify_scope(
        dir,
        &record.worktree,
        &record.task_id,
        &record.repo_identity,
    )?;
    let runtime = crate::harness_runtime::load(&repo, &record.identity.harness)?;
    if record.launch_command.planner_binding_digest.as_deref()
        != Some(runtime.binding_digest().as_str())
    {
        return Err(refused());
    }
    let common = crate::harness_runtime::native_params(&repo, runtime.descriptor())?;
    let home = common
        .get("home")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(refused)?;
    let frozen = Frozen {
        schema: 1,
        task_id: record.task_id.clone(),
        task_dir: dir.canonicalize()?,
        repo_identity: record.repo_identity.clone(),
        harness: record.identity.harness.clone(),
        worktree: record.worktree.clone(),
        runtime_digest: runtime.binding_digest(),
        trust_supported: runtime
            .descriptor()
            .operations
            .contains(&crate::harness_runtime::Operation::TrustPrepare),
        home,
        task_identity_digest: crate::util::digest_bytes(&serde_json::to_vec(&record.identity)?),
    };
    if frozen.trust_supported
        && !runtime
            .descriptor()
            .operations
            .contains(&crate::harness_runtime::Operation::TrustCleanup)
    {
        return Err(refused());
    }
    let path = dir.join(BINDING);
    crate::state::confine_file(&path)?;
    let bytes = serde_json::to_vec(&frozen)?;
    if !crate::private_io::atomic_create(&path, &bytes, crate::private_io::Durability::Durable)? {
        let prior: Frozen = crate::state::read_json_strict(&path)?;
        if prior != frozen {
            return Err(refused());
        }
    }
    // Persist binding before dispatch. A lost response must retain this identity.
    if !frozen.trust_supported {
        return Ok(());
    }
    let params = trust_params(dir, record, &frozen, &common)?;
    validate_result(runtime.call("trust_prepare", params)?)
}
pub(crate) fn cleanup(dir: &Path, record: &TaskRecord) -> Result<()> {
    let path = dir.join(BINDING);
    crate::state::confine_file(&path)?;
    if std::fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        // Legacy tasks cannot be rebound to today's plugin. Their provider may
        // still own a receipt: require explicit legacy recovery, never guess.
        if std::fs::symlink_metadata(dir.join("native-trust.json"))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            return Ok(());
        }
        return Err(refused());
    }
    let frozen: Frozen = crate::state::read_json_strict(&path)?;
    if frozen.task_dir != dir.canonicalize()?
        || frozen.schema != 1
        || frozen.task_id != record.task_id
        || frozen.repo_identity != record.repo_identity
        || frozen.harness != record.identity.harness
        || frozen.worktree != record.worktree
        || frozen.task_identity_digest
            != crate::util::digest_bytes(&serde_json::to_vec(&record.identity)?)
    {
        return Err(refused());
    }
    if !frozen.trust_supported {
        return Ok(());
    }
    let repo = crate::git::discover(&record.repo_root)?;
    if repo.identity() != record.repo_identity {
        return Err(refused());
    }
    crate::state::confine_existing_dir(dir)?;
    let runtime = crate::harness_runtime::load(&repo, &frozen.harness)?;
    if runtime.binding_digest() != frozen.runtime_digest {
        return Err(refused());
    }
    let common = crate::harness_runtime::native_params(&repo, runtime.descriptor())?;
    if common.get("home").and_then(Value::as_str) != frozen.home.to_str() {
        return Err(refused());
    }
    validate_result(runtime.call(
        "trust_cleanup",
        trust_params(dir, record, &frozen, &common)?,
    )?)
}
fn trust_params(dir: &Path, record: &TaskRecord, frozen: &Frozen, common: &Value) -> Result<Value> {
    let _environment = common
        .get("environment")
        .and_then(Value::as_object)
        .ok_or_else(refused)?;
    Ok(
        json!({"task_dir":dir,"worktree":record.worktree,"home":frozen.home,"permissions":record.identity.permissions,"mode":"interactive","overrides":std::env::vars_os().filter_map(|(key,_)|key.into_string().ok()).collect::<Vec<_>>()}),
    )
}
fn validate_result(value: Value) -> Result<()> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Outcome {
        status: String,
        receipt_path: Option<PathBuf>,
    }
    let outcome: Outcome = serde_json::from_value(value).map_err(|_| refused())?;
    if !["complete", "not_applicable"].contains(&outcome.status.as_str())
        || outcome
            .receipt_path
            .as_ref()
            .is_some_and(|p| !p.is_absolute())
    {
        return Err(refused());
    }
    Ok(())
}
