//! Stateless, bounded projection of local collector OTLP JSONL exports.
//! Selection is supplied by the operator, never inferred from observed spans.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::util::{Error, Result};

const MAX_LINE: u64 = 16 * 1024 * 1024;
const MAX_SPANS: usize = 200_000;
const METRICS: &[&str] = &[
    "ahu.tokens.input",
    "ahu.tokens.output",
    "ahu.tokens.cached",
    "ahu.tokens.cache_write",
    "ahu.tokens.reasoning",
    "ahu.tokens.total",
    "ahu.cost.harness_reported_usd",
    "ahu.kiro.credits.observed",
    "ahu.kiro.elapsed.observed_ms",
    "ahu.kiro.context.last_percent",
    "ahu.kiro.context.peak_percent",
    "ahu.kiro.prompt_completions",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub task_id: String,
    pub attempt: u32,
    pub harness: String,
    pub model: String,
    pub mode: String,
    pub role: String,
    /// An operator label, not independently observed provider execution.
    pub provider: Option<String>,
    pub telemetry_enabled: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(harness: &str, mode: &str) -> Selection {
        Selection {
            task_id: "task-one".into(),
            attempt: 1,
            harness: harness.into(),
            model: "model".into(),
            mode: mode.into(),
            role: "implementation".into(),
            provider: None,
            telemetry_enabled: Some(true),
        }
    }
    fn attribute(key: &str, value: Value) -> Value {
        json!({"key":key,"value":if value.is_string() {json!({"stringValue":value})} else if value.is_u64() {json!({"intValue":value.as_u64().unwrap().to_string()})} else {json!({"doubleValue":value})}})
    }
    fn export(harness: &str, name: &str, extra: Vec<Value>) -> Value {
        json!({"resourceSpans":[{"resource":{"attributes":[
            attribute("ahu.release.id",json!("candidate")), attribute("ahu.task.id",json!("task-one")),
            attribute("ahu.task.attempt",json!("1")), attribute("ahu.harness",json!(harness)), attribute("ahu.model",json!("model")),
            attribute("service.name",json!("ahu-mcp")), attribute("service.version",json!("0.10.0"))
        ]},"scopeSpans":[{"spans":[{"name":name,"traceId":"11111111111111111111111111111111","spanId":"2222222222222222","attributes":extra}]}]}]})
    }
    #[test]
    fn supported_harnesses_preserve_zero_missing_and_partial_accounting() {
        for harness in ["codex", "claude-code", "antigravity", "opencode", "kiro"] {
            let mut r = Report::new(vec![selected(harness, "headless")], "candidate").unwrap();
            let mut a = vec![
                attribute("ahu.status", json!("succeeded")),
                attribute("ahu.events.coverage", json!("partial")),
                attribute("private.payload", json!("secret")),
            ];
            match harness {
                "kiro" => a.push(attribute("ahu.kiro.credits.observed", json!(0.25))),
                "antigravity" => (),
                _ => a.push(attribute("ahu.tokens.input", json!(0))),
            }
            r.consume(&export(harness, "ahu.harness.run", a)).unwrap();
            let out = r.finish();
            assert_eq!(out["included_attempts"], 1);
            assert_eq!(out["attempts"][0]["outcome"], "succeeded");
            assert!(out["attempts"][0]["metrics"]["ahu.tokens.output"].is_null());
            assert!(out["total_cost_usd"].is_null());
            assert!(!out.to_string().contains("secret"));
            if harness == "kiro" {
                assert_eq!(
                    out["attempts"][0]["metrics"]["ahu.kiro.credits.observed"],
                    0.25
                );
            } else if harness != "antigravity" {
                assert_eq!(out["attempts"][0]["metrics"]["ahu.tokens.input"], 0);
            }
        }
    }
    #[test]
    fn deduplication_conflicts_and_multiple_terminals_are_explicit() {
        let event = export(
            "codex",
            "ahu.harness.run",
            vec![attribute("ahu.tokens.input", json!(10))],
        );
        let mut r = Report::new(vec![selected("codex", "headless")], "candidate").unwrap();
        r.consume(&event).unwrap();
        r.consume(&event).unwrap();
        let out = r.finish();
        assert_eq!(out["duplicate_spans"], 1);
        assert_eq!(out["attempts"][0]["metrics"]["ahu.tokens.input"], 10);
        for same_id in [true, false] {
            let mut r = Report::new(vec![selected("codex", "headless")], "candidate").unwrap();
            r.consume(&event).unwrap();
            let mut changed = event.clone();
            let span = &mut changed["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
            span["attributes"] = json!([attribute("ahu.tokens.input", json!(20))]);
            if !same_id {
                span["spanId"] = json!("3333333333333333");
            }
            r.consume(&changed).unwrap();
            let out = r.finish();
            assert_eq!(out["attempts"][0]["coverage"], "conflicting_observations");
            assert!(out["attempts"][0]["metrics"]["ahu.tokens.input"].is_null());
        }
    }
    #[test]
    fn explicit_denominators_preserve_missing_interactive_disabled_and_retry_gaps() {
        let mut a = selected("codex", "interactive");
        a.attempt = 2;
        let mut b = selected("codex", "headless");
        b.task_id = "task-two".into();
        b.telemetry_enabled = Some(false);
        let r = Report::new(vec![selected("codex", "headless"), a, b], "candidate")
            .unwrap()
            .finish();
        assert_eq!(r["included_attempts"], 3);
        assert_eq!(r["attempts"][2]["coverage"], "telemetry_disabled");
        for group in r["groups"].as_array().unwrap() {
            assert_eq!(group["metrics"]["ahu.tokens.input"]["observed_attempts"], 0);
        }
        let mut r = Report::new(vec![selected("codex", "interactive")], "candidate").unwrap();
        r.consume(&export("codex", "ahu.mcp.tool.call", vec![]))
            .unwrap();
        let out = r.finish();
        assert_eq!(
            out["attempts"][0]["coverage"],
            "interactive_usage_unavailable"
        );
        assert_eq!(out["attempts"][0]["mcp_tool_calls"], 1);
    }
    #[test]
    fn cost_is_source_bound_and_never_converted_or_summed() {
        for (harness, source, expected) in [
            ("opencode", "opencode_step_finish_sum", true),
            ("claude-code", "claude_code_result_total", true),
            ("kiro", "opencode_step_finish_sum", false),
            ("codex", "unknown", false),
        ] {
            let mut r = Report::new(vec![selected(harness, "headless")], "candidate").unwrap();
            r.consume(&export(
                harness,
                "ahu.harness.run",
                vec![
                    attribute("ahu.cost.source", json!(source)),
                    attribute("ahu.cost.unit", json!("USD")),
                    attribute("ahu.cost.harness_reported_usd", json!(0)),
                ],
            ))
            .unwrap();
            let out = r.finish();
            assert_eq!(
                out["attempts"][0]["metrics"]["ahu.cost.harness_reported_usd"].is_number(),
                expected
            );
            assert!(out["total_cost_usd"].is_null());
        }
    }
    #[test]
    fn malformed_unselected_mismatched_and_bounded_inputs_fail_safely() {
        assert!(Report::new(vec![], "candidate").is_err());
        let s = selected("codex", "headless");
        assert!(Report::new(vec![s.clone(), s], "candidate").is_err());
        let mut r = Report::new(vec![selected("codex", "headless")], "candidate").unwrap();
        let event = export("kiro", "ahu.harness.run", vec![]);
        r.consume(&event).unwrap();
        assert_eq!(
            r.finish()["attempts"][0]["coverage"],
            "conflicting_observations"
        );
        let mut r = Report::new(vec![selected("codex", "headless")], "other").unwrap();
        r.consume(&event).unwrap();
        assert_eq!(r.finish()["attempts"][0]["spans"], 0);
        let mut r = Report::new(vec![selected("codex", "headless")], "candidate").unwrap();
        assert!(r.consume(&json!({})).is_err());
        let mut bad = export("codex", "ahu.harness.run", vec![]);
        bad["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["spanId"] = json!("0000000000000000");
        assert!(r.consume(&bad).is_err());
        assert!(
            attrs(&json!([
                attribute("ahu.status", json!("ok")),
                attribute("ahu.status", json!("ok"))
            ]))
            .is_err()
        );
        assert!(attrs(&json!([attribute("ahu.status", json!("\u{1b}secret"))])).is_err());
        r.scanned = MAX_SPANS;
        assert!(
            r.consume(&export("codex", "ahu.mcp.tool.call", vec![]))
                .is_err()
        );
    }
    #[cfg(unix)]
    #[test]
    fn special_inputs_are_rejected_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("capture");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: the NUL-terminated path lives through the call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(open_regular(&fifo).is_err());
        assert!(open_regular(dir.path()).is_err());
        assert!(open_regular(&dir.path().join("absent")).is_err());
    }

    #[test]
    fn local_file_report_writes_no_state_and_rejects_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let selection = dir.path().join("selection.json");
        let capture = dir.path().join("traces.jsonl");
        std::fs::write(
            &selection,
            serde_json::to_vec(&vec![selected("codex", "headless")]).unwrap(),
        )
        .unwrap();
        std::fs::write(
            &capture,
            format!("{}\n", export("codex", "ahu.mcp.tool.call", vec![])),
        )
        .unwrap();
        assert_eq!(
            report(std::slice::from_ref(&capture), &selection, "candidate").unwrap()["included_attempts"],
            1
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        std::fs::write(&capture, b"invalid private content").unwrap();
        let err = report(&[capture], &selection, "candidate")
            .unwrap_err()
            .to_string();
        assert!(!err.contains("private content"));
        assert!(report(&[], &selection, "candidate").is_err());
    }
}

fn invalid() -> Error {
    Error::new("invalid or oversized local OTLP report input")
}
fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
}

