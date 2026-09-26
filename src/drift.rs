//! Configuration and context drift against previous launches.
//!
//! ahu does not automate agent releases. What it does do is refuse to let
//! a version label quietly cover changed inputs: if `chris@1.2.0` launches today
//! with a different instruction digest or a different configuration snapshot
//! than the last `chris@1.2.0` launch, that is reported as drift and treated as
//! a pending bundle for the next version bump.

use crate::task::TaskRecord;
use crate::util::display_safe;

/// The first 12 characters of a digest, for a message.
///
/// Digests here come out of `task.json`, which is an ordinary file: a partial
/// write or a hand edit can leave one shorter than 12 characters. Slicing it
/// blind would panic in the middle of the interactive flow, so every truncation
/// goes through this.
fn short(digest: &str) -> &str {
    // By characters, not bytes: a hand-edited record is not guaranteed to be
    // hex, and slicing across a UTF-8 boundary panics just as readily.
    let end = digest
        .char_indices()
        .map(|(index, ch)| index + ch.len_utf8())
        .take(12)
        .last()
        .unwrap_or(0);
    &digest[..end]
}

/// The three digests that describe a named agent, kept apart on purpose.
///
/// `identity` folds manifest fields and both file digests together. Separate
/// file and instruction digests identify frontmatter-only edits without
/// implying that the delivered instruction text changed.
#[derive(Debug, Clone, Copy)]
pub struct AgentDigests<'a> {
    /// `ResolvedAgent::identity_digest()`.
    pub identity: &'a str,
    /// Digest of the whole file the instructions are read from, frontmatter included.
    pub source: &'a str,
    /// Digest of exactly the instruction text ahu delivers.
    pub instructions: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    pub agent_label: String,
    pub previous_task_id: String,
    pub previous_launched_at: String,
    pub changes: Vec<String>,
}

/// Compare a launch about to happen with the most recent launch of the same
/// agent at the same version.
pub fn detect(
    agent_label: &str,
    agent: Option<AgentDigests<'_>>,
    snapshot_digest: &str,
    policy_digest: &str,
    hooks_digest: &str,
    previous: &[(std::path::PathBuf, TaskRecord)],
) -> Option<Drift> {
    let last = previous
        .iter()
        .find(|(_, record)| record.agent_label() == agent_label)?;
    let record = &last.1;
    let mut changes = Vec::new();

    if let (Some(now), Some(before)) = (
        agent.map(|a| a.identity),
        record.identity.identity_digest.as_deref(),
    ) && now != before
    {
        // Name the digest that actually moved. A reader comparing a digest
        // against a file needs to know which bytes it covers, and the two
        // answers differ for a frontmatter-only edit.
        let agent = agent.expect("the identity digest came from it");
        let instructions_changed = matches!(
            record.identity.instructions_digest.as_deref(),
            Some(before) if before != agent.instructions
        );
        let source_changed = matches!(
            record.identity.source_digest.as_deref(),
            Some(before) if before != agent.source
        );
        if instructions_changed {
            changes.push(format!(
                "the instruction text ahu delivers changed: {} -> {} (digest of the delivered \
                 text, after any frontmatter is stripped)",
                short(record.identity.instructions_digest.as_deref().unwrap_or("")),
                short(agent.instructions)
            ));
        }
        if source_changed {
            changes.push(format!(
                "the agent's source file changed: {} -> {} (digest of the whole file, \
                 frontmatter included){}",
                short(record.identity.source_digest.as_deref().unwrap_or("")),
                short(agent.source),
                if instructions_changed {
                    ""
                } else {
                    " -- the text ahu delivers is unchanged, so this is a frontmatter or \
                     metadata edit"
                }
            ));
        }
        if !instructions_changed && !source_changed {
            // Either the manifest moved, or the record predates one of the two
            // fields. Say which digest this is rather than implying it names a
            // file.
            changes.push(format!(
                "the agent's identity changed: {} -> {} (combined digest of the manifest's name, \
                 version, harness and model with both file digests)",
                short(before),
                short(now)
            ));
        }
    }
    if record.config_snapshot_digest != snapshot_digest {
        changes.push(format!(
            "the repository agent configuration changed: {} -> {}",
            short(&record.config_snapshot_digest),
            short(snapshot_digest)
        ));
    }
    // The configuration snapshot already covers hooks declared inside the
    // repository. This also catches user- and machine-scoped hooks, which never
    // travel into a worktree and can change without any repository edit.
    if !record.hooks_digest.is_empty() && record.hooks_digest != hooks_digest {
        changes.push(format!(
            "the hooks in effect changed: {} -> {}",
            short(&record.hooks_digest),
            short(hooks_digest)
        ));
    }
    if record.policy_digest != policy_digest {
        changes.push(format!(
            "the project policy changed: {} -> {}",
            short(&record.policy_digest),
            short(policy_digest)
        ));
    }
    if changes.is_empty() {
        return None;
    }
    Some(Drift {
        agent_label: agent_label.to_string(),
        previous_task_id: record.task_id.clone(),
        previous_launched_at: record.created_at.clone(),
        changes,
    })
}

