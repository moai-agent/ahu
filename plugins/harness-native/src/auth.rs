use crate::util::{Error, Result, digest_bytes};
use serde_json::{Value, json};
use std::{io::Read, path::Path};
pub struct Identity {
    pub harness: String,
    pub identity_kind: String,
    pub principal: String,
    pub organization: Option<String>,
    pub workspace: Option<String>,
}

impl Identity {
    fn fingerprint(&self) -> Result<String> {
        let bytes = serde_json::to_vec(&json!({
            "harness": self.harness,
            "identity_kind": self.identity_kind,
            "principal": self.principal,
            "organization": self.organization,
            "workspace": self.workspace,
        }))?;
        Ok(digest_bytes(&bytes))
    }
}
fn probe_ollama(configured: &str) -> Result<Identity> {
    let endpoint = if configured.contains("://") {
        configured.to_owned()
    } else {
        format!("http://{configured}")
    };
    let mut base = reqwest::Url::parse(&endpoint)
        .map_err(|_| Error::new("OLLAMA_HOST is not a valid local server address"))?;
    let local = base
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|address| address.is_loopback())
        || matches!(base.host_str(), Some("localhost"));
    if !local || !matches!(base.scheme(), "http" | "https") {
        return Err(Error::new(
            "Ollama account binding only supports a loopback Ollama server",
        ));
    }
    base.set_path("/api/me");
    base.set_query(None);
    base.set_fragment(None);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(4))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| Error::new("cannot initialize the Ollama identity check"))?;
    let response = client
        .post(base)
        .json(&json!({}))
        .send()
        .map_err(|_| Error::new("cannot reach the local Ollama account identity endpoint"))?;
    if !response.status().is_success() {
        return Err(Error::new(
            "local Ollama did not provide a signed-in account identity",
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > 64 * 1024)
    {
        return Err(Error::new("Ollama account identity response is too large"));
    }
    let mut bytes = Vec::new();
    response
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::new("cannot read the Ollama account identity response"))?;
    if bytes.len() > 64 * 1024 {
        return Err(Error::new("Ollama account identity response is too large"));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::new("Ollama account identity response is invalid"))?;
    let principal = value
        .get("email")
        .and_then(Value::as_str)
        .filter(|email| email.contains('@') && !email.chars().any(char::is_control))
        .ok_or_else(|| Error::new("Ollama is not signed in or returned no account email"))?;
    let account_id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::new("Ollama returned no stable account identifier"))?;
    Ok(Identity {
        harness: "ollama".into(),
        identity_kind: "ollama-account".into(),
        principal: principal.into(),
        organization: None,
        workspace: Some(account_id.into()),
    })
}

fn kiro_identity(output: &[u8]) -> Result<Identity> {
    let value: Value = serde_json::from_slice(output)
        .map_err(|_| Error::new("Kiro whoami returned invalid JSON; details omitted"))?;
    let account_type = value
        .get("accountType")
        .and_then(Value::as_str)
        .filter(|s| *s == "SocialGitHub")
        .ok_or_else(|| {
            Error::new("Kiro account binding currently supports verified GitHub social login only")
        })?;
    let principal = value
        .get("email")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| Error::new("Kiro whoami did not expose a verifiable account identity"))?;
    Ok(Identity {
        harness: "kiro".into(),
        identity_kind: account_type.into(),
        principal: principal.into(),
        organization: None,
        workspace: None,
    })
}

