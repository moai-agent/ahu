//! Explicit, data-only project package installation. No build, checkout filters,
//! hooks, package managers, credential helpers, or package executables are run.
use crate::plugin_binding::{self, Binding, SourceKind};
use crate::util::{Error, Result};
use crate::{bail, state};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const MAX_FILES: usize = 4096;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_PACKAGE: u64 = 128 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    binding: Binding,
}

// Start with an empty environment: no injected Git config, URL rewriting,
// askpass, proxies, alternate object stores, SSH commands, or native auth homes.
// All package Git operations use our own fresh repository, never source config.
fn git(root: &Path, args: &[&str], network: bool, limit: u64) -> Result<Vec<u8>> {
    git_with_ca(root, args, network, limit, None)
}

fn git_with_ca(
    root: &Path,
    args: &[&str],
    network: bool,
    limit: u64,
    ca_info: Option<&Path>,
) -> Result<Vec<u8>> {
    let mut command = Command::new(crate::selection::resolve_utility("git")?);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", "/dev/null")
        .env("XDG_CONFIG_HOME", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_ALLOW_PROTOCOL", if network { "https" } else { "" });
    command.args(["--no-pager", "--no-replace-objects"]);
    if let Some(ca_info) = ca_info {
        let ca_info = ca_info
            .to_str()
            .ok_or_else(|| Error::new("test CA path must be UTF-8"))?;
        let setting = format!("http.sslCAInfo={ca_info}");
        command.args(["-c", &setting]);
    }
    command
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "credential.helper=",
            "-c",
            "credential.interactive=false",
            "-c",
            "http.followRedirects=false",
            "-c",
            "http.sslVerify=true",
            "-c",
            "http.extraHeader=",
            "-c",
            "http.proxy=",
            "-c",
            "http.lowSpeedLimit=1",
            "-c",
            "http.lowSpeedTime=30",
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
        ])
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command.process_group(0);
    // SAFETY: setrlimit is async-signal-safe; this closure touches no heap/locks.
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: MAX_PACKAGE,
                rlim_max: MAX_PACKAGE,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    supervise_git(command.spawn()?, limit, Duration::from_secs(120))
}

fn supervise_git(mut child: Child, limit: u64, timeout: Duration) -> Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = stdout.take(limit + 1).read_to_end(&mut bytes);
        let _ = tx.send((read, bytes));
    });
    let output = (|| {
        let mut output = None;
        loop {
            if output.is_none() {
                match rx.try_recv() {
                    Ok((Ok(_), bytes)) if bytes.len() as u64 <= limit => output = Some(bytes),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    _ => bail!("plugin Git operation timed out or exceeded its output limit"),
                }
            }
            // Observe exit without reaping. Even an exited leader must reserve
            // its PID until output validation and process-group cleanup finish.
            match crate::headless::child_exited(child.id()) {
                Ok(true) => break,
                Ok(false) => {}
                Err(_) => bail!("plugin Git process status is unreadable"),
            }
            if Instant::now() >= deadline {
                bail!("plugin Git operation timed out or exceeded its output limit");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        match output {
            Some(bytes) => Ok(bytes),
            None => {
                let grace =
                    Duration::from_secs(1).min(deadline.saturating_duration_since(Instant::now()));
                match rx.recv_timeout(grace) {
                    Ok((Ok(_), bytes)) if bytes.len() as u64 <= limit => Ok(bytes),
                    _ => Err(Error::new(
                        "plugin Git output is unreadable or exceeds its limit",
                    )),
                }
            }
        }
    })();

    // All paths converge here before the only reap. Also stop descendants that
    // closed stdout, so successful output cannot bypass process-group cleanup.
    #[cfg(test)]
    assert!(
        crate::headless::child_exited(child.id()).is_ok(),
        "Git leader was reaped before process-group cleanup"
    );
    // SAFETY: this child started its own group and its PID is still unreaped.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let status = child.wait();
    // A descendant that escaped the group may still hold stdout. Do not let a
    // reader join extend the operation's deadline; its read remains size-bounded.
    if reader.is_finished() {
        let _ = reader.join();
    }
    let bytes = output?;
    let status = status.map_err(|_| Error::new("plugin Git process status is unreadable"))?;
    if !status.success() {
        bail!(
            "plugin Git operation failed; source and pinned objects must be available (credentials and redirects are disabled)"
        );
    }
    Ok(bytes)
}

fn text(bytes: Vec<u8>) -> Result<String> {
    String::from_utf8(bytes).map_err(|_| Error::new("plugin Git metadata must be UTF-8"))
}

// Inspection is read-only and never canonicalizes away a supplied symlink.
fn directory(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("plugin paths must be absolute");
    }
    let mut current = PathBuf::from("/");
    for part in path.components() {
        match part {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(part) => {
                current.push(part);
                let meta = fs::symlink_metadata(&current)?;
                if !meta.is_dir() || meta.file_type().is_symlink() {
                    bail!("plugin path contains a symlink or non-directory");
                }
            }
            _ => bail!("plugin paths require normal components"),
        }
    }
    Ok(())
}

