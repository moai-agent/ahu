//! Generic runtime setup proposal facade. The setup coordinator owns publication.
use crate::git::Repo;
use crate::util::{Error, Result, digest_bytes};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub path: PathBuf,
    pub expected_before_sha256: Option<String>,
    pub text: String,
}
fn invalid() -> Error {
    Error::new(
        "native setup proposal is invalid, outside its authorized scope, or conflicts with current files",
    )
}
/// Return complete proposed writes. This function never writes native files.
/// `allowed_roots` must reflect the operator's setup scope, not plugin output.
pub fn plan(
    repo: &Repo,
    harness: &str,
    executable: &Path,
    skills: Value,
    allowed_roots: &[PathBuf],
) -> Result<Vec<Plan>> {
    let runtime = crate::harness_runtime::load(repo, harness)?;
    let home = crate::harness_runtime::native_home()?;
    let value = runtime.call(
        "setup",
        json!({"project":repo.root,"home":home,"ahu_executable":executable,"skills":skills}),
    )?;
    parse_proposals(value, allowed_roots)
}
pub fn parse_proposals(value: Value, allowed_roots: &[PathBuf]) -> Result<Vec<Plan>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        changes: Vec<Plan>,
        publication: String,
    }
    if serde_json::to_vec(&value)?.len() > 4 * 1024 * 1024 {
        return Err(invalid());
    }
    let response: Response = serde_json::from_value(value).map_err(|_| invalid())?;
    if response.publication != "host_transaction_required" || response.changes.len() > 256 {
        return Err(invalid());
    }
    let mut paths = std::collections::BTreeSet::new();
    for change in &response.changes {
        if !change.path.is_absolute()
            || change.path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            || !paths.insert(change.path.clone())
            || change.text.len() > 1024 * 1024
            || !allowed_roots.iter().any(|root| {
                root.is_absolute() && change.path.starts_with(root) && change.path != *root
            })
            || change
                .expected_before_sha256
                .as_ref()
                .is_some_and(|d| d.len() != 64 || !d.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(invalid());
        }
        change.check()?;
    }
    Ok(response.changes)
}
impl Plan {
    /// Recheck immediately before the setup coordinator's atomic transaction.
    pub fn check(&self) -> Result<()> {
        for ancestor in self.path.ancestors() {
            match std::fs::symlink_metadata(ancestor) {
                Ok(m) if m.file_type().is_symlink() => return Err(invalid()),
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(_) => return Err(invalid()),
            }
        }
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        let current = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)
        {
            Ok(file) => {
                let meta = file.metadata()?;
                if !meta.is_file() || meta.len() > 1024 * 1024 {
                    return Err(invalid());
                }
                let mut bytes = Vec::new();
                file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
                if bytes.len() > 1024 * 1024 {
                    return Err(invalid());
                }
                Some(digest_bytes(&bytes))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(invalid()),
        };
        if current != self.expected_before_sha256 {
            return Err(invalid());
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invented_proposals_refuse_escape_conflict_and_unknown_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let path = root.join("provider.conf");
        let value = json!({"changes":[{"path":path,"expected_before_sha256":null,"text":"synthetic"}],"publication":"host_transaction_required"});
        let plans = parse_proposals(value.clone(), std::slice::from_ref(&root)).unwrap();
        assert!(!path.exists());
        std::fs::write(&path, "concurrent").unwrap();
        assert!(plans[0].check().is_err());
        assert!(parse_proposals(value, &[]).is_err());
    }
}
