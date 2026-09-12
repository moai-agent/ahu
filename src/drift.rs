//! Configuration and context drift against previous launches.
//!
//! ahu 0.1.1 does not automate agent releases. What it does do is refuse to let
//! a version label quietly cover changed inputs: if `chris@1.2.0` launches today
//! with a different instruction digest or a different configuration snapshot
//! than the last `chris@1.2.0` launch, that is reported as drift and treated as
//! a pending bundle for the next version bump.

use crate::task::TaskRecord;

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
    identity_digest: Option<&str>,
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

    if let (Some(now), Some(before)) = (identity_digest, record.identity.identity_digest.as_deref())
        && now != before
    {
        changes.push(format!(
            "the agent's instructions or manifest changed: {} -> {}",
            &before[..12.min(before.len())],
            &now[..12.min(now.len())]
        ));
    }
    if record.config_snapshot_digest != snapshot_digest {
        changes.push(format!(
            "the repository agent configuration changed: {} -> {}",
            &record.config_snapshot_digest[..12],
            &snapshot_digest[..12]
        ));
    }
    // The configuration snapshot already covers hooks declared inside the
    // repository. This also catches user- and machine-scoped hooks, which never
    // travel into a worktree and can change without any repository edit.
    if !record.hooks_digest.is_empty() && record.hooks_digest != hooks_digest {
        changes.push(format!(
            "the hooks in effect changed: {} -> {}",
            &record.hooks_digest[..12],
            &hooks_digest[..12]
        ));
    }
    if record.policy_digest != policy_digest {
        changes.push(format!(
            "the project policy changed: {} -> {}",
            &record.policy_digest[..12],
            &policy_digest[..12]
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
        drift.agent_label, drift.previous_task_id, drift.previous_launched_at
    ));
    for change in &drift.changes {
        out.push_str(&format!("  - {change}\n"));
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
