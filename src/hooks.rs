//! Harness hooks: shell commands the harness runs on its own lifecycle events.
//!
//! ahu reports visible hooks with their scope, event, and digest. Hooks outside
//! shared project policy raise a consistency warning because they can differ
//! between teammates. Local repository settings travel with the snapshot; home
//! and managed settings remain at their native locations.
//!
//! ahu never adds, edits, removes, or disables hooks. Hook and approval-settings
//! inspection covers Claude Code; other harnesses report unknown coverage.
//! Hook commands are not serialized because they can contain credentials.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::util::{Result, digest_bytes, display_safe};

/// Where a hook is configured, which decides whether it is project policy and
/// whether it travels into a task worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// `<repo>/.claude/settings.json` — shared project policy.
    Project,
    /// `<repo>/.claude/settings.local.json` — in the repository, but typically
    /// ignored by Git, so not necessarily shared or reviewed.
    ProjectLocal,
    /// `~/.claude/settings.json` — this machine's user only.
    User,
    /// Machine policy. Required, and not removable by ahu.
    Managed,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::ProjectLocal => "project-local",
            Scope::User => "user",
            Scope::Managed => "managed",
        }
    }

    /// Whether a hook at this scope is part of the shared project policy that
    /// every ahu user in the project is supposed to be running.
    pub fn is_project_policy(self) -> bool {
        matches!(self, Scope::Project)
    }

    /// Why a hook at this scope is not shared project policy.
    ///
    /// The reasons genuinely differ: a `settings.local.json` hook does travel
    /// into the worktree but is typically Git-ignored, while a user hook is the
    /// opposite. Saying "does not travel" for both would be wrong.
    pub fn why_not_project_policy(self) -> &'static str {
        match self {
            Scope::Project => "it is project policy",
            Scope::ProjectLocal => {
                "in the repository and it does travel into the task worktree, but this file is \
                 conventionally Git-ignored, so it may never have been shared or reviewed"
            }
            Scope::User => {
                "configured in a home directory: it does not travel into the task worktree and \
                 may differ for every teammate"
            }
            Scope::Managed => {
                "machine policy: it does not travel into the task worktree and ahu cannot \
                 remove or override it"
            }
        }
    }

    /// Whether configuration at this scope is copied into a task worktree.
    ///
    /// Only paths inside the repository travel; a hook in the user's home
    /// directory keeps its native handling and applies from wherever the
    /// harness reads it.
    pub fn travels_into_worktree(self) -> bool {
        matches!(self, Scope::Project | Scope::ProjectLocal)
    }
}

/// One configured hook command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hook {
    /// Harness lifecycle event, e.g. `PreToolUse`.
    pub event: String,
    /// Matcher the event is filtered by, when the event supports one.
    pub matcher: Option<String>,
    /// `command` for a shell hook; anything else is recorded as declared.
    pub kind: String,
    /// The shell command, when this is a command hook.
    ///
    /// Never serialized: hook commands routinely embed credentials, and task
    /// records are ordinary files. `command_digest` identifies the command for
    /// drift detection without storing it.
    #[serde(skip)]
    pub command: Option<String>,
    /// Digest of the command, which is what gets persisted and compared.
    #[serde(default)]
    pub command_digest: String,
    pub scope: Scope,
    /// Settings file the hook is declared in, repository-relative when inside
    /// the repository and absolute otherwise.
    pub source: String,
}

impl Hook {
    /// A short, stable label for terminal output.
    ///
    /// Only the command's program is shown, not its arguments: hook commands
    /// routinely carry tokens in their arguments, and an inventory must not leak
    /// a credential merely to describe a hook. The digest identifies the whole
    /// command for drift purposes.
    pub fn label(&self) -> String {
        let target = match &self.command {
            Some(command) => program_label(command),
            None => format!("<{} hook>", display_safe(&self.kind)),
        };
        let event = display_safe(&self.event);
        match &self.matcher {
            Some(matcher) if !matcher.is_empty() => {
                format!("{event}:{} → {target}", display_safe(matcher))
            }
            _ => format!("{event} → {target}"),
        }
    }

    /// The settings file this hook came from, safe to print.
    pub fn source_label(&self) -> String {
        display_safe(&self.source)
    }

