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
    name: Option<String>,
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
        if spec.schema_version != 2 {
            bail!(
                "legacy dispatch is unsupported; use the original runner and its owning supervisor"
            );
        }
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
                Err(_) => {
                    headless::durable_json(&claim, &json!({"state":"refused"}))?;
                    headless::durable_json(
                        &response,
                        &json!({"ok":false,"error":"invalid or unreadable broker request"}),
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
                .env_remove("AHU_RUNTIME_DIR")
                .env_remove("AHU_TASK_INDEX_DIR")
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
            if let Some(name) = request.name {
                command.arg("--name").arg(name);
            }
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
                    if !output.status.success() { return Ok(json!({"ok":false,"error":"child dispatch failed","exit_code":output.status.code()})); }
                    let launch:Value=serde_json::from_slice(&output.stdout).map_err(|e|Error::new(e.to_string()))?;
                    Ok(json!({"ok":true,"launch":{"schema_version":2,"backend":"headless","task_id":launch["task_id"],"task_handle":launch["task_handle"],"runtime":launch["runtime"],"worktree":launch["worktree"],"acceptance":"not assessed"}}))
                })();
                let value=result.unwrap_or_else(|_|json!({"ok":false,"error":"child dispatch unavailable"}));
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
    let dir = headless::lookup(repo, &parent).map_err(|_| Error::new("owning broker is unavailable in this repository; legacy workers must use their original runner and legacy store"))?;
    let spec: headless::Spec = headless::read_json(&dir.join("headless.json"))?;
    if spec.schema_version != 2 {
        bail!(
            "legacy dispatch is unsupported by this binary; use the original runner and its owning supervisor"
        );
    }
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
        name: display.name.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const TASK: &str = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5b";

    fn repo() -> (tempfile::TempDir, crate::git::Repo) {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let repo = crate::git::discover(dir.path()).unwrap();
        (dir, repo)
    }

    fn write_config(root: &Path) {
        let dir = root.join(crate::config::CONFIG_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            format!(
                "schema_version = 1\n\
                 harness_preferences = [\"claude-code\"]\n\
                 model_selection = \"project-ranked\"\n\
                 catalog_version = \"{}\"\n",
                crate::catalog::CATALOG_VERSION
            ),
        )
        .unwrap();
    }

    fn write_agent(root: &Path, name: &str, permissions: &str, native_helpers: Option<&str>) {
        let dir = root.join(crate::config::AGENTS_RELATIVE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let helpers = native_helpers
            .map(|policy| format!("native_helpers: {policy}\n"))
            .unwrap_or_default();
        std::fs::write(
            dir.join(format!("{name}.md")),
            format!(
                "---\n\
                 okf_version: 0.2\n\
                 type: ahu:agent\n\
                 title: {name}\n\
                 description: A synthetic agent used by broker tests\n\
                 status: stable\n\
                 harness: claude-code\n\
                 model: claude-opus-5\n\
                 permissions: {permissions}\n\
                 {helpers}\
                 version: 1.0.0\n\
                 ---\n\n\
                 You are {name}.\n"
            ),
        )
        .unwrap();
    }

    fn owner(repo: &crate::git::Repo) -> task::TaskRecord {
        serde_json::from_value(json!({
            "schema_version": task::TASK_SCHEMA_VERSION,
            "task_id": TASK,
            "title": "broker owner",
            "created_at": "2026-01-01T00:00:00Z",
            "repo_identity": repo.identity(),
            "repo_root": repo.root,
            "branch": "ahu/owner",
            "worktree": repo.root,
            "base_commit": null,
            "identity": {
                "mode": "named",
                "agent": "coordinator",
                "agent_version": "1.0.0",
                "permissions": "auto",
                "harness": "claude-code",
                "model": "claude-opus-5",
                "instructions_source": null,
                "source_digest": null,
                "instructions_digest": null,
                "identity_digest": null,
                "selection_basis": null
            },
            "policy_digest": "0".repeat(64),
            "catalog_version": crate::catalog::CATALOG_VERSION,
            "config_snapshot": {"entries": [], "skipped_directories": []},
            "config_snapshot_digest": "snapshot-digest",
            "materialize": {"written": [], "removed": [], "concurrently_modified": []},
            "launch_command": {"program": "claude", "args": []},
            "delivery": {"nonce": "n", "agent_instructions": null, "digest": "d"},
            "prompt_digest": "p",
            "enforcement": {
                "harness": "claude-code",
                "harness_version": null,
                "model_fixed_for_session": false,
                "gaps": [],
                "applied_controls": []
            },
            "reliability_warning": null,
            "cmux_group_id": null,
            "cmux_workspace_id": null,
            "cmux_window_id": null,
            "state": "running"
        }))
        .unwrap()
    }

    fn spec(schema_version: u32, grants: Vec<ChildGrant>) -> headless::Spec {
        headless::Spec {
            schema_version,
            options: headless::Options {
                timeout_seconds: 900,
                ..Default::default()
            },
            harness_version: "2.1.270".into(),
            executable_digest: "0".repeat(64),
            parent_task: None,
            parent_attempt: None,
            root_task: None,
            broker_request: None,
            child_grants: grants,
            depth: 0,
            attempt: 1,
            session: None,
            broker_dir: None,
            native_profile: None,
            native_controls: Vec::new(),
            gaps: Vec::new(),
        }
    }

    fn grant(agent: &str, permissions: crate::agent::Permissions, helpers: &str) -> ChildGrant {
        ChildGrant {
            agent: agent.into(),
            identity_digest: "a".repeat(64),
            permissions,
            hooks_digest: "b".repeat(64),
            native_helpers: helpers.into(),
        }
    }

    /// The mailbox and the private attempt directory the supervisor would own.
    fn mailbox(repo: &crate::git::Repo) -> PathBuf {
        crate::state::checkout_root(&repo.root)
            .unwrap()
            .join("repos")
            .join(repo.identity())
            .join("headless")
            .join(TASK)
            .join("requests")
    }

    fn submitted(token: &str, agent: &str) -> Request {
        Request {
            schema_version: 1,
            capability: token.into(),
            parent_task: TASK.into(),
            agent: agent.into(),
            prompt: "review the diff".into(),
            title: None,
            summary: None,
            name: None,
            timeout_seconds: 60,
            native_helpers: None,
            allow_widened_approvals: false,
        }
    }

    fn submit(dir: &Path, id: &str, body: &[u8]) {
        crate::state::write_private_file(&dir.join(format!("{id}.request.json")), body).unwrap();
    }

    fn refusal(private: &Path, id: &str) -> String {
        let claim: Value = headless::read_json(&private.join(format!("{id}.claim.json"))).unwrap();
        assert_eq!(claim["state"], "refused", "{claim}");
        let response: Value =
            headless::read_json(&private.join(format!("{id}.response.json"))).unwrap();
        assert_eq!(response["ok"], false, "{response}");
        response["error"].as_str().unwrap().to_string()
    }

    /// `Request` deliberately has no `Debug`: its capability and prompt must not
    /// reach a panic message, so refusals are compared as text instead.
    fn read_failure(path: &Path) -> String {
        read_request(path)
            .map(|_| ())
            .expect_err("the request is refused")
            .to_string()
    }

    #[test]
    fn freezing_a_grant_records_the_identity_hooks_and_default_helper_policy() {
        let (_dir, repo) = repo();
        write_config(&repo.root);
        write_agent(&repo.root, "reviewer", "prompt", None);
        let grants = freeze_grants(&repo, &["reviewer".into()], &[]).unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].agent, "reviewer");
        assert_eq!(grants[0].permissions, crate::agent::Permissions::Prompt);
        // No manifest policy and no project override: helpers stay off.
        assert_eq!(grants[0].native_helpers, "disabled");
        let agent = crate::agent::find(&repo.root, "reviewer").unwrap();
        assert_eq!(grants[0].identity_digest, agent.identity_digest());
        let mut hooks = crate::hooks::collect(&repo.root, "claude-code").unwrap();
        hooks.wrapper_injected = false;
        assert_eq!(grants[0].hooks_digest, hooks.digest());
    }

    #[test]
    fn a_widening_child_must_be_named_at_host_submission() {
        let (_dir, repo) = repo();
        write_config(&repo.root);
        write_agent(&repo.root, "writer", "auto", None);
        let error = freeze_grants(&repo, &["writer".into()], &[])
            .expect_err("an unnamed widening child is refused")
            .to_string();
        assert!(error.contains("--allow-child-widened @writer"), "{error}");
        let grants = freeze_grants(&repo, &[], &["writer".into()]).unwrap();
        assert_eq!(grants[0].permissions, crate::agent::Permissions::Auto);
        // Ordinary and widened children are frozen together, in that order.
        write_agent(&repo.root, "reviewer", "prompt", None);
        let grants = freeze_grants(&repo, &["reviewer".into()], &["writer".into()]).unwrap();
        assert_eq!(
            grants.iter().map(|g| g.agent.as_str()).collect::<Vec<_>>(),
            ["reviewer", "writer"]
        );
    }

    #[test]
    fn a_manifest_helper_policy_is_frozen_into_the_grant() {
        let (_dir, repo) = repo();
        write_config(&repo.root);
        write_agent(&repo.root, "reviewer", "prompt", Some("bounded"));
        let grants = freeze_grants(&repo, &["reviewer".into()], &[]).unwrap();
        assert_eq!(grants[0].native_helpers, "bounded");
    }

    #[test]
    fn freezing_requires_project_configuration_and_a_registered_agent() {
        let (_dir, repo) = repo();
        write_agent(&repo.root, "reviewer", "prompt", None);
        let error = freeze_grants(&repo, &["reviewer".into()], &[])
            .expect_err("a project without configuration cannot grant children")
            .to_string();
        assert!(error.contains("configuration required"), "{error}");
        write_config(&repo.root);
        let error = freeze_grants(&repo, &["ghost".into()], &[])
            .expect_err("an unregistered child is refused")
            .to_string();
        assert!(error.contains("no agent named \"ghost\""), "{error}");
    }

    #[test]
    fn a_legacy_attempt_cannot_open_a_broker() {
        let (_dir, repo) = repo();
        let record = owner(&repo);
        let error = Broker::new(&mailbox(&repo), &record, &spec(1, Vec::new()))
            .map(|_| ())
            .expect_err("schema 1 attempts have no broker")
            .to_string();
        assert!(error.contains("legacy dispatch is unsupported"), "{error}");
    }

    #[test]
    fn the_worker_is_handed_a_fresh_capability_and_never_a_dispatch_marker() {
        let (_dir, repo) = repo();
        let record = owner(&repo);
        let spec = spec(2, Vec::new());
        let broker = Broker::new(&mailbox(&repo), &record, &spec).unwrap();
        assert_eq!(broker.token.len(), 16);
        assert!(broker.token.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(broker.private_dir.ends_with("attempt-1/broker"));
        assert_eq!(
            std::fs::metadata(&broker.private_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let mut command = Command::new("/usr/bin/true");
        command.env("AHU_BROKER_DISPATCH", "stale");
        broker.configure_worker(&mut command);
        let environment: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(
            environment.contains(&("AHU_BROKER_TOKEN".to_string(), Some(broker.token.clone())))
        );
        // A worker never inherits a dispatch marker: that names a child request.
        assert!(environment.contains(&("AHU_BROKER_DISPATCH".to_string(), None)));
        // Each attempt mints its own capability.
        let other = Broker::new(&mailbox(&repo), &record, &spec).unwrap();
        assert_ne!(broker.token, other.token);
    }

    #[test]
    fn unusable_mailbox_names_are_ignored_without_a_claim() {
        let (_dir, repo) = repo();
        let record = owner(&repo);
        let spec = spec(2, Vec::new());
        let mut broker = Broker::new(&mailbox(&repo), &record, &spec).unwrap();
        let dir = mailbox(&repo);
        for name in [
            "0123456789abcde.request.json",   // fifteen digits
            "0123456789abcdef0.request.json", // seventeen digits
            "0123456789abcdeg.request.json",  // not hexadecimal
            "0123456789abcdef.request.txt",
            "notes.txt",
        ] {
            crate::state::write_private_file(&dir.join(name), b"{}").unwrap();
        }
        broker.poll().unwrap();
        broker.finish().unwrap();
        let claims = std::fs::read_dir(mailbox(&repo).with_file_name("attempt-1").join("broker"))
            .unwrap()
            .count();
        assert_eq!(claims, 0);
    }

    #[test]
    fn every_admission_refusal_is_claimed_and_answered_once() {
        let (_dir, repo) = repo();
        let record = owner(&repo);
        let spec = spec(
            2,
            vec![
                grant("reviewer", crate::agent::Permissions::Prompt, "disabled"),
                grant("writer", crate::agent::Permissions::Auto, "bounded"),
            ],
        );
        let dir = mailbox(&repo);
        let mut broker = Broker::new(&dir, &record, &spec).unwrap();
        let private = broker.private_dir.clone();
        let token = broker.token.clone();
        submit(&dir, &"1".repeat(16), b"{ not json");
        let mut wrong_capability = submitted(&token, "reviewer");
        wrong_capability.capability = "0".repeat(16);
        submit(
            &dir,
            &"2".repeat(16),
            &serde_json::to_vec(&wrong_capability).unwrap(),
        );
        let mut wrong_parent = submitted(&token, "reviewer");
        wrong_parent.parent_task = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5c".into();
        submit(
            &dir,
            &"3".repeat(16),
            &serde_json::to_vec(&wrong_parent).unwrap(),
        );
        let mut wrong_schema = submitted(&token, "reviewer");
        wrong_schema.schema_version = 2;
        submit(
            &dir,
            &"4".repeat(16),
            &serde_json::to_vec(&wrong_schema).unwrap(),
        );
        submit(
            &dir,
            &"5".repeat(16),
            &serde_json::to_vec(&submitted(&token, "ungranted")).unwrap(),
        );
        submit(
            &dir,
            &"6".repeat(16),
            &serde_json::to_vec(&submitted(&token, "writer")).unwrap(),
        );
        let mut wrong_helpers = submitted(&token, "reviewer");
        wrong_helpers.native_helpers = Some("bounded".into());
        submit(
            &dir,
            &"7".repeat(16),
            &serde_json::to_vec(&wrong_helpers).unwrap(),
        );
        let mut no_timeout = submitted(&token, "reviewer");
        no_timeout.timeout_seconds = 0;
        submit(
            &dir,
            &"8".repeat(16),
            &serde_json::to_vec(&no_timeout).unwrap(),
        );
        for _ in 0..4 {
            broker.poll().unwrap();
        }
        broker.finish().unwrap();
        assert!(
            refusal(&private, &"1".repeat(16)).contains("invalid or unreadable broker request")
        );
        for id in ["2", "3", "4"] {
            let error = refusal(&private, &id.repeat(16));
            assert!(
                error.contains("not bound to this live owner attempt"),
                "{error}"
            );
        }
        for id in ["5", "6", "7", "8"] {
            let error = refusal(&private, &id.repeat(16));
            assert!(error.contains("outside the frozen host grant"), "{error}");
        }
        // Nothing was dispatched, so no prompt left the mailbox.
        assert!(!std::fs::read_dir(&private).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".prompt.txt")
        }));
    }

    #[test]
    fn a_claimed_request_is_never_admitted_twice() {
        let (_dir, repo) = repo();
        let record = owner(&repo);
        let spec = spec(
            2,
            vec![grant(
                "reviewer",
                crate::agent::Permissions::Prompt,
                "disabled",
            )],
        );
        let dir = mailbox(&repo);
        let mut broker = Broker::new(&dir, &record, &spec).unwrap();
        let private = broker.private_dir.clone();
        let token = broker.token.clone();
        let id = "abcdef0123456789";
        // A request that was already claimed by this attempt is left alone.
        headless::durable_json(
            &private.join(format!("{id}.claim.json")),
            &json!({"state":"refused"}),
        )
        .unwrap();
        submit(
            &dir,
            id,
            &serde_json::to_vec(&submitted(&token, "reviewer")).unwrap(),
        );
        broker.poll().unwrap();
        assert!(!private.join(format!("{id}.response.json")).exists());
        // Once admitted, removing the durable claim does not reopen admission.
        let replay = "fedcba9876543210";
        let mut invalid = submitted(&token, "reviewer");
        invalid.timeout_seconds = 0;
        submit(&dir, replay, &serde_json::to_vec(&invalid).unwrap());
        broker.poll().unwrap();
        assert!(refusal(&private, replay).contains("outside the frozen host grant"));
        std::fs::remove_file(private.join(format!("{replay}.claim.json"))).unwrap();
        std::fs::remove_file(private.join(format!("{replay}.response.json"))).unwrap();
        broker.poll().unwrap();
        broker.finish().unwrap();
        assert!(!private.join(format!("{replay}.claim.json")).exists());
    }

    #[test]
    fn admission_stops_at_the_request_limit_for_one_attempt() {
        let (_dir, repo) = repo();
        let record = owner(&repo);
        let spec = spec(2, Vec::new());
        let dir = mailbox(&repo);
        let mut broker = Broker::new(&dir, &record, &spec).unwrap();
        for index in 0..264u32 {
            std::fs::write(dir.join(format!("{index:016x}.request.json")), b"{").unwrap();
        }
        let mut error = None;
        for _ in 0..40 {
            if let Err(failure) = broker.poll() {
                error = Some(failure.to_string());
                break;
            }
        }
        broker.finish().unwrap();
        let error = error.expect("admission stops at the retention limit");
        assert!(
            error.contains("broker request limit (256 per attempt)"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_dir(broker_private(&repo))
                .unwrap()
                .filter(|entry| {
                    entry
                        .as_ref()
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".claim.json")
                })
                .count(),
            256
        );
    }

    fn broker_private(repo: &crate::git::Repo) -> PathBuf {
        mailbox(repo).with_file_name("attempt-1").join("broker")
    }

    #[test]
    fn admission_stops_when_the_mailbox_holds_too_many_entries() {
        let (_dir, repo) = repo();
        let record = owner(&repo);
        let spec = spec(2, Vec::new());
        let dir = mailbox(&repo);
        let mut broker = Broker::new(&dir, &record, &spec).unwrap();
        // Names that can never be requests still cost a directory entry, so the
        // scan itself has to be bounded.
        for index in 0..4097u32 {
            std::fs::write(dir.join(format!("junk-{index}")), b"").unwrap();
        }
        let mut error = None;
        for _ in 0..200 {
            if let Err(failure) = broker.poll() {
                error = Some(failure.to_string());
                break;
            }
        }
        broker.finish().unwrap();
        let error = error.expect("the mailbox scan is bounded");
        assert!(
            error.contains("broker mailbox entry limit (4096)"),
            "{error}"
        );
    }

    #[test]
    fn a_dispatch_claim_is_durable_before_the_child_starts() {
        let (_dir, repo) = repo();
        let record = owner(&repo);
        let spec = spec(
            2,
            vec![grant("writer", crate::agent::Permissions::Auto, "bounded")],
        );
        let dir = mailbox(&repo);
        let mut broker = Broker::new(&dir, &record, &spec).unwrap();
        let private = broker.private_dir.clone();
        // Closing admission before the dispatch keeps this test from depending on
        // a child launch, while still exercising the claim, the prompt hand-off
        // and the bounded dispatch itself.
        headless::durable_json(
            &private.with_file_name("admission-closed.json"),
            &json!({"at": task::now_rfc3339()}),
        )
        .unwrap();
        let id = "0f1e2d3c4b5a6978";
        let mut request = submitted(&broker.token, "writer");
        request.allow_widened_approvals = true;
        request.native_helpers = Some("bounded".into());
        request.title = Some("Review".into());
        request.summary = Some("A synthetic assignment".into());
        request.name = Some("reviewer-1".into());
        submit(&dir, id, &serde_json::to_vec(&request).unwrap());
        broker.poll().unwrap();
        let claim: Value = headless::read_json(&private.join(format!("{id}.claim.json"))).unwrap();
        assert_eq!(claim["state"], "dispatching", "{claim}");
        assert_eq!(claim["request_id"], id);
        assert_eq!(claim["parent_attempt"], 1);
        assert_eq!(
            claim["request_digest"],
            crate::util::digest_bytes(&serde_json::to_vec(&request).unwrap())
        );
        let prompt = private.join(format!("{id}.prompt.txt"));
        assert_eq!(std::fs::read(&prompt).unwrap(), b"review the diff");
        assert_eq!(
            std::fs::metadata(&prompt).unwrap().permissions().mode() & 0o777,
            0o600
        );
        broker.finish().unwrap();
        let response: Value =
            headless::read_json(&private.join(format!("{id}.response.json"))).unwrap();
        assert_eq!(response["ok"], false, "{response}");
        assert_eq!(response["error"], "child dispatch unavailable");
    }

    #[test]
    fn a_child_that_fails_to_start_is_reported_with_its_exit_status() {
        let (_dir, repo) = repo();
        let record = owner(&repo);
        let spec = spec(
            2,
            vec![grant(
                "reviewer",
                crate::agent::Permissions::Prompt,
                "disabled",
            )],
        );
        let dir = mailbox(&repo);
        let mut broker = Broker::new(&dir, &record, &spec).unwrap();
        let private = broker.private_dir.clone();
        let id = "1a2b3c4d5e6f7089";
        submit(
            &dir,
            id,
            &serde_json::to_vec(&submitted(&broker.token, "reviewer")).unwrap(),
        );
        // The dispatch re-executes this binary, which is the test harness here and
        // rejects the launch arguments. What is under test is the reporting: a
        // child that did not start is a failure carrying its exit status, never a
        // launch the requester could mistake for an accepted assignment.
        broker.poll().unwrap();
        broker.finish().unwrap();
        let response: Value =
            headless::read_json(&private.join(format!("{id}.response.json"))).unwrap();
        assert_eq!(response["ok"], false, "{response}");
        assert_eq!(response["error"], "child dispatch failed", "{response}");
        assert!(
            response["exit_code"].as_i64().is_some_and(|code| code != 0),
            "{response}"
        );
        assert!(response["launch"].is_null(), "{response}");
    }

    #[test]
    fn a_bounded_dispatch_captures_both_streams() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "printf '{\"task_id\":\"child\"}'; printf warning >&2"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = bounded_dispatch(
            &mut command,
            &dir.path().join("admission-closed.json"),
            &dir.path().join("cancel.json"),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, br#"{"task_id":"child"}"#);
        assert_eq!(output.stderr, b"warning");
    }

    #[test]
    fn a_cancelled_parent_stops_a_pending_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let cancelled = dir.path().join("cancel.json");
        std::fs::write(&cancelled, b"{}").unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "printf ok"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let error = bounded_dispatch(
            &mut command,
            &dir.path().join("admission-closed.json"),
            &cancelled,
        )
        .expect_err("a cancelling parent stops the dispatch")
        .to_string();
        assert!(error.contains("broker admission closed"), "{error}");
    }

    #[test]
    fn dispatch_output_beyond_the_capture_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "dd if=/dev/zero bs=1048576 count=3 2>/dev/null"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let error = bounded_dispatch(
            &mut command,
            &dir.path().join("admission-closed.json"),
            &dir.path().join("cancel.json"),
        )
        .expect_err("more than 2 MiB of output is refused")
        .to_string();
        assert!(error.contains("capture limit"), "{error}");
    }

    #[test]
    fn a_request_file_must_be_an_owner_only_regular_file() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("request.json");
        let request = Request {
            schema_version: 1,
            capability: "0".repeat(16),
            parent_task: TASK.into(),
            agent: "reviewer".into(),
            prompt: "review".into(),
            title: None,
            summary: None,
            name: None,
            timeout_seconds: 60,
            native_helpers: None,
            allow_widened_approvals: false,
        };
        crate::state::write_private_file(&path, &serde_json::to_vec(&request).unwrap()).unwrap();
        let read = read_request(&path).unwrap();
        assert_eq!(read.agent, "reviewer");
        assert_eq!(read.prompt, "review");

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = read_failure(&path);
        assert!(error.contains("owner-only regular file"), "{error}");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let linked = dir.path().join("linked.json");
        std::fs::hard_link(&path, &linked).unwrap();
        let error = read_failure(&linked);
        assert!(error.contains("one link"), "{error}");
        std::fs::remove_file(&linked).unwrap();

        let alias = dir.path().join("alias.json");
        symlink(&path, &alias).unwrap();
        assert!(read_request(&alias).is_err(), "a symlink is never followed");
        assert!(
            read_request(dir.path()).is_err(),
            "a directory is not a request"
        );

        let unknown = dir.path().join("unknown.json");
        crate::state::write_private_file(&unknown, br#"{"schema_version":1,"extra":true}"#)
            .unwrap();
        let error = read_failure(&unknown);
        assert!(error.contains("invalid broker request"), "{error}");

        let oversized = dir.path().join("oversized.json");
        crate::state::write_private_file(&oversized, &vec![b'a'; 2 * 1024 * 1024 + 1]).unwrap();
        let error = read_failure(&oversized);
        assert!(error.contains("exceeds 2 MiB"), "{error}");
    }
}