fn probe_claude(
    executable: &Path,
    cwd: &Path,
    home: &Path,
    environment: &crate::Environment,
) -> Result<Identity> {
    let output = crate::process::run(
        executable,
        &["auth", "status", "--json"],
        cwd,
        home,
        environment,
        std::time::Duration::from_secs(8),
    )?;
    let value: Value = serde_json::from_slice(&output)
        .map_err(|_| Error::new("Claude auth status returned invalid JSON"))?;
    if value.get("loggedIn").and_then(Value::as_bool) != Some(true) {
        return Err(Error::diagnosed(crate::Diagnostic::NotSignedIn));
    }
    let principal = value
        .get("email")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Error::new("Claude auth status does not expose a verifiable account identity")
        })?;
    let org = value
        .get("orgId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let auth_method = value
        .get("authMethod")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    Ok(Identity {
        harness: "claude-code".into(),
        identity_kind: format!(
            "{auth_method}/{}",
            value
                .get("apiProvider")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ),
        principal: principal.into(),
        organization: org.map(str::to_owned),
        workspace: None,
    })
}

fn codex_identity(account: &Value) -> Result<Identity> {
    // account/read returns an envelope with `account` and routing metadata;
    // tolerate a flattened shape as well for compatible app-server versions.
    let account_details = account
        .get("account")
        .filter(|value| value.is_object())
        .unwrap_or(account);
    if account_details.get("type").and_then(Value::as_str) != Some("chatgpt") {
        return Err(Error::new(
            "Codex API-key authentication does not expose a verifiable user principal; account binding is unavailable",
        ));
    }
    let routing = account
        .get("workspaceRouting")
        .or_else(|| account_details.get("workspaceRouting"))
        .unwrap_or(&Value::Null);
    let principal = account_details
        .get("email")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| routing.get("chatgptAccountId").and_then(Value::as_str))
        .ok_or_else(|| Error::new("Codex account metadata has no verifiable account identifier"))?;
    Ok(Identity {
        harness: "codex".into(),
        identity_kind: account_details
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .into(),
        principal: principal.into(),
        organization: None,
        workspace: match (
            routing.get("chatgptAccountId").and_then(Value::as_str),
            routing.get("backendOrigin").and_then(Value::as_str),
        ) {
            (Some(id), Some(origin)) => Some(format!("{id}@{origin}")),
            (Some(id), None) => Some(id.to_owned()),
            (None, Some(origin)) => Some(origin.to_owned()),
            (None, None) => None,
        },
    })
}

fn parse_antigravity_identity(output: &[u8]) -> Result<Identity> {
    if antigravity_needs_folder_trust(output) {
        return Err(antigravity_folder_trust_error());
    }
    let principal = extract_email(output).ok_or_else(|| {
        Error::new("Antigravity startup did not expose a signed-in account email")
    })?;
    let visible = String::from_utf8_lossy(output);
    let identity_kind = if visible.contains("Agent Platform") {
        "agent-platform"
    } else if visible.contains("Gemini API key") {
        return Err(Error::new(
            "Antigravity is using API-key authentication and exposes no user principal",
        ));
    } else {
        "antigravity-account"
    };
    Ok(Identity {
        harness: "antigravity".into(),
        identity_kind: identity_kind.into(),
        principal,
        organization: None,
        workspace: extract_labeled_value(&visible, "GCP Project:"),
    })
}

fn antigravity_needs_folder_trust(output: &[u8]) -> bool {
    let visible = String::from_utf8_lossy(output);
    visible.contains("Do you trust the contents of this project?")
        || visible.contains("Do you trust this folder?")
}

// Open agy in the checkout interactively and review its prompt; ahu never
// approves native folder trust automatically.
fn antigravity_folder_trust_error() -> Error {
    Error::diagnosed(crate::Diagnostic::NeedsNativeTrust)
}

