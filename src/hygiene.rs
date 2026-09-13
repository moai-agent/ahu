//! Project-scheduled context hygiene.
//!
//! On an agent's first load, and then on the project's cadence, ahu inspects the
//! memory and skills that could influence the agent and offers a concrete
//! cleanup proposal. It never deletes or disables anything on its own, and the
//! cadence is a project setting: there is no personal interval and no permanent
//! personal dismissal.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::LoadedConfig;
use crate::harness::EnforcementReport;
use crate::inventory::{Category, Inventory};
use crate::state;
use crate::task::format_rfc3339;
use crate::util::{Result, display_safe};

/// Operational record of when a review last ran. Timestamps are state, not
/// policy: they never change what the project's cadence is.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewState {
    /// Agent label -> Unix seconds of the last review.
    #[serde(default)]
    pub last_reviewed: BTreeMap<String, i64>,
}

fn state_path(repo_identity: &str) -> Result<std::path::PathBuf> {
    Ok(state::repo_dir(repo_identity)?.join("hygiene.json"))
}

pub fn load_state(repo_identity: &str) -> Result<ReviewState> {
    state::read_json(&state_path(repo_identity)?)
}

pub fn record_review(repo_identity: &str, agent_key: &str) -> Result<()> {
    let mut current = load_state(repo_identity)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    current.last_reviewed.insert(agent_key.to_string(), now);
    state::write_json(&state_path(repo_identity)?, &current)
}

/// Why a review is being shown now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    FirstLoad,
    /// The interval elapsed. Includes reviews that came due while ahu was not
    /// running: there is no background scheduler, so an overdue review simply
    /// runs on the next load.
    Overdue,
    /// Explicitly requested with `ahu hygiene`.
    Requested,
    NotDue,
}

/// Decide whether a review is due for `agent_key`.
pub fn due(loaded: &LoadedConfig, review_state: &ReviewState, agent_key: &str) -> Trigger {
    let hygiene = &loaded.config.context_hygiene;
    let Some(last) = review_state.last_reviewed.get(agent_key) else {
        return if hygiene.review_on_first_load {
            Trigger::FirstLoad
        } else {
            Trigger::NotDue
        };
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let interval = i64::from(hygiene.review_interval_days) * 86_400;
    if now - last >= interval {
        Trigger::Overdue
    } else {
        Trigger::NotDue
    }
}

/// A cleanup suggestion. Nothing here has happened yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub what: String,
    pub location: Option<String>,
    pub scope: String,
    pub shared: bool,
    pub supported_action: String,
}

/// The review ahu presents. It is a proposal, not an action log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub agent_key: String,
    pub suggestions: Vec<Suggestion>,
    /// Per-agent operations this adapter genuinely supports, and their scope.
    pub supported_controls: Vec<String>,
    /// Operations the harness does not expose per agent.
    pub unsupported_controls: Vec<String>,
    pub next_due: Option<String>,
}

impl Review {
    pub fn is_empty(&self) -> bool {
        self.suggestions.is_empty()
    }
}