    pub fn digest(&self) -> String {
        digest_bytes(
            format!(
                "{}|{}|{}|{}|{}",
                self.scope.as_str(),
                self.event,
                self.matcher.as_deref().unwrap_or(""),
                self.kind,
                self.command_digest
            )
            .as_bytes(),
        )
    }
}

/// Name the program a hook command runs, without printing a credential.
///
/// The first whitespace-separated word is a program name only when the command
/// does not open with shell assignments. `API_TOKEN=secret checker` puts the
/// credential in exactly that position, and truncating it to 48 characters is
/// not redaction. Quoting makes the following words unsafe to guess at too — in
/// `API_TOKEN="a b" checker` the second word is still part of the value — so ahu
/// stops trying to identify a program the moment it sees an assignment and
/// leaves the command to its digest.
fn program_label(command: &str) -> String {
    let mut words = command.split_whitespace();
    let Some(first) = words.next() else {
        return "<empty command>".to_string();
    };
    if first.contains('=') {
        return "<command opens with an inline environment assignment; identified by digest only>"
            .to_string();
    }
    let rendered = display_safe(&truncate(first, 48));
    if words.next().is_some() {
        format!("{rendered} …")
    } else {
        rendered
    }
}

fn truncate(value: &str, limit: usize) -> String {
    let flat = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let kept: String = flat.chars().take(limit - 1).collect();
    format!("{}…", kept.trim_end())
}

/// What a settings file says about the session's approval boundary.
///
/// Native settings can affect approvals independently of manifest flags, so
/// the inventory reports their relevant declarations without inferring an
/// effective boundary.
///
/// Values are only ever repeated for keys whose values are policy. `env` is the
/// exception: its values are routinely credentials, so only the names are kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsFacts {
    /// Settings file this came from, labelled as the hook scan labels it.
    pub source: String,
    pub scope: Scope,
    /// `permissions.defaultMode`, e.g. `bypassPermissions`.
    pub default_mode: Option<String>,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub ask: Vec<String>,
    pub additional_directories: Vec<String>,
    pub enabled_plugins: Vec<String>,
    pub enable_all_project_mcp_servers: Option<bool>,
    pub enabled_mcpjson_servers: Vec<String>,
    /// Names only. A settings `env` block routinely holds API tokens.
    pub env_names: Vec<String>,
    /// Top-level keys ahu does not interpret, named rather than silently
    /// dropped — the same treatment the hook scan already gives a shape it
    /// cannot walk.
    pub uninterpreted_keys: Vec<String>,
}

impl SettingsFacts {
    fn new(source: &str, scope: Scope) -> Self {
        SettingsFacts {
            source: source.to_string(),
            scope,
            default_mode: None,
            allow: Vec::new(),
            deny: Vec::new(),
            ask: Vec::new(),
            additional_directories: Vec::new(),
            enabled_plugins: Vec::new(),
            enable_all_project_mcp_servers: None,
            enabled_mcpjson_servers: Vec::new(),
            env_names: Vec::new(),
            uninterpreted_keys: Vec::new(),
        }
    }

    /// Whether this file says anything about the approval boundary at all.
    pub fn is_empty(&self) -> bool {
        self.default_mode.is_none()
            && self.allow.is_empty()
            && self.deny.is_empty()
            && self.ask.is_empty()
            && self.additional_directories.is_empty()
            && self.enabled_plugins.is_empty()
            && self.enable_all_project_mcp_servers.is_none()
            && self.enabled_mcpjson_servers.is_empty()
            && self.env_names.is_empty()
            && self.uninterpreted_keys.is_empty()
    }

    /// Whether this file widens what the harness would otherwise ask about.
    ///
    /// Deliberately generous: anything that pre-approves tools, adds a directory
    /// the session may reach, or removes a trust prompt counts.
    pub fn widens_approvals(&self) -> bool {
        matches!(
            self.default_mode.as_deref(),
            Some("bypassPermissions") | Some("acceptEdits") | Some("auto") | Some("dontAsk")
        ) || !self.allow.is_empty()
            || !self.additional_directories.is_empty()
            || self.enable_all_project_mcp_servers == Some(true)
            || !self.enabled_mcpjson_servers.is_empty()
            || !self.enabled_plugins.is_empty()
    }
}

