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
    let bytes = serde_json::to_vec(value)?;
    crate::private_io::atomic_create(path, &bytes, crate::private_io::Durability::Durable)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AGENTS_RELATIVE_DIR;

    fn git_cmd(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A repository with one registered agent whose body is its instructions.
    fn fixture() -> (tempfile::TempDir, Repo, ResolvedAgent) {
        let root = tempfile::tempdir().unwrap();
        git_cmd(root.path(), &["init", "-q", "-b", "main"]);
        let dir = root.path().join(AGENTS_RELATIVE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("builder.md"),
            format!(
                "---\nokf_version: {}\ntype: ahu:agent\ntitle: builder\nversion: \
                 1.0.0\ndescription: builds\nharness: claude-code\nmodel: \
                 claude-sonnet-5\n---\n\nDo the work.\n",
                crate::agent::OKF_VERSION
            ),
        )
        .unwrap();
        let repo = crate::git::discover(root.path()).unwrap();
        let agent = crate::agent::find(&repo.root, "builder").unwrap();
        (root, repo, agent)
    }

    fn binding_files(repo: &Repo, agent: &ResolvedAgent) -> (PathBuf, PathBuf) {
        let (ids, digests) = paths(repo, false).unwrap();
        let id = id_from_digest(&agent.identity_digest());
        (
            ids.join(format!("{id}.json")),
            digests.join(format!("{}.json", agent.identity_digest())),
        )
    }

    /// Overwrite a binding file in place, keeping the owner-only mode `read`
    /// requires so the test exercises validation rather than the metadata check.
    fn overwrite(path: &Path, contents: &[u8]) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::remove_file(path).unwrap();
        std::fs::write(path, contents).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn a_reference_displays_and_parses_only_as_a_canonical_agent_uuid() {
        let id = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5b";
        assert_eq!(display(id), format!("ahu:agent:{id}"));
        assert_eq!(parse(&display(id)).unwrap(), id);

        // A missing prefix is a usage error, not an unknown reference: the input
        // never named an agent reference at all.
        for input in ["", id, "ahu:task:018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5b"] {
            let error = parse(input).unwrap_err();
            assert_eq!(error.kind(), crate::util::ErrorKind::Usage);
            assert!(
                error.to_string().contains("expected an agent reference"),
                "{input:?} -> {error}"
            );
        }
        for input in [
            "ahu:agent:",
            "ahu:agent:not-a-uuid",
            "ahu:agent:018F1A2B-3C4D-7E5F-8A9B-0C1D2E3F4A5B",
        ] {
            let error = parse(input).unwrap_err();
            assert_eq!(error.kind(), crate::util::ErrorKind::Usage);
            assert!(
                error.to_string().contains("canonical UUID"),
                "{input:?} -> {error}"
            );
        }
    }

    #[test]
    fn an_identity_digest_becomes_a_canonical_uuid_without_becoming_an_execution_id() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let id = id_from_digest(digest);
        assert!(crate::task::is_canonical_task_uuid(&id));
        // The version and variant nibbles are pinned so the id is a valid UUID
        // rather than raw digest bytes in UUID shape.
        assert_eq!(id.as_bytes()[14], b'5');
        assert_eq!(id.as_bytes()[19], b'8');
        // Derivation is pure: the same digest always yields the same reference.
        assert_eq!(id_from_digest(digest), id);
        // An uppercase digest normalises to the same canonical id.
        assert_eq!(id_from_digest(&digest.to_ascii_uppercase()), id);
        assert_ne!(
            id_from_digest("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"),
            id
        );
    }

    #[test]
    fn valid_digest_accepts_only_lowercase_or_uppercase_sha256_hex() {
        assert!(valid_digest(&"a".repeat(64)));
        assert!(valid_digest(&"F".repeat(64)));
        assert!(!valid_digest(&"a".repeat(63)));
        assert!(!valid_digest(&"a".repeat(65)));
        assert!(!valid_digest(""));
        assert!(!valid_digest(&format!("{}z", "a".repeat(63))));
    }

    #[test]
    fn ensure_is_idempotent_and_writes_both_lookup_directions() {
        let (_root, repo, agent) = fixture();
        let reference = ensure(&repo, &agent).unwrap();
        let id = parse(&reference).unwrap();
        assert_eq!(id, id_from_digest(&agent.identity_digest()));

        let (id_path, digest_path) = binding_files(&repo, &agent);
        assert!(id_path.is_file());
        assert!(digest_path.is_file());
        // Both directions record the same binding, so either lookup agrees.
        assert_eq!(
            read(&id_path, &repo).unwrap().unwrap(),
            read(&digest_path, &repo).unwrap().unwrap()
        );

        // A second ensure returns the recorded reference from the digest lookup
        // rather than minting a second one.
        assert_eq!(ensure(&repo, &agent).unwrap(), reference);
        assert_eq!(
            resolve(&repo, &reference).unwrap().identity_digest(),
            agent.identity_digest()
        );
    }

    #[test]
    fn resolving_an_unbound_but_well_formed_reference_is_a_usage_error() {
        let (_root, repo, agent) = fixture();
        // The directories must exist before `paths(create: false)` will confine
        // them, so bind something else first.
        ensure(&repo, &agent).unwrap();
        let error = resolve(&repo, "ahu:agent:018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5b").unwrap_err();
        assert_eq!(error.kind(), crate::util::ErrorKind::Usage);
        assert!(
            error.to_string().contains("unknown agent reference"),
            "{error}"
        );
    }

    #[test]
    fn a_binding_larger_than_four_kibibytes_is_refused_before_it_is_parsed() {
        let (_root, repo, agent) = fixture();
        ensure(&repo, &agent).unwrap();
        let (id_path, _) = binding_files(&repo, &agent);

        let mut padded = read(&id_path, &repo).unwrap().unwrap();
        padded.name = "b".repeat(5000);
        overwrite(&id_path, &serde_json::to_vec(&padded).unwrap());

        let error = read(&id_path, &repo).unwrap_err();
        assert!(error.to_string().contains("exceeds 4 KiB"), "{error}");
    }

    #[test]
    fn a_binding_must_declare_this_schema_this_repository_and_well_formed_identity() {
        let (_root, repo, agent) = fixture();
        ensure(&repo, &agent).unwrap();
        let (id_path, _) = binding_files(&repo, &agent);
        let good = read(&id_path, &repo).unwrap().unwrap();
        let hex = "a".repeat(64);

        // Every field `read` validates, each broken on its own so no single
        // check can stand in for another.
        let broken = |field: &str| {
            let mut b = clone_binding(&good);
            match field {
                "schema_version" => b.schema_version = 2,
                "repo_identity" => b.repo_identity = "0123456789abcdef".to_string(),
                "agent_id" => b.agent_id = "not-a-uuid".to_string(),
                "identity_digest" => b.identity_digest = "short".to_string(),
                "source_digest" => b.source_digest = "short".to_string(),
                "instructions_digest" => b.instructions_digest = "short".to_string(),
                "name" => b.name = String::new(),
                "version" => b.version = String::new(),
                "harness" => b.harness = String::new(),
                "model" => b.model = String::new(),
                other => panic!("unhandled field {other}"),
            }
            b
        };
        for field in [
            "schema_version",
            "repo_identity",
            "agent_id",
            "identity_digest",
            "source_digest",
            "instructions_digest",
            "name",
            "version",
            "harness",
            "model",
        ] {
            let value = broken(field);
            overwrite(&id_path, &serde_json::to_vec(&value).unwrap());
            let error = read(&id_path, &repo).unwrap_err();
            assert!(
                error.to_string().contains("invalid identity or schema"),
                "{field} -> {error}"
            );
        }

        // A field the schema does not define is refused rather than ignored, so
        // a binding cannot carry unreviewed identity material.
        overwrite(
            &id_path,
            format!(
                "{{\"schema_version\":1,\"repo_identity\":\"{}\",\"agent_id\":\"{}\",\
                 \"identity_digest\":\"{hex}\",\"name\":\"builder\",\"version\":\"1.0.0\",\
                 \"harness\":\"claude-code\",\"model\":\"claude-sonnet-5\",\
                 \"source_digest\":\"{hex}\",\"instructions_digest\":\"{hex}\",\"extra\":1}}",
                repo.identity(),
                good.agent_id
            )
            .as_bytes(),
        );
        assert!(read(&id_path, &repo).is_err());
    }

    fn clone_binding(value: &Binding) -> Binding {
        Binding {
            schema_version: value.schema_version,
            repo_identity: value.repo_identity.clone(),
            agent_id: value.agent_id.clone(),
            identity_digest: value.identity_digest.clone(),
            name: value.name.clone(),
            version: value.version.clone(),
            harness: value.harness.clone(),
            model: value.model.clone(),
            source_digest: value.source_digest.clone(),
            instructions_digest: value.instructions_digest.clone(),
        }
    }

    #[test]
    fn a_digest_binding_that_names_another_identity_is_never_retargeted() {
        let (_root, repo, agent) = fixture();
        ensure(&repo, &agent).unwrap();
        let (_, digest_path) = binding_files(&repo, &agent);

        // A schema-valid binding filed under this agent's digest that describes
        // a different identity. Retargeting it would silently rebind a
        // reference other worktrees already hold.
        let mut value = read(&digest_path, &repo).unwrap().unwrap();
        value.identity_digest = "b".repeat(64);
        overwrite(&digest_path, &serde_json::to_vec(&value).unwrap());

        let error = ensure(&repo, &agent).unwrap_err();
        assert!(
            error.to_string().contains("refusing to retarget it"),
            "{error}"
        );
    }

    #[test]
    fn a_reference_whose_registered_agent_changed_is_reported_as_stale() {
        let (root, repo, agent) = fixture();
        let reference = ensure(&repo, &agent).unwrap();

        std::fs::write(
            root.path().join(AGENTS_RELATIVE_DIR).join("builder.md"),
            format!(
                "---\nokf_version: {}\ntype: ahu:agent\ntitle: builder\nversion: \
                 1.0.0\ndescription: builds\nharness: claude-code\nmodel: \
                 claude-sonnet-5\n---\n\nDifferent instructions.\n",
                crate::agent::OKF_VERSION
            ),
        )
        .unwrap();
        let changed = crate::agent::find(&repo.root, "builder").unwrap();
        assert_ne!(changed.identity_digest(), agent.identity_digest());

        let error = resolve(&repo, &reference).unwrap_err();
        assert!(error.to_string().contains("is stale"), "{error}");
        assert!(
            error.to_string().contains("create a new reference"),
            "{error}"
        );
        // The changed agent gets its own reference; the old one is not reused.
        assert_ne!(ensure(&repo, &changed).unwrap(), reference);
    }

    #[test]
    fn a_reference_cannot_resolve_once_its_agent_is_no_longer_registered() {
        let (root, repo, agent) = fixture();
        let reference = ensure(&repo, &agent).unwrap();
        std::fs::remove_file(root.path().join(AGENTS_RELATIVE_DIR).join("builder.md")).unwrap();
        // The binding still exists, but the agent it names does not, so
        // resolution fails rather than inventing a definition for it.
        let error = resolve(&repo, &reference).unwrap_err();
        assert_eq!(error.kind(), crate::util::ErrorKind::UnknownAgent);
    }

    #[test]
    fn a_colliding_id_entry_is_refused_rather_than_written_through() {
        let (_root, repo, agent) = fixture();
        let (ids, _) = paths(&repo, true).unwrap();
        let id = id_from_digest(&agent.identity_digest());
        // Something already occupies the id entry that `ensure` would mint, and
        // the digest entry does not exist, so there is no recorded binding to
        // return. `create_new` refuses to write through it.
        std::fs::create_dir(ids.join(format!("{id}.json"))).unwrap();

        let error = ensure(&repo, &agent).unwrap_err();
        assert!(error.to_string().contains("collided"), "{error}");
    }

    #[test]
    fn concurrent_ensure_calls_never_mint_a_second_reference() {
        let (_root, repo, agent) = fixture();
        // The subject here is the binding write, so the state directories are
        // created first. Creating them concurrently races on the checkout
        // ignore file, which is a separate concern from agent identity.
        paths(&repo, true).unwrap();
        let results: Vec<Result<String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| ensure(&repo, &agent)))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let expected = display(&id_from_digest(&agent.identity_digest()));
        let mut succeeded = 0;
        for result in results {
            match result {
                // The digest is the key, so no caller can come away with a
                // different reference for the same identity.
                Ok(reference) => {
                    assert_eq!(reference, expected);
                    succeeded += 1;
                }
                // A caller that loses the race to write the id entry is told to
                // retry rather than being handed a second identity. The entry it
                // lost to is the same binding, so the refusal is a contention
                // signal, not a divergence.
                Err(error) => assert!(
                    error.to_string().contains("collided; retry"),
                    "unexpected failure: {error}"
                ),
            }
        }
        assert!(
            succeeded >= 1,
            "no concurrent caller established the binding"
        );
        // Retrying after the race resolves it: the recorded binding is found by
        // digest and returned, so contention is transient.
        assert_eq!(ensure(&repo, &agent).unwrap(), expected);
    }
}
