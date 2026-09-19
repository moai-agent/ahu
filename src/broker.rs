//! Registered child dispatch from the owning supervisor, outside worker sandboxes.
//! The capability binds requests to one live owner attempt. Same-UID code is not
//! isolated from this broker; the manifest and normal launch gates still apply.
use crate::util::{Error, Result};
use crate::{bail, headless, task};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildGrant {
    pub agent: String,
    pub identity_digest: String,
    pub permissions: crate::agent::Permissions,
    pub hooks_digest: String,
    pub native_helpers: String,
}

pub fn freeze_grants(
    repo: &crate::git::Repo,
    ordinary: &[String],
    widened: &[String],
) -> Result<Vec<ChildGrant>> {
    let loaded =
        crate::config::load(&repo.root)?.ok_or_else(|| Error::new("configuration required"))?;
    let project: toml::Value = toml::from_str(&std::fs::read_to_string(loaded.path)?)
        .map_err(|e| Error::new(e.to_string()))?;
    let mut grants = Vec::new();
    for name in ordinary.iter().chain(widened) {
        let agent = crate::agent::find(&repo.root, name)?;
        if agent.manifest.permissions.widens_defaults() && !widened.contains(name) {
            bail!(
                "child @{name} requires explicit --allow-child-widened @{name} at host submission"
            );
        }
        let policy = match &agent.manifest.native_helpers {
            Some(policy) => policy.as_str(),
            None => match project
                .get("execution")
                .and_then(|v| v.get("native_helpers"))
            {
                None => "disabled",
                Some(value) => value.as_str().ok_or_else(|| {
                    Error::new("child native_helpers must be disabled or bounded")
                })?,
            },
        };
        if !matches!(policy, "disabled" | "bounded") {
            bail!("child native_helpers must be disabled or bounded");
        }
        let mut hooks = crate::hooks::collect(&repo.root, &agent.manifest.harness)?;
        hooks.wrapper_injected = false;
        grants.push(ChildGrant {
            agent: name.clone(),
            identity_digest: agent.identity_digest(),
            permissions: agent.manifest.permissions,
            hooks_digest: hooks.digest(),
            native_helpers: policy.into(),
        });
    }
    Ok(grants)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    capability: String,
    parent_task: String,
    agent: String,
    prompt: String,
    title: Option<String>,
    summary: Option<String>,
    timeout_seconds: u64,
    native_helpers: Option<String>,
    allow_widened_approvals: bool,
}