/// An MCP server a repository asks the harness to start.
///
/// A repository MCP declaration is copied with configuration and reported as a
/// potential subprocess source. Whether it runs depends on the selected harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServer {
    pub source: String,
    pub name: String,
    /// The command as declared, including its arguments. This is a process the
    /// session will start, so it is shown whole rather than reduced to a program
    /// name — unlike a hook label, an MCP `command` is not a place credentials
    /// conventionally live, and the reader needs to see `sh -c curl … | sh`.
    pub command: String,
}

/// A plugin module an `opencode.json` declares.
///
/// OpenCode installs and runs these at startup, so a name here is executable
/// configuration the reader is trusting, not an inert setting. ahu records the
/// module string exactly as declared — it may be an npm specifier that resolves
/// to code ahu never sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredPlugin {
    pub source: String,
    pub module: String,
}

/// Every hook ahu could find, plus an honest account of what it could not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookInventory {
    pub hooks: Vec<Hook>,
    /// What the settings files say beyond their `hooks` key.
    #[serde(default)]
    pub settings: Vec<SettingsFacts>,
    /// MCP servers declared by the repository's `.mcp.json`.
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    /// Plugin modules declared by the repository's `opencode.json(c)`.
    ///
    /// Skipped when empty so a record written by a build without this field and
    /// a record written by one with it are byte-identical. Task records are
    /// frozen and digested; a new always-present `[]` would change the digest of
    /// every existing record that declares no plugin, which would read as
    /// configuration drift where nothing about the configuration moved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declared_plugins: Vec<DeclaredPlugin>,
    /// Settings files that exist but could not be parsed. Their hooks are
    /// unknown, which is reported rather than treated as "none".
    pub unreadable: Vec<String>,
    /// Settings files ahu looked for and did not find. Used to say which scopes
    /// were actually checked.
    pub checked: Vec<String>,
    /// Set when ahu has no implementation of this launch's harness hook surface,
    /// so "none found" would be a false negative rather than a result.
    #[serde(default)]
    pub unscanned_harness: Option<String>,
    /// True when ahu is running under a cmux terminal, whose Claude wrapper
    /// injects hooks of its own that ahu cannot enumerate.
    pub wrapper_injected: bool,
}

impl HookInventory {
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Hooks that are not shared project policy. These raise the consistency
    /// warning: they change behaviour and can differ for every teammate.
    pub fn outside_project_policy(&self) -> Vec<&Hook> {
        self.hooks
            .iter()
            .filter(|hook| !hook.scope.is_project_policy())
            .collect()
    }

    /// Hooks copied into the task worktree, which will run there.
    pub fn travelling(&self) -> Vec<&Hook> {
        self.hooks
            .iter()
            .filter(|hook| hook.scope.travels_into_worktree())
            .collect()
    }

    /// Settings files whose approval keys widen what the harness would ask about.
    pub fn widening_settings(&self) -> Vec<&SettingsFacts> {
        self.settings
            .iter()
            .filter(|facts| facts.widens_approvals())
            .collect()
    }

    /// Digest over every hook ahu can see, at every scope.
    ///
    /// The configuration snapshot digest already covers hooks declared inside
    /// the repository. This one also covers user and managed scopes, so a task
    /// launched after a teammate's personal hook changed is reported as drifted
    /// rather than presented as the same effective inputs.
    ///
    /// The settings and MCP facts are in it for the same reason: a change to
    /// `permissions.defaultMode` in a user's own settings file changes the
    /// session's approval boundary without touching anything in the repository
    /// snapshot, and a launch after that change is not the same launch.
    pub fn digest(&self) -> String {
        let mut buffer = String::new();
        let mut digests: Vec<String> = self.hooks.iter().map(|hook| hook.digest()).collect();
        digests.sort();
        for digest in digests {
            buffer.push_str(&digest);
            buffer.push('\n');
        }
        let mut facts: Vec<String> = self
            .settings
            .iter()
            .map(|f| digest_bytes(format!("{f:?}").as_bytes()))
            .chain(
                self.mcp_servers
                    .iter()
                    .map(|m| digest_bytes(format!("{m:?}").as_bytes())),
            )
            // Empty contributes nothing, so a repository declaring no plugin
            // digests exactly as it did before this field existed.
            .chain(
                self.declared_plugins
                    .iter()
                    .map(|p| digest_bytes(format!("{p:?}").as_bytes())),
            )
            .collect();
        facts.sort();
        for digest in facts {
            buffer.push_str(&digest);
            buffer.push('\n');
        }
        if let Some(harness) = &self.unscanned_harness {
            buffer.push_str(harness);
            buffer.push_str("-unscanned\n");
        }
        if self.wrapper_injected {
            buffer.push_str("cmux-wrapper\n");
        }
        digest_bytes(buffer.as_bytes())
    }

