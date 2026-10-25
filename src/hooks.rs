//! Harness hooks: shell commands the harness runs on its own lifecycle events.
//!
//! ahu reports visible hooks with their scope, event, and digest. Hooks outside
//! shared project policy raise a consistency warning because they can differ
//! between teammates. Local repository settings travel with the snapshot; home
//! and managed settings remain at their native locations.
//!
//! ahu never adds, edits, removes, or disables hooks. Hook and approval-settings
//! inspection is supplied by the selected installed runtime.
//! Hook commands are not serialized because they can contain credentials.

use std::path::Path;

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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct SettingsFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approvals_widened: Option<bool>,
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
        self.approvals_widened.unwrap_or(true)
    }
}

/// An MCP server a repository asks the harness to start.
///
/// A repository MCP declaration is copied with configuration and reported as a
/// potential subprocess source. Whether it runs depends on the selected harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct DeclaredPlugin {
    pub source: String,
    pub module: String,
}

/// Every hook ahu could find, plus an honest account of what it could not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
        digest_bytes(buffer.as_bytes())
    }

    pub fn short_digest(&self) -> String {
        self.digest()[..12].to_string()
    }
}

/// Inspect through the selected installed provider; no native formats live in core.
pub fn collect(repo_root: &Path, harness_id: &str) -> Result<HookInventory> {
    let repo = crate::git::discover(repo_root)?;
    let runtime = crate::harness_runtime::load(&repo, harness_id)?;
    let common = crate::harness_runtime::native_params(&repo, runtime.descriptor())?;
    let result=runtime.call("native_sources",serde_json::json!({
        "project":repo_root,"home":common.get("home"),"settings":{"view":"hooks","environment_names":std::env::vars_os().filter_map(|(name,_)|name.into_string().ok()).collect::<Vec<_>>()}
    }))?;
    validate_inventory(result)
}
fn validate_inventory(value: serde_json::Value) -> Result<HookInventory> {
    if serde_json::to_vec(&value)?.len() > 1024 * 1024 {
        return Err(crate::util::Error::new("invalid native hook inventory"));
    }
    let inventory: HookInventory = serde_json::from_value(value)
        .map_err(|_| crate::util::Error::new("invalid native hook inventory"))?;
    if inventory.hooks.len() > 4096
        || inventory.settings.len() > 1024
        || inventory.mcp_servers.len() > 1024
        || inventory.declared_plugins.len() > 1024
        || inventory.hooks.iter().any(|h| {
            !h.command_digest.is_empty()
                && (h.command_digest.len() != 64
                    || !h.command_digest.bytes().all(|b| b.is_ascii_hexdigit()))
        })
    {
        return Err(crate::util::Error::new("invalid native hook inventory"));
    }
    Ok(inventory)
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
    } else if inventory.is_empty() {
        out.push_str("  none found in the settings files ahu can read\n");
    }

    out.push_str(
        "  Executable wrapper and plugin hook injection is not established by this inventory.\n",
    );

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