fn regular(path: &Path) -> Result<File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.permissions().mode() & 0o6000 != 0 {
        bail!("plugin content requires regular non-setid files, without symlinks");
    }
    Ok(file)
}

// Empty directories carry no package content. File names, execute bits and bytes
// are length-framed and sorted so local and Git sources have the same digest.
fn files(root: &Path) -> Result<Vec<String>> {
    fn walk(root: &Path, relative: &str, found: &mut Vec<String>, nodes: &mut usize) -> Result<()> {
        let dir = root.join(relative);
        directory(&dir)?;
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            *nodes += 1;
            if *nodes > MAX_FILES {
                bail!("plugin tree exceeds 4096 entries");
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::new("plugin filenames must be UTF-8"))?;
            let name = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            if !plugin_binding::plain_relative(&name) {
                bail!("plugin tree contains an unsafe path");
            }
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_dir() {
                walk(root, &name, found, nodes)?;
            } else if meta.is_file() {
                found.push(name);
            } else {
                bail!("plugin trees cannot contain symlinks or special files");
            }
        }
        Ok(())
    }
    let mut found = Vec::new();
    walk(root, "", &mut found, &mut 0)?;
    found.sort();
    Ok(found)
}

fn put(root: &Path, name: &str, bytes: &[u8], executable: bool) -> Result<()> {
    if !plugin_binding::plain_relative(name) {
        bail!("unsafe plugin file path");
    }
    let path = crate::util::resolve_within(root, name, true)?;
    // create_new also rejects case-folding aliases and file/directory collisions.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if executable { 0o700 } else { 0o600 })
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let mut total = 0;
    for name in files(source)? {
        let mut file = regular(&source.join(&name))?;
        let executable = file.metadata()?.permissions().mode() & 0o111 != 0;
        let bytes = crate::util::read_limited(&mut file, MAX_FILE)?;
        total += bytes.len() as u64;
        if total > MAX_PACKAGE {
            bail!("plugin tree exceeds 128 MiB");
        }
        put(destination, &name, &bytes, executable)?;
    }
    Ok(())
}