#[derive(Default)]
struct Observation {
    spans: usize,
    tool_calls: usize,
    terminals: usize,
    conflict: bool,
    outcome: Option<String>,
    coverage: Option<String>,
    cost_source: Option<String>,
    metrics: BTreeMap<String, Value>,
    traces: BTreeSet<String>,
    services: BTreeSet<(String, String)>,
}

fn attrs(value: &Value) -> Result<BTreeMap<String, Value>> {
    let Some(items) = value.as_array() else {
        return Ok(BTreeMap::new());
    };
    let mut out = BTreeMap::new();
    for item in items {
        let Some(key) = item["key"].as_str() else {
            return Err(invalid());
        };
        // Arbitrary attributes and payloads never enter the projection.
        if !matches!(
            key,
            "ahu.release.id"
                | "ahu.task.id"
                | "ahu.task.attempt"
                | "ahu.harness"
                | "ahu.model"
                | "ahu.execution.mode"
                | "ahu.status"
                | "ahu.events.coverage"
                | "ahu.cost.source"
                | "ahu.cost.unit"
                | "service.name"
                | "service.version"
        ) && !METRICS.contains(&key)
        {
            continue;
        }
        let v = &item["value"];
        let scalar = if let Some(s) = v["stringValue"].as_str() {
            if !label(s) {
                return Err(invalid());
            };
            json!(s)
        } else if let Some(n) = v["intValue"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| v["intValue"].as_u64())
        {
            json!(n)
        } else if let Some(n) = v["doubleValue"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
        {
            json!(n)
        } else {
            return Err(invalid());
        };
        if out.insert(key.to_owned(), scalar).is_some() {
            return Err(invalid());
        }
    }
    Ok(out)
}

fn string<'a>(a: &'a BTreeMap<String, Value>, key: &str) -> Option<&'a str> {
    a.get(key).and_then(Value::as_str)
}