pub struct Broker {
    dir: PathBuf,
    private_dir: PathBuf,
    token: String,
    owner: task::TaskRecord,
    spec: headless::Spec,
    pending: Vec<std::thread::JoinHandle<()>>,
    consumed: std::collections::BTreeSet<String>,
    scan: Option<std::fs::ReadDir>,
    scanned: usize,
}
impl Broker {
    pub fn new(dir: &Path, owner: &task::TaskRecord, spec: &headless::Spec) -> Result<Self> {
        headless::confined(dir, true)?;
        let private_dir = dir
            .parent()
            .ok_or_else(|| Error::new("missing broker owner"))?
            .join(format!("attempt-{}/broker", spec.attempt));
        headless::confined(&private_dir, true)?;
        Ok(Self {
            private_dir,
            dir: dir.into(),
            token: crate::orchestration::new_nonce()?,
            owner: owner.clone(),
            spec: spec.clone(),
            pending: Vec::new(),
            consumed: std::collections::BTreeSet::new(),
            scan: None,
            scanned: 0,
        })
    }
    pub fn configure_worker(&self, command: &mut Command) {
        command
            .env("AHU_BROKER_TOKEN", &self.token)
            .env_remove("AHU_BROKER_DISPATCH");
    }
    pub fn poll(&mut self) -> Result<()> {
        self.pending.retain(|thread| !thread.is_finished());
        if self.pending.len() >= 8 {
            return Ok(());
        }
        if self.scan.is_none() {
            self.scan = Some(std::fs::read_dir(&self.dir)?);
            self.scanned = 0;
        }
        let mut processed = 0;
        // Bound every directory entry, including invalid names and replayed requests.
        for _ in 0..32 {
            let Some(entry) = self.scan.as_mut().and_then(Iterator::next) else {
                self.scan = None;
                break;
            };
            self.scanned += 1;
            if self.scanned > 4096 {
                bail!("broker mailbox entry limit (4096) exceeded; admission stopped");
            }
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name
                .strip_suffix(".request.json")
                .is_some_and(|id| id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                continue;
            }
            let stem = name.strip_suffix(".request.json").unwrap_or_default();
            let claim = self.private_dir.join(format!("{stem}.claim.json"));
            let response = self.private_dir.join(format!("{stem}.response.json"));
            if self.consumed.contains(stem) || claim.exists() {
                continue;
            }
            if processed >= 8 {
                break;
            }
            processed += 1;
            if self.consumed.len() >= 256 {
                bail!("broker request limit (256 per attempt) exceeded; admission stopped");
            }
            self.consumed.insert(stem.to_string());
            let request = match read_request(&path) {
                Ok(request) => request,
                Err(error) => {
                    headless::durable_json(&claim, &json!({"state":"refused"}))?;
                    headless::durable_json(
                        &response,
                        &json!({"ok":false,"error":error.to_string()}),
                    )?;
                    continue;
                }
            };
            if request.schema_version != 1
                || request.capability != self.token
                || request.parent_task != self.owner.task_id
            {
                headless::durable_json(&claim, &json!({"state":"refused"}))?;
                headless::durable_json(
                    &response,
                    &json!({"ok":false,"error":"request is not bound to this live owner attempt"}),
                )?;
                continue;
            }
            let granted = self
                .spec
                .child_grants
                .iter()
                .find(|grant| grant.agent == request.agent);
            let allowed = granted.is_some_and(|grant| {
                (!grant.permissions.widens_defaults() || request.allow_widened_approvals)
                    && request
                        .native_helpers
                        .as_ref()
                        .is_none_or(|p| p == &grant.native_helpers)
                    && request.timeout_seconds > 0
            });
            if !allowed {
                headless::durable_json(&claim, &json!({"state":"refused"}))?;
                headless::durable_json(
                    &response,
                    &json!({"ok":false,"error":"child is outside the frozen host grant, exceeds its limits, or the approved configuration changed"}),
                )?;
                continue;
            }
            let grant = granted.ok_or_else(|| Error::new("missing child grant"))?;
            // Persist before dispatch; a lost supervisor never replays a claim.
            headless::durable_json(
                &claim,
                &json!({"state":"dispatching","at":task::now_rfc3339(),"parent_attempt":self.spec.attempt,"request_id":stem,"request_digest":crate::util::digest_bytes(&serde_json::to_vec(&request)?)}),
            )?;
            let prompt = self.private_dir.join(format!("{stem}.prompt.txt"));
            crate::state::write_private_file(&prompt, request.prompt.as_bytes())?;
            let mut command = Command::new(std::env::current_exe()?);
            command
                .current_dir(&self.owner.worktree)
                .args([
                    "launch",
                    &format!("@{}", request.agent),
                    "--headless",
                    "--background",
                    "--output",
                    "json",
                    "--prompt-file",
                ])
                .arg(&prompt)
                .arg("--timeout")
                .arg(
                    request
                        .timeout_seconds
                        .min(self.spec.options.timeout_seconds)
                        .to_string(),
                )
                .env("AHU_EXECUTION_BACKEND", "headless")
                .env("AHU_PARENT_TASK", &self.owner.task_id)
                .env("AHU_BROKER_DISPATCH", stem)
                .env_remove("AHU_BROKER_TOKEN")
                .env_remove("AHU_EXPECTED_DIGEST")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command.args(["--native-helpers", &grant.native_helpers]);
            command
                .env(
                    "AHU_FROZEN_CHILD_GRANTS",
                    serde_json::to_string(&self.spec.child_grants)?,
                )
                .env("AHU_EXPECTED_CHILD_IDENTITY", &grant.identity_digest)
                .env(
                    "AHU_EXPECTED_CHILD_SNAPSHOT",
                    &self.owner.config_snapshot_digest,
                )
                .env("AHU_EXPECTED_CHILD_HOOKS", &grant.hooks_digest)
                .env("AHU_PARENT_ATTEMPT", self.spec.attempt.to_string());
            if request.allow_widened_approvals {
                command.arg("--allow-widened-approvals");
            }
            if let Some(title) = request.title {
                command.arg("--title").arg(title);
            }
            if let Some(summary) = request.summary {
                command.arg("--summary").arg(summary);
            }
            let log = self.private_dir.join(format!("{stem}.dispatch.log"));
            let closed = self
                .private_dir
                .parent()
                .ok_or_else(|| Error::new("missing broker attempt"))?
                .join("admission-closed.json");
            let cancelled = self
                .dir
                .parent()
                .ok_or_else(|| Error::new("missing broker task"))?
                .join("cancel.json");
            self.pending.push(std::thread::spawn(move || {
                let result=(||->Result<Value>{
                    let output=bounded_dispatch(&mut command, &closed, &cancelled)?;
                    crate::state::write_private_file(&log,&output.stderr)?;
                    if !output.status.success() { return Ok(json!({"ok":false,"error":String::from_utf8_lossy(&output.stderr),"exit_code":output.status.code()})); }
                    let launch:Value=serde_json::from_slice(&output.stdout).map_err(|e|Error::new(e.to_string()))?;
                    Ok(json!({"ok":true,"launch":launch}))
                })();
                let value=result.unwrap_or_else(|e|json!({"ok":false,"error":e.to_string()}));
                let _=headless::durable_json(&response,&value);
            }));
            if self.pending.len() >= 8 {
                break;
            }
        }
        Ok(())
    }
    pub fn finish(mut self) -> Result<()> {
        for thread in self.pending.drain(..) {
            thread
                .join()
                .map_err(|_| Error::new("broker dispatch thread failed"))?;
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub fn request(
    repo: &crate::git::Repo,
    agent: &str,
    prompt: &str,
    display: &crate::launch::DisplayMetadata,
    options: &headless::Options,
    allow: bool,
    json_output: bool,
) -> Result<i32> {
    let parent =
        std::env::var("AHU_PARENT_TASK").map_err(|_| Error::new("missing broker parent"))?;
    let dir = headless::lookup(repo, &parent)?;
    let record = task::load(&dir)?;
    if record.worktree.canonicalize()? != repo.root.canonicalize()? {
        bail!("broker request must originate in its owning worktree");
    }
    if dir.join("cancel.json").exists() {
        bail!("parent is cancelling; child admission refused");
    }
    let capability=std::env::var("AHU_BROKER_TOKEN").map_err(|_|Error::new("owning supervisor has no live child broker; do not launch a nested harness or widen its sandbox"))?;
    if record.identity.harness == "codex"
        && record.identity.permissions == crate::agent::Permissions::Prompt
    {
        bail!(
            "broker_transport_unavailable_for_read_only: this Codex task retains read-only sandboxing; submit from the host with an explicitly authorized workspace-write coordinator and frozen child grant"
        );
    }
    let request = Request {
        schema_version: 1,
        capability,
        parent_task: parent,
        agent: agent.into(),
        prompt: prompt.into(),
        title: display.title.clone(),
        summary: display.summary.clone(),
        timeout_seconds: options.timeout_seconds,
        native_helpers: options
            .native_helpers_explicit
            .then(|| options.native_helpers.clone()),
        allow_widened_approvals: allow,
    };
    let nonce = crate::orchestration::new_nonce()?;
    let requests = dir.join("requests");
    let input = requests.join(format!("{nonce}.request.json"));
    let parent_spec: headless::Spec = headless::read_json(&dir.join("headless.json"))?;
    let response = dir.join(format!(
        "attempt-{}/broker/{nonce}.response.json",
        parent_spec.attempt
    ));
    headless::durable_json(&input, &request)?;
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        if response.exists() {
            let value: Value = headless::read_json(&response)?;
            if value["ok"] != true {
                bail!("registered child dispatch failed: {}", value["error"]);
            }
            if options.background {
                headless::emit(&value["launch"], json_output)?;
                return Ok(0);
            }
            let id = value["launch"]["task_id"]
                .as_str()
                .ok_or_else(|| Error::new("broker response lacks task identity"))?;
            return headless::control(repo, "wait", id, None, json_output);
        }
        if Instant::now() >= deadline {
            bail!(
                "broker acknowledgement timed out; request {} is retained. Inspect child tasks before submitting again; no automatic replay",
                nonce
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn read_request(path: &Path) -> Result<Request> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        bail!("broker request must be an owner-only regular file with one link");
    }
    let mut bytes = Vec::new();
    file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 2 * 1024 * 1024 {
        bail!("broker request exceeds 2 MiB");
    }
    serde_json::from_slice(&bytes).map_err(|e| Error::new(format!("invalid broker request: {e}")))
}

fn bounded_dispatch(
    command: &mut Command,
    closed: &Path,
    cancelled: &Path,
) -> Result<std::process::Output> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    let mut child = command.process_group(0).spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::new("missing dispatch stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::new("missing dispatch stderr"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        let mut data = Vec::new();
        let result = stdout.take(2 * 1024 * 1024 + 1).read_to_end(&mut data);
        let _ = tx.send((true, result.is_ok(), data));
    });
    std::thread::spawn(move || {
        let mut data = Vec::new();
        let result = stderr.take(2 * 1024 * 1024 + 1).read_to_end(&mut data);
        let _ = tx2.send((false, result.is_ok(), data));
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while !headless::child_exited(child.id())?
        && Instant::now() < deadline
        && !closed.exists()
        && !cancelled.exists()
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    let timed_out = Instant::now() >= deadline;
    // SAFETY: the owned child is not reaped, so its process-group ID cannot be reused.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let status = child.wait()?;
    if closed.exists() || cancelled.exists() {
        bail!(
            "broker admission closed; pending dispatch stopped; inspect recorded children before any new assignment"
        );
    }
    if timed_out {
        bail!("broker dispatch timed out; inspect recorded children, do not replay this request");
    }
    let mut out = Vec::new();
    let mut err = Vec::new();
    for _ in 0..2 {
        let (stdout, ok, data) = rx
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| Error::new("dispatch output incomplete"))?;
        if !ok || data.len() > 2 * 1024 * 1024 {
            bail!("dispatch output exceeded capture limit");
        }
        if stdout {
            out = data;
        } else {
            err = data;
        }
    }
    Ok(std::process::Output {
        status,
        stdout: out,
        stderr: err,
    })
}
