//! The context inventory.
//!
//! The goal is an honest picture of everything that can influence an agent's
//! context: what is merely available for discovery, what is known to be loaded,
//! what is disabled, and what ahu cannot see at all. It is never labelled
//! complete — for an opaque harness that would be a claim ahu cannot make.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::ResolvedAgent;
use crate::config::LoadedConfig;
use crate::harness::EnforcementReport;
use crate::hooks::HookInventory;
use crate::snapshot::ConfigSnapshot;
use crate::util::{Result, digest_file, display_safe};

/// How much ahu actually knows about one source.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Visibility {
    /// ahu passed it to the harness, or the harness reports it as loaded.
    Loaded,
    /// Present and discoverable by the harness; whether it is loaded into the
    /// model's context is not observable from outside the session.
    Available,
    /// Present but turned off through a native control.
    Disabled,
    /// ahu knows a source of this kind exists but cannot read its contents.
    Opaque,
    /// Nothing found.
    Absent,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Loaded => "loaded",
            Visibility::Available => "available",
            Visibility::Disabled => "disabled",
            Visibility::Opaque => "opaque",
            Visibility::Absent => "absent",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    /// Fixed identity: name, harness, model, and the file the agent's
    /// instructions were read from. ahu delivers those instructions in the
    /// prompt; nothing selects them through the harness.
    Identity,
    /// Repository guidance, not an optional skill or memory.
    RepositoryInstructions,
    /// Guidance from the user's home directory or an ancestor directory.
    PersonalInstructions,
    /// Managed or policy-supplied instructions.
    Managed,
    Skill,
    Memory,
    Mcp,
    /// A shell command the harness runs on one of its lifecycle events.
    Hook,
    /// The submitted task prompt.
    TaskPrompt,
    /// Anything the harness or provider injects that ahu cannot read.
    HarnessInjected,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Identity => "identity",
            Category::RepositoryInstructions => "repository-instructions",
            Category::PersonalInstructions => "personal-instructions",
            Category::Managed => "managed",
            Category::Skill => "skill",
            Category::Memory => "memory",
            Category::Mcp => "mcp",
            Category::Hook => "hook",
            Category::TaskPrompt => "task-prompt",
            Category::HarnessInjected => "harness-injected",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Item {
    pub category: Category,
    pub name: String,
    pub location: Option<String>,
    /// `repository`, `user`, `machine`, or `provider`.
    pub scope: String,
    pub visibility: Visibility,
    pub digest: Option<String>,
    /// Whether this source is shared with agents or worktrees other than this one.
    pub shared: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Inventory {
    pub items: Vec<Item>,
    /// Things ahu knows it cannot see. Never empty for a real harness.
    pub coverage_gaps: Vec<String>,
}

impl Inventory {
    pub fn items_in(&self, category: Category) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(move |i| i.category == category)
    }

    /// Sources that could influence behaviour and are candidates for cleanup.
    pub fn cleanup_candidates(&self) -> Vec<&Item> {
        self.items
            .iter()
            .filter(|i| {
                matches!(i.category, Category::Skill | Category::Memory)
                    && matches!(i.visibility, Visibility::Available | Visibility::Loaded)
            })
            .collect()
    }
}

/// Everything an inventory is built from.
///
/// `agent` is absent for an automatic launch, which has no named identity but
/// still has a frozen harness and model.
pub struct Subject<'a> {
    pub repo_root: &'a Path,
    pub loaded_config: &'a LoadedConfig,
    pub snapshot: &'a ConfigSnapshot,
    pub agent: Option<&'a ResolvedAgent>,
    pub harness: &'a str,
    pub model: &'a str,
    pub enforcement: &'a EnforcementReport,
    pub hooks: &'a HookInventory,
    pub prompt: Option<&'a str>,
}