fn open_regular(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| invalid())?;
    if !file.metadata().map_err(|_| invalid())?.is_file() {
        return Err(invalid());
    }
    Ok(file)
}

/// No repository, task store, billing API or writable database is accessed.
pub fn report(paths: &[std::path::PathBuf], selection: &Path, release: &str) -> Result<Value> {
    let file = open_regular(selection)?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() > 1024 * 1024 {
        return Err(invalid());
    }
    let selected: Vec<Selection> = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let mut engine = Report::new(selected, release)?;
    if paths.is_empty() || paths.len() > 32 {
        return Err(invalid());
    }
    for path in paths {
        let file = open_regular(path)?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if !metadata.is_file() || metadata.len() > 128 * 1024 * 1024 {
            return Err(invalid());
        }
        let mut input = BufReader::new(file);
        let mut total = 0u64;
        loop {
            let mut line = Vec::new();
            let count = (&mut input)
                .take(MAX_LINE + 1)
                .read_until(b'\n', &mut line)
                .map_err(|_| invalid())?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if count as u64 > MAX_LINE || total > 128 * 1024 * 1024 {
                return Err(invalid());
            }
            let value: Value = serde_json::from_slice(&line).map_err(|_| invalid())?;
            engine.consume(&value)?;
        }
    }
    Ok(engine.finish())
}

