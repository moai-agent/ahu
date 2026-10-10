//! Read-only advice over an explicit external record file and fixed case pool.
//! Evidence is locally editable, not identity attestation or provider billing.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{GroupKey, Record, case, key_for, provenance, stats, token_amount};
use crate::util::{Error, Result};

const MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RECORDS: usize = 4096;
const COST: &str = "ahu.cost.harness_reported_usd";
const TOKENS: [&str; 6] = [
    "ahu.tokens.input",
    "ahu.tokens.output",
    "ahu.tokens.cached",
    "ahu.tokens.cache_write",
    "ahu.tokens.reasoning",
    "ahu.tokens.total",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    case_id: String,
    case_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    records_path: String,
    cases: Vec<Case>,
    candidates: Vec<String>,
    minimum_quality: f64,
    minimum_samples_per_case: usize,
    maximum_age_seconds: u64,
}

pub(crate) fn tool_definition() -> Value {
    json!({
        "name":"ahu_agents_recommend",
        "description":"Read-only advisory comparison of registered agents over explicit external eval JSONL and a fixed case/candidate pool. Quality is the per-case 95% Wilson lower bound on deterministic answer passes over all attempts. Unknown, stale, mixed, unmatched or unsupported cost evidence abstains. Compares only observed harness-reported USD, never provider quota, prices or billing; does not launch or reconfigure agents.",
        "annotations":{"readOnlyHint":true,"destructiveHint":false,"openWorldHint":false},
        "inputSchema":{"type":"object","additionalProperties":false,
            "required":["records_path","cases","candidates","minimum_quality","minimum_samples_per_case","maximum_age_seconds"],
            "properties":{
                "records_path":{"type":"string","minLength":1,"maxLength":4096,"description":"Absolute path to external eval JSONL; at most 8 MiB and 4096 records."},
                "cases":{"type":"array","minItems":1,"maxItems":16,"items":{
                    "type":"object","additionalProperties":false,"required":["case_id","case_digest"],
                    "properties":{"case_id":{"type":"string","minLength":1,"maxLength":128},"case_digest":{"type":"string","pattern":"^[a-f0-9]{64}$"}}}},
                "candidates":{"type":"array","minItems":1,"maxItems":8,"uniqueItems":true,"items":{"type":"string","pattern":"^@[a-zA-Z0-9_-]{1,64}$"}},
                "minimum_quality":{"type":"number","exclusiveMinimum":0,"maximum":1},
                "minimum_samples_per_case":{"type":"integer","minimum":1,"maximum":4096},
                "maximum_age_seconds":{"type":"integer","minimum":1,"maximum":31536000}
            }}
    })
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn validate(arguments: &Value) -> Result<Request> {
    // Errors deliberately exclude caller paths, unknown field names and payloads.
    let invalid = || Error::new("invalid agent recommendation arguments");
    if serde_json::to_vec(arguments)?.len() > 16384 {
        return Err(invalid());
    }
    let request: Request = serde_json::from_value(arguments.clone()).map_err(|_| invalid())?;
    if request.records_path.len() > 4096
        || !Path::new(&request.records_path).is_absolute()
        || request.records_path.chars().any(char::is_control)
        || !(1..=16).contains(&request.cases.len())
        || !(1..=8).contains(&request.candidates.len())
        || !request.minimum_quality.is_finite()
        || !(0.0 < request.minimum_quality && request.minimum_quality <= 1.0)
        || !(1..=MAX_RECORDS).contains(&request.minimum_samples_per_case)
        || !(1..=31536000).contains(&request.maximum_age_seconds)
        || request
            .cases
            .iter()
            .any(|c| !provenance::safe_identity(&c.case_id) || !digest(&c.case_digest))
        || request.candidates.iter().any(|c| {
            let Some(name) = c.strip_prefix('@') else {
                return true;
            };
            name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
        || request.candidates.iter().collect::<BTreeSet<_>>().len() != request.candidates.len()
        || request
            .cases
            .iter()
            .map(|c| &c.case_id)
            .collect::<BTreeSet<_>>()
            .len()
            != request.cases.len()
    {
        return Err(invalid());
    }
    Ok(request)
}

#[derive(Clone)]
struct Candidate {
    name: String,
    version: String,
    harness: String,
    model: String,
    identity: String,
}

pub(crate) fn call(repo: &crate::git::Repo, arguments: &Value) -> Result<Value> {
    let request = validate(arguments)?;
    let text = read_records(repo, Path::new(&request.records_path))?;
    let records = super::parse_records(Path::new("external-eval-input"), &text)
        .map_err(|_| Error::new("invalid external eval records"))?;
    let agents = crate::agent::load_all(&repo.root)?;
    let candidates: Vec<_> = request
        .candidates
        .iter()
        .map(|name| {
            agents
                .iter()
                .find(|a| format!("@{}", a.manifest.name) == *name)
                .map(|a| Candidate {
                    name: name.clone(),
                    version: a.manifest.version.clone(),
                    harness: a.manifest.harness.clone(),
                    model: a.manifest.model.clone(),
                    identity: a.identity_digest(),
                })
        })
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::new("observation clock unavailable"))?
        .as_secs();
    let mut result = recommend(&request, &records, &candidates, now);
    result["records_fingerprint"] = crate::util::digest_bytes(text.as_bytes()).into();
    if serde_json::to_vec(&result)?.len() > 256 * 1024 {
        return Err(Error::new("agent recommendation output limit exceeded"));
    }
    Ok(result)
}

fn read_records(repo: &crate::git::Repo, path: &Path) -> Result<String> {
    let real = super::records_path_outside_repository(repo, path)
        .map_err(|_| Error::new("eval input must be a readable file outside the repository"))?;
    let read = || -> std::io::Result<String> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        // Refuse special files without blocking if the path is replaced after resolution.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
        }
        let file = options.open(&real)?;
        if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_BYTES {
            return Err(std::io::Error::other("record limit"));
        }
        let mut text = String::new();
        file.take(MAX_BYTES + 1).read_to_string(&mut text)?;
        if text.len() as u64 > MAX_BYTES
            || text.lines().filter(|l| !l.trim().is_empty()).count() > MAX_RECORDS
        {
            return Err(std::io::Error::other("record limit"));
        }
        Ok(text)
    };
    read().map_err(|_| {
        Error::new("eval input must be regular UTF-8 JSONL within 8 MiB and 4096 records")
    })
}