    pub fn short_digest(&self) -> String {
        self.digest()[..12].to_string()
    }
}

/// Where the non-repository settings files live on this machine.
///
/// Taken as a parameter rather than read from the environment inside the scan so
/// the whole hook inventory is testable without mutating process-wide state.
#[derive(Debug, Clone, Default)]
pub struct Locations {
    /// The user's home directory, holding `~/.claude/settings.json`.
    pub home: Option<PathBuf>,
    /// Machine policy settings.
    pub managed: Option<PathBuf>,
    /// Whether a cmux Claude wrapper is in play, injecting hooks ahu cannot read.
    pub cmux_wrapper: bool,
}

impl Locations {
    pub fn detect() -> Self {
        Locations {
            home: std::env::var_os("HOME").map(PathBuf::from),
            managed: Some(PathBuf::from(
                "/Library/Application Support/ClaudeCode/managed-settings.json",
            )),
            // The installed cmux states that its Claude wrapper injects Claude
            // Code hooks automatically. ahu launches Claude through cmux, so
            // inside one this source is always present and never readable.
            cmux_wrapper: std::env::var_os("CMUX_CLAUDE_WRAPPER_SHIM").is_some()
                || std::env::var_os("CMUX_SOCKET_PATH").is_some(),
        }
    }
}

/// One settings file ahu will try to read hooks from.
struct SettingsFile {
    scope: Scope,
    path: PathBuf,
    display: String,
    /// Repository-root-relative path, for the scopes that live in the
    /// repository. `None` for files in a home directory or machine policy,
    /// which are the user's own and are not resolved against the repository.
    relative: Option<&'static str>,
}

/// Settings files ahu reads hooks from, in the harness's own precedence order.
fn settings_files(repo_root: &Path, locations: &Locations) -> Vec<SettingsFile> {
    let mut found = vec![
        SettingsFile {
            scope: Scope::Project,
            path: repo_root.join(".claude/settings.json"),
            display: ".claude/settings.json".to_string(),
            relative: Some(".claude/settings.json"),
        },
        SettingsFile {
            scope: Scope::ProjectLocal,
            path: repo_root.join(".claude/settings.local.json"),
            display: ".claude/settings.local.json".to_string(),
            relative: Some(".claude/settings.local.json"),
        },
    ];
    if let Some(home) = &locations.home {
        let path = home.join(".claude/settings.json");
        let display = path.to_string_lossy().to_string();
        found.push(SettingsFile {
            scope: Scope::User,
            path,
            display,
            relative: None,
        });
    }
    if let Some(managed) = &locations.managed {
        let display = managed.to_string_lossy().to_string();
        found.push(SettingsFile {
            scope: Scope::Managed,
            path: managed.clone(),
            display,
            relative: None,
        });
    }
    found
}

/// Whether ahu has an implementation of a harness's hook configuration.
///
/// Only Claude Code settings are enumerated. Other harnesses need an explicit
/// unknown-coverage report rather than a misleading empty inventory.
pub fn hook_surface_is_implemented(harness_id: &str) -> bool {
    harness_id == "claude-code"
}

/// Read every hook ahu can see for a launch of `harness_id` from `repo_root`.
pub fn collect(repo_root: &Path, harness_id: &str) -> Result<HookInventory> {
    collect_for(repo_root, harness_id, &Locations::detect())
}

/// Read hooks for a Claude Code launch, using explicit machine locations.
pub fn collect_in(repo_root: &Path, locations: &Locations) -> Result<HookInventory> {
    collect_for(repo_root, "claude-code", locations)
}

