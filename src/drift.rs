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

/// What the agent's manifest and files say right now.
///
/// Drift is a comparison against `LaunchIdentity` in a task record, so this
/// carries the same fields: a reader is owed "the model changed" before any
/// digest, and only a field-by-field comparison can say that.
///
/// The three digests stay apart on purpose. `identity_digest` folds manifest
/// fields and both file digests together. Separate file and instruction digests
/// identify frontmatter-only edits without implying that the delivered
/// instruction text changed.
#[derive(Debug, Clone, Copy)]
pub struct AgentIdentity<'a> {
    /// The manifest version, which is what drift says is now inaccurate.
    pub version: &'a str,
    pub harness: &'a str,
    pub model: &'a str,
    pub permissions: crate::agent::Permissions,
    /// Repository-relative path of the file the instructions are read from,
    /// as `ResolvedAgent::relative_source` spells it.
    pub instructions_source: Option<&'a str>,
    /// `ResolvedAgent::identity_digest()`.
    pub identity_digest: &'a str,
    /// Digest of the whole file the instructions are read from, frontmatter included.
    pub source_digest: &'a str,
    /// Digest of exactly the instruction text ahu delivers.
    pub instructions_digest: &'a str,
}

/// The approval boundary in the words the launch preview uses.
///
/// `Permissions::as_str()` is the manifest spelling; "prompt" in a drift line
/// would read as a value ahu passes, and ahu passes nothing in that case.
fn permissions_words(permissions: crate::agent::Permissions) -> &'static str {
    match permissions {
        crate::agent::Permissions::Prompt => "harness defaults",
        widened => widened.as_str(),
    }
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
    agent: Option<AgentIdentity<'_>>,
    snapshot_digest: &str,
    policy_digest: &str,
    hooks_digest: &str,
    previous: &[(std::path::PathBuf, TaskRecord)],
) -> Option<Drift> {
    let last = previous
        .iter()
        .find(|(_, record)| record.agent_label() == agent_label)?;
    let record = &last.1;
    let before = &record.identity;

    // What counts as drift is decided here, by digest, exactly as before. The
    // field comparisons further down only put the finding into words: a launch
    // that overrode a manifest field for one run must not become drift because
    // the record and the manifest disagree about it.
    let identity_changed = matches!(
        (
            agent.map(|a| a.identity_digest),
            before.identity_digest.as_deref()
        ),
        (Some(now), Some(was)) if now != was
    );
    let snapshot_changed = record.config_snapshot_digest != snapshot_digest;
    // The configuration snapshot already covers hooks declared inside the
    // repository. The hook digest also catches user- and machine-scoped hooks,
    // which never travel into a worktree and can change without any repository
    // edit. An older record carries no hook digest at all.
    let hooks_changed = !record.hooks_digest.is_empty() && record.hooks_digest != hooks_digest;
    let policy_changed = record.policy_digest != policy_digest;
    if !(identity_changed || snapshot_changed || hooks_changed || policy_changed) {
        return None;
    }

    let mut changes = Vec::new();
    // Named fields first, in words. A digest pair tells a reader that something
    // moved; only these tell them what to look at.
    let mut named_fields_explain_identity = false;
    if let Some(now) = agent {
        if before.model != now.model {
            changes.push(format!("model: {} -> {}", before.model, now.model));
            named_fields_explain_identity = true;
        }
        if before.harness != now.harness {
            changes.push(format!("harness: {} -> {}", before.harness, now.harness));
            named_fields_explain_identity = true;
        }
        if before.permissions != now.permissions {
            changes.push(format!(
                "permissions: {} -> {}",
                permissions_words(before.permissions),
                permissions_words(now.permissions)
            ));
        }
        // A missing digest in the record is a field an older schema did not
        // carry, not a changed file: it cannot be compared, so it is not
        // reported as a change.
        let instructions_changed = matches!(
            before.instructions_digest.as_deref(),
            Some(was) if was != now.instructions_digest
        );
        let source_changed = matches!(
            before.source_digest.as_deref(),
            Some(was) if was != now.source_digest
        );
        if let (Some(was), Some(is)) = (
            before.instructions_source.as_deref(),
            now.instructions_source,
        ) && was != is
        {
            changes.push(format!("instructions source: {was} -> {is}"));
        }
        // Name the file, and say which bytes moved. "the agent changed" cannot
        // distinguish an edit to the delivered text from an edit to frontmatter,
        // and only one of those changes what the model is told.
        let file = now
            .instructions_source
            .or(before.instructions_source.as_deref())
            .unwrap_or("the agent's instruction source");
        if instructions_changed {
            changes.push(format!("instructions: {file} changed, text ahu delivers"));
            named_fields_explain_identity = true;
        } else if source_changed {
            changes.push(format!(
                "instructions: {file} changed, frontmatter or metadata only -- the text ahu \
                 delivers is unchanged"
            ));
            named_fields_explain_identity = true;
        }
    }

    // The identity digest folds the manifest's name, version, harness and model
    // together with both file digests. Every one of those has its own line
    // above, so this is reached only by a record that predates the file digests:
    // there is nothing left to name, and the digest pair is all there is to say.
    if identity_changed && !named_fields_explain_identity {
        changes.push(format!(
            "the agent's identity changed: {} -> {} (combined digest of the manifest's name, \
             version, harness and model with both file digests)",
            short(before.identity_digest.as_deref().unwrap_or("")),
            short(agent.map(|a| a.identity_digest).unwrap_or(""))
        ));
    }
    // No named field covers the rest, so they stay digest pairs -- with the
    // inputs each one spans said in words.
    if snapshot_changed {
        changes.push(format!(
            "the repository agent configuration changed: {} -> {}",
            short(&record.config_snapshot_digest),
            short(snapshot_digest)
        ));
    }
    if hooks_changed {
        changes.push(format!(
            "the hooks in effect changed: {} -> {}",
            short(&record.hooks_digest),
            short(hooks_digest)
        ));
    }
    if policy_changed {
        changes.push(format!(
            "the project policy changed: {} -> {}",
            short(&record.policy_digest),
            short(policy_digest)
        ));
    }

    // Drift exists to catch exactly this: inputs moved and the label did not.
    // It rides on the first change so the reader sees both facts at once.
    if let Some(now) = agent
        && before.agent_version.as_deref() == Some(now.version)
        && let Some(first) = changes.first_mut()
    {
        first.push_str(&format!(" (version still {})", now.version));
    }

    // Every digest the gate above accepts contributes a line, either a named
    // field or its own pair, so a reported drift always says something. An
    // empty list here would mean ahu had found drift and then described none
    // of it.
    debug_assert!(!changes.is_empty(), "drift was detected but not described");
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
