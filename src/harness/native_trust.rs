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
    let common = json!({"home":crate::harness_runtime::native_home()?,"environment":{}});
    if common.get("home").and_then(Value::as_str) != frozen.home.to_str() {
        return Err(refused());
    }
    validate_result(runtime.call(
        "trust_cleanup",
        trust_params(dir, record, &frozen, &common)?,
    )?)
}
fn trust_params(dir: &Path, record: &TaskRecord, frozen: &Frozen, common: &Value) -> Result<Value> {
    let environment = common
        .get("environment")
        .and_then(Value::as_object)
        .ok_or_else(refused)?;
    Ok(
        json!({"task_dir":dir,"worktree":record.worktree,"home":frozen.home,"permissions":record.identity.permissions,"mode":"interactive","overrides":environment.keys().collect::<Vec<_>>()}),
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