/// Read hooks and settings using explicit machine locations.
pub fn collect_for(
    repo_root: &Path,
    harness_id: &str,
    locations: &Locations,
) -> Result<HookInventory> {
    let mut inventory = HookInventory {
        wrapper_injected: locations.cmux_wrapper,
        ..HookInventory::default()
    };
    // `.mcp.json` is read for every harness: it is repository configuration that
    // travels into the task worktree either way. Whether a declared server
    // starts depends on the selected harness.
    collect_mcp_servers(repo_root, &mut inventory);
    // `opencode.json` likewise travels into the task worktree whatever the
    // harness is, and its `plugin` entries are modules OpenCode installs and
    // executes at startup.
    collect_opencode_plugins(repo_root, &mut inventory);
    if !hook_surface_is_implemented(harness_id) {
        inventory.unscanned_harness = Some(harness_id.to_string());
        return Ok(inventory);
    }

    for SettingsFile {
        scope,
        path,
        display,
        relative,
    } in settings_files(repo_root, locations)
    {
        // Resolve without following repository symlinks. Otherwise an external
        // settings file could be mislabeled as shared project policy, hiding
        // its warning and falsely claiming it travels into the task worktree.
        let path = match relative {
            Some(relative) => match crate::util::resolve_existing_within(repo_root, relative) {
                Ok(Some(resolved)) => resolved,
                Ok(None) => continue,
                Err(_) => {
                    inventory.unreadable.push(format!(
                        "{display} (not read: it or one of its parent directories is a symlink, \
                         so its hooks are not this repository's)"
                    ));
                    continue;
                }
            },
            None => path,
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                // Present but unreadable: report the gap rather than assume none.
                inventory.unreadable.push(display);
                continue;
            }
        };
        inventory.checked.push(display.clone());
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            inventory.unreadable.push(display);
            continue;
        };
        let facts = parse_settings(&value, scope, &display);
        if !facts.is_empty() {
            inventory.settings.push(facts);
        }
        match parse_hooks(&value, scope, &display) {
            Some(hooks) => inventory.hooks.extend(hooks),
            None => inventory.unreadable.push(display),
        }
    }

    inventory
        .hooks
        .sort_by(|a, b| (a.scope, &a.event, &a.matcher).cmp(&(b.scope, &b.event, &b.matcher)));
    Ok(inventory)
}

/// Read the repository's `.mcp.json`, naming the command of each server.
///
/// Use the same no-follow resolver as hook settings so repository symlinks
/// cannot redirect the read.
/// Record the plugin modules an `opencode.json` or `opencode.jsonc` declares.
///
/// These are named, not resolved: OpenCode installs them itself, from wherever
/// the specifier points. Reporting the names is the whole point — the snapshot
/// already carries the file and digests it, but a digest of a file naming a
/// remote module tells the reader nothing about what will run, and an npm
/// specifier is not a file the executable-bit scan can see.
///
/// A `.jsonc` with comments will not parse as JSON. That is recorded as
/// unreadable rather than as "no plugins", because the difference matters.
fn collect_opencode_plugins(repo_root: &Path, inventory: &mut HookInventory) {
    for source in ["opencode.json", "opencode.jsonc"] {
        let path = match crate::util::resolve_existing_within(repo_root, source) {
            Ok(Some(path)) => path,
            Ok(None) => continue,
            Err(_) => {
                inventory.unreadable.push(format!(
                    "{source} (not read: it or one of its parent directories is a symlink, so \
                     its plugin declarations are not this repository's)"
                ));
                continue;
            }
        };
        let Ok(bytes) = std::fs::read(&path) else {
            inventory.unreadable.push(source.to_string());
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            inventory.unreadable.push(format!(
                "{source} (not valid JSON to ahu, so its plugin declarations are unknown rather \
                 than absent; JSONC comments are not parsed)"
            ));
            continue;
        };
        let Some(plugins) = value.get("plugin") else {
            continue;
        };
        let Some(plugins) = plugins.as_array() else {
            inventory.unreadable.push(format!(
                "{source} (its plugin key is not a shape ahu understands, so its plugins are \
                 unknown rather than absent)"
            ));
            continue;
        };
        for module in plugins {
            inventory.declared_plugins.push(DeclaredPlugin {
                source: source.to_string(),
                module: module
                    .as_str()
                    .unwrap_or("(non-string plugin entry)")
                    .to_string(),
            });
        }
    }
}

