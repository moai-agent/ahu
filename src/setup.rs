//! Generic setup: installed runtime proposals, explicit UI and guarded publication.
mod harness_plugins;
pub mod native_mcp;
use crate::config;
use crate::git::Repo;
use crate::harness_runtime::{self, Descriptor, Operation};
use crate::launcher::Console;
use crate::selection;
use crate::util::{Error, Result, display_path};
pub use harness_plugins::{list as list_harness_plugins, prepare as prepare_harness_plugin};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_PLAN_BYTES: usize = 4 * MAX_FILE_BYTES;
const MAX_CHANGES: usize = 256;

/// Native setup returns proposals only; core never accepts an executable action.
/// Null expected_before_sha256 requires an absent destination.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedWrite {
    pub path: PathBuf,
    #[serde(deserialize_with = "Option::deserialize")]
    pub expected_before_sha256: Option<String>,
    pub text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    changes: Vec<ProposedWrite>,
    publication: String,
}

pub fn run(console: &mut Console<'_>, repo: &Repo) -> Result<i32> {
    if !console.interactive {
        return Err(Error::new(
            "ahu setup requires a terminal for project choices; nothing changed",
        )
        .with_kind(crate::util::ErrorKind::Prerequisite));
    }
    crate::migration::ensure_idle(&repo.root)?;
    if crate::util::resolve_existing_within(&repo.root, crate::context_lock::LEGACY_LOCK_PATH)?
        .is_some()
    {
        return Err(Error::new("migrate the legacy context lock before setup"));
    }
    let descriptors = harness_runtime::discover(repo)?;
    if descriptors.is_empty() {
        return Err(Error::new(
            "no enabled installed harness runtimes; prepare, install and commit runtime bindings first",
        ));
    }
    let loaded = config::load(&repo.root)?;
    let mut config = match loaded.as_ref() {
        Some(loaded) => loaded.config.clone(),
        None => {
            let Some(config) = configure_project(console, repo, &descriptors)? else {
                return Ok(1);
            };
            config
        }
    };
    let mut plan = Vec::new();
    let existing_agents = crate::agent::load_all(&repo.root)?;
    let home = harness_runtime::native_home()?;
    let executable = selection::resolve_executable("ahu")
        .ok_or_else(|| Error::new("ahu must be on PATH outside repositories before MCP setup"))?;
    for harness in &config.harness_preferences {
        let runtime = harness_runtime::load(repo, harness)?;
        let descriptor = runtime.descriptor();
        if descriptor.id != *harness {
            return Err(Error::new("setup policy requires exact harness IDs"));
        }
        let prerequisite = selection::check_prerequisite(repo, harness)?;
        if !prerequisite.satisfied() {
            return Err(Error::new(
                "selected native executable is unavailable; selection was not changed",
            ));
        }
        if !descriptor.operations.contains(&Operation::Setup) {
            return Err(Error::new("selected runtime does not support setup"));
        }
        let ranked = config.model_rankings.entry(harness.clone()).or_default();
        if ranked.is_empty() {
            let Some(models) = choose_models(console, repo, harness)? else {
                return Ok(1);
            };
            *ranked = models;
        }
        let model = ranked
            .first()
            .ok_or_else(|| Error::new("setup requires an exact ranked model"))?;
        let proposal = runtime.call("setup", serde_json::json!({
            "project": repo.root.canonicalize()?, "home": home,
            "ahu_executable": executable,
            "skills": crate::mcp::BUNDLED_SKILLS.iter().map(|(name,text)| serde_json::json!({"name":name,"text":text})).collect::<Vec<_>>(),
        }))?;
        plan.extend(prepare_runtime_proposal(repo, &home, descriptor, proposal)?);
        let relative = format!(".agents/ahu/agents/dev-{harness}.md");
        let text = dev_agent(harness, model)?;
        let intended = repo.root.join(&relative);
        if existing_agents.iter().any(|agent| {
            agent.manifest.name == format!("dev-{harness}") && agent.manifest_path != intended
        }) {
            return Err(Error::new(
                "setup agent identity already exists at another path",
            ));
        }
        crate::agent::parse_manifest(&text, &intended)?;
        plan.push(create_only(&repo.root, &relative, text.into_bytes())?);
    }
    // Core owns shared skills and project policy; native paths belong to packages.
    for (name, text) in crate::mcp::BUNDLED_SKILLS {
        plan.push(create_only(
            &repo.root,
            &format!(".agents/skills/{name}/SKILL.md"),
            text.as_bytes().to_vec(),
        )?);
    }
    let rendered = config::try_render(&config)?;
    if loaded.is_none() {
        plan.push(create_only(
            &repo.root,
            config::CONFIG_RELATIVE_PATH,
            rendered.into_bytes(),
        )?);
    } else if loaded.as_ref().is_some_and(|old| old.config != config) {
        return Err(Error::new(
            "existing project has missing rankings; update and review policy before setup",
        ));
    }
    let plan = normalize_plan(plan)?;
    for change in &plan {
        console.say(&format!(
            "{} {}\n",
            if change.before.is_some() {
                "update"
            } else {
                "create"
            },
            display_path(&change.path)
        ))?;
    }
    if !crate::launcher::confirm(console, "Apply these reviewed setup paths? [y/N]: ")? {
        console.say("Cancelled. Nothing was changed.\n")?;
        return Ok(1);
    }
    check_mcp_server(repo)?;
    apply_changes(console, &plan)?;
    crate::agent::load_all(&repo.root)?;
    config::load(&repo.root)?;
    let snapshot = crate::snapshot::collect(&repo.root)?;
    let lock = crate::context_lock::refresh_for_setup(repo, &snapshot)?;
    console.say(&format!("MCP handshake passed. Refreshed {}. Review and commit project setup and context lock; native account and approval behavior remains provider-owned.\n", display_path(&lock)))?;
    Ok(0)
}