pub(super) fn supported_cost_source(source: &str) -> bool {
    matches!(
        source,
        "claude_code_result_total" | "opencode_step_finish_sum"
    )
}

fn cost_source(record: &Record) -> Option<&str> {
    match (
        record.harness.as_str(),
        record.reported_cost_source.as_deref(),
    ) {
        ("claude-code", Some("claude_code_result_total")) => Some("claude_code_result_total"),
        ("opencode", Some("opencode_step_finish_sum")) => Some("opencode_step_finish_sum"),
        _ => None,
    }
}

fn complete_identity(identity: &provenance::Identity, observations: u64) -> bool {
    observations > 0
        && identity.status() == "single"
        && identity.values.values().copied().sum::<u64>() == observations
}

fn model_matches(record: &Record, candidate: &Candidate) -> bool {
    record.harness_provenance.as_ref().is_some_and(|p| {
        p.valid() && p.model_mismatches == 0 && p.provider_mismatches == 0
            && complete_identity(&p.reported_model, p.observations)
            && p.reported_model.values.contains_key(&candidate.model)
            && complete_identity(&p.reported_provider, p.observations)
            // A partial/conflicting requested identity cannot silently override reported evidence.
            && complete_identity(&p.requested_model, p.observations)
            && p.requested_model.values.contains_key(&candidate.model)
            && complete_identity(&p.requested_provider, p.observations)
            && p.requested_provider.values.contains_key(&candidate.harness)
            && p.unattributed_model.conflicting == 0
            && p.unattributed_model.redacted == 0
            && p.unattributed_model.limited == 0
            && p.unattributed_model.values.keys().all(|m| m == &candidate.model)
    })
}

/// Keep every existing comparison dimension except the candidate being compared.
/// Different input fingerprints are expected across candidates, never within one.
fn pool_key(record: &Record) -> GroupKey {
    let mut key = key_for(record);
    key.agent.clear();
    key.agent_version.clear();
    key.agent_identity_digest.clear();
    key.harness.clear();
    key.harness_version.clear();
    key.model.clear();
    key.input_fingerprint.clear();
    key
}

fn mean(values: &[f64]) -> Option<f64> {
    // Divide first to avoid overflow and retain small/zero reported amounts exactly.
    (!values.is_empty())
        .then(|| values.iter().map(|v| v / values.len() as f64).sum::<f64>())
        .filter(|v| v.is_finite())
}