pub fn render(drift: &Drift) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Drift since the last {} launch (task {}, {})\n",
        display_safe(&drift.agent_label),
        display_safe(&drift.previous_task_id),
        display_safe(&drift.previous_launched_at)
    ));
    for change in &drift.changes {
        out.push_str(&format!("  - {}\n", display_safe(change)));
    }
    out.push_str(
        "\nThe version label has not changed, but the effective inputs have. Any of these\n\
         can make the agent behave differently from earlier runs under the same name.\n\
         Treat this as a pending behavior bundle: bump the agent's version and record what\n\
         changed. ahu does not do that for you, and it does not claim this change is a\n\
         harmless patch.\n",
    );
    out
}

/// One drifted agent as `ahu doctor` reports it.
///
/// `ahu doctor` names the agent the way `ahu agents` does and shows the launch
/// the comparison is against, so the two commands can be read together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drifted {
    /// The manifest name, without the sigil.
    pub agent_name: String,
    /// The manifest version, which is exactly what drift says is now inaccurate.
    pub agent_version: String,
    /// The previous launch, by task handle when one is recorded, else its id.
    pub previous_task: String,
    pub drift: Drift,
}

/// How many drifted agents `ahu doctor` describes in full.
///
/// A repository with a dozen registered agents can drift all of them at once —
/// a user-scoped hooks edit does it — and the diagnostic report has other
/// sections after this one.
const DOCTOR_DETAIL_LIMIT: usize = 3;

/// The `drift` section of `ahu doctor`, header line included.
///
/// Empty input has no section: doctor says "no registered agents are drifted"
/// itself, because that line is not about any particular agent.
pub fn render_doctor(drifted: &[Drifted]) -> String {
    let mut out = String::new();
    if drifted.is_empty() {
        return out;
    }
    out.push_str(&format!(
        "drift        {} registered agent{} drifted\n",
        drifted.len(),
        if drifted.len() == 1 { "" } else { "s" }
    ));
    for entry in drifted.iter().take(DOCTOR_DETAIL_LIMIT) {
        out.push_str(&format!(
            "  @{} {}  since task {} ({})\n",
            display_safe(&entry.agent_name),
            display_safe(&entry.agent_version),
            display_safe(&entry.previous_task),
            display_safe(launch_day(&entry.drift.previous_launched_at))
        ));
        for change in &entry.drift.changes {
            out.push_str(&format!("    - {}\n", display_safe(change)));
        }
    }
    if let Some(rest) = drifted
        .len()
        .checked_sub(DOCTOR_DETAIL_LIMIT)
        .filter(|n| *n > 0)
    {
        out.push_str(&format!(
            "  {rest} more drifted agent{} not shown; `ahu agents` lists every drifted agent.\n",
            if rest == 1 { " is" } else { "s are" }
        ));
    }
    out.push_str(
        "  Bump the version in the agent's manifest and record what changed, or restore it.\n",
    );
    out
}

/// The calendar day of an RFC 3339 launch timestamp.
///
/// Doctor has one line per agent, and the day is what a reader needs to place
/// the comparison. A record is an ordinary file, so a value that is not a
/// timestamp is shown whole rather than truncated into something that looks
/// like a date.
fn launch_day(timestamp: &str) -> &str {
    match timestamp.split_once('T') {
        Some((day, _)) if day.len() == 10 => day,
        _ => timestamp,
    }
}