fn antigravity_startup(
    executable: &Path,
    cwd: &Path,
    home: &Path,
    environment: &crate::Environment,
) -> Result<Vec<u8>> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut master_fd = -1;
    let mut slave_fd = -1;
    let mut size = libc::winsize {
        ws_row: 30,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty initializes two owned descriptors, which are wrapped
    // immediately below. No name or termios override is requested.
    if unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            // libc exposes a mutable winsize pointer on macOS and a const
            // pointer on Linux; a raw pointer coerces correctly on both.
            &raw mut size,
        )
    } != 0
    {
        return Err(Error::new(
            "cannot create bounded Antigravity status terminal",
        ));
    }
    // SAFETY: each descriptor was freshly returned by openpty and has one owner.
    let mut master = unsafe { std::fs::File::from_raw_fd(master_fd) };
    // SAFETY: each descriptor was freshly returned by openpty and has one owner.
    let slave = unsafe { std::fs::File::from_raw_fd(slave_fd) };
    let stdin = slave.try_clone()?;
    let stdout = slave.try_clone()?;
    let mut command = Command::new(executable);
    crate::process::configure(&mut command, home, environment);
    command
        .current_dir(cwd)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(slave));
    command
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|_| Error::new("cannot start Antigravity identity probe"))?;
    // SAFETY: master owns a live PTY descriptor; only descriptor flags are changed.
    let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        crate::process::signal_group(child.id(), libc::SIGKILL);
        let _ = child.wait();
        return Err(Error::new("cannot bound Antigravity startup reads"));
    }
    let deadline = Instant::now() + Duration::from_secs(12);
    // The startup TUI can split the email, auth mode, and GCP project across
    // PTY writes. A recognized mode does not mean the fingerprint is complete:
    // wait for a bounded quiet window, including after "Agent Platform". The
    // CLI exposes no explicit identity-complete event, so this reduces late
    // field races without claiming that silence proves completeness. This does
    // not extend the overall probe deadline.
    let identity_settle = Duration::from_millis(2_500);
    let mut output = Vec::new();
    let mut last_output_at: Option<Instant> = None;
    let mut buffer = [0u8; 4096];
    let outcome = loop {
        if antigravity_needs_folder_trust(&output) {
            break Err(antigravity_folder_trust_error());
        }
        if extract_email(&output).is_some()
            && last_output_at.is_some_and(|last| last.elapsed() >= identity_settle)
        {
            break Ok(output);
        }
        if output.len() >= 64 * 1024 {
            break Err(Error::new(
                "Antigravity startup exceeded the identity output limit",
            ));
        }
        if Instant::now() >= deadline {
            break Err(Error::new("Antigravity startup identity check timed out"));
        }
        let mut descriptor = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: descriptor is one initialized pollfd and master retains the fd.
        let ready = unsafe { libc::poll(&mut descriptor, 1, 150) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break Err(Error::new("cannot wait for Antigravity startup identity"));
        }
        if ready > 0 {
            match master.read(&mut buffer) {
                Ok(0) => break Err(Error::new("Antigravity closed its startup terminal")),
                Ok(count) => {
                    output.extend_from_slice(&buffer[..count]);
                    last_output_at = Some(Instant::now());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => (),
                Err(_) => break Err(Error::new("cannot read Antigravity startup identity")),
            }
        }
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                break Err(Error::new(
                    "Antigravity exited before exposing its account identity",
                ));
            }
            Err(_) => break Err(Error::new("cannot inspect Antigravity probe process")),
            _ => (),
        }
    };
    crate::process::signal_group(child.id(), libc::SIGKILL);
    let _ = child.wait();
    outcome
}

fn extract_email(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    for token in
        text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>' | '"' | '\''))
    {
        let token = token.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | ']' | '['));
        let Some((local, domain)) = token.split_once('@') else {
            continue;
        };
        if local.is_empty()
            || domain.is_empty()
            || !domain.contains('.')
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-' | b'@'))
        {
            continue;
        }
        return Some(token.to_owned());
    }
    None
}

fn extract_labeled_value(text: &str, label: &str) -> Option<String> {
    text.split_once(label)?
        .1
        .lines()
        .next()?
        .split_whitespace()
        .next()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn normalize_rate_window(value: &Value) -> Option<(f64, u64, Option<u64>)> {
    let used = value.get("usedPercent")?.as_f64()?;
    if !used.is_finite() || !(0.0..=100.0).contains(&used) {
        return None;
    }
    let duration = value.get("windowDurationMins")?.as_u64()?;
    if duration == 0 || duration > 525_600 {
        return None;
    }
    let reset = match value.get("resetsAt") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64()?),
    };
    Some((used, duration, reset))
}