/// Build a review from an inventory.
pub fn review(
    agent_key: &str,
    inventory: &Inventory,
    enforcement: &EnforcementReport,
    loaded: &LoadedConfig,
    review_state: &ReviewState,
) -> Review {
    let mut suggestions = Vec::new();
    for item in inventory.cleanup_candidates() {
        let supported_action = match (item.category, item.scope.as_str()) {
            (Category::Skill, "repository") => {
                "remove or move the skill directory in the repository; it is an ordinary Git change you review and commit yourself"
                    .to_string()
            }
            (Category::Skill, _) => {
                "detach it from this project with the harness's own settings; ahu will not delete a skill shared with every project on this machine"
                    .to_string()
            }
            (Category::Memory, "repository") => {
                "edit or delete the file in the repository; it is an ordinary Git change you review and commit yourself"
                    .to_string()
            }
            _ => "no per-agent control for this source; see the unsupported list".to_string(),
        };
        suggestions.push(Suggestion {
            what: item.name.clone(),
            location: item.location.clone(),
            scope: item.scope.clone(),
            shared: item.shared,
            supported_action,
        });
    }

    let mut supported_controls = vec![
        "list every skill and memory source ahu can see, with its scope and whether it is shared"
            .to_string(),
        "preview an exact cleanup scope before anything changes".to_string(),
    ];
    let mut unsupported_controls = Vec::new();
    if enforcement.harness == "claude-code" {
        supported_controls.push(
            "repository-scoped skills and instruction files can be changed as ordinary repository edits"
                .to_string(),
        );
        unsupported_controls.extend([
            "ahu cannot disable an individual skill for one agent without changing a setting that applies more widely, so it does not offer that as a per-agent operation"
                .to_string(),
            "Claude Code's memory reading and memory writing are not separately switchable per agent from outside a session, so ahu cannot report \"memory off\""
                .to_string(),
            "clearing a source shared with other agents or projects is not offered as a per-agent operation; ahu will not present a global deletion as a local one"
                .to_string(),
        ]);
    }

    let next_due = review_state.last_reviewed.get(agent_key).map(|last| {
        format_rfc3339(
            last + i64::from(loaded.config.context_hygiene.review_interval_days) * 86_400,
        )
    });

    Review {
        agent_key: agent_key.to_string(),
        suggestions,
        supported_controls,
        unsupported_controls,
        next_due,
    }
}

/// Render a review for a terminal.
pub fn render(review: &Review, trigger: Trigger, loaded: &LoadedConfig) -> String {
    let mut out = String::new();
    let reason = match trigger {
        Trigger::FirstLoad => "first load of this agent in this repository",
        Trigger::Overdue => "the project review interval has elapsed",
        Trigger::Requested => "requested",
        Trigger::NotDue => "not due",
    };
    out.push_str(&format!(
        "Context hygiene review for {} ({reason})\n",
        display_safe(&review.agent_key)
    ));
    out.push_str(&format!(
        "Project cadence: every {} day(s), set by context_hygiene.review_interval_days in {}.\n\n",
        loaded.config.context_hygiene.review_interval_days,
        crate::util::display_path(&loaded.path)
    ));

    if review.suggestions.is_empty() {
        out.push_str(
            "No skills or memory sources were found that ahu can see influencing this agent.\n\
             Nothing is being proposed for deletion. This is a report of what was found,\n\
             not a recommendation to remove data ahu did not inspect.\n\n",
        );
    } else {
        out.push_str("Sources that may influence this agent:\n");
        for suggestion in &review.suggestions {
            // `what` and `location` are repository-derived paths and labels.
            out.push_str(&format!(
                "  - {} [{}{}]\n",
                display_safe(&suggestion.what),
                suggestion.scope,
                if suggestion.shared { ", shared" } else { "" }
            ));
            if let Some(location) = &suggestion.location {
                out.push_str(&format!("      at {}\n", display_safe(location)));
            }
            out.push_str(&format!(
                "      {}\n",
                display_safe(&suggestion.supported_action)
            ));
        }
        out.push('\n');
        out.push_str(
            "Nothing has been changed. ahu does not purge memory, prune skills, disable\n\
             imported capabilities, or stage or commit anything on your behalf.\n\n",
        );
    }

    out.push_str("What ahu can do here:\n");
    for control in &review.supported_controls {
        out.push_str(&format!("  - {control}\n"));
    }
    if !review.unsupported_controls.is_empty() {
        let style = crate::style::stdout();
        let role = crate::style::Role::Gap;
        out.push_str(&style.paint(role, "\nWhat this harness does not let ahu do per agent:\n"));
        for control in &review.unsupported_controls {
            out.push_str(&format!(
                "  - {}\n",
                style.paint(role, &display_safe(control))
            ));
        }
    }
    out.push_str(
        "\nChanging a file does not remove content already loaded into a running session,\n\
         a compaction summary, or the harness's caches. Start a fresh task after a change,\n\
         and recheck the inventory before treating the new session as clean.\n",
    );
    if let Some(next) = &review.next_due {
        out.push_str(&format!("\nNext scheduled review after {next}.\n"));
    }
    out
}