pub fn digest(root: &Path) -> Result<String> {
    directory(root)?;
    let mut hash = Sha256::new();
    hash.update(b"ahu-plugin-tree-v1\0");
    let mut total = 0;
    for name in files(root)? {
        let mut file = regular(&root.join(&name))?;
        let executable = file.metadata()?.permissions().mode() & 0o111 != 0;
        let bytes = crate::util::read_limited(&mut file, MAX_FILE)?;
        total += bytes.len() as u64;
        if total > MAX_PACKAGE {
            bail!("plugin tree exceeds 128 MiB");
        }
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name.as_bytes());
        hash.update([u8::from(executable)]);
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verify(root: &Path, binding: &Binding) -> Result<()> {
    if digest(root)? != binding.source.sha256 {
        bail!("plugin content SHA-256 pin mismatch");
    }
    let status = crate::interactive::plugins::local::read_package(root)?;
    if status.id != binding.id || status.host_api != binding.compatibility.host_api {
        bail!("installed manifest identity or host API does not match the project binding");
    }
    let cargo_path = status
        .cargo_manifest
        .expect("local manifest has Cargo path");
    let cargo: toml::Value = toml::from_str(&text(crate::okf::read(&cargo_path)?)?)
        .map_err(|_| Error::new("invalid package Cargo manifest"))?;
    if cargo
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        != Some(&binding.version)
    {
        bail!("package version must be explicit and match the project binding");
    }
    Ok(())
}

fn materialize(stage: &Path, binding: &Binding) -> Result<PathBuf> {
    materialize_with_ca(stage, binding, None)
}

fn materialize_with_ca(stage: &Path, binding: &Binding, ca_info: Option<&Path>) -> Result<PathBuf> {
    let package = stage.join("package");
    fs::create_dir(&package)?;
    if binding.source.kind == SourceKind::Local {
        let mut source = PathBuf::from(&binding.source.repository);
        if let Some(sub) = &binding.source.subdirectory {
            source.push(sub);
        }
        directory(&source)?;
        copy_tree(&source, &package)?;
    } else {
        let gitdir = stage.join("git");
        fs::create_dir(&gitdir)?;
        let format = if binding.source.revision.len() == 64 {
            "--object-format=sha256"
        } else {
            "--object-format=sha1"
        };
        git(
            &gitdir,
            &["init", "--bare", "--template=", format, "."],
            false,
            4096,
        )?;
        if plugin_binding::local_path(&binding.source.repository) {
            let source = PathBuf::from(&binding.source.repository);
            directory(&source)?;
            let metadata = source.join(".git");
            let objects = if metadata.exists() || fs::symlink_metadata(&metadata).is_ok() {
                directory(&metadata)?;
                metadata.join("objects")
            } else {
                source.join("objects")
            };
            directory(&objects)?;
            // No upload-pack: even a file:// fetch can run source-controlled
            // packObjectsHook. Copy bounded regular objects into our empty repo.
            for path in ["info/alternates", "info/http-alternates"] {
                if fs::symlink_metadata(objects.join(path)).is_ok() {
                    bail!("local Git sources with object alternates are unsupported");
                }
            }
            copy_tree(&objects, &gitdir.join("objects"))?;
        } else {
            git_with_ca(
                &gitdir,
                &[
                    "fetch",
                    "--no-tags",
                    "--no-recurse-submodules",
                    "--depth=1",
                    "--",
                    &binding.source.repository,
                    &binding.source.revision,
                ],
                true,
                4096,
                ca_info,
            )?;
        }
        let revision = &binding.source.revision;
        let kind = text(git(&gitdir, &["cat-file", "-t", revision], false, 128)?)?;
        if kind.trim() != "commit" {
            bail!("plugin Git revision must identify a commit, not a tag or tree");
        }
        let tree = match &binding.source.subdirectory {
            Some(sub) => format!("{revision}:{sub}"),
            None => format!("{revision}^{{tree}}"),
        };
        if text(git(&gitdir, &["cat-file", "-t", &tree], false, 128)?)?.trim() != "tree" {
            bail!("plugin subdirectory must identify a Git tree");
        }
        let listing = git(&gitdir, &["ls-tree", "-r", "-z", &tree], false, 1024 * 1024)?;
        let mut total = 0;
        for (index, entry) in listing
            .split(|b| *b == 0)
            .filter(|e| !e.is_empty())
            .enumerate()
        {
            if index >= MAX_FILES {
                bail!("plugin tree exceeds 4096 files");
            }
            let entry =
                std::str::from_utf8(entry).map_err(|_| Error::new("invalid plugin Git tree"))?;
            let (header, name) = entry
                .split_once('\t')
                .ok_or_else(|| Error::new("invalid plugin Git tree"))?;
            let header: Vec<_> = header.split(' ').collect();
            if header.len() != 3 || header[1] != "blob" || !matches!(header[0], "100644" | "100755")
            {
                bail!("plugin Git tree contains symlinks, submodules, or unsupported entries");
            }
            let bytes = git(&gitdir, &["cat-file", "blob", header[2]], false, MAX_FILE)?;
            total += bytes.len() as u64;
            if total > MAX_PACKAGE {
                bail!("plugin tree exceeds 128 MiB");
            }
            put(&package, name, &bytes, header[0] == "100755")?;
        }
        // No mutable Git configuration or object database is published.
        fs::remove_dir_all(&gitdir)?;
    }
    verify(&package, binding)?;
    Ok(package)
}

fn key(binding: &Binding) -> Result<String> {
    // Enablement/grants are project policy, not a new copy of package content.
    Ok(crate::util::digest_bytes(&serde_json::to_vec(&(
        &binding.id,
        &binding.version,
        &binding.source,
        &binding.compatibility,
    ))?))
}

fn installed(store: &Path, binding: &Binding) -> Result<PathBuf> {
    Ok(store.join(&binding.id).join(key(binding)?))
}

fn verify_installed(path: &Path, binding: &Binding) -> Result<()> {
    state::confine_existing_dir(path)?;
    let receipt: Receipt = state::read_json_strict(&path.join("receipt.json"))?;
    if receipt.schema_version != 1 || key(&receipt.binding)? != key(binding)? {
        bail!("plugin installation receipt does not match its binding");
    }
    verify(&path.join("package"), binding)
}

fn committed(repo: &crate::git::Repo, id: &str) -> Result<Option<Binding>> {
    Ok(crate::interactive::plugins::project::inventory(&repo.root)?
        .bindings
        .into_iter()
        .find(|s| s.binding.id == id)
        .map(|s| s.binding))
}

// Compare raw HEAD/index entries and working bytes. Worktree `git diff` can
// invoke clean filters and can trust assume-unchanged/skip-worktree flags; neither
// is appropriate for protecting policy edits. This also catches metadata-only
// edits, staged deletion/conflicts, and an untracked replacement of a removed binding.
fn require_clean_binding(repo: &crate::git::Repo, id: &str) -> Result<()> {
    let dirty = || {
        Error::new(
            "staged or unstaged binding changes; commit or resolve them before enable, disable, or remove",
        )
    };
    let relative = format!("{}/{id}.md", plugin_binding::DIRECTORY);
    let head = repo.head.as_deref().ok_or_else(dirty)?;
    let committed = text(git(
        &repo.root,
        &["ls-tree", "--full-tree", "-z", head, "--", &relative],
        false,
        4096,
    )?)?;
    let index = text(git(
        &repo.root,
        &["ls-files", "--stage", "--full-name", "-z", "--", &relative],
        false,
        4096,
    )?)?;
    let working = crate::util::resolve_existing_within(&repo.root, &relative)?;
    if committed.is_empty() {
        if !index.is_empty() || working.is_some() {
            return Err(dirty());
        }
        return Ok(());
    }
    let (header, name) = committed
        .strip_suffix('\0')
        .and_then(|entry| entry.split_once('\t'))
        .ok_or_else(dirty)?;
    let fields: Vec<_> = header.split(' ').collect();
    if fields.len() != 3
        || fields[1] != "blob"
        || !matches!(fields[0], "100644" | "100755")
        || name != relative
    {
        return Err(dirty());
    }
    if index != format!("{} {} 0\t{}\0", fields[0], fields[2], relative) {
        return Err(dirty());
    }
    let path = working.ok_or_else(dirty)?;
    let bytes = crate::okf::read(&path)?;
    let expected = git(
        &repo.root,
        &["cat-file", "blob", fields[2]],
        false,
        crate::okf::MAX_BYTES,
    )?;
    let executable = fs::symlink_metadata(path)?.permissions().mode() & 0o111 != 0;
    if bytes != expected || executable != (fields[0] == "100755") {
        return Err(dirty());
    }
    Ok(())
}

fn cleanup_allowed(repo: &crate::git::Repo) -> Result<crate::headless::PluginCleanupGuard> {
    let mut guard = crate::headless::plugin_cleanup_guard(repo)?;
    let tasks = crate::task::list(repo)?;
    // Old records do not freeze plugin installation pins. Treat any live task
    // as a possible reference, including unknown/incomplete/corrupt records.
    if !tasks.unreadable.is_empty()
        || !tasks.notes.is_empty()
        || tasks
            .records
            .iter()
            .any(|(_, record)| record.state.is_live())
    {
        bail!(
            "plugin cleanup refused: project tasks are live or their state is uncertain; all installed versions are retained"
        );
    }
    for (dir, _) in &tasks.records {
        if crate::headless::review::is_headless(dir) {
            if crate::headless::supervisor_owns_attempt(dir).ok() != Some(false) {
                bail!("plugin cleanup refused: task supervisor is live or its state is uncertain");
            }
            // Hold ownership after the observation: a concurrent resume must
            // fail or finish publishing its live state before cleanup proceeds.
            guard.retain_terminal_owner(dir)?;
        }
    }
    Ok(guard)
}

// A plugin can retain many individually bounded versions. Cleanup validates the
// deletion tree, not the content-digest path budget of one package.
fn validate_cleanup(path: &Path, depth: usize, entries: &mut usize) -> Result<()> {
    directory(path)?;
    if depth > 64 {
        bail!("plugin cleanup tree exceeds depth limit");
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        *entries += 1;
        if *entries > 2_000_000 {
            bail!("plugin cleanup tree exceeds entry limit");
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_dir() {
            validate_cleanup(&entry.path(), depth + 1, entries)?;
        } else if !metadata.is_file() {
            bail!("plugin cleanup refuses symlinks and special files");
        }
    }
    Ok(())
}

pub fn run(root: &Path, action: &str, id: &str) -> Result<()> {
    if action == "digest" {
        if !plugin_binding::local_path(id) {
            bail!("plugins digest requires an absolute plain package path");
        }
        println!("{}", digest(Path::new(id))?);
        return Ok(());
    }
    if !matches!(
        action,
        "install" | "update" | "enable" | "disable" | "remove"
    ) {
        bail!("unknown plugin lifecycle action");
    }
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        bail!("invalid plugin identity");
    }
    let repo = crate::git::discover(root)?;
    let binding = committed(&repo, id)?;
    if binding.is_none() && action != "remove" {
        bail!("plugin requires a committed project binding");
    }
    if matches!(action, "enable" | "disable" | "remove") {
        require_clean_binding(&repo, id)?;
    }
    let primary = repo.primary_root()?;
    for checkout in [&repo.root, &primary] {
        if !git(
            checkout,
            &["ls-files", "-z", "--", ".ahu/state/plugins"],
            false,
            4096,
        )?
        .is_empty()
        {
            bail!("plugin installation state must not be tracked by Git");
        }
    }
    let store = state::ensure_checkout_state(&primary)?.join("plugins");
    // The same primary-owned lock used by task launches closes admission vs.
    // cleanup races. No live version is replaced by install or update.
    let _lock = state::LaunchLock::acquire_at(state::coordination_dir(&repo)?.join("launch.lock"))?;
    state::create_private_dir_all(&store)?;
    let _cleanup = if matches!(action, "disable" | "remove") {
        Some(cleanup_allowed(&repo)?)
    } else {
        None
    };
    if action == "remove" {
        require_clean_binding(&repo, id)?;
        if binding.as_ref().is_some_and(|binding| binding.enabled)
            || plugin_binding::load(&repo.root, id)?.is_some_and(|(binding, _)| binding.enabled)
        {
            bail!("disable the plugin and commit its binding before removing installed data");
        }
        let plugin = store.join(id);
        state::confine_existing_dir(&plugin)?;
        if plugin.exists() {
            validate_cleanup(&plugin, 0, &mut 0)?;
            fs::remove_dir_all(&plugin)?;
        }
        println!("Removed installed data for {id}; project bindings were not modified.");
        return Ok(());
    }
    let binding = binding.expect("non-removal requires a committed binding");
    let target = installed(&store, &binding)?;
    match action {
        "install" | "update" => {
            if binding.compatibility.ahu != env!("CARGO_PKG_VERSION") {
                bail!("plugin binding requires a different ahu version");
            }
            state::confine_existing_dir(&target)?;
            if target.exists() {
                verify_installed(&target, &binding)?;
            } else {
                let plugin = store.join(id);
                state::create_private_dir_all(&plugin)?;
                if action == "install" && fs::read_dir(&plugin)?.next().is_some() {
                    bail!(
                        "plugin has another installed version or unfinished staging; use plugins update"
                    );
                }
                let stage = tempfile::Builder::new()
                    .prefix(".install-")
                    .tempdir_in(&plugin)?;
                materialize(stage.path(), &binding)?;
                state::write_json(
                    &stage.path().join("receipt.json"),
                    &Receipt {
                        schema_version: 1,
                        binding: binding.clone(),
                    },
                )?;
                fs::rename(stage.path(), &target)?;
                File::open(&plugin)?.sync_all()?;
            }
            println!(
                "Verified {}@{} installed. Existing versions retained; activation remains an explicit project binding decision. No package code executed.",
                binding.id, binding.version
            );
        }
        "enable" | "disable" => {
            if action == "enable" {
                if binding.compatibility.ahu != env!("CARGO_PKG_VERSION") {
                    bail!("plugin ahu compatibility mismatch");
                }
                verify_installed(&target, &binding)?;
            }
            let (mut working, mut doc) = plugin_binding::load(&repo.root, id)?
                .ok_or_else(|| Error::new("working project binding is missing"))?;
            require_clean_binding(&repo, id)?;
            if working != binding {
                bail!("binding changed while preparing its policy edit");
            }
            working.enabled = action == "enable";
            doc.set_settings(&working)?;
            let path = crate::util::resolve_within(
                &repo.root,
                &format!("{}/{id}.md", plugin_binding::DIRECTORY),
                false,
            )?;
            crate::private_io::atomic_write(
                &path,
                doc.render()?.as_bytes(),
                crate::private_io::Durability::Durable,
            )?;
            println!(
                "Plugin {id} enabled={}. Review and commit the binding and refresh the context lock. This does not execute or select a host.",
                working.enabled
            );
        }
        _ => bail!("unknown plugin lifecycle action"),
    }
    Ok(())
}

/// Offline inspection: no state creation, cleanup, fetch, selection or execution.
pub fn list(root: &Path, output_json: bool) -> Result<()> {
    let repo = crate::git::discover(root)?;
    let bindings = crate::interactive::plugins::project::inventory(&repo.root)?;
    let store = state::checkout_root(&repo.primary_root()?)?.join("plugins");
    state::confine_existing_dir(&store)?;
    let mut rows = Vec::new();
    if store.exists() {
        for entry in fs::read_dir(&store)? {
            let entry = entry?;
            state::confine_existing_dir(&entry.path())?;
            for version in fs::read_dir(entry.path())? {
                let path = version?.path();
                // An interrupted install is not an installation. Never remove
                // it during discovery; explicit remove can clean staging.
                if path
                    .file_name()
                    .is_some_and(|s| s.to_string_lossy().starts_with(".install-"))
                {
                    continue;
                }
                state::confine_existing_dir(&path)?;
                let receipt: Receipt = state::read_json_strict(&path.join("receipt.json"))?;
                let binding = &receipt.binding;
                let document =
                    crate::okf::Document::new("ahu:plugin-binding", binding, "")?.render()?;
                plugin_binding::parse(&document, &binding.id)?;
                if installed(&store, binding)? != path {
                    bail!("plugin receipt path or identity mismatch");
                }
                verify_installed(&path, binding)?;
                let current = bindings
                    .bindings
                    .iter()
                    .find(|s| s.binding.id == binding.id);
                let matches = current.map(|s| key(&s.binding)).transpose()?.as_deref()
                    == Some(&key(binding)?);
                rows.push(serde_json::json!({
                    "id": binding.id, "version": binding.version, "source": binding.source,
                    "installation": path, "source_verified": true,
                    "matches_committed_binding": matches,
                    "enabled": matches && current.is_some_and(|s| s.binding.enabled),
                    "activation_supported": false,
                }));
                if rows.len() > 256 {
                    bail!("installed plugin inventory exceeds 256 versions");
                }
            }
        }
    }
    rows.sort_by_key(|row| {
        (
            row["id"].as_str().unwrap().to_string(),
            row["installation"].as_str().unwrap().to_string(),
        )
    });
    if output_json {
        println!(
            "{}",
            serde_json::json!({"schema_version":1, "origin":"project-installed-packages", "installations":rows})
        );
    } else {
        for row in rows {
            println!(
                "{}@{}: source_verified=true, matches_committed_binding={}, enabled={}",
                row["id"].as_str().unwrap(),
                row["version"].as_str().unwrap(),
                row["matches_committed_binding"],
                row["enabled"]
            );
        }
        println!("Installed source inspection only; no package code executed or host activated.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    // stderr is a separate lifetime probe: EOF proves every fixture descendant
    // holding it was stopped, without kill(pid, 0) races or zombie assumptions.
    fn fixture(script: &str) -> (Child, mpsc::Receiver<Vec<u8>>) {
        let mut child = Command::new("/bin/sh")
            .env_clear()
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).unwrap();
            let _ = tx.send(bytes);
        });
        (child, rx)
    }

    fn await_exit(child: &Child) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !crate::headless::child_exited(child.id()).unwrap() {
            assert!(Instant::now() < deadline, "fixture leader did not exit");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn assert_cleaned(pid: u32, probe: mpsc::Receiver<Vec<u8>>) {
        assert_eq!(
            probe
                .recv_timeout(Duration::from_secs(5))
                .expect("descendant still holds pipe"),
            b"private fixture diagnostic"
        );
        assert_eq!(
            crate::headless::child_exited(pid)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ECHILD),
            "leader was not reaped after cleanup"
        );
    }

    #[test]
    fn https_git_fetch_rejects_an_untrusted_local_certificate() {
        use std::net::{SocketAddr, TcpStream};

        struct HttpsServer(Child);
        impl Drop for HttpsServer {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let cert = temp.path().join("server.pem");
        let key = temp.path().join("server-key.pem");
        let generated = Command::new("openssl")
            .args([
                "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-keyout",
            ])
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .args([
                "-subj",
                "/CN=127.0.0.1",
                "-addext",
                "subjectAltName=IP:127.0.0.1",
            ])
            .output()
            .expect("OpenSSL is installed for the local transport fixture");
        assert!(generated.status.success());

        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);
        let server = Command::new("openssl")
            .args([
                "s_server",
                "-quiet",
                "-www",
                "-accept",
                &address.port().to_string(),
                "-cert",
            ])
            .arg(&cert)
            .arg("-key")
            .arg(&key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("OpenSSL TLS server starts");
        let _server = HttpsServer(server);
        let address = SocketAddr::from(([127, 0, 0, 1], address.port()));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "local HTTPS fixture did not start"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let repo = temp.path().join("client");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "--quiet"], false, 4096).unwrap();
        let url = format!("https://127.0.0.1:{}/missing", address.port());
        let error = git(
            &repo,
            &[
                "fetch",
                "--no-tags",
                "--no-recurse-submodules",
                "--depth=1",
                "--",
                &url,
                "HEAD",
            ],
            true,
            4096,
        )
        .unwrap_err();
        assert!(error.to_string().contains("plugin Git operation failed"));
    }

    #[test]
    fn https_package_acquisition_fetches_a_pinned_commit_with_a_trusted_local_ca() {
        use std::net::{SocketAddr, TcpStream};

        struct HttpsServer(Child);
        impl Drop for HttpsServer {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        const SERVER: &str = r#"
import http.server, os, ssl, subprocess, sys, urllib.parse
root, cert, key, port, exec_path = sys.argv[1:]
backend = os.path.join(exec_path, 'git-http-backend')
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self): self.handle_git()
    def do_POST(self): self.handle_git()
    def handle_git(self):
        parsed = urllib.parse.urlsplit(self.path)
        length = int(self.headers.get('Content-Length', '0'))
        body = self.rfile.read(length) if length else b''
        env = {
            'PATH': exec_path + ':/usr/bin:/bin',
            'GIT_PROJECT_ROOT': root,
            'GIT_HTTP_EXPORT_ALL': '1',
            'PATH_INFO': urllib.parse.unquote(parsed.path),
            'QUERY_STRING': parsed.query,
            'REQUEST_METHOD': self.command,
            'CONTENT_TYPE': self.headers.get('Content-Type', ''),
            'CONTENT_LENGTH': str(length),
            'REMOTE_USER': '',
            'REMOTE_ADDR': self.client_address[0],
        }
        if self.headers.get('Git-Protocol'):
            env['HTTP_GIT_PROTOCOL'] = self.headers['Git-Protocol']
        result = subprocess.run([backend], env=env, input=body,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        headers, separator, payload = result.stdout.partition(b'\r\n\r\n')
        if not separator:
            self.send_error(500)
            return
        lines = headers.decode('latin1').split('\r\n')
        status = 200
        for line in lines:
            if line.startswith('Status:'):
                status = int(line.split()[1])
        self.send_response(status)
        for line in lines:
            if ':' in line and not line.startswith('Status:'):
                name, value = line.split(':', 1)
                self.send_header(name, value.strip())
        self.end_headers()
        self.wfile.write(payload)
    def log_message(self, *_args): pass
server = http.server.ThreadingHTTPServer(('127.0.0.1', int(port)), Handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(cert, key)
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
"#;

        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let cert = root.join("server.pem");
        let key = root.join("server-key.pem");
        let generated = Command::new("openssl")
            .args([
                "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-keyout",
            ])
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .args([
                "-subj",
                "/CN=127.0.0.1",
                "-addext",
                "subjectAltName=IP:127.0.0.1",
            ])
            .output()
            .expect("OpenSSL is installed for the local transport fixture");
        assert!(generated.status.success());

        let source_repo = root.join("source");
        let package = source_repo.join("plugins/fixture");
        let remote = root.join("fixture.git");
        let git_bin = crate::selection::resolve_utility("git").unwrap();
        let fixture_git = |cwd: &Path, args: &[&str]| {
            let output = Command::new(&git_bin)
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_ALLOW_PROTOCOL", "file")
                .args(["-C", cwd.to_str().unwrap()])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("ahu-plugin.md"),
            "---\nokf_version: '0.2'\ntype: ahu:plugin\nschema_version: 1\nid: fixture\npackage: fixture-package\nhost_api: 1\ncargo_manifest: Cargo.toml\n---\nfixture\n",
        )
        .unwrap();
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname = 'fixture-package'\nversion = '1.0.0'\n",
        )
        .unwrap();
        fixture_git(&source_repo, &["init", "--quiet"]);
        fixture_git(&source_repo, &["add", "--all"]);
        fixture_git(
            &source_repo,
            &[
                "-c",
                "user.name=ahu-test",
                "-c",
                "user.email=ahu-test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        );
        let revision = String::from_utf8(fixture_git(&source_repo, &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_string();
        fixture_git(
            &root,
            &["init", "--bare", "--quiet", remote.to_str().unwrap()],
        );
        fixture_git(
            &source_repo,
            &[
                "push",
                "--quiet",
                remote.to_str().unwrap(),
                "HEAD:refs/heads/main",
            ],
        );
        let remote_arg = format!("--git-dir={}", remote.display());
        fixture_git(&root, &[&remote_arg, "update-server-info"]);

        let exec_path = String::from_utf8(fixture_git(&root, &["--exec-path"]))
            .unwrap()
            .trim()
            .to_string();
        let port_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = port_probe.local_addr().unwrap().port();
        drop(port_probe);
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let server = Command::new("python3")
            .args(["-c", SERVER])
            .arg(&root)
            .arg(&cert)
            .arg(&key)
            .arg(port.to_string())
            .arg(exec_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("Python 3 starts the local Git HTTPS fixture");
        let _server = HttpsServer(server);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "local Git HTTPS server did not start"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let url = format!("https://127.0.0.1:{port}/fixture.git");
        let binding = Binding {
            schema_version: 1,
            id: "fixture".into(),
            version: "1.0.0".into(),
            source: crate::plugin_binding::Source {
                kind: SourceKind::Git,
                repository: url,
                revision,
                sha256: digest(&package).unwrap(),
                subdirectory: Some("plugins/fixture".into()),
            },
            compatibility: crate::plugin_binding::Compatibility {
                ahu: env!("CARGO_PKG_VERSION").into(),
                host_api: 1,
            },
            enabled: false,
            grants: vec![],
        };
        let stage = root.join("stage");
        fs::create_dir(&stage).unwrap();
        let acquired = materialize_with_ca(&stage, &binding, Some(&cert)).unwrap();
        assert_eq!(digest(&acquired).unwrap(), binding.source.sha256);
        assert_eq!(
            fs::read(acquired.join("ahu-plugin.md")).unwrap(),
            fs::read(package.join("ahu-plugin.md")).unwrap()
        );
    }

    #[test]
    fn oversized_git_output_cleans_group_before_reaping_exited_leader() {
        let (child, probe) = fixture(
            "printf 'private fixture diagnostic' >&2; printf '123456789'; /bin/sleep 30 & exit 0",
        );
        let pid = child.id();
        await_exit(&child);
        let error = supervise_git(child, 8, Duration::from_secs(5)).unwrap_err();
        assert!(matches!(
            error.to_string().as_str(),
            "plugin Git operation timed out or exceeded its output limit"
                | "plugin Git output is unreadable or exceeds its limit"
        ));
        assert_cleaned(pid, probe);
    }

    #[test]
    fn git_output_timeout_cleans_descendants_of_unreaped_exited_leader() {
        let (child, probe) =
            fixture("printf 'private fixture diagnostic' >&2; /bin/sleep 30 & exit 0");
        let pid = child.id();
        // The leader is definitely exited and the descendant definitely holds
        // stdout open. A try_wait here would deterministically reap too early.
        await_exit(&child);
        let started = Instant::now();
        let error = supervise_git(child, 8, Duration::from_millis(100)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "plugin Git output is unreadable or exceeds its limit"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_cleaned(pid, probe);
    }

    #[test]
    fn git_deadline_stops_live_leader_even_after_stdout_closes() {
        let (child, probe) =
            fixture("printf 'private fixture diagnostic' >&2; exec 1>&-; exec /bin/sleep 30");
        let pid = child.id();
        let started = Instant::now();
        let error = supervise_git(child, 8, Duration::from_millis(100)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "plugin Git operation timed out or exceeded its output limit"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_cleaned(pid, probe);
    }

    #[test]
    fn exact_git_output_limit_succeeds_and_cleans_descendants_without_stdout() {
        let (child, probe) = fixture(
            "printf 'private fixture diagnostic' >&2; printf '12345678'; /bin/sleep 30 >/dev/null & exit 0",
        );
        let pid = child.id();
        await_exit(&child);
        assert_eq!(
            supervise_git(child, 8, Duration::from_secs(5)).unwrap(),
            b"12345678"
        );
        assert_cleaned(pid, probe);
    }

    #[test]
    fn failed_git_status_remains_sanitized() {
        let (child, probe) =
            fixture("printf 'private fixture diagnostic' >&2; printf 'private stdout'; exit 1");
        let pid = child.id();
        await_exit(&child);
        let error = supervise_git(child, 64, Duration::from_secs(5)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "plugin Git operation failed; source and pinned objects must be available (credentials and redirects are disabled)"
        );
        assert_cleaned(pid, probe);
    }
}