fn recommend(
    request: &Request,
    records: &[Record],
    candidates: &[Option<Candidate>],
    now: u64,
) -> Value {
    let mut rows = Vec::new();
    let mut pool_context: BTreeMap<&str, BTreeSet<GroupKey>> = BTreeMap::new();
    let mut pool_sources = BTreeSet::new();
    let mut task_counts = BTreeMap::new();
    for record in records {
        if selected(request, record) {
            if let Some(task) = record.task_id.as_deref() {
                *task_counts.entry(task).or_insert(0usize) += 1;
            }
            pool_context
                .entry(&record.case_id)
                .or_default()
                .insert(pool_key(record));
            if let Some(source) = cost_source(record) {
                pool_sources.insert(source);
            }
        }
    }
    let mixed_context = pool_context.values().any(|keys| keys.len() > 1);
    let mixed_cost = pool_sources.len() > 1;
    for (index, candidate) in candidates.iter().enumerate() {
        let mut reasons = BTreeSet::new();
        let mut cases = Vec::new();
        let mut case_costs = Vec::new();
        let mut quality_below = false;
        if mixed_context {
            reasons.insert("mixed_comparison_context");
        }
        if mixed_cost {
            reasons.insert("mixed_cost_sources");
        }
        let Some(candidate) = candidate else {
            rows.push(json!({"candidate":request.candidates[index],"status":"unknown","reasons":["unregistered_candidate"],"cases":[]}));
            continue;
        };
        for case in &request.cases {
            let items: Vec<_> = records
                .iter()
                .filter(|r| {
                    selected(request, r)
                        && r.case_id == case.case_id
                        && agent_matches(r, &candidate.name)
                })
                .collect();
            let n = items.len();
            let mut quality = 0;
            let mut passes = 0;
            let mut identity = 0;
            let mut fresh = 0;
            let mut ages = Vec::new();
            let mut costs = Vec::new();
            let mut sources = BTreeSet::new();
            let mut cost_provenance = 0;
            let mut providers = BTreeSet::new();
            let mut tokens: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
            let mut keys = BTreeSet::new();
            for r in &items {
                keys.insert(key_for(r));
                if let Some(p) = &r.harness_provenance {
                    providers.extend(p.reported_provider.values.keys());
                }
                if r.case_digest.as_deref() != Some(&case.case_digest) {
                    reasons.insert("case_digest_mismatch");
                }
                if r.fingerprint_completeness.as_deref() != Some("complete")
                    || !r.input_fingerprint.as_deref().is_some_and(digest)
                    || !r
                        .target_repo_head
                        .as_deref()
                        .is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                    || !r.ahu_build_digest.as_deref().is_some_and(digest)
                    || !r.skill_digest.as_deref().is_some_and(digest)
                    || !r.tool_definitions_digest.as_deref().is_some_and(digest)
                    || !r
                        .harness_version
                        .as_deref()
                        .is_some_and(provenance::safe_identity)
                    || !r
                        .ahu_version
                        .as_deref()
                        .is_some_and(provenance::safe_identity)
                    || !r
                        .corpus_version
                        .as_deref()
                        .is_some_and(provenance::safe_identity)
                    || r.case_schema_version != Some(u64::from(case::CASE_SCHEMA_VERSION))
                    || r.prompt_profile.as_deref()
                        != Some(case::PromptProfile::ToolNeutralV3.as_str())
                    || r.prompt_version.is_none_or(|n| n == 0)
                    || r.scoring_version.is_none_or(|n| n == 0)
                    || !matches!(
                        r.evaluator_kind.as_deref(),
                        Some("none" | "agent" | "typed_decision")
                    )
                {
                    reasons.insert("incomplete_comparison_provenance");
                }
                if r.agent_version.as_deref() == Some(&candidate.version)
                    && r.agent_identity_digest.as_deref() == Some(&candidate.identity)
                    && r.harness == candidate.harness
                    && r.model == candidate.model
                    && model_matches(r, candidate)
                {
                    identity += 1;
                } else {
                    reasons.insert("identity_missing_mixed_or_unmatched");
                }
                if let Some(age) = r
                    .observed_at_unix_seconds
                    .and_then(|t| (t > 0).then_some(t))
                    .and_then(|t| now.checked_sub(t))
                {
                    ages.push(age);
                    if age <= request.maximum_age_seconds {
                        fresh += 1;
                    }
                }
                if r.run_id
                    .as_deref()
                    .is_none_or(|s| !provenance::safe_identity(s))
                    || r.task_id
                        .as_deref()
                        .is_none_or(|s| !provenance::safe_identity(s))
                    || r.attempt != Some(1)
                    || r.attempts != Some(1.0)
                {
                    reasons.insert("run_task_or_attempt_unknown_or_retried");
                }
                if r.task_id
                    .as_deref()
                    .is_some_and(|t| task_counts.get(t).copied().unwrap_or(0) > 1)
                {
                    reasons.insert("duplicate_or_overlapping_task");
                }
                if r.score_policy.as_deref() == Some("deterministic_answer_v1")
                    && r.answer_passed.is_some()
                    && r.answer_passed == Some(r.passed)
                    && ((r.answer_status.as_deref() == Some("scored")
                        && r.answer_score.is_some()
                        && !super::is_no_answer_status(r.terminal_status.as_deref().unwrap_or("")))
                        || (r.answer_status.as_deref() == Some("no_answer")
                            && r.answer_passed == Some(false)
                            && r.answer_score.is_none()))
                {
                    quality += 1;
                    passes += usize::from(r.answer_pass());
                }
                if let Some(amount) = r
                    .reported_tokens
                    .as_ref()
                    .and_then(|m| m.get(COST))
                    .and_then(token_amount)
                {
                    costs.push(amount);
                    if let Some(source) = cost_source(r) {
                        sources.insert(source);
                        cost_provenance += 1;
                    }
                }
                for field in TOKENS {
                    if let Some(amount) = r
                        .reported_tokens
                        .as_ref()
                        .and_then(|m| m.get(field))
                        .and_then(token_amount)
                    {
                        tokens.entry(field).or_default().push(amount);
                    }
                }
            }
            if n == 0 {
                reasons.insert("missing_evidence");
            }
            if n < request.minimum_samples_per_case {
                reasons.insert("insufficient_samples");
            }
            if fresh != n {
                reasons.insert("freshness_missing_stale_or_future");
            }
            if keys.len() > 1 {
                reasons.insert("mixed_candidate_configuration");
            }
            if quality != n {
                reasons.insert("quality_unknown_or_conflicting");
            }
            if costs.len() != n || cost_provenance != n {
                reasons.insert("cost_missing_or_unsupported_provenance");
            }
            if providers.len() > 1 {
                reasons.insert("mixed_reported_providers");
            }
            let interval = stats::Interval::wilson_unrounded(passes, quality);
            let qualifies = interval.is_some_and(|i| i.lower >= request.minimum_quality);
            quality_below |= quality == n && n > 0 && !qualifies;
            let cost = mean(&costs);
            if !costs.is_empty() && cost.is_none() {
                reasons.insert("numeric_range_limited");
            }
            if let Some(cost) = cost {
                case_costs.push(cost);
            }
            cases.push(json!({
                "case_id":case.case_id,"case_digest":case.case_digest,"sample_count":n,
                "coverage":{"quality":quality,"identity":identity,"fresh":fresh,"reported_usd":costs.len(),"supported_cost_provenance":cost_provenance},
                "age_seconds":{"youngest":ages.iter().min(),"oldest":ages.iter().max(),"observations":ages.len()},
                "outcomes":{"passed":passes,"failed":quality-passes,"unknown":n-quality,
                    "no_answer":items.iter().filter(|r| r.answer_status.as_deref() == Some("no_answer")).count()},
                "quality":{"pass_rate":(quality > 0).then(|| passes as f64 / quality as f64),"interval":stats::interval_json(interval),"meets_minimum":qualifies},
                "reported_usd":{"mean":cost,"observations":costs.len(),"sources":sources},
                "tokens":tokens.iter().map(|(k,v)| (*k,json!({"mean":mean(v),"observations":v.len()}))).collect::<BTreeMap<_,_>>()
            }));
        }
        let pool_cost = (case_costs.len() == request.cases.len())
            .then(|| mean(&case_costs))
            .flatten();
        if case_costs.len() == request.cases.len() && pool_cost.is_none() {
            reasons.insert("numeric_range_limited");
        }
        let status = if !reasons.is_empty() {
            "unknown"
        } else if quality_below {
            "below_quality"
        } else {
            "eligible"
        };
        if quality_below {
            reasons.insert("quality_lower_bound_below_minimum");
        }
        rows.push(
            json!({"candidate":candidate.name,"registered_identity_digest":candidate.identity,
            "status":status,"reasons":reasons,"cases":cases,
            "equal_case_mean_reported_usd":pool_cost}),
        );
    }
    // Do not silently discard an unknown member of the explicitly requested pool.
    let unknown = rows.iter().any(|r| r["status"] == "unknown");
    let best = (!unknown)
        .then(|| {
            rows.iter()
                .filter(|r| r["status"] == "eligible")
                .filter_map(|r| r["equal_case_mean_reported_usd"].as_f64())
                .min_by(f64::total_cmp)
        })
        .flatten();
    let recommendations: Vec<_> = rows
        .iter()
        .filter(|r| {
            best.is_some()
                && r["status"] == "eligible"
                && r["equal_case_mean_reported_usd"].as_f64() == best
        })
        .map(|r| r["candidate"].clone())
        .collect();
    json!({"schema_version":1,"advisory":true,"status":if best.is_some() {"recommended"} else {"unknown"},
        "recommended":recommendations,"candidates":rows,"record_count":records.len(),
        "selected_record_count":records.iter().filter(|r| selected(request,r)).count(),
        "requirements":{"minimum_quality":request.minimum_quality,"minimum_samples_per_case":request.minimum_samples_per_case,"maximum_age_seconds":request.maximum_age_seconds},
        "uncertainty":["Local records are caller-supplied evidence, not authenticated model attestation.",
            "Wilson intervals describe observed answer passes; they do not establish future quality or independent samples.",
            "Reported USD is an observed native amount, not billing or total workflow cost; no price is inferred.",
            "Cross-attempt and parent-child additivity are not established. Retries and repeated task identities abstain; child overlap remains unknown.",
            "Costs use equal case weights; no quality or cost improvement is presumed."],
        "token_scope":"Native event usage fields observed per attempt; fields may overlap and are not additive.",
        "quota":"not_consulted"})
}