fn collect_mcp_servers(repo_root: &Path, inventory: &mut HookInventory) {
    const SOURCE: &str = ".mcp.json";
    let path = match crate::util::resolve_existing_within(repo_root, SOURCE) {
        Ok(Some(path)) => path,
        Ok(None) => return,
        Err(_) => {
            inventory.unreadable.push(format!(
                "{SOURCE} (not read: it or one of its parent directories is a symlink, so its \
                 MCP servers are not this repository's)"
            ));
            return;
        }
    };
    let Ok(bytes) = std::fs::read(&path) else {
        inventory.unreadable.push(SOURCE.to_string());
        return;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        inventory.unreadable.push(SOURCE.to_string());
        return;
    };
    let Some(servers) = value.get("mcpServers") else {
        return;
    };
    let Some(servers) = servers.as_object() else {
        inventory.unreadable.push(format!(
            "{SOURCE} (its mcpServers key is not a shape ahu understands, so its servers are \
             unknown rather than absent)"
        ));
        return;
    };
    for (name, server) in servers {
        let command = server
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("(no command declared)");
        let args = server
            .get("args")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().unwrap_or("(non-string)").to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        inventory.mcp_servers.push(McpServer {
            source: SOURCE.to_string(),
            name: name.clone(),
            command: if args.is_empty() {
                command.to_string()
            } else {
                format!("{command} {args}")
            },
        });
    }
}

/// Top-level settings keys ahu interprets. Anything else is named as unknown.
const INTERPRETED_KEYS: &[&str] = &[
    "hooks",
    "permissions",
    "enabledPlugins",
    "enableAllProjectMcpServers",
    "enabledMcpjsonServers",
    "env",
];

/// Pull the approval-relevant keys out of one settings document.
fn parse_settings(value: &serde_json::Value, scope: Scope, source: &str) -> SettingsFacts {
    let mut facts = SettingsFacts::new(source, scope);
    let Some(object) = value.as_object() else {
        return facts;
    };
    for key in object.keys() {
        if !INTERPRETED_KEYS.contains(&key.as_str()) {
            facts.uninterpreted_keys.push(key.clone());
        }
    }
    if let Some(permissions) = object.get("permissions") {
        facts.default_mode = permissions
            .get("defaultMode")
            .and_then(|m| m.as_str())
            .map(str::to_string);
        facts.allow = string_list(permissions.get("allow"));
        facts.deny = string_list(permissions.get("deny"));
        facts.ask = string_list(permissions.get("ask"));
        facts.additional_directories = string_list(permissions.get("additionalDirectories"));
    }
    facts.enabled_plugins = string_list(object.get("enabledPlugins"));
    facts.enable_all_project_mcp_servers = object
        .get("enableAllProjectMcpServers")
        .and_then(|v| v.as_bool());
    facts.enabled_mcpjson_servers = string_list(object.get("enabledMcpjsonServers"));
    // Names only: a settings `env` block is a common place for an API token, and
    // an inventory must not leak a credential to describe a setting.
    if let Some(env) = object.get("env").and_then(|e| e.as_object()) {
        facts.env_names = env.keys().cloned().collect();
    }
    facts
}