/// Inert registry choices followed by explicit model inspection of selected IDs.
pub fn configure_project(
    console: &mut Console<'_>,
    repo: &Repo,
    descriptors: &[Descriptor],
) -> Result<Option<config::ProjectConfig>> {
    if !console.interactive {
        return Err(Error::new(
            "project configuration requires an interactive terminal",
        ));
    }
    for (index, descriptor) in descriptors.iter().enumerate() {
        console.say(&format!(
            "{}. {} ({})\n",
            index + 1,
            crate::util::display_safe(&descriptor.display_name),
            descriptor.id
        ))?;
    }
    let Some(answer) = console.ask("Harness order, best first, by number: ")? else {
        return Ok(None);
    };
    let indexes = choose_indexes(&answer, descriptors.len(), false)?;
    if indexes.is_empty() {
        return Ok(None);
    }
    let mut harness_preferences = Vec::new();
    let mut model_rankings = BTreeMap::new();
    for index in indexes {
        let descriptor = &descriptors[index];
        let Some(models) = choose_models(console, repo, &descriptor.id)? else {
            return Ok(None);
        };
        harness_preferences.push(descriptor.id.clone());
        model_rankings.insert(descriptor.id.clone(), models);
    }
    Ok(Some(config::ProjectConfig {
        schema_version: config::SUPPORTED_SCHEMA_VERSION,
        harness_preferences,
        model_selection: "project-ranked".into(),
        harness_version_pins: BTreeMap::new(),
        model_rankings,
        knowledge: Default::default(),
        headless: Default::default(),
        telemetry: Default::default(),
    }))
}
fn choose_models(
    console: &mut Console<'_>,
    repo: &Repo,
    harness: &str,
) -> Result<Option<Vec<String>>> {
    let options = crate::models::for_harness(repo, harness)?;
    if options.models.is_empty() {
        return Err(Error::new(
            "runtime returned no model choices; configure the provider before setup",
        ));
    }
    console.say(&format!(
        "Models for {harness} ({:?}; entitlement checked: {}):\n",
        options.source, options.entitlement_checked
    ))?;
    for (index, model) in options.models.iter().enumerate() {
        console.say(&format!(
            "{}. {}\n",
            index + 1,
            crate::util::display_safe(model)
        ))?;
    }
    let Some(answer) = console.ask("Model order by number, or blank for suggested order: ")? else {
        return Ok(None);
    };
    let indexes = choose_indexes(&answer, options.models.len(), true)?;
    Ok(Some(
        indexes
            .into_iter()
            .map(|i| options.models[i].clone())
            .collect(),
    ))
}
fn choose_indexes(answer: &str, count: usize, default_all: bool) -> Result<Vec<usize>> {
    if answer.trim().is_empty() {
        return Ok(if default_all {
            (0..count).collect()
        } else {
            Vec::new()
        });
    }
    let mut indexes = Vec::new();
    for value in answer.split(',') {
        let index = value
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|i| i.checked_sub(1))
            .filter(|i| *i < count)
            .ok_or_else(|| Error::new("choose listed numbers only"))?;
        if indexes.contains(&index) {
            return Err(Error::new("duplicate setup choice"));
        }
        indexes.push(index);
    }
    Ok(indexes)
}