/// Build an inventory for a launch.
pub fn build(subject: &Subject<'_>) -> Result<Inventory> {
    let Subject {
        repo_root,
        loaded_config,
        snapshot,
        agent,
        harness,
        model,
        enforcement,
        hooks,
        prompt,
    } = *subject;
    let mut inventory = Inventory::default();

    // 1. Fixed identity.
    inventory.items.push(Item {
        category: Category::Identity,
        name: match agent {
            Some(agent) => agent.label(),
            None => "auto".to_string(),
        },
        location: agent.map(|a| relative(repo_root, &a.manifest_path)),
        scope: "repository".to_string(),
        visibility: Visibility::Loaded,
        digest: agent.map(|a| a.identity_digest()[..12].to_string()),
        shared: false,
        notes: {
            let mut notes = vec![
                format!(
                    "harness {harness} {}",
                    enforcement
                        .harness_version
                        .clone()
                        .unwrap_or_else(|| "(version unknown)".to_string())
                ),
                format!("model {model}"),
                format!("policy {}", &loaded_config.digest[..12]),
            ];
            if let Some(agent) = agent {
                // Not "system prompt": ahu delivers these bytes in the prompt on
                // every harness. Both digests are named, because they answer
                // different questions and only one of them describes what the
                // model was given.
                notes.push(format!(
                    "instructions delivered in the prompt, read from {}",
                    relative(repo_root, &agent.source_path)
                ));
                notes.push(format!(
                    "file digest {} covers the whole file at that path, frontmatter included",
                    &agent.source_digest[..12]
                ));
                notes.push(format!(
                    "instructions digest {} covers exactly the text ahu delivers{}",
                    &agent.instructions_digest[..12],
                    if agent.manifest.source.format.has_frontmatter() {
                        ", which is that file with its YAML frontmatter stripped"
                    } else {
                        "; this format has no frontmatter, so the two cover the same bytes"
                    }
                ));
                if !agent.native_settings.is_empty() {
                    notes.push(format!(
                        "native settings preserved in place: {}",
                        agent
                            .native_settings
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            } else {
                notes.push(
                    "no named agent: the harness's own default system prompt applies, and ahu \
                     delivers only its delegation contract"
                        .to_string(),
                );
            }
            notes
        },
    });

    inventory.items.push(Item {
        category: Category::Identity,
        name: "ahu delegation contract v1".to_string(),
        location: None,
        scope: "ahu launcher".to_string(),
        // `available`, not `loaded`: ahu puts the bytes in the prompt, which is
        // the strongest thing it can say. Whether the model treats them as
        // instructions is not observable from outside the session, and no
        // harness flag makes them binding.
        visibility: Visibility::Available,
        digest: Some(
            crate::util::digest_bytes(crate::orchestration::INSTRUCTIONS.as_bytes())[..12]
                .to_string(),
        ),
        shared: true,
        notes: vec![
            "delivered as prompt text on every harness, inside a fence tagged with a per-launch \
             nonce, before the agent's instructions and the task prompt"
                .to_string(),
            "not an enforced system prompt: ahu passes no agent-selection or system-prompt flag \
             on any harness, and the task prompt that follows can contradict it"
                .to_string(),
        ],
    });

    // 2. Repository configuration actually carried into the task worktree.
    for entry in &snapshot.entries {
        let category = classify(&entry.path);
        inventory.items.push(Item {
            category,
            name: entry.path.clone(),
            location: Some(entry.path.clone()),
            scope: "repository".to_string(),
            visibility: Visibility::Available,
            digest: Some(entry.digest[..12].to_string()),
            shared: true,
            notes: vec!["inherited into the task worktree at its native path".to_string()],
        });
    }
    for skipped in &snapshot.skipped_directories {
        // The task worktree is a checkout of the base commit, so anything
        // committed under a skipped path arrives in it regardless. The gap is
        // in the inventory, not in the inheritance, and saying otherwise would
        // tell the reader the opposite of the truth.
        inventory.coverage_gaps.push(format!(
            "configuration under {skipped} was not scanned, so it is not inventoried; committed files under it are still present in the task worktree"
        ));
    }
    for found in &snapshot.unscanned_config {
        inventory.coverage_gaps.push(format!(
            "{found} sits inside a directory the scan does not enter: it is not inventoried, but the checkout carries it into the task worktree"
        ));
    }

    // 3. Personal and managed sources outside the repository.
    for (path, category, scope, note) in personal_sources(harness) {
        let visibility = if path.exists() {
            Visibility::Available
        } else {
            Visibility::Absent
        };
        if visibility == Visibility::Absent {
            continue;
        }
        let digest = if path.is_file() {
            digest_file(&path).ok().map(|d| d[..12].to_string())
        } else {
            None
        };
        inventory.items.push(Item {
            category,
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string()),
            location: Some(path.to_string_lossy().to_string()),
            scope: scope.to_string(),
            visibility,
            digest,
            shared: true,
            notes: vec![note.to_string()],
        });
    }

    // 4. Hooks. These are executable code and they run on the harness's own
    // events, so they belong in the inventory beside instructions and skills.
    for hook in &hooks.hooks {
        inventory.items.push(Item {
            category: Category::Hook,
            name: hook.label(),
            location: Some(hook.source.clone()),
            scope: hook.scope.as_str().to_string(),
            visibility: Visibility::Available,
            digest: Some(hook.digest()[..12].to_string()),
            shared: !matches!(hook.scope, crate::hooks::Scope::User),
            notes: {
                let mut notes = vec![format!("runs on {}", hook.event)];
                if hook.scope.travels_into_worktree() {
                    notes.push(
                        "declared in this repository, so it is copied into the task worktree and runs there"
                            .to_string(),
                    );
                } else {
                    notes.push(
                        "not copied into the task worktree; it applies from its native location"
                            .to_string(),
                    );
                }
                if !hook.scope.is_project_policy() {
                    notes.push(format!(
                        "not project policy: {}",
                        hook.scope.why_not_project_policy()
                    ));
                }
                notes
            },
        });
    }
    if hooks.wrapper_injected {
        inventory.items.push(Item {
            category: Category::Hook,
            name: "cmux Claude wrapper hooks".to_string(),
            location: None,
            scope: "machine".to_string(),
            visibility: Visibility::Opaque,
            digest: None,
            shared: true,
            notes: vec![
                "cmux injects Claude Code hooks through its own wrapper; ahu cannot enumerate them"
                    .to_string(),
            ],
        });
    }
    for unreadable in &hooks.unreadable {
        inventory.coverage_gaps.push(format!(
            "{unreadable} exists but its hooks could not be read, so they are unknown rather than absent"
        ));
    }
    if hooks.wrapper_injected {
        inventory.coverage_gaps.push(
            "hooks injected by the cmux Claude wrapper are not enumerable by ahu".to_string(),
        );
    }
    inventory.coverage_gaps.push(
        "hooks contributed by plugins are not enumerated; only settings files are read".to_string(),
    );

    // 5. The task prompt.
    if let Some(prompt) = prompt {
        inventory.items.push(Item {
            category: Category::TaskPrompt,
            name: crate::util::task_title_from_prompt(prompt),
            location: None,
            scope: "task".to_string(),
            visibility: Visibility::Loaded,
            digest: Some(crate::util::digest_bytes(prompt.as_bytes())[..12].to_string()),
            shared: false,
            notes: vec![format!(
                "{} characters, delivered to the harness as a single argument",
                prompt.chars().count()
            )],
        });
    }

    // 6. What ahu cannot see.
    inventory.items.push(Item {
        category: Category::HarnessInjected,
        name: format!("{harness} built-in system instructions"),
        location: None,
        scope: "provider".to_string(),
        visibility: Visibility::Opaque,
        digest: None,
        shared: true,
        notes: vec![
            "the harness's own system prompt, tool descriptions, and any provider-side context are not readable by ahu"
                .to_string(),
        ],
    });

    inventory.coverage_gaps.extend([
        format!(
            "{harness} does not report to ahu which of the available sources it actually loaded into the model's context"
        ),
        "retrieval, compaction summaries, and conversation transformations inside a running session are not observable from outside it".to_string(),
        "provider-side or account-level context, if any, is not visible to ahu".to_string(),
    ]);
    for gap in &enforcement.gaps {
        inventory.coverage_gaps.push(gap.clone());
    }

    Ok(inventory)
}

fn classify(path: &str) -> Category {
    if path.contains("/skills/") || path.starts_with("skills/") {
        Category::Skill
    } else if path.ends_with(".mcp.json") || path.ends_with("mcp.json") {
        Category::Mcp
    } else if path.ends_with("CLAUDE.local.md") {
        Category::Memory
    } else {
        Category::RepositoryInstructions
    }
}

/// Sources outside the repository that can still influence a session.
fn personal_sources(harness: &str) -> Vec<(PathBuf, Category, &'static str, &'static str)> {
    let mut found = Vec::new();
    if harness != "claude-code" {
        return found;
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return found;
    };
    found.push((
        home.join(".claude/CLAUDE.md"),
        Category::PersonalInstructions,
        "user",
        "personal instructions; they apply to every project on this machine and are not part of project policy",
    ));
    found.push((
        home.join(".claude/settings.json"),
        Category::PersonalInstructions,
        "user",
        "personal Claude Code settings; may set a model, permissions, or hooks",
    ));
    found.push((
        home.join(".claude/agents"),
        Category::PersonalInstructions,
        "user",
        "personal agent definitions; they are discoverable alongside the project's",
    ));
    found.push((
        home.join(".claude/skills"),
        Category::Skill,
        "user",
        "personal skills; available in every project on this machine and shared between agents",
    ));
    found.push((
        home.join(".claude/plugins"),
        Category::Skill,
        "user",
        "personal plugins; may add skills, hooks, MCP servers, and agents",
    ));
    found.push((
        PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json"),
        Category::Managed,
        "machine",
        "managed policy settings; required, not optional, and not removable by ahu",
    ));
    found
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

/// Render the inventory for a terminal.
pub fn render(inventory: &Inventory) -> String {
    use crate::style::{self, Role};
    let style = style::stdout();
    let mut out = String::new();
    out.push_str("Context inventory\n");
    out.push_str("=================\n\n");
    let groups = [
        (Category::Identity, "Fixed identity"),
        (Category::RepositoryInstructions, "Repository instructions"),
        (Category::Skill, "Skills"),
        (Category::Memory, "Memory"),
        (Category::Mcp, "MCP configuration"),
        (Category::Hook, "Hooks (executable, run by the harness)"),
        (
            Category::PersonalInstructions,
            "Personal (not project policy)",
        ),
        (Category::Managed, "Managed / required"),
        (Category::TaskPrompt, "Task prompt"),
        (Category::HarnessInjected, "Harness-injected"),
    ];
    for (category, heading) in groups {
        let items: Vec<_> = inventory.items_in(category).collect();
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("{heading}\n"));
        for item in items {
            // Names, locations, and notes carry repository-derived text such as
            // file names, hook labels, and native settings keys.
            out.push_str(&format!(
                "  [{}] {}{}\n",
                item.visibility.as_str(),
                display_safe(&item.name),
                item.digest
                    .as_ref()
                    .map(|d| format!(" ({})", display_safe(d)))
                    .unwrap_or_default()
            ));
            if let Some(location) = &item.location
                && location != &item.name
            {
                out.push_str(&format!("      at {}\n", display_safe(location)));
            }
            for note in &item.notes {
                out.push_str(&format!("      {}\n", display_safe(note)));
            }
        }
        out.push('\n');
    }
    out.push_str(&style.paint(Role::Gap, "What ahu cannot see\n"));
    for gap in &inventory.coverage_gaps {
        out.push_str(&format!(
            "  - {}\n",
            style.paint(Role::Gap, &display_safe(gap))
        ));
    }
    out.push_str(
        "\nThis inventory is not complete. `available` means a source is discoverable by the\n\
         harness, not that its contents reached the model.\n",
    );
    out
}
