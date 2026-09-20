//! Project-scheduled context hygiene.
//!
//! On an agent's first load, and then on the project's cadence, ahu inspects the
//! memory and skills that could influence the agent and reports what it found:
//! each source, its category and scope, whether it is shared, and which kind of
//! per-agent control exists for it. What to do about any of that is the
//! `context-hygiene` skill's subject, not this module's — the review names the
//! skill instead of carrying its prose. ahu never deletes or disables anything
//! on its own, and the cadence is a project setting: there is no personal
//! interval and no permanent personal dismissal.

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

fn state_path(repo: &crate::git::Repo) -> Result<std::path::PathBuf> {
    Ok(crate::storage::CheckoutStorage::new(&repo.root)
        .repo_dir(&repo.identity())?
        .join("hygiene.json"))
}

pub fn load_state(repo: &crate::git::Repo) -> Result<ReviewState> {
    state::read_json(&state_path(repo)?)
}

pub fn record_review(repo: &crate::git::Repo, agent_key: &str) -> Result<()> {
    let mut current = load_state(repo)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    current.last_reviewed.insert(agent_key.to_string(), now);
    state::write_json(&state_path(repo)?, &current)
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

/// The kind of per-agent control that exists for a source, as a stable key.
///
/// A key names an operation's availability, not a recommendation to perform it.
/// What each key means for the reader, and when it should be acted on, is
/// documented in the bundled `context-hygiene` skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Control {
    /// ahu can list every skill and memory source it can see.
    InventoryListing,
    /// An exact cleanup scope can be previewed before anything changes.
    CleanupPreview,
    /// The source is repository-scoped: an ordinary Git change.
    RepositoryEdit,
    /// The source is shared beyond this project; the harness's own settings
    /// are what detach it.
    HarnessSetting,
    /// Disabling one skill for one agent.
    SkillDisable,
    /// Switching memory reading or writing per agent.
    MemoryToggle,
    /// Clearing a source shared with other agents or projects.
    SharedSourceClear,
    /// No per-agent control exists for this source.
    None,
}

impl Control {
    pub fn as_str(self) -> &'static str {
        match self {
            Control::InventoryListing => "inventory-listing",
            Control::CleanupPreview => "cleanup-preview",
            Control::RepositoryEdit => "repository-edit",
            Control::HarnessSetting => "harness-setting",
            Control::SkillDisable => "skill-disable",
            Control::MemoryToggle => "memory-toggle",
            Control::SharedSourceClear => "shared-source-clear",
            Control::None => "none",
        }
    }
}

fn keys(controls: &[Control]) -> String {
    controls
        .iter()
        .map(|control| control.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// One source found, with the facts about it. Nothing here has happened yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub what: String,
    pub location: Option<String>,
    pub category: Category,
    pub scope: String,
    pub shared: bool,
    pub control: Control,
}

/// The review ahu presents. It is a report of what was found, not an action log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub agent_key: String,
    pub findings: Vec<Finding>,
    /// Per-agent operations this adapter genuinely supports.
    pub supported_controls: Vec<Control>,
    /// Operations the harness does not expose per agent.
    pub unsupported_controls: Vec<Control>,
    pub next_due: Option<String>,
}

impl Review {
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty()
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
    let mut findings = Vec::new();
    for item in inventory.cleanup_candidates() {
        let control = match (item.category, item.scope.as_str()) {
            (Category::Skill | Category::Memory, "repository") => Control::RepositoryEdit,
            (Category::Skill, _) => Control::HarnessSetting,
            _ => Control::None,
        };
        findings.push(Finding {
            what: item.name.clone(),
            location: item.location.clone(),
            category: item.category,
            scope: item.scope.clone(),
            shared: item.shared,
            control,
        });
    }

    let mut supported_controls = vec![Control::InventoryListing, Control::CleanupPreview];
    let mut unsupported_controls = Vec::new();
    if enforcement.harness == "claude-code" {
        supported_controls.push(Control::RepositoryEdit);
        unsupported_controls.extend([
            Control::SkillDisable,
            Control::MemoryToggle,
            Control::SharedSourceClear,
        ]);
    }

    let next_due = review_state.last_reviewed.get(agent_key).map(|last| {
        format_rfc3339(
            last + i64::from(loaded.config.context_hygiene.review_interval_days) * 86_400,
        )
    });

    Review {
        agent_key: agent_key.to_string(),
        findings,
        supported_controls,
        unsupported_controls,
        next_due,
    }
}

/// Render a review for a terminal.
///
/// Facts only: the sources found, their scope, and the control keys. The
/// recommendations that used to be spelled out here are the `context-hygiene`
/// skill's, and the reader is pointed at it.
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

    if review.findings.is_empty() {
        out.push_str(
            "No skills or memory sources were found that ahu can see influencing this agent.\n\
             This is a report of what was found, not a statement about data ahu did not\n\
             inspect.\n\n",
        );
    } else {
        out.push_str("Sources that may influence this agent:\n");
        for finding in &review.findings {
            // `what` and `location` are repository-derived paths and labels.
            out.push_str(&format!(
                "  - {} [{}, {}{}]\n",
                display_safe(&finding.what),
                finding.category.as_str(),
                finding.scope,
                if finding.shared { ", shared" } else { "" }
            ));
            if let Some(location) = &finding.location {
                out.push_str(&format!("      at {}\n", display_safe(location)));
            }
            out.push_str(&format!("      control: {}\n", finding.control.as_str()));
        }
        out.push('\n');
        out.push_str(
            "Nothing has been changed. ahu does not purge memory, prune skills, disable\n\
             imported capabilities, or stage or commit anything on your behalf.\n\n",
        );
    }

    out.push_str(&format!(
        "Per-agent controls ahu offers here: {}\n",
        keys(&review.supported_controls)
    ));
    if !review.unsupported_controls.is_empty() {
        let style = crate::style::stdout();
        let role = crate::style::Role::Gap;
        out.push_str(&style.paint(
            role,
            &format!(
                "Per-agent controls this harness does not expose: {}\n",
                keys(&review.unsupported_controls)
            ),
        ));
    }
    out.push_str(&format!(
        "\nHow to read these facts and what to do about them: the `{}` skill at {}.\n\
         `ahu mcp setup` writes it into a repository for review and commit.\n",
        crate::mcp::CONTEXT_HYGIENE_SKILL,
        crate::mcp::skill_path(crate::mcp::CONTEXT_HYGIENE_SKILL)
    ));
    if let Some(next) = &review.next_due {
        out.push_str(&format!("\nNext scheduled review after {next}.\n"));
    }
    out
}
