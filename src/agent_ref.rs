//! Immutable repository-scoped references to resolved agent identities.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::agent::ResolvedAgent;
use crate::git::Repo;
use crate::util::{Error, Result};
use crate::{bail, state};

const MAX_ENTRY: u64 = 4096;
const PREFIX: &str = "ahu:agent:";

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    schema_version: u32,
    repo_identity: String,
    agent_id: String,
    identity_digest: String,
    name: String,
    version: String,
    harness: String,
    model: String,
    source_digest: String,
    instructions_digest: String,
}

fn root(repo: &Repo) -> Result<PathBuf> {
    Ok(state::coordination_dir(repo)?.join("agent-identities"))
}

fn paths(repo: &Repo, create: bool) -> Result<(PathBuf, PathBuf)> {
    let root = root(repo)?;
    if create {
        state::create_private_dir_all(&root)?;
        state::create_private_dir_all(&root.join("ids"))?;
        state::create_private_dir_all(&root.join("digests"))?;
    }
    state::confine_existing_dir(&root)?;
    state::confine_existing_dir(&root.join("ids"))?;
    state::confine_existing_dir(&root.join("digests"))?;
    Ok((root.join("ids"), root.join("digests")))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn read(path: &Path, repo: &Repo) -> Result<Option<Binding>> {
    if state::confine_file(path)?.is_none() {
        return Ok(None);
    }
    let mut file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    crate::storage::validate_owned_metadata(&meta, true)?;
    if meta.len() > MAX_ENTRY {
        bail!("agent identity binding exceeds 4 KiB");
    }
    let mut bytes = Vec::new();
    file.by_ref().take(MAX_ENTRY + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_ENTRY {
        bail!("agent identity binding exceeds 4 KiB");
    }
    let value: Binding = serde_json::from_slice(&bytes)?;
    if value.schema_version != 1
        || value.repo_identity != repo.identity()
        || !crate::task::is_canonical_task_uuid(&value.agent_id)
        || !valid_digest(&value.identity_digest)
        || !valid_digest(&value.source_digest)
        || !valid_digest(&value.instructions_digest)
        || value.name.is_empty()
        || value.version.is_empty()
        || value.harness.is_empty()
        || value.model.is_empty()
    {
        bail!("agent identity binding has invalid identity or schema");
    }
    Ok(Some(value))
}

fn create(path: &Path, value: &Binding) -> Result<bool> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    serde_json::to_writer(&mut file, value)?;
    file.sync_all()?;
    Ok(true)
}

fn binding(repo: &Repo, agent: &ResolvedAgent, id: String) -> Binding {
    Binding {
        schema_version: 1,
        repo_identity: repo.identity(),
        agent_id: id,
        identity_digest: agent.identity_digest(),
        name: agent.manifest.name.clone(),
        version: agent.manifest.version.clone(),
        harness: agent.manifest.harness.clone(),
        model: agent.manifest.model.clone(),
        source_digest: agent.source_digest.clone(),
        instructions_digest: agent.instructions_digest.clone(),
    }
}

fn matches_agent(value: &Binding, agent: &ResolvedAgent) -> bool {
    value.identity_digest == agent.identity_digest()
        && value.name == agent.manifest.name
        && value.version == agent.manifest.version
        && value.harness == agent.manifest.harness
        && value.model == agent.manifest.model
        && value.source_digest == agent.source_digest
        && value.instructions_digest == agent.instructions_digest
}

fn id_from_digest(digest: &str) -> String {
    // The content digest is already a repository-independent immutable key.
    // Formatting its first 128 bits as a UUID keeps references stable across
    // processes and linked checkouts without making the UUID an execution id.
    let mut hex = digest[..32].to_ascii_lowercase();
    hex.replace_range(12..13, "5");
    hex.replace_range(16..17, "8");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

pub fn display(id: &str) -> String {
    format!("{PREFIX}{id}")
}

pub fn parse(input: &str) -> Result<String> {
    let id = input.strip_prefix(PREFIX).ok_or_else(|| {
        Error::new("expected an agent reference: ahu:agent:<uuid>")
            .with_kind(crate::util::ErrorKind::Usage)
    })?;
    if !crate::task::is_canonical_task_uuid(id) {
        bail!(kind: crate::util::ErrorKind::Usage, "agent references require a canonical UUID");
    }
    Ok(id.to_string())
}

pub fn ensure(repo: &Repo, agent: &ResolvedAgent) -> Result<String> {
    let (ids, digests) = paths(repo, true)?;
    let digest = agent.identity_digest();
    let digest_path = digests.join(format!("{digest}.json"));
    if let Some(value) = read(&digest_path, repo)? {
        if !matches_agent(&value, agent) {
            bail!("agent identity digest binding is inconsistent; refusing to retarget it");
        }
        return Ok(display(&value.agent_id));
    }
    let id = id_from_digest(&digest);
    let value = binding(repo, agent, id.clone());
    if !create(&ids.join(format!("{id}.json")), &value)? {
        bail!("generated agent identity reference collided; retry");
    }
    if !create(&digest_path, &value)? {
        let existing = read(&digest_path, repo)?.ok_or_else(|| {
            Error::new("agent identity binding appeared without readable contents")
        })?;
        if !matches_agent(&existing, agent) {
            bail!("agent identity digest binding is inconsistent; refusing to retarget it");
        }
        return Ok(display(&existing.agent_id));
    }
    Ok(display(&id))
}

pub fn resolve(repo: &Repo, input: &str) -> Result<ResolvedAgent> {
    let id = parse(input)?;
    let (ids, _) = paths(repo, false)?;
    let value = read(&ids.join(format!("{id}.json")), repo)?.ok_or_else(|| {
        Error::new("unknown agent reference in this repository")
            .with_kind(crate::util::ErrorKind::Usage)
    })?;
    let agent = crate::agent::find(&repo.root, &value.name)?;
    if !matches_agent(&value, &agent) {
        bail!(
            "agent reference is stale: the registered agent identity changed; create a new reference"
        );
    }
    Ok(agent)
}