// No Debug: setup text can include native configuration.
struct Change {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}

fn safe_new_path(root: &Path, relative: &str) -> Result<PathBuf> {
    if !crate::plugin_binding::plain_relative(relative) {
        return Err(Error::new("invalid setup path"));
    }
    let path = root.join(relative);
    inspect_path(&path)?;
    Ok(path)
}
fn inspect_path(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.components().any(|p| {
            !matches!(
                p,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(Error::new("setup requires a plain absolute path"));
    }
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(m) if m.file_type().is_symlink() || (ancestor != path && !m.is_dir()) => {
                return Err(Error::new("unsafe setup path"));
            }
            Ok(m) if ancestor == path && !m.is_file() => {
                return Err(Error::new("setup destination is not a regular file"));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(Error::new("cannot inspect setup path")),
        }
    }
    Ok(())
}
fn read_before(path: &Path) -> Result<Option<Vec<u8>>> {
    use std::os::unix::fs::OpenOptionsExt;
    inspect_path(path)?;
    let file = match std::fs::File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::new("cannot read setup destination")),
    };
    if !file.metadata()?.is_file() {
        return Err(Error::new("setup destination is not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::new("setup file exceeds size limit"));
    }
    Ok(Some(bytes))
}
fn create_only(root: &Path, relative: &str, after: Vec<u8>) -> Result<Change> {
    let path = safe_new_path(root, relative)?;
    let before = read_before(&path)?;
    if before.as_ref().is_some_and(|bytes| bytes != &after) {
        return Err(Error::new(
            "setup refuses to overwrite existing project content",
        ));
    }
    Ok(Change {
        path,
        before,
        after,
    })
}
fn prepare_runtime_proposal(
    repo: &Repo,
    home: &Path,
    descriptor: &Descriptor,
    value: serde_json::Value,
) -> Result<Vec<Change>> {
    let proposal: Proposal =
        serde_json::from_value(value).map_err(|_| Error::new("invalid setup proposal"))?;
    if proposal.publication != "host_transaction_required" || proposal.changes.len() > MAX_CHANGES {
        return Err(Error::new(
            "invalid setup proposal bounds or publication mode",
        ));
    }
    let mut result = Vec::new();
    for change in proposal.changes {
        let relative = change
            .path
            .strip_prefix(&repo.root)
            .or_else(|_| change.path.strip_prefix(home))
            .ok()
            .and_then(Path::to_str)
            .ok_or_else(|| Error::new("setup path is outside the selected project/home"))?;
        if !crate::plugin_binding::plain_relative(relative) || relative.starts_with(".agents/ahu/")
        {
            return Err(Error::new(
                "setup proposal cannot modify core policy or unsafe paths",
            ));
        }
        let allowed = relative.starts_with(".agents/skills/")
            || descriptor
                .context_paths
                .iter()
                .chain(&descriptor.skill_paths)
                .any(|declared| {
                    if declared.ends_with('/') {
                        relative.starts_with(declared)
                    } else {
                        relative == declared
                    }
                });
        if !allowed {
            return Err(Error::new(
                "setup proposal targets an undeclared native path",
            ));
        }
        let before = read_before(&change.path)?;
        if before.as_ref().map(|b| crate::util::digest_bytes(b)) != change.expected_before_sha256 {
            return Err(Error::new(
                "setup destination changed since plugin planning",
            ));
        }
        result.push(Change {
            path: change.path,
            before,
            after: change.text.into_bytes(),
        });
    }
    normalize_plan(result)
}
fn normalize_plan(changes: Vec<Change>) -> Result<Vec<Change>> {
    let mut plan: BTreeMap<PathBuf, Change> = BTreeMap::new();
    let mut bytes = 0usize;
    for change in changes {
        if change.after.len() > MAX_FILE_BYTES {
            return Err(Error::new("setup file exceeds size limit"));
        }
        if let Some(old) = plan.get(&change.path) {
            if old.before != change.before || old.after != change.after {
                return Err(Error::new(
                    "conflicting setup proposals for one destination",
                ));
            }
            continue;
        }
        bytes = bytes.saturating_add(change.after.len());
        if bytes > MAX_PLAN_BYTES || plan.len() >= MAX_CHANGES {
            return Err(Error::new("setup plan exceeds size limit"));
        }
        plan.insert(change.path.clone(), change);
    }
    Ok(plan.into_values().collect())
}
fn check_change(change: &Change) -> Result<()> {
    if read_before(&change.path)? != change.before {
        return Err(Error::new("setup destination changed after review"));
    }
    Ok(())
}
fn apply_changes(console: &mut Console<'_>, changes: &[Change]) -> Result<()> {
    for change in changes {
        check_change(change)?;
    }
    let mut applied: Vec<&Change> = Vec::new();
    let outcome = (|| -> Result<()> {
        for change in changes {
            check_change(change)?;
            if change.before.as_ref() == Some(&change.after) {
                continue;
            }
            inspect_path(&change.path)?;
            if let Some(parent) = change.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            inspect_path(&change.path)?;
            // Include uncertain publications in guarded rollback.
            applied.push(change);
            if change.before.is_some() {
                let permissions = std::fs::metadata(&change.path)?.permissions();
                crate::private_io::atomic_write_with(
                    &change.path,
                    crate::private_io::Durability::Durable,
                    |file| {
                        file.write_all(&change.after)?;
                        file.set_permissions(permissions)?;
                        check_change(change)
                            .map_err(|_| std::io::Error::other("setup destination changed"))?;
                        Ok(())
                    },
                )?;
            } else if !crate::private_io::atomic_create(
                &change.path,
                &change.after,
                crate::private_io::Durability::Durable,
            )? {
                return Err(Error::new("setup destination appeared during publication"));
            }
            console.say(&format!("updated {}\n", display_path(&change.path)))?;
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        let mut incomplete = false;
        for change in applied.into_iter().rev() {
            let rollback = (|| -> Result<()> {
                let current = read_before(&change.path)?;
                if current == change.before {
                    return Ok(());
                }
                if current.as_ref() != Some(&change.after) {
                    return Err(Error::new("setup rollback found a concurrent edit"));
                }
                match &change.before {
                    Some(before) => {
                        let permissions = std::fs::metadata(&change.path)?.permissions();
                        crate::private_io::atomic_write_with(
                            &change.path,
                            crate::private_io::Durability::Durable,
                            |file| {
                                file.write_all(before)?;
                                file.set_permissions(permissions)?;
                                if read_before(&change.path)
                                    .map_err(|_| {
                                        std::io::Error::other("rollback inspection failed")
                                    })?
                                    .as_ref()
                                    != Some(&change.after)
                                {
                                    return Err(std::io::Error::other(
                                        "setup changed during rollback",
                                    ));
                                }
                                Ok(())
                            },
                        )
                    }
                    None => std::fs::remove_file(&change.path).map_err(Error::from),
                }
            })();
            incomplete |= rollback.is_err();
        }
        return Err(if incomplete {
            Error::new("setup failed; rollback incomplete because destinations changed")
        } else {
            error
        });
    }
    Ok(())
}
// Package preparation uses the same transaction with create-only authority.
fn validate_plan(writes: &[(PathBuf, Vec<u8>)]) -> Result<()> {
    for (path, bytes) in writes {
        if read_before(path)?.is_some_and(|before| before != *bytes) {
            return Err(Error::new(
                "setup refuses to overwrite existing project content",
            ));
        }
    }
    Ok(())
}
fn apply_plan(console: &mut Console<'_>, writes: &[(PathBuf, Vec<u8>)]) -> Result<()> {
    validate_plan(writes)?;
    let changes = writes
        .iter()
        .map(|(path, bytes)| {
            Ok(Change {
                path: path.clone(),
                before: read_before(path)?,
                after: bytes.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    apply_changes(console, &normalize_plan(changes)?)
}
fn dev_agent(harness: &str, model: &str) -> Result<String> {
    if !ahu_harness_api::identifier(harness) || !crate::models::valid_id(model) {
        return Err(Error::new("invalid setup agent identity"));
    }
    // JSON scalar quoting is valid YAML and prevents model IDs from adding fields.
    Ok(format!(
        "---\nokf_version: '0.2'\ntype: ahu:agent\ntitle: dev-{harness}\nversion: 1.0.0\ndescription: Development agent\nharness: {}\nmodel: {}\npermissions: prompt\nstatus: stable\n---\nFollow committed repository instructions and use available ahu tools.\n",
        serde_json::to_string(harness)?,
        serde_json::to_string(model)?
    ))
}
fn check_mcp_server(repo: &Repo) -> Result<()> {
    let executable =
        selection::resolve_executable("ahu").ok_or_else(|| Error::new("ahu is not on PATH"))?;
    check_mcp_server_at(repo, Path::new(&executable), Duration::from_secs(10))
}

// Keep executable discovery at the edge so the protocol can be checked with
// local deterministic servers without changing process-wide PATH.
fn check_mcp_server_at(repo: &Repo, executable: &Path, timeout: Duration) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let mut child = Command::new(executable)
        .args(["mcp", "serve"])
        .process_group(0)
        .current_dir(&repo.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let input = child.stdin.take().expect("piped stdin");
    let output = child.stdout.take().expect("piped stdout");
    let requests = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"ahu-setup\",\"version\":\"1\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n"
    );
    std::thread::spawn(move || {
        let mut input = input;
        let _ = input.write_all(requests.as_bytes());
        let _ = input.flush();
    });
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(output.take(MAX_FILE_BYTES as u64 + 1));
        let mut lines = Vec::new();
        for _ in 0..2 {
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() || line.is_empty() {
                break;
            }
            lines.push(line);
        }
        let _ = tx.send(lines);
    });
    let response = rx.recv_timeout(timeout);
    crate::headless::signal_group(child.id(), libc::SIGKILL);
    let _ = child.kill();
    let deadline = std::time::Instant::now() + Duration::from_millis(250);
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) if std::time::Instant::now() >= deadline => {
                return Err(Error::new("MCP handshake cleanup could not be confirmed"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    let lines = response.map_err(|_| Error::new("ahu MCP handshake timed out"))?;
    if lines.iter().map(String::len).sum::<usize>() > MAX_FILE_BYTES {
        return Err(Error::new("MCP handshake response exceeds limit"));
    }
    let parsed: Vec<serde_json::Value> = lines
        .iter()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    if parsed
        .iter()
        .any(|v| v.get("id") == Some(&serde_json::json!(1)) && v.get("result").is_some())
        && parsed.iter().any(|v| {
            v.get("id") == Some(&serde_json::json!(2))
                && v.pointer("/result/tools")
                    .and_then(serde_json::Value::as_array)
                    .is_some()
        })
    {
        Ok(())
    } else {
        Err(Error::new(
            "ahu MCP server did not complete initialize and tools/list; nothing was changed",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    fn descriptor() -> Descriptor {
        serde_json::from_value(json!({"id":"sixth","display_name":"Sixth","program":"sixth-native",
            "modes":["interactive"],"operations":["setup"],"context_paths":[".sixth/"],"skill_paths":[".sixth/skills/"]})).unwrap()
    }
    fn repo(root: &Path) -> Repo {
        Repo {
            root: root.into(),
            common_dir: root.join(".git"),
            head: None,
        }
    }
    fn console<'a>(
        input: &'a mut std::io::Cursor<Vec<u8>>,
        output: &'a mut Vec<u8>,
    ) -> Console<'a> {
        Console {
            input,
            output,
            interactive: true,
        }
    }
    #[test]
    fn proposal_checks_digests_scopes_duplicates_and_bounds_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join(".sixth/config.json");
        let home = tempfile::tempdir().unwrap();
        let repo = repo(&root);
        let proposal = |path: &Path, digest| json!({"publication":"host_transaction_required","changes":[{"path":path,"expected_before_sha256":digest,"text":"new"}]});
        let plan = prepare_runtime_proposal(
            &repo,
            home.path(),
            &descriptor(),
            proposal(&path, Value::Null),
        )
        .unwrap();
        assert!(!path.parent().unwrap().exists());
        assert_eq!(plan.len(), 1);
        for invalid in [
            root.join(".agents/ahu/config.md"),
            root.join(".git/config"),
            root.join("unknown"),
            root.join(".sixth/../outside"),
        ] {
            assert!(
                prepare_runtime_proposal(
                    &repo,
                    home.path(),
                    &descriptor(),
                    proposal(&invalid, Value::Null)
                )
                .is_err()
            );
        }
        std::fs::create_dir(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "before").unwrap();
        assert!(
            prepare_runtime_proposal(
                &repo,
                home.path(),
                &descriptor(),
                proposal(&path, Value::Null)
            )
            .is_err()
        );
        let digest = crate::util::digest_bytes(b"before");
        assert!(
            prepare_runtime_proposal(
                &repo,
                home.path(),
                &descriptor(),
                proposal(&path, json!(digest))
            )
            .is_ok()
        );
        let mut duplicate = proposal(&path, json!(digest));
        let mut second = duplicate["changes"][0].clone();
        second["text"] = json!("different");
        duplicate["changes"].as_array_mut().unwrap().push(second);
        assert!(prepare_runtime_proposal(&repo, home.path(), &descriptor(), duplicate).is_err());
        let mut oversized = proposal(&path, json!(digest));
        oversized["changes"][0]["text"] = json!("x".repeat(MAX_FILE_BYTES + 1));
        assert!(prepare_runtime_proposal(&repo, home.path(), &descriptor(), oversized).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"before");
    }
    use serde_json::Value;
    #[test]
    fn guarded_transaction_preserves_modes_and_refuses_stale_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("config");
        std::fs::write(&path, "before").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let changes = [Change {
            path: path.clone(),
            before: Some(b"before".to_vec()),
            after: b"after".to_vec(),
        }];
        let mut input = std::io::Cursor::new(Vec::new());
        let mut output = Vec::new();
        apply_changes(&mut console(&mut input, &mut output), &changes).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"after");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(apply_changes(&mut console(&mut input, &mut output), &changes).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"after");
    }
    #[test]
    fn publication_failure_rolls_back_earlier_updates() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("first");
        std::fs::write(&path, "before").unwrap();
        let changes = [Change {
            path: path.clone(),
            before: Some(b"before".to_vec()),
            after: b"after".to_vec(),
        }];
        struct FailingOutput;
        impl Write for FailingOutput {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("synthetic write failure"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut input = std::io::Cursor::new(Vec::new());
        let mut output = FailingOutput;
        let mut console = Console {
            input: &mut input,
            output: &mut output,
            interactive: true,
        };
        assert!(apply_changes(&mut console, &changes).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"before");
    }
    #[test]
    fn setup_refuses_symlink_ancestors_and_missing_digest_fields() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
        assert!(safe_new_path(root.path(), "linked/file").is_err());
        assert!(
            serde_json::from_value::<ProposedWrite>(json!({"path":"/synthetic","text":"new"}))
                .is_err()
        );
        assert!(choose_indexes("1,1", 2, false).is_err());
        assert!(choose_indexes("0", 2, false).is_err());
        assert_eq!(choose_indexes("2,1", 2, false).unwrap(), [1, 0]);
    }
    #[test]
    fn noninteractive_setup_does_not_inspect_or_create_paths() {
        let root = tempfile::tempdir().unwrap();
        let repo = repo(root.path());
        let mut input = std::io::Cursor::new(Vec::new());
        let mut output = Vec::new();
        let mut console = console(&mut input, &mut output);
        console.interactive = false;
        assert!(run(&mut console, &repo).is_err());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