fn selected(request: &Request, r: &Record) -> bool {
    r.stage.as_deref().unwrap_or("candidate") == "candidate"
        && request.cases.iter().any(|c| c.case_id == r.case_id)
        && r.agent.as_deref().is_some_and(|a| {
            request
                .candidates
                .iter()
                .any(|c| a == c || Some(a) == c.strip_prefix('@'))
        })
}

fn agent_matches(record: &Record, name: &str) -> bool {
    record
        .agent
        .as_deref()
        .is_some_and(|a| a == name || Some(a) == name.strip_prefix('@'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments() -> Value {
        json!({"records_path":"/external/runs.jsonl",
            "cases":[{"case_id":"case-1","case_digest":"a".repeat(64)}],
            "candidates":["@careful","@cheap"],"minimum_quality":0.5,
            "minimum_samples_per_case":4,"maximum_age_seconds":60})
    }

    fn candidate(name: &str) -> Candidate {
        Candidate {
            name: name.into(),
            version: "1.0.0".into(),
            harness: "opencode".into(),
            model: "fixture-model".into(),
            identity: crate::util::digest_bytes(name.as_bytes()),
        }
    }

    fn fixture() -> (Request, Vec<Record>, Vec<Option<Candidate>>) {
        let candidates = vec![Some(candidate("@careful")), Some(candidate("@cheap"))];
        let mut records = Vec::new();
        for c in candidates.iter().flatten() {
            for i in 0..4 {
                let mut p = provenance::Provenance {
                    observations: 1,
                    ..Default::default()
                };
                for identity in [&mut p.requested_model, &mut p.reported_model] {
                    identity.values.insert(c.model.clone(), 1);
                }
                for identity in [&mut p.requested_provider, &mut p.reported_provider] {
                    identity.values.insert(c.harness.clone(), 1);
                }
                let pass = c.name == "@careful";
                let row = json!({"schema_version":2,"case_id":"case-1","case_digest":"a".repeat(64),
                    "stage":"candidate","agent":c.name,"agent_version":c.version,"agent_identity_digest":c.identity,
                    "model":c.model,"harness":c.harness,"harness_version":"1.0",
                    "score":if pass {1.0} else {0.0},"passed":pass,"score_policy":"deterministic_answer_v1",
                    "answer_status":"scored","answer_score":if pass {1.0} else {0.0},"answer_passed":pass,
                    "harness_provenance":p,"observed_at_unix_seconds":1000,
                    "run_id":"fixture-run","task_id":format!("{}-{i}",c.name.trim_start_matches('@')),"attempt":1,"attempts":1,
                    "reported_cost_source":"opencode_step_finish_sum",
                    "reported_tokens":{"ahu.cost.harness_reported_usd":{"kind":"observed_float","value":if pass {0.1} else {0.0}},"ahu.tokens.input":0},
                    "fingerprint_completeness":"complete","input_fingerprint":c.identity,
                    "target_repo_head":"b".repeat(40),"ahu_build_digest":"c".repeat(64),"skill_digest":"d".repeat(64),
                    "prompt_version":3,"scoring_version":1,"case_schema_version":case::CASE_SCHEMA_VERSION,
                    "prompt_profile":"tool_neutral_v3","evaluator_kind":"none","corpus_version":"1.0.0",
                    "tool_definitions_digest":"e".repeat(64),"ahu_version":"1.0.0"});
                records.extend(
                    super::super::parse_records(Path::new("fixture"), &row.to_string()).unwrap(),
                );
            }
        }
        (validate(&arguments()).unwrap(), records, candidates)
    }

    fn evaluate(records: &[Record]) -> Value {
        let (request, _, candidates) = fixture();
        recommend(&request, records, &candidates, 1030)
    }

    fn has_reason(result: &Value, reason: &str) -> bool {
        result["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["reasons"].as_array().unwrap().contains(&json!(reason)))
    }

    #[test]
    fn advice_uses_unrounded_quality_bounds() {
        let (mut request, mut records, candidates) = fixture();
        drop(records.drain(2..4));
        request.minimum_samples_per_case = 2;
        request.minimum_quality = 0.3424;
        let result = recommend(&request, &records, &candidates, 1030);
        assert_eq!(result["candidates"][0]["status"], "below_quality");
        assert!(result["recommended"].as_array().unwrap().is_empty());
        let lower = result["candidates"][0]["cases"][0]["quality"]["interval"]["lower"]
            .as_f64()
            .unwrap();
        assert!((lower - 0.342380227506653).abs() < 1e-14);
        request.minimum_quality = 0.34238;
        assert_eq!(
            recommend(&request, &records, &candidates, 1030)["recommended"],
            json!(["@careful"])
        );
    }

    #[test]
    fn native_model_coverage_and_missing_provider_block_advice() {
        use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
        for (model, provider, expected) in [
            (Some("fixture-model"), false, "unknown"),
            (None, true, "unknown"),
            (Some("other-model"), true, "unknown"),
            (Some("fixture-model"), true, "recommended"),
        ] {
            let (_, mut records, _) = fixture();
            let mut events = crate::headless::Events::default();
            events.observe(
                "opencode",
                &serde_json::to_vec(
                    &json!({"type":"step_start", "model":"fixture-model", "part":{"id":"start"}}),
                )
                .unwrap(),
            );
            let mut end = json!({"type":"step_finish", "part":{"id":"end", "reason":"stop"}});
            if let Some(model) = model {
                end["model"] = model.into();
            }
            events.observe("opencode", &serde_json::to_vec(&end).unwrap());
            let mut fields =
                crate::headless::identity::requested_attributes("opencode", "fixture-model")
                    .to_vec();
            fields.extend(events.reported_model_attributes());
            if provider {
                fields.push(("ahu.harness.provider.reported", "fixture-provider".into()));
            }
            let attributes: Vec<_> = fields
                .into_iter()
                .map(|(key, value)| KeyValue {
                    key: key.into(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(value)),
                    }),
                })
                .collect();
            let mut p = provenance::Provenance::default();
            p.observe(&attributes, "ahu.harness");
            for r in &mut records {
                r.harness_provenance = Some(p.clone());
            }
            assert_eq!(evaluate(&records)["status"], expected);
        }
    }

    #[test]
    fn fixed_pool_quality_guard_beats_free_low_quality_and_preserves_zero() {
        let (_, records, _) = fixture();
        let result = evaluate(&records);
        assert_eq!(result["recommended"], json!(["@careful"]));
        assert_eq!(result["candidates"][1]["status"], "below_quality");
        assert_eq!(
            result["candidates"][1]["cases"][0]["reported_usd"]["mean"],
            0.0
        );
        assert_eq!(
            result["candidates"][0]["cases"][0]["coverage"]["quality"],
            4
        );
        assert_eq!(
            result["candidates"][0]["cases"][0]["age_seconds"]["oldest"],
            30
        );
        assert_eq!(
            result["candidates"][0]["cases"][0]["tokens"]["ahu.tokens.input"]["mean"],
            0.0
        );
        assert!(result["uncertainty"].to_string().contains("parent-child"));
        assert_eq!(result["quota"], "not_consulted");
    }

    #[test]
    fn cheaper_qualified_candidate_can_win_but_unknown_quality_never_does() {
        let (_, mut records, _) = fixture();
        for r in &mut records[4..] {
            r.passed = true;
            r.answer_passed = Some(true);
            r.answer_score = Some(1.0);
            r.score = Some(1.0);
        }
        assert_eq!(evaluate(&records)["recommended"], json!(["@cheap"]));
        records[4].answer_passed = None;
        let result = evaluate(&records);
        assert_eq!(result["status"], "unknown");
        assert!(has_reason(&result, "quality_unknown_or_conflicting"));
    }

    #[test]
    fn missing_stale_conflicting_and_mixed_evidence_abstains() {
        type Mutation = fn(&mut Record);
        let mutations: [(Mutation, &str); 19] = [
            (
                |r| r.observed_at_unix_seconds = None,
                "freshness_missing_stale_or_future",
            ),
            (
                |r| r.observed_at_unix_seconds = Some(1),
                "freshness_missing_stale_or_future",
            ),
            (
                |r| r.observed_at_unix_seconds = Some(1040),
                "freshness_missing_stale_or_future",
            ),
            (
                |r| r.harness = "codex".into(),
                "identity_missing_mixed_or_unmatched",
            ),
            (
                |r| r.model = "other".into(),
                "identity_missing_mixed_or_unmatched",
            ),
            (
                |r| r.agent_identity_digest = Some("f".repeat(64)),
                "identity_missing_mixed_or_unmatched",
            ),
            (
                |r| r.harness_provenance = None,
                "identity_missing_mixed_or_unmatched",
            ),
            (
                |r| {
                    let model = &mut r.harness_provenance.as_mut().unwrap().reported_model;
                    model.values.clear();
                    model.conflicting = 1;
                },
                "identity_missing_mixed_or_unmatched",
            ),
            (
                |r| {
                    let p = r.harness_provenance.as_mut().unwrap();
                    p.observations = 2;
                    p.reported_model.values.insert("other".into(), 1);
                },
                "identity_missing_mixed_or_unmatched",
            ),
            (
                |r| r.harness_provenance.as_mut().unwrap().observations = 2,
                "identity_missing_mixed_or_unmatched",
            ),
            (
                |r| r.reported_cost_source = None,
                "cost_missing_or_unsupported_provenance",
            ),
            (
                |r| r.reported_cost_source = Some("quota_percent".into()),
                "cost_missing_or_unsupported_provenance",
            ),
            (
                |r| r.reported_tokens = None,
                "cost_missing_or_unsupported_provenance",
            ),
            (
                |r| r.fingerprint_completeness = None,
                "incomplete_comparison_provenance",
            ),
            (
                |r| r.input_fingerprint = Some("e".repeat(64)),
                "mixed_candidate_configuration",
            ),
            (
                |r| r.case_digest = Some("e".repeat(64)),
                "case_digest_mismatch",
            ),
            (
                |r| r.answer_passed = Some(false),
                "quality_unknown_or_conflicting",
            ),
            (
                |r| r.attempt = Some(2),
                "run_task_or_attempt_unknown_or_retried",
            ),
            (
                |r| r.run_id = None,
                "run_task_or_attempt_unknown_or_retried",
            ),
        ];
        for (change, reason) in mutations {
            let (_, mut records, _) = fixture();
            change(&mut records[0]);
            let result = evaluate(&records);
            assert_eq!(result["status"], "unknown", "{reason}: {result}");
            assert!(result["recommended"].as_array().unwrap().is_empty());
            assert!(has_reason(&result, reason), "{reason}: {result}");
        }
    }

    #[test]
    fn missing_is_not_zero_and_unsupported_cost_still_exposes_observed_amount() {
        let (_, mut records, _) = fixture();
        records[4].reported_cost_source = None;
        let result = evaluate(&records);
        assert_eq!(
            result["candidates"][1]["cases"][0]["reported_usd"]["mean"],
            0.0
        );
        assert_eq!(
            result["candidates"][1]["cases"][0]["coverage"]["supported_cost_provenance"],
            3
        );
        for r in &mut records[4..] {
            r.reported_tokens = None;
        }
        let result = evaluate(&records);
        assert!(result["candidates"][1]["cases"][0]["reported_usd"]["mean"].is_null());
        assert_eq!(
            result["candidates"][1]["cases"][0]["reported_usd"]["observations"],
            0
        );
    }

    #[test]
    fn duplicate_tasks_retries_changed_context_and_sources_do_not_pool() {
        let (_, mut records, _) = fixture();
        records.push(records[0].clone());
        assert!(has_reason(
            &evaluate(&records),
            "duplicate_or_overlapping_task"
        ));
        let (_, mut records, _) = fixture();
        records[0].target_repo_head = Some("e".repeat(40));
        assert!(has_reason(&evaluate(&records), "mixed_comparison_context"));
        let (request, mut records, mut candidates) = fixture();
        candidates[1].as_mut().unwrap().harness = "claude-code".into();
        for r in &mut records[4..] {
            r.harness = "claude-code".into();
            let p = r.harness_provenance.as_mut().unwrap();
            p.requested_provider.values = BTreeMap::from([("claude-code".into(), 1)]);
            p.reported_provider.values = p.requested_provider.values.clone();
            r.reported_cost_source = Some("claude_code_result_total".into());
        }
        let result = recommend(&request, &records, &candidates, 1030);
        assert!(has_reason(&result, "mixed_cost_sources"));
        assert_eq!(result["status"], "unknown");
    }

    #[test]
    fn explicit_pool_requires_every_case_and_candidate_without_survivor_selection() {
        let (mut request, records, mut candidates) = fixture();
        let result = evaluate(&records[..4]);
        assert!(has_reason(&result, "missing_evidence"));
        assert_eq!(result["status"], "unknown");
        candidates[0] = None;
        assert!(has_reason(
            &recommend(&request, &records, &candidates, 1030),
            "unregistered_candidate"
        ));
        request.cases.push(Case {
            case_id: "case-2".into(),
            case_digest: "b".repeat(64),
        });
        assert_eq!(
            recommend(&request, &records, &candidates, 1030)["status"],
            "unknown"
        );
        let (_, mut records, _) = fixture();
        let mut unrelated = records[0].clone();
        unrelated.case_id = "outside-pool".into();
        records.push(unrelated);
        assert_eq!(evaluate(&records)["recommended"], json!(["@careful"]));
        assert_eq!(evaluate(&records)["selected_record_count"], 8);
    }

    #[test]
    fn minimum_samples_and_uncertainty_guard_are_per_case() {
        let (mut request, records, candidates) = fixture();
        request.minimum_samples_per_case = 5;
        assert!(has_reason(
            &recommend(&request, &records, &candidates, 1030),
            "insufficient_samples"
        ));
        request.minimum_samples_per_case = 4;
        request.minimum_quality = 0.9;
        let result = recommend(&request, &records, &candidates, 1030);
        assert_eq!(result["status"], "unknown");
        assert_eq!(result["candidates"][0]["status"], "below_quality");
        assert_eq!(
            result["candidates"][0]["cases"][0]["quality"]["pass_rate"],
            1.0
        );
    }

    #[test]
    fn failed_attempts_count_against_quality_and_cases_have_equal_weight() {
        let (mut request, mut records, candidates) = fixture();
        for r in &mut records[4..] {
            r.answer_status = Some("no_answer".into());
            r.answer_score = None;
            r.score = None;
            r.terminal_status = Some("candidate_timed_out".into());
        }
        let result = recommend(&request, &records, &candidates, 1030);
        assert_eq!(result["recommended"], json!(["@careful"]));
        assert_eq!(
            result["candidates"][1]["cases"][0]["outcomes"]["no_answer"],
            4
        );
        assert_eq!(
            result["candidates"][1]["cases"][0]["coverage"]["quality"],
            4
        );
        request.cases.push(Case {
            case_id: "case-2".into(),
            case_digest: "b".repeat(64),
        });
        let mut second = records.clone();
        for r in &mut second {
            r.case_id = "case-2".into();
            r.case_digest = Some("b".repeat(64));
            r.task_id = r.task_id.as_ref().map(|s| format!("{s}-second"));
            if r.agent.as_deref() == Some("@careful") {
                r.reported_tokens
                    .as_mut()
                    .unwrap()
                    .insert(COST.into(), json!(0.3));
            }
        }
        // Extra repetitions of the cheaper case must not change case weighting.
        let mut extra = records[..4].to_vec();
        for r in &mut extra {
            r.task_id = r.task_id.as_ref().map(|s| format!("{s}-extra"));
        }
        records.extend(extra);
        records.extend(second);
        let result = recommend(&request, &records, &candidates, 1030);
        assert_eq!(result["recommended"], json!(["@careful"]));
        let cost = result["candidates"][0]["equal_case_mean_reported_usd"]
            .as_f64()
            .unwrap();
        assert!((cost - 0.2).abs() < 1e-12);
    }

    #[test]
    fn legacy_records_remain_readable_and_advice_unknown() {
        let row = json!({"schema_version":2,"case_id":"case-1","agent":"careful","harness":"opencode","model":"fixture-model","score":1,"passed":true});
        let records = super::super::parse_records(Path::new("fixture"), &row.to_string()).unwrap();
        assert_eq!(super::super::group(&records).len(), 1);
        assert_eq!(evaluate(&records)["status"], "unknown");
    }

    #[test]
    fn strict_bounded_arguments_reject_unknown_fields_and_bad_requirements() {
        for (field, value) in [
            ("quota", json!(99)),
            ("minimum_quality", json!(0)),
            ("minimum_quality", json!(1.1)),
            ("minimum_samples_per_case", json!(0)),
            ("minimum_samples_per_case", json!(1.5)),
            ("maximum_age_seconds", json!(0)),
            ("maximum_age_seconds", json!(31536001u64)),
            ("records_path", json!("relative.jsonl")),
            ("records_path", json!("/external/\nprivate")),
            ("candidates", json!(["@careful", "@careful"])),
            ("candidates", json!(["@private/secret"])),
            (
                "cases",
                json!([{"case_id":"case-1","case_digest":"a".repeat(64),"payload":"secret"}]),
            ),
        ] {
            let mut args = arguments();
            args[field] = value;
            assert!(validate(&args).is_err(), "{field}");
        }
        let mut args = arguments();
        args["candidates"] = json!(vec!["@careful"; 9]);
        assert!(validate(&args).is_err());
        assert!(validate(&Value::Null).is_err());
        assert!(validate(&arguments()).is_ok());
    }

    #[test]
    fn external_file_reader_rejects_repository_symlinks_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        crate::git::run_ok(dir.path(), &["init", "-q"]).unwrap();
        let repo = crate::git::discover(dir.path()).unwrap();
        let inside = dir.path().join("records.jsonl");
        std::fs::write(&inside, "").unwrap();
        assert!(read_records(&repo, &inside).is_err());
        let external = tempfile::tempdir().unwrap();
        let file = external.path().join("records.jsonl");
        std::fs::write(&file, "\n").unwrap();
        assert_eq!(read_records(&repo, &file).unwrap(), "\n");
        #[cfg(unix)]
        {
            let link = external.path().join("link.jsonl");
            std::os::unix::fs::symlink(&inside, &link).unwrap();
            assert!(read_records(&repo, &link).is_err());
        }
        std::fs::write(&file, "{}\n".repeat(MAX_RECORDS + 1)).unwrap();
        assert!(read_records(&repo, &file).is_err());
        std::fs::File::create(&file)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(read_records(&repo, &file).is_err());
    }

    #[test]
    fn recorder_projects_only_supported_cost_source_and_observation_time() {
        let trial = super::super::Trial {
            index: 1,
            case_index: 0,
            agent_index: 0,
            run_index: 1,
        };
        let fields = super::super::base_fields("fixture-run", &trial, "candidate");
        assert!(fields["observed_at_unix_seconds"].as_u64().unwrap() > 0);
        for source in [
            "opencode_step_finish_sum",
            "claude_code_result_total",
            "private-marker",
        ] {
            let mut fields = serde_json::Map::new();
            super::super::insert_trajectory_fields(
                &mut fields,
                Some(&json!({"harness":{"cost":{"usd":0,"source":source}}})),
                None,
                &Default::default(),
            )
            .unwrap();
            assert_eq!(
                fields.contains_key("reported_cost_source"),
                supported_cost_source(source)
            );
            assert!(!json!(fields).to_string().contains("private-marker"));
        }
    }
}