struct Report {
    release: String,
    selected: Vec<Selection>,
    rows: Vec<Observation>,
    seen: BTreeMap<(String, String), (usize, String)>,
    scanned: usize,
    duplicates: usize,
}

impl Report {
    fn new(selected: Vec<Selection>, release: &str) -> Result<Self> {
        if !label(release) || release.len() > 64 || selected.is_empty() || selected.len() > 4096 {
            return Err(invalid());
        }
        let mut ids = BTreeSet::new();
        for s in &selected {
            if s.attempt == 0
                || !ids.insert((&s.task_id, s.attempt))
                || ![&s.task_id, &s.harness, &s.model, &s.role]
                    .into_iter()
                    .all(|v| label(v))
                || !matches!(s.mode.as_str(), "interactive" | "headless")
                || s.provider.as_ref().is_some_and(|v| !label(v))
            {
                return Err(invalid());
            }
        }
        let rows = (0..selected.len())
            .map(|_| Observation::default())
            .collect();
        Ok(Self {
            release: release.into(),
            selected,
            rows,
            seen: BTreeMap::new(),
            scanned: 0,
            duplicates: 0,
        })
    }

    fn consume(&mut self, export: &Value) -> Result<()> {
        let resources = export["resourceSpans"].as_array().ok_or_else(invalid)?;
        for resource in resources {
            let a = attrs(&resource["resource"]["attributes"])?;
            if string(&a, "ahu.release.id") != Some(&self.release) {
                continue;
            }
            let task = string(&a, "ahu.task.id");
            let attempt = a
                .get("ahu.task.attempt")
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()));
            let Some(index) = self.selected.iter().position(|s| {
                Some(s.task_id.as_str()) == task && Some(s.attempt as u64) == attempt
            }) else {
                continue;
            };
            let s = &self.selected[index];
            if let (Some(name), Some(version)) =
                (string(&a, "service.name"), string(&a, "service.version"))
            {
                if self.rows[index].services.len() >= 32 {
                    return Err(invalid());
                }
                self.rows[index]
                    .services
                    .insert((name.into(), version.into()));
            }
            if string(&a, "ahu.harness") != Some(&s.harness)
                || string(&a, "ahu.model") != Some(&s.model)
                || string(&a, "ahu.execution.mode").is_some_and(|mode| mode != s.mode)
            {
                self.rows[index].conflict = true;
                continue;
            }
            for scope in resource["scopeSpans"].as_array().ok_or_else(invalid)? {
                for span in scope["spans"].as_array().ok_or_else(invalid)? {
                    self.span(index, span)?;
                }
            }
        }
        Ok(())
    }

    fn span(&mut self, index: usize, span: &Value) -> Result<()> {
        self.scanned += 1;
        if self.scanned > MAX_SPANS {
            return Err(invalid());
        }
        let id = |key: &str, len: usize| -> Result<String> {
            let s = span[key].as_str().ok_or_else(invalid)?;
            if s.len() != len
                || !s.bytes().all(|b| b.is_ascii_hexdigit())
                || s.bytes().all(|b| b == b'0')
            {
                return Err(invalid());
            }
            Ok(s.to_ascii_lowercase())
        };
        let key = (id("traceId", 32)?, id("spanId", 16)?);
        let digest = crate::util::digest_bytes(span.to_string().as_bytes());
        if let Some((previous, old)) = self.seen.get(&key) {
            self.duplicates += 1;
            if old != &digest || *previous != index {
                self.rows[*previous].conflict = true;
                self.rows[index].conflict = true;
            }
            return Ok(());
        }
        self.seen.insert(key.clone(), (index, digest));
        let row = &mut self.rows[index];
        row.spans += 1;
        row.traces.insert(key.0);
        if span["name"] == "ahu.mcp.tool.call" {
            row.tool_calls += 1;
        }
        if span["name"] != "ahu.harness.run" {
            return Ok(());
        }
        row.terminals += 1;
        if row.terminals > 1 {
            row.conflict = true;
            return Ok(());
        }
        let a = attrs(&span["attributes"])?;
        row.outcome = string(&a, "ahu.status")
            .filter(|s| {
                matches!(
                    *s,
                    "succeeded"
                        | "failed"
                        | "cancelled"
                        | "timed_out"
                        | "supervisor_error"
                        | "capture_failed"
                        | "boundary_violation"
                )
            })
            .map(str::to_owned);
        row.coverage = string(&a, "ahu.events.coverage")
            .filter(|s| matches!(*s, "partial" | "unavailable" | "complete_observed_stream"))
            .map(str::to_owned);
        row.cost_source = string(&a, "ahu.cost.source")
            .filter(|s| {
                (*s == "opencode_step_finish_sum" && self.selected[index].harness == "opencode")
                    || (*s == "claude_code_result_total"
                        && self.selected[index].harness == "claude-code")
            })
            .map(str::to_owned);
        for metric in METRICS {
            if *metric == "ahu.cost.harness_reported_usd"
                && (row.cost_source.is_none() || string(&a, "ahu.cost.unit") != Some("USD"))
            {
                continue;
            }
            if let Some(value) = a.get(*metric).filter(|v| v.is_number()) {
                row.metrics.insert((*metric).into(), value.clone());
            }
        }
        Ok(())
    }

    fn finish(self) -> Value {
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut attempts = Vec::new();
        for (i, (s, r)) in self.selected.iter().zip(&self.rows).enumerate() {
            let key = json!([s.harness, s.provider, s.model, s.mode, s.role]).to_string();
            groups.entry(key).or_default().push(i);
            let gap = if r.conflict {
                "conflicting_observations"
            } else if s.telemetry_enabled == Some(false) {
                "telemetry_disabled"
            } else if r.spans == 0 {
                "missing_telemetry"
            } else if s.mode == "interactive" {
                "interactive_usage_unavailable"
            } else if r.terminals == 0 {
                "missing_terminal_span"
            } else if r.metrics.is_empty() {
                "usage_unavailable"
            } else {
                "partial_accounting"
            };
            attempts.push(json!({"selection":s,"coverage":gap,"spans":r.spans,"mcp_tool_calls":r.tool_calls,
                "outcome":if r.conflict {None} else {r.outcome.as_deref()},"event_coverage":r.coverage,
                "cost_source":if r.conflict {None} else {r.cost_source.as_deref()},
                "metrics":METRICS.iter().map(|m| ((*m).to_string(), if r.conflict || s.mode == "interactive" || s.telemetry_enabled == Some(false) {Value::Null} else {r.metrics.get(*m).cloned().unwrap_or(Value::Null)})).collect::<BTreeMap<_,_>>(),
                "trace_count":r.traces.len(),"observed_service_versions":r.services}));
        }
        let summaries: Vec<_> = groups.into_iter().map(|(key, ids)| {
            let metrics: BTreeMap<_,_> = METRICS.iter().map(|metric| {
                let values: Vec<_> = ids.iter().filter_map(|i| attempts[*i]["metrics"][*metric].as_f64()).collect();
                ((*metric).to_string(), json!({"observed_attempts":values.len(),"unknown_attempts":ids.len()-values.len(),
                    "maximum_observed":values.iter().copied().reduce(f64::max),"sum":null}))
            }).collect();
            json!({"dimensions":serde_json::from_str::<Value>(&key).expect("serialized dimensions"),"included_attempts":ids.len(),"attempts_with_mcp_calls":ids.iter().filter(|i|self.rows[**i].tool_calls>0).count(),"metrics":metrics})
        }).collect();
        json!({"schema_version":1,"release_id":self.release,"source":"local_collector_otlp_jsonl",
            "selection_semantics":"explicit_operator_selection; provider and role are declared, not attested",
            "aggregation":"per_attempt_observations_only; cross_attempt_and_parent_child_additivity_not_established",
            "cost_basis":"harness_reported_not_invoice_grade; no price_conversion",
            "complete_cross_harness_total":false,"total_cost_usd":null,"included_attempts":attempts.len(),
            "duplicate_spans":self.duplicates,"attempts":attempts,"groups":summaries})
    }
}
