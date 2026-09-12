//! Harness hooks: shell commands the harness runs on its own lifecycle events.
//!
//! Hooks matter more than almost anything else ahu inventories. A `PreToolUse`
//! hook can block a tool call; a `UserPromptSubmit` hook can put arbitrary text
//! into the model's context. They are executable code, and they are frequently
//! stored in directories that are ignored by Git and therefore never reviewed.
//!
//! ahu's position on them follows the charter's existing rules rather than
//! inventing a new one:
//!
//!   - **Report them.** Hooks are effective native settings, so they belong in
//!     the context inventory with their scope, event, and digest.
//!   - **Warn when they are not project policy.** A hook configured in a user's
//!     home directory or by machine policy affects behaviour, does not travel
//!     into a task worktree, and can differ for every teammate. That is exactly
//!     the machine-specific influence the charter says must raise the same
//!     consistency warning for everyone, not become a customization channel.
//!   - **Do not write them.** ahu never adds, edits, or removes a hook. Doing so
//!     by default would be the invisible behaviour modification the charter
//!     forbids, and hooks are stronger than prose.
//!
//! Verified against the Claude Code settings schema referenced by the installed
//! CLI's own settings file on 2026-09-12.

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
            Some(command) => {
                let program = command.split_whitespace().next().unwrap_or_default();
                let rendered = display_safe(&truncate(program, 48));
                if command.split_whitespace().nth(1).is_some() {
                    format!("{rendered} …")
                } else {
                    rendered
                }
            }
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

fn truncate(value: &str, limit: usize) -> String {
    let flat = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let kept: String = flat.chars().take(limit - 1).collect();
    format!("{}…", kept.trim_end())
}

/// Every hook ahu could find, plus an honest account of what it could not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookInventory {
    pub hooks: Vec<Hook>,
    /// Settings files that exist but could not be parsed. Their hooks are
    /// unknown, which is reported rather than treated as "none".
    pub unreadable: Vec<String>,
    /// Settings files ahu looked for and did not find. Used to say which scopes
    /// were actually checked.
    pub checked: Vec<String>,
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

    /// Digest over every hook ahu can see, at every scope.
    ///
    /// The configuration snapshot digest already covers hooks declared inside
    /// the repository. This one also covers user and managed scopes, so a task
    /// launched after a teammate's personal hook changed is reported as drifted
    /// rather than presented as the same effective inputs.
    pub fn digest(&self) -> String {
        let mut buffer = String::new();
        let mut digests: Vec<String> = self.hooks.iter().map(|hook| hook.digest()).collect();
        digests.sort();
        for digest in digests {
            buffer.push_str(&digest);
            buffer.push('\n');
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

/// Read every hook ahu can see for a Claude Code launch from `repo_root`.
pub fn collect(repo_root: &Path) -> Result<HookInventory> {
    collect_in(repo_root, &Locations::detect())
}

/// Read hooks using explicit machine locations.
pub fn collect_in(repo_root: &Path, locations: &Locations) -> Result<HookInventory> {
    let mut inventory = HookInventory {
        wrapper_injected: locations.cmux_wrapper,
        ..HookInventory::default()
    };

    for SettingsFile {
        scope,
        path,
        display,
        relative,
    } in settings_files(repo_root, locations)
    {
        // A repository can commit a symlink at `.claude`, or at the settings
        // file itself. Reading through one takes hooks from outside the
        // repository and labels them `project` — the single scope whose
        // `why_not_project_policy` is "it is project policy", so
        // `outside_project_policy` drops them and the non-project-hook warning
        // is suppressed for hooks that are not project policy at all. The
        // preview would also claim they travel into the task worktree, which is
        // false: `materialize` deletes configuration symlinks from it.
        // `snapshot::collect` already refuses to follow one; this makes the hook
        // scan agree with it. Both errors pointed the permissive way.
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
    if inventory.is_empty() && !inventory.wrapper_injected {
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