fn normalize_window(name: &str, (used, duration, reset): (f64, u64, Option<u64>)) -> Value {
    json!({
        "name": name,
        "used_percent": used,
        "remaining_percent": 100.0 - used,
        "window_duration_minutes": duration,
        "resets_at_unix": reset
    })
}

pub(crate) fn call(
    harness: &str,
    p: &crate::ProbeParams,
    budget: bool,
    model: Option<&str>,
    ollama_host: Option<&str>,
    requested_identity: Option<&str>,
) -> Result<Value> {
    if requested_identity
        .is_some_and(|id| id != harness && !(harness == "opencode" && id == "ollama"))
    {
        return Err(Error::invalid());
    }
    if budget {
        if harness != "codex" {
            return Ok(json!({"status":"unsupported","windows":[],"signal_type":"unknown"}));
        }
        let (account, response) = crate::codex_metadata::read_budget(
            &p.native_executable,
            &p.project,
            &p.home,
            &p.environment,
        )?;
        let identity = codex_identity(&account)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Error::invalid())?
            .as_secs();
        let limits = response.get("rateLimits").unwrap_or(&response);
        let mut windows = Vec::new();
        let mut incomplete = false;
        for key in ["primary", "secondary"] {
            if let Some(raw) = limits.get(key).filter(|v| !v.is_null()) {
                match normalize_rate_window(raw) {
                    Some(values) if values.2.is_none_or(|reset| reset > now) => {
                        windows.push(normalize_window(key, values))
                    }
                    _ => incomplete = true,
                }
            }
        }
        if incomplete {
            windows.clear();
        }
        return Ok(
            json!({"status":if windows.is_empty(){"unknown"}else{"available"},"fingerprint":identity.fingerprint()?,"binding_harness":"codex","signal_type":if windows.is_empty(){"unknown"}else{"rate_windows"},"source":"codex_app_server","scope":"shared_account","observed_at_unix":now,"windows":windows}),
        );
    }
    if harness == "kiro" && p.environment.contains_key("KIRO_API_KEY") {
        return Err(Error::invalid());
    }
    let identity = match harness {
        "codex" => codex_identity(&crate::codex_metadata::read_account(
            &p.native_executable,
            &p.project,
            &p.home,
            &p.environment,
        )?)?,
        "claude-code" => probe_claude(&p.native_executable, &p.project, &p.home, &p.environment)?,
        "kiro" => kiro_identity(&crate::process::run(
            &p.native_executable,
            &["whoami", "--format", "json"],
            &p.project,
            &p.home,
            &p.environment,
            std::time::Duration::from_secs(8),
        )?)?,
        "antigravity" => parse_antigravity_identity(&antigravity_startup(
            &p.native_executable,
            &p.project,
            &p.home,
            &p.environment,
        )?)?,
        "opencode" => {
            let model = model.unwrap_or("").to_ascii_lowercase();
            let cloud = model.split_once('/').is_some_and(|(provider, name)| {
                provider == "ollama" && (name.contains(":cloud") || name.ends_with("-cloud"))
            });
            if !cloud && requested_identity != Some("ollama") {
                return Ok(
                    json!({"status":"unsupported","binding_harness":null,"fingerprint":null}),
                );
            }
            probe_ollama(
                ollama_host
                    .or_else(|| p.environment.get("OLLAMA_HOST").map(String::as_str))
                    .unwrap_or("http://127.0.0.1:11434"),
            )?
        }
        _ => return Err(Error::invalid()),
    };
    Ok(
        json!({"status":"available","binding_harness":identity.harness,"fingerprint":identity.fingerprint()?,"effective_provider_address_verified":harness!="opencode","identity_kind":identity.identity_kind}),
    )
}