fn string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| match item.as_str() {
                    Some(text) => text.to_string(),
                    None => item.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Pull hook commands out of one settings document.
///
/// Returns `None` when a `hooks` key exists but is not shaped the way ahu
/// understands, so the caller can report it as unreadable instead of silently
/// reporting no hooks.
fn parse_hooks(value: &serde_json::Value, scope: Scope, source: &str) -> Option<Vec<Hook>> {
    let Some(hooks) = value.get("hooks") else {
        return Some(Vec::new());
    };
    let events = hooks.as_object()?;
    let mut found = Vec::new();
    for (event, entries) in events {
        let entries = entries.as_array()?;
        for entry in entries {
            let matcher = entry
                .get("matcher")
                .and_then(|m| m.as_str())
                .map(str::to_string);
            // The usual shape nests the commands under `hooks`; some events take
            // a bare command entry instead.
            let commands = match entry.get("hooks").and_then(|h| h.as_array()) {
                Some(commands) => commands.clone(),
                None => vec![entry.clone()],
            };
            for command in commands {
                let kind = command
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let body = command
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(str::to_string);
                found.push(Hook {
                    event: event.clone(),
                    matcher: matcher.clone(),
                    command_digest: digest_bytes(body.as_deref().unwrap_or("").as_bytes()),
                    command: body,
                    kind,
                    scope,
                    source: source.to_string(),
                });
            }
        }
    }
    Some(found)
}

/// The warning shown when hooks outside project policy are in effect.
pub const NON_PROJECT_HOOK_WARNING: &str =
    "Hooks configured outside this project are not project policy.";

/// The explanation printed under [`NON_PROJECT_HOOK_WARNING`], already wrapped.
///
/// Scope-neutral on purpose: why a hook falls outside project policy differs by
/// scope, so the specific reason is printed per hook.
pub const NON_PROJECT_HOOK_DETAIL: &[&str] = &[
    "They can block tool calls and put text into the model's context, and they are",
    "not part of the configuration every ahu user in this project shares.",
];

/// Render the launch preview's hook section.
pub fn render_for_preview(inventory: &HookInventory, executable_config_files: usize) -> String {
    let mut out = String::new();
    out.push_str("\nHooks\n");
    if let Some(harness) = &inventory.unscanned_harness {
        // "none found" would be the result of looking in Claude Code's settings
        // files for a launch of a harness that does not read them. Unknown is
        // not absent, and this heading's whole job is to tell the reader whether
        // repository-supplied code will run in the session.
        out.push_str(&format!(
            "  !! ahu does not read {}'s hook or lifecycle configuration.\n     \
             Hooks for this launch are unknown, not absent. The harness's own configuration\n     \
             directories are carried into the task worktree by the checkout and by ahu's\n     \
             configuration snapshot, and whatever they declare will apply there.\n",
            display_safe(harness)
        ));
    } else if inventory.is_empty() && !inventory.wrapper_injected {
        out.push_str("  none found in the settings files ahu can read\n");
    }

    let travelling = inventory.travelling();
    if !travelling.is_empty() {
        out.push_str(&format!(
            "  + {} hook(s) declared in this repository travel into the task worktree and run there\n",
            travelling.len()
        ));
        for hook in &travelling {
            out.push_str(&format!(
                "     {} [{}] {}\n",
                hook.label(),
                hook.scope.as_str(),
                hook.source_label()
            ));
        }
    }
    if executable_config_files > 0 {
        out.push_str(&format!(
            "  + {executable_config_files} inherited configuration file(s) are executable and will be \
             copied into the task worktree with that bit set\n"
        ));
    }

    let outside = inventory.outside_project_policy();
    if !outside.is_empty() {
        out.push_str(&format!("\n  !! {NON_PROJECT_HOOK_WARNING}\n"));
        for line in NON_PROJECT_HOOK_DETAIL {
            out.push_str(&format!("     {line}\n"));
        }
        for hook in &outside {
            out.push_str(&format!(
                "     - {} [{}] {}\n",
                hook.label(),
                hook.scope.as_str(),
                hook.source_label()
            ));
            out.push_str(&format!("       {}\n", hook.scope.why_not_project_policy()));
        }
        out.push_str(
            "     ahu does not add, edit, or remove hooks, and it will not disable one for you.\n\
             \x20    Move a hook into this repository's settings to make it project policy.\n",
        );
    }
    if inventory.wrapper_injected {
        out.push_str(
            "\n  !! cmux injects its own Claude Code hooks through its Claude wrapper.\n\
             \x20    ahu cannot enumerate them, so this inventory is incomplete by construction.\n",
        );
    }
    for unreadable in &inventory.unreadable {
        out.push_str(&format!(
            "\n  !! {} exists but ahu could not read its hooks; treat them as unknown.\n",
            display_safe(unreadable)
        ));
    }
    out
}

/// Render what the settings files and `.mcp.json` say about the approval
/// boundary, for the launch preview's Approvals section.
///
/// Report native declarations alongside manifest-requested permission flags;
/// neither alone establishes the effective approval boundary.
pub fn render_settings_for_preview(inventory: &HookInventory) -> String {
    let mut out = String::new();
    let facts: Vec<&SettingsFacts> = inventory
        .settings
        .iter()
        .filter(|f| !f.is_empty())
        .collect();
    // What ahu did not read is not retracted by finding something it did read.
    // `.mcp.json` and `opencode.json` are repository configuration ahu parses
    // for every harness, while the harness's own settings can still be a surface
    // it has no implementation for. Emitting this independently stops "unknown"
    // becoming "none" the moment one of those files happens to be present.
    if let Some(harness) = &inventory.unscanned_harness {
        out.push_str(&format!(
            "  ahu did not read {}'s settings; what it allows, denies, or\n  \
             pre-approves is unknown to ahu, not known to be empty.\n",
            display_safe(harness)
        ));
    }
    if facts.is_empty() && inventory.mcp_servers.is_empty() && inventory.declared_plugins.is_empty()
    {
        if inventory.unscanned_harness.is_none() {
            out.push_str(
                "  the settings files ahu read declare no permission, plugin, MCP, or env keys\n",
            );
        }
        return out;
    }

    for fact in facts {
        let marker = if fact.widens_approvals() { "!!" } else { "  " };
        out.push_str(&format!(
            "  {marker} {} [{}]\n",
            display_safe(&fact.source),
            fact.scope.as_str()
        ));
        if let Some(mode) = &fact.default_mode {
            out.push_str(&format!(
                "       permissions.defaultMode {}\n",
                display_safe(mode)
            ));
        }
        for (label, values) in [
            ("permissions.allow", &fact.allow),
            ("permissions.deny", &fact.deny),
            ("permissions.ask", &fact.ask),
            (
                "permissions.additionalDirectories",
                &fact.additional_directories,
            ),
            ("enabledPlugins", &fact.enabled_plugins),
            ("enabledMcpjsonServers", &fact.enabled_mcpjson_servers),
        ] {
            if !values.is_empty() {
                out.push_str(&format!(
                    "       {label} {}\n",
                    display_safe(&values.join(", "))
                ));
            }
        }
        if let Some(enabled) = fact.enable_all_project_mcp_servers {
            out.push_str(&format!("       enableAllProjectMcpServers {enabled}\n"));
        }
        if !fact.env_names.is_empty() {
            out.push_str(&format!(
                "       env sets {} variable(s): {} (values not shown)\n",
                fact.env_names.len(),
                display_safe(&fact.env_names.join(", "))
            ));
        }
        if !fact.uninterpreted_keys.is_empty() {
            out.push_str(&format!(
                "       keys ahu does not interpret, so their effect is unknown: {}\n",
                display_safe(&fact.uninterpreted_keys.join(", "))
            ));
        }
        if fact.scope.travels_into_worktree() {
            out.push_str("       this file travels into the task worktree and applies there\n");
        }
    }

    if !inventory.mcp_servers.is_empty() {
        out.push_str(&format!(
            "  !! {} MCP server(s) declared by this repository. Each is a process the harness\n     \
             starts, with this session's privileges:\n",
            inventory.mcp_servers.len()
        ));
        for server in &inventory.mcp_servers {
            out.push_str(&format!(
                "       {} → {}\n",
                display_safe(&server.name),
                display_safe(&server.command)
            ));
        }
    }
    if !inventory.declared_plugins.is_empty() {
        out.push_str(&format!(
            "  !! {} plugin module(s) declared by this repository. OpenCode installs and runs\n     \
             these at startup when it is the launched harness; another harness does not read\n     \
             this file. ahu does not resolve, pin, or sandbox what they fetch:\n",
            inventory.declared_plugins.len()
        ));
        for plugin in &inventory.declared_plugins {
            out.push_str(&format!(
                "       {} → {}\n",
                display_safe(&plugin.source),
                display_safe(&plugin.module)
            ));
        }
    }
    // Qualified rather than fixed: on a harness whose settings ahu has no
    // implementation for, "ahu reads these files" would describe the repository
    // files it parsed as though they were the harness's approval configuration.
    if inventory.unscanned_harness.is_some() {
        out.push_str(
            "  the files above are the repository configuration ahu parses; it did not read\n  \
             this harness's own settings, and it does not set, override, or remove any of them.\n",
        );
    } else {
        out.push_str(
            "  ahu reads these files; it does not set, override, or remove any of them, and it\n  \
             cannot tell you which keys the installed harness honours from a project settings file.\n",
        );
    }
    out
}
