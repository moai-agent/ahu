//! Host-side validation and a persistent installed-runtime decoder worker.
//! No native event shape is parsed here. Raw lines live only in bounded queues.
use super::{Events, SessionCheckpoint, SessionOwner, durable_json, observe_writes};
use crate::util::{Error, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const SNAPSHOT_LIMIT: usize = 1024 * 1024;
const SNAPSHOT_FIELDS: &[&str] = &[
    "schema_version",
    "harness",
    "finished",
    "session",
    "terminal",
    "terminal_evidence",
    "failed",
    "summary",
    "blockers",
    "unknown_events",
    "stderr_diagnostics",
    "stderr_unclassified_lines",
    "stderr_informational_lines",
    "usage",
    "cost",
    "cost_tracking_limited",
    "model",
    "model_attributes",
    "skills",
    "skill_unknown_events",
    "skill_observation",
    "native",
    "native_completeness",
    "native_blockers",
    "trajectory",
    "write_target_candidates",
];

fn invalid() -> Error {
    Error::new("invalid normalized runtime observation")
}

fn bounded(value: &Value, depth: usize, nodes: &mut usize) -> bool {
    *nodes = nodes.saturating_add(1);
    if depth > 16 || *nodes > 16384 {
        return false;
    }
    match value {
        Value::String(text) => text.len() <= 4096 && !text.contains('\0'),
        Value::Array(items) => {
            items.len() <= 256 && items.iter().all(|v| bounded(v, depth + 1, nodes))
        }
        Value::Object(map) => {
            map.len() <= 256
                && map
                    .iter()
                    .all(|(k, v)| k.len() <= 4096 && bounded(v, depth + 1, nodes))
        }
        Value::Number(number) => number.as_f64().is_some_and(f64::is_finite),
        _ => true,
    }
}

struct Snapshot {
    events: Events,
    write_targets: Vec<String>,
}
impl Snapshot {
    fn parse(mut value: Value, harness: &str, finished: bool) -> Result<Self> {
        if serde_json::to_vec(&value)?.len() > SNAPSHOT_LIMIT {
            return Err(invalid());
        }
        let object = value.as_object_mut().ok_or_else(invalid)?;
        if object.len() != SNAPSHOT_FIELDS.len()
            || !SNAPSHOT_FIELDS.iter().all(|k| object.contains_key(*k))
            || object.get("schema_version").and_then(Value::as_u64) != Some(1)
            || object.get("harness").and_then(Value::as_str) != Some(harness)
            || object.get("finished").and_then(Value::as_bool) != Some(finished)
            || object
                .get("summary")
                .and_then(Value::as_str)
                .is_none_or(|s| s.len() > 64 * 1024)
        {
            return Err(invalid());
        }
        let mut nodes = 0;
        if !object
            .iter()
            .filter(|(k, _)| k.as_str() != "summary")
            .all(|(_, v)| bounded(v, 0, &mut nodes))
        {
            return Err(invalid());
        }
        let writes: Vec<String> = serde_json::from_value(object["write_target_candidates"].clone())
            .map_err(|_| invalid())?;
        if writes.iter().any(|s| s.chars().any(char::is_control)) {
            return Err(invalid());
        }
        let _: crate::native::Completeness =
            serde_json::from_value(object["native_completeness"].clone()).map_err(|_| invalid())?;
        let _: Vec<String> =
            serde_json::from_value(object["native_blockers"].clone()).map_err(|_| invalid())?;
        if object["native"]
            .get("stream_incomplete")
            .and_then(Value::as_bool)
            .is_none()
        {
            return Err(invalid());
        }
        let cost_limited = object["cost_tracking_limited"]
            .as_bool()
            .ok_or_else(invalid)?;
        // Legacy Events has internal-only observations which are never supplied by a plugin.
        object.insert("native_observations".into(), json!([]));
        let mut events: Events = serde_json::from_value(value).map_err(|_| invalid())?;
        events.cost_tracking_limited = cost_limited;
        if events
            .session
            .as_deref()
            .is_some_and(|s| !super::valid_session(s))
            || events.summary.len() > 64 * 1024
            || events.blockers.len() > 32
            || events.stderr_diagnostics.len() > 32
            || events.skills.len() > 128
            || !events.native.validate()
            || events.cost.usd.is_some_and(|n| !n.is_finite() || n < 0.0)
            || !matches!(
                (events.cost.usd, events.cost.source.as_deref()),
                (None, None)
                    | (
                        Some(_),
                        Some("native_cumulative_usd" | "native_step_sum_usd")
                    )
            )
            || events
                .trajectory
                .observation
                .metering
                .as_ref()
                .is_some_and(|m| !m.valid())
            || events
                .model
                .as_deref()
                .is_some_and(|m| !crate::eval::provenance::safe_identity(m))
        {
            return Err(invalid());
        }
        let attributes = &events.model_attributes;
        if attributes.len() > 2
            || attributes.keys().any(|k| {
                !matches!(
                    k.as_str(),
                    "ahu.harness.model.reported" | "ahu.harness.model.reported.coverage"
                )
            })
            || !matches!(
                attributes
                    .get("ahu.harness.model.reported.coverage")
                    .map(String::as_str),
                Some("partial" | "complete" | "conflicting" | "redacted")
            )
            || attributes
                .get("ahu.harness.model.reported")
                .is_some_and(|m| !crate::eval::provenance::safe_identity(m))
        {
            return Err(invalid());
        }
        let coverage = attributes
            .get("ahu.harness.model.reported.coverage")
            .map(String::as_str);
        if matches!(coverage, Some("conflicting" | "redacted"))
            && attributes.contains_key("ahu.harness.model.reported")
            || coverage == Some("complete")
                && (!events.terminal
                    || events.unknown_events != 0
                    || !events.trajectory.observation.complete()
                    || !attributes.contains_key("ahu.harness.model.reported"))
        {
            return Err(invalid());
        }
        if events.skills.iter().any(|s| {
            s.name.is_empty()
                || s.name.len() > 128
                || !s
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
                || s.harness.as_deref() != Some(harness)
                || !matches!(s.status.as_str(), "invoked" | "completed" | "failed")
                || s.source.is_some()
                || s.digest.is_some()
        }) {
            return Err(invalid());
        }
        Ok(Self {
            events,
            write_targets: writes,
        })
    }

    fn apply(self, previous: &mut Events, worktree: &Path) -> Result<()> {
        let mut next = self.events;
        if previous.session.is_some() && next.session != previous.session
            || next.unknown_events < previous.unknown_events
            || next.stderr_unclassified_lines < previous.stderr_unclassified_lines
            || next.stderr_informational_lines < previous.stderr_informational_lines
            || next.skill_unknown_events < previous.skill_unknown_events
        {
            return Err(invalid());
        }
        for ((_, prior), (_, current)) in previous
            .usage
            .normalized_fields()
            .into_iter()
            .zip(next.usage.normalized_fields())
        {
            if prior.is_some_and(|p| current.is_none_or(|c| c < p)) {
                return Err(invalid());
            }
        }
        if previous.cost.source.is_some() && previous.cost.source != next.cost.source {
            return Err(invalid());
        }
        if previous
            .cost
            .usd
            .is_some_and(|p| next.cost.usd.is_none_or(|c| c < p))
        {
            return Err(invalid());
        }
        // Receipt timestamps belong to the supervising host, not provider payloads.
        for (index, skill) in next.skills.iter_mut().enumerate() {
            let prior = previous
                .skills
                .get(index)
                .filter(|old| old.name == skill.name && old.harness == skill.harness);
            skill.observed_at = prior
                .and_then(|old| old.observed_at.clone())
                .or_else(|| Some(crate::task::now_rfc3339()));
            skill.completed_at = if matches!(skill.status.as_str(), "completed" | "failed") {
                prior
                    .filter(|old| old.status == skill.status)
                    .and_then(|old| old.completed_at.clone())
                    .or_else(|| Some(crate::task::now_rfc3339()))
            } else {
                None
            };
        }
        next.failed |= previous.failed;
        next.cost_tracking_limited |= previous.cost_tracking_limited;
        if previous.trajectory.observation.stream_incomplete {
            next.trajectory.incomplete();
        }
        if previous.native.stream_incomplete {
            next.native.incomplete();
        }
        next.structured_result_target = previous.structured_result_target.clone();
        next.writes_outside_worktree = std::mem::take(&mut previous.writes_outside_worktree);
        for blocker in &previous.blockers {
            if next.blockers.len() < 128 && !next.blockers.contains(blocker) {
                next.blockers.push(blocker.clone());
            }
        }
        if next.trajectory.observation.stream_incomplete {
            next.model_attributes.insert(
                "ahu.harness.model.reported.coverage".into(),
                "partial".into(),
            );
        }
        observe_writes(&mut next, &self.write_targets, worktree);
        *previous = next;
        Ok(())
    }
}

pub(super) enum Input {
    Line { stream: &'static str, line: String },
    End(&'static str),
}

/// Bounded queue backpressure never holds the supervisor's observation lock.
#[derive(Clone)]
pub(super) struct Sender {
    input: mpsc::SyncSender<Input>,
    cancelled: Arc<AtomicBool>,
}
impl Sender {
    pub fn send(&self, mut input: Input) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
                return Err(Error::new("runtime decoder queue cancelled or timed out"));
            }
            match self.input.try_send(input) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err(Error::new("runtime decoder disconnected"));
                }
                Err(mpsc::TrySendError::Full(pending)) => input = pending,
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// The supervisor owns cancellation even while a plugin call is blocked.
pub(super) struct Worker {
    pub input: Sender,
    pub outcome: mpsc::Receiver<std::result::Result<(), String>>,
    cancelled: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    pub fn poll(&self) -> Option<std::result::Result<(), String>> {
        match self.outcome.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                "runtime decoder disconnected; evidence incomplete".into(),
            )),
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(super) fn start(
    runtime: &crate::harness_runtime::Runtime,
    owner: SessionOwner,
    checkpoint: PathBuf,
    worktree: PathBuf,
    events: Arc<Mutex<Events>>,
) -> Result<Worker> {
    let mut session = runtime.open_stream()?;
    let first = session.call("decode_start", json!({"harness":owner.harness}))?;
    let initial = Snapshot::parse(first, &owner.harness, false)?;
    if initial.events.terminal
        || initial.events.failed
        || initial.events.session.is_some()
        || initial.events.unknown_events != 0
        || !initial.events.summary.is_empty()
    {
        return Err(invalid());
    }
    initial.apply(&mut *events.lock().map_err(|_| invalid())?, &worktree)?;
    let (input, receiver) = mpsc::sync_channel(16);
    let (completion, outcome) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel = Arc::clone(&cancelled);
    let thread = std::thread::spawn(move || {
        let run = || -> Result<()> {
            let mut stdout_done = false;
            let mut stderr_done = false;
            loop {
                if cancel.load(Ordering::Acquire) {
                    return Err(Error::new("runtime decoder cancelled"));
                }
                let input = match receiver.recv_timeout(Duration::from_millis(50)) {
                    Ok(input) => input,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => return Err(Error::new("runtime decoder input disconnected")),
                };
                match input {
                    Input::Line { stream, line } => {
                        if (stream == "stdout" && stdout_done)
                            || (stream == "stderr" && stderr_done)
                        {
                            return Err(invalid());
                        }
                        let value = session.call_cancellable(
                            "decode_event",
                            json!({"stream":stream,"line":line}),
                            &cancel,
                        )?;
                        let snapshot = Snapshot::parse(value, &owner.harness, false)?;
                        let checkpoint_session = {
                            let mut current = events.lock().map_err(|_| invalid())?;
                            let prior = current.session.clone();
                            snapshot.apply(&mut current, &worktree)?;
                            if prior.is_none() {
                                current.session.clone()
                            } else {
                                None
                            }
                        };
                        if let Some(native_session) = checkpoint_session {
                            durable_json(
                                &checkpoint,
                                &SessionCheckpoint {
                                    schema_version: 2,
                                    task_id: owner.task_id.clone(),
                                    attempt: owner.attempt,
                                    harness: owner.harness.clone(),
                                    session: native_session,
                                    native_data_location: None,
                                },
                            )?;
                        }
                    }
                    Input::End("stdout") if !stdout_done => stdout_done = true,
                    Input::End("stderr") if !stderr_done => stderr_done = true,
                    Input::End(_) => return Err(invalid()),
                }
                if stdout_done && stderr_done {
                    let value = session.call_cancellable("decode_finish", json!({}), &cancel)?;
                    Snapshot::parse(value, &owner.harness, true)?
                        .apply(&mut *events.lock().map_err(|_| invalid())?, &worktree)?;
                    return Ok(());
                }
            }
        };
        let mut run = run;
        let result = run().and_then(|()| session.finish());
        if result.is_err()
            && let Ok(mut events) = events.lock()
        {
            events.incomplete("runtime decoder failed; evidence incomplete");
        }
        let _ = completion
            .send(result.map_err(|_| "runtime decoder failed; evidence incomplete".into()));
    });
    Ok(Worker {
        input: Sender {
            input,
            cancelled: Arc::clone(&cancelled),
        },
        outcome,
        cancelled,
        thread: Some(thread),
    })
}

pub(super) fn validate_recovery(value: Value) -> Result<bool> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Recovery {
        recovered: bool,
    }
    let recovery: Recovery = serde_json::from_value(value).map_err(|_| invalid())?;
    Ok(recovery.recovered)
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn fixture() -> Value {
        let mut value = serde_json::to_value(Events::default()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .retain(|k, _| SNAPSHOT_FIELDS.contains(&k.as_str()));
        value["schema_version"] = json!(1);
        value["harness"] = json!("invented");
        value["finished"] = json!(false);
        value["terminal_evidence"] = Value::Null;
        value["summary"] = json!("");
        value["cost_tracking_limited"] = json!(false);
        value["native"] = serde_json::to_value(crate::native::Observations::default()).unwrap();
        value["native_completeness"] =
            serde_json::to_value(crate::native::Completeness::default()).unwrap();
        value["native_blockers"] = json!([]);
        value["write_target_candidates"] = json!([]);
        value["model_attributes"] = json!({"ahu.harness.model.reported.coverage":"partial"});
        value
    }
    #[test]
    fn normalized_schema_and_harness_are_required_without_provider_registry() {
        let value = fixture();
        assert!(Snapshot::parse(value.clone(), "invented", false).is_ok());
        assert!(Snapshot::parse(value.clone(), "different", false).is_err());
        assert!(Snapshot::parse(value.clone(), "invented", true).is_err());
        for field in SNAPSHOT_FIELDS {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(*field);
            assert!(
                Snapshot::parse(missing, "invented", false).is_err(),
                "{field}"
            );
        }
        let mut extra = value;
        extra["raw_event"] = json!("PRIVATE");
        assert!(Snapshot::parse(extra, "invented", false).is_err());
    }
    #[test]
    fn malformed_snapshots_reject_oversized_unsafe_and_wrong_typed_evidence() {
        for (path, bad) in [
            ("/summary", json!("x".repeat(65537))),
            ("/session", json!("bad\nidentity")),
            ("/native/helpers", json!(vec![Value::Null; 257])),
            ("/cost", json!({"usd":-1})),
            (
                "/model_attributes",
                json!({"ahu.harness.model.reported.coverage":"complete","private.key":"PRIVATE"}),
            ),
            ("/write_target_candidates", json!(["bad\0path"])),
            ("/native_completeness", json!(true)),
            ("/native_blockers", json!([42])),
        ] {
            let mut value = fixture();
            *value.pointer_mut(path).unwrap() = bad;
            assert!(Snapshot::parse(value, "invented", false).is_err(), "{path}");
        }
    }
    #[test]
    fn snapshots_replace_counts_but_preserve_host_failure_and_reject_session_drift() {
        let dir = tempfile::tempdir().unwrap();
        let mut events = Events::default();
        let mut value = fixture();
        value["session"] = json!("first");
        value["usage"]["input"] = json!(12);
        Snapshot::parse(value.clone(), "invented", false)
            .unwrap()
            .apply(&mut events, dir.path())
            .unwrap();
        Snapshot::parse(value.clone(), "invented", false)
            .unwrap()
            .apply(&mut events, dir.path())
            .unwrap();
        assert_eq!(events.usage.input, Some(12));
        events.incomplete("host observed failure");
        value["terminal"] = json!(true);
        Snapshot::parse(value.clone(), "invented", false)
            .unwrap()
            .apply(&mut events, dir.path())
            .unwrap();
        assert!(events.failed && events.trajectory.observation.stream_incomplete);
        assert!(events.blockers.iter().any(|b| b == "host observed failure"));
        value["session"] = json!("second");
        assert!(
            Snapshot::parse(value, "invented", false)
                .unwrap()
                .apply(&mut events, dir.path())
                .is_err()
        );
        assert_eq!(events.session.as_deref(), Some("first"));
    }
    #[test]
    fn skill_receipt_times_are_host_owned_and_stable_until_evidence_is_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let mut events = Events::default();
        let mut value = fixture();
        value["skills"] = json!([{"name":"review","harness":"invented","status":"completed",
            "completed_at":"provider-supplied","observed_at":"provider-supplied",
            "execution":"observed","evidence":"observed"}]);
        Snapshot::parse(value.clone(), "invented", false)
            .unwrap()
            .apply(&mut events, dir.path())
            .unwrap();
        let observed = events.skills[0].observed_at.clone();
        let completed = events.skills[0].completed_at.clone();
        assert!(completed.as_ref().is_some_and(|time| time.ends_with('Z')));
        assert_ne!(observed.as_deref(), Some("provider-supplied"));
        Snapshot::parse(value.clone(), "invented", false)
            .unwrap()
            .apply(&mut events, dir.path())
            .unwrap();
        assert_eq!(events.skills[0].observed_at, observed);
        assert_eq!(events.skills[0].completed_at, completed);
        value["skills"][0]["status"] = json!("invoked");
        value["skills"][0]["execution"] = json!("unverified");
        Snapshot::parse(value, "invented", false)
            .unwrap()
            .apply(&mut events, dir.path())
            .unwrap();
        assert!(events.skills[0].completed_at.is_none());
    }

    #[test]
    fn arbitrary_plugin_trajectory_and_metering_are_validated_without_provider_catalog() {
        let mut value = fixture();
        value["trajectory"]["source"] = json!("new-provider.session-v9");
        value["trajectory"]["metering"] =
            json!({"observed_credits":0.5,"credit_observations":1,"prompt_completions":1});
        let parsed = Snapshot::parse(value.clone(), "invented", false).unwrap();
        assert_eq!(
            parsed.events.trajectory.observation.source.as_str(),
            "new-provider.session-v9"
        );
        for source in [
            "",
            "https://private.invalid",
            "raw payload",
            &"x".repeat(129),
        ] {
            value["trajectory"]["source"] = json!(source);
            assert!(Snapshot::parse(value.clone(), "invented", false).is_err());
        }
        value["trajectory"]["source"] = json!("new-provider.session-v9");
        value["trajectory"]["metering"]["observed_credits"] = json!(-1);
        assert!(Snapshot::parse(value.clone(), "invented", false).is_err());
        value["trajectory"]["metering"] = json!({"raw":"PRIVATE"});
        assert!(Snapshot::parse(value, "invented", false).is_err());
    }

    #[test]
    fn cost_sources_are_observed_neutral_and_cannot_change_midstream() {
        let mut value = fixture();
        value["cost"] = json!({"usd":0.0,"source":"native_cumulative_usd"});
        let first = Snapshot::parse(value.clone(), "invented", false).unwrap();
        let mut events = Events::default();
        let dir = tempfile::tempdir().unwrap();
        first.apply(&mut events, dir.path()).unwrap();
        value["cost"]["source"] = json!("native_step_sum_usd");
        assert!(
            Snapshot::parse(value.clone(), "invented", false)
                .unwrap()
                .apply(&mut events, dir.path())
                .is_err()
        );
        for source in [json!("unknown_provider_cost"), Value::Null] {
            value["cost"]["source"] = source;
            assert!(Snapshot::parse(value.clone(), "invented", false).is_err());
        }
        value["cost"] = json!({});
        assert!(Snapshot::parse(value, "invented", false).is_ok());
    }

    #[test]
    fn recovery_response_requires_exact_schema() {
        assert!(validate_recovery(json!({"recovered":true,"raw":"PRIVATE"})).is_err());
        assert!(!validate_recovery(json!({"recovered":false})).unwrap());
    }
    #[test]
    fn queue_backpressure_is_bounded_and_cancellable() {
        let (input, receiver) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let sender = Sender {
            input,
            cancelled: Arc::clone(&cancelled),
        };
        sender.send(Input::End("stdout")).unwrap();
        let blocked = sender.clone();
        let completion = std::thread::spawn(move || blocked.send(Input::End("stderr")));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            Input::End("stdout")
        ));
        completion.join().unwrap().unwrap();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            Input::End("stderr")
        ));
        cancelled.store(true, Ordering::Release);
        assert!(sender.send(Input::End("stdout")).is_err());
    }
    #[test]
    fn worker_disconnect_is_failure_and_drop_joins_cancelled_owner() {
        let (input, _receiver) = mpsc::sync_channel(1);
        let (completion, outcome) = mpsc::channel();
        drop(completion);
        let cancelled = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancelled);
        let observed = Arc::clone(&stopped);
        let thread = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(1);
            while !cancel.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            observed.store(cancel.load(Ordering::Acquire), Ordering::Release);
        });
        let worker = Worker {
            input: Sender {
                input,
                cancelled: Arc::clone(&cancelled),
            },
            outcome,
            cancelled,
            thread: Some(thread),
        };
        assert!(matches!(worker.poll(), Some(Err(_))));
        drop(worker);
        assert!(stopped.load(Ordering::Acquire));
    }
}

#[cfg(test)]
#[path = "decoder_tests.rs"]
mod bridge_tests;
