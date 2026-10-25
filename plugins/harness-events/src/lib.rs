//! In-memory native event normalization for external harness runtime plugins.
mod events;
mod identity;
mod kiro;
mod native;
pub mod recovery;
mod skills;
mod trajectory;
mod writes;
use events::{Events, SkillInvocation};
use serde_json::{Value, json};

/// Evidence about skill events, never a claim that a skill executed correctly.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum SkillEvidence {
    Observed,
    #[default]
    Unavailable,
    Unverified,
}

impl SkillEvidence {
    fn unverified() -> Self {
        Self::Unverified
    }
}
fn safe_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-:/".contains(&b))
        && !value.contains("..")
        && !value.contains(":/")
        && value.matches('/').count() <= 1
        && value.matches(':').count() <= 1
        && !value.starts_with("sk-")
        && value != "localhost"
        && value.parse::<std::net::IpAddr>().is_err()
        && value
            .split(':')
            .nth(1)
            .is_none_or(|tag| tag.bytes().any(|b| b.is_ascii_alphabetic()))
}

/// Non-sensitive boundary errors; native payloads are never included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(&'static str);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

/// One attempt's state. Snapshots are ephemeral; never persist `summary`.
/// This type deliberately does not implement Debug or Serialize.
pub struct Decoder {
    harness: String,
    events: Events,
    finished: bool,
    bytes: usize,
    write_targets: std::collections::BTreeSet<String>,
}
impl Decoder {
    pub fn new(harness: &str) -> Result<Self> {
        if !matches!(
            harness,
            "codex" | "claude-code" | "antigravity" | "opencode" | "kiro"
        ) {
            return Err(Error("unsupported harness"));
        }
        Ok(Self {
            harness: harness.into(),
            events: Events::default(),
            finished: false,
            bytes: 0,
            write_targets: Default::default(),
        })
    }
    fn reject(&mut self, reason: &'static str) -> Error {
        self.events.failed = true;
        self.events.trajectory.incomplete();
        self.events.native.incomplete();
        if self.events.blockers.len() < 32 && !self.events.blockers.iter().any(|b| b == reason) {
            self.events.blockers.push(reason.into());
        }
        Error(reason)
    }
    fn bound(&mut self, line: &[u8]) -> Result<()> {
        if self.finished {
            return Err(self.reject("decoder already finished"));
        }
        self.bytes = self.bytes.saturating_add(line.len().max(1));
        if line.len() > 1024 * 1024 || self.bytes > 64 * 1024 * 1024 {
            return Err(self.reject("native stream evaluation limit exceeded"));
        }
        Ok(())
    }
    pub fn observe_stdout(&mut self, line: &[u8]) -> Result<Value> {
        self.bound(line)?;
        let event: Value = match serde_json::from_slice(line) {
            Ok(Value::Object(map)) => Value::Object(map),
            _ => {
                self.events.trajectory.malformed();
                return Err(self.reject("malformed or truncated event"));
            }
        };
        if !bounded_depth(&event, 0) {
            self.events.trajectory.malformed();
            return Err(self.reject("native event depth limit exceeded"));
        }
        let paths = if self.harness == "kiro" {
            kiro::write_paths(&event)
        } else {
            writes::collect_write_paths(&event)
        };
        for path in paths {
            if path.len() > 4096
                || path.chars().any(char::is_control)
                || self.write_targets.len() >= 256 && !self.write_targets.contains(&path)
            {
                return Err(self.reject("native write target bound exceeded"));
            }
            self.write_targets.insert(path);
        }
        self.events.observe_value(&self.harness, &event);
        self.events.blockers.sort();
        self.events.blockers.dedup();
        self.events.blockers.truncate(32);
        self.events.stderr_diagnostics.sort();
        self.events.stderr_diagnostics.dedup();
        if self.events.summary.len() > 64 * 1024 {
            self.events.summary.clear();
            return Err(self.reject("native summary bound exceeded"));
        }
        Ok(self.snapshot())
    }
    pub fn observe_stderr(&mut self, line: &[u8]) -> Result<Value> {
        self.bound(line)?;
        self.events.observe_stderr(line);
        self.events.stderr_diagnostics.sort();
        self.events.stderr_diagnostics.dedup();
        Ok(self.snapshot())
    }
    /// Idempotent finalization after both streams drain; does not accept a task.
    pub fn finish(&mut self) -> Result<Value> {
        if !self.finished {
            self.finished = true;
        }
        Ok(self.snapshot())
    }
    pub fn snapshot(&self) -> Value {
        let e = &self.events;
        json!({
            "schema_version": 1, "harness": self.harness, "finished": self.finished,
            "session": e.session, "terminal": e.terminal, "terminal_evidence": e.terminal_evidence,
            "failed": e.failed, "summary": e.summary, "blockers": e.blockers,
            "unknown_events": e.unknown_events,
            "stderr_diagnostics": e.stderr_diagnostics,
            "stderr_unclassified_lines": e.stderr_unclassified_lines,
            "stderr_informational_lines": e.stderr_informational_lines,
            "usage": e.usage, "cost": e.cost, "cost_tracking_limited": e.cost_tracking_limited,
            "model": e.model.as_deref().filter(|m| safe_identity(m)),
            "model_attributes": e.reported_model_attributes().into_iter().collect::<std::collections::BTreeMap<_,_>>(),
            "skills": e.skills, "skill_unknown_events": e.skill_unknown_events,
            "skill_observation": e.skill_observation,
            "native": e.native, "native_completeness": e.native.completeness(), "native_blockers": e.native.completeness().blockers(),
            "trajectory": e.trajectory, "write_target_candidates": self.write_targets,
        })
    }
}

#[cfg(test)]
mod skill_tests;

fn bounded_depth(value: &Value, depth: usize) -> bool {
    if depth >= 64 {
        return false;
    }
    match value {
        Value::Array(a) => a.iter().all(|v| bounded_depth(v, depth + 1)),
        Value::Object(o) => o.values().all(|v| bounded_depth(v, depth + 1)),
        _ => true,
    }
}

#[cfg(test)]
mod parity_tests;
