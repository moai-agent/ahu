//! Optional model-neutral typed-decision service exposed through MCP.

use crate::util::{Error, Result};
use serde_json::{Map, Value, json};
use std::io::Read;
use std::path::Path;

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub(super) fn tool_definition() -> Value {
    json!({
        "name":"ahu_typed_decide",
        "description":"Ask the configured typed decision provider for bounded classification, scoring, or probability estimates; treat results as evidence and make the final decision yourself. If TypeSafe Jev is configured, this sends state and questions to TypeSafe AI over HTTPS. A local provider can be selected with AHU_DECISION_URL.",
        "inputSchema":{
            "type":"object",
            "properties":{
                "state":{"description":"The text or structured data to evaluate.","oneOf":[{"type":"string"},{"type":"object"},{"type":"array"}]},
                "questions":{
                    "type":"object","minProperties":1,"maxProperties":20,
                    "description":"Named questions. Each question has a portable type and a clear instruction.",
                    "additionalProperties":{
                        "type":"object","properties":{
                            "type":{"type":"string","enum":["choice","score","probability"],"description":"choice selects one option; score estimates a number between min and max; probability estimates whether the instruction is true, from 0 to 1."},
                            "telemetry_key":{"type":"string","pattern":"^[a-z][a-z0-9_.-]{0,47}$","description":"Optional stable, non-sensitive evaluation dimension; recorded in telemetry, never used as an instruction."},
                            "instructions":{"type":"string","minLength":1,"maxLength":2048},
                            "options":{"type":"object","minProperties":2,"maxProperties":32,"description":"Required for choice; map each stable answer key to a short description.","additionalProperties":{"type":"string"}},
                            "min":{"type":"number","description":"Required for score; inclusive lower bound."},"max":{"type":"number","description":"Required for score; inclusive upper bound."}
                        },"required":["type","instructions"],"additionalProperties":false
                    }
                }
            },"required":["state","questions"],"additionalProperties":false
        }
    })
}

pub(super) fn validate_arguments(arguments: &Value) -> Result<()> {
    let object = arguments
        .as_object()
        .ok_or_else(|| Error::new("arguments must be an object"))?;
    if serde_json::to_vec(arguments)?.len() > MAX_REQUEST_BYTES {
        return Err(Error::new("typed decision request exceeds 64 KiB"));
    }
    if object
        .keys()
        .any(|key| key != "state" && key != "questions")
    {
        return Err(Error::new(
            "typed decision accepts only state and questions",
        ));
    }
    match object.get("state") {
        Some(Value::String(_)) | Some(Value::Object(_)) | Some(Value::Array(_)) => {}
        _ => return Err(Error::new("state must be a string, object, or array")),
    }
    let questions = object
        .get("questions")
        .and_then(Value::as_object)
        .filter(|questions| !questions.is_empty() && questions.len() <= 20)
        .ok_or_else(|| Error::new("questions must be a nonempty object with at most 20 entries"))?;
    let mut telemetry_keys = std::collections::BTreeSet::new();
    for (name, question) in questions {
        if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
            return Err(Error::new("invalid typed decision question name"));
        }
        let q = question
            .as_object()
            .ok_or_else(|| Error::new(format!("question {name:?} must be an object")))?;
        if q.keys().any(|key| {
            ![
                "type",
                "instructions",
                "telemetry_key",
                "options",
                "min",
                "max",
            ]
            .contains(&key.as_str())
        }) {
            return Err(Error::new(format!(
                "question {name:?} has an unknown field"
            )));
        }
        if let Some(key) = q.get("telemetry_key") {
            let key = key
                .as_str()
                .filter(|key| {
                    (1..=48).contains(&key.len())
                        && key.as_bytes()[0].is_ascii_lowercase()
                        && key.bytes().all(|byte| {
                            byte.is_ascii_lowercase()
                                || byte.is_ascii_digit()
                                || matches!(byte, b'_' | b'.' | b'-')
                        })
                })
                .ok_or_else(|| {
                    Error::new("telemetry_key must be a lowercase non-sensitive identifier")
                })?;
            if !telemetry_keys.insert(key) {
                return Err(Error::new(
                    "telemetry_key values must be unique within a decision request",
                ));
            }
        }
        let kind = q["type"]
            .as_str()
            .ok_or_else(|| Error::new(format!("question {name:?} requires a type")))?;
        if !["choice", "score", "probability"].contains(&kind) {
            return Err(Error::new(format!("unsupported question type {kind:?}")));
        }
        if !q["instructions"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty() && s.len() <= 2048)
        {
            return Err(Error::new(format!(
                "question {name:?} requires instructions"
            )));
        }
        match kind {
            "choice" => {
                let options = q["options"]
                    .as_object()
                    .filter(|options| (2..=32).contains(&options.len()))
                    .ok_or_else(|| {
                        Error::new(format!("choice {name:?} requires 2 to 32 options"))
                    })?;
                if q.contains_key("min") || q.contains_key("max") {
                    return Err(Error::new(format!("choice {name:?} cannot set min or max")));
                }
                if options.iter().any(|(key, description)| {
                    key.is_empty()
                        || key.len() > 128
                        || !description
                            .as_str()
                            .is_some_and(|s| !s.trim().is_empty() && s.len() <= 512)
                }) {
                    return Err(Error::new(format!("choice {name:?} has invalid options")));
                }
            }
            "score" => {
                if q.contains_key("options")
                    || !q["min"].as_f64().is_some_and(f64::is_finite)
                    || !q["max"].as_f64().is_some_and(f64::is_finite)
                    || q["min"].as_f64().unwrap() >= q["max"].as_f64().unwrap()
                {
                    return Err(Error::new(format!(
                        "score {name:?} requires finite min and max with min < max"
                    )));
                }
            }
            "probability" => {
                if q.contains_key("options") || q.contains_key("min") || q.contains_key("max") {
                    return Err(Error::new(format!(
                        "probability {name:?} cannot set options, min, or max"
                    )));
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

pub(super) fn call(arguments: &Value, repo: &crate::git::Repo) -> Result<Value> {
    validate_arguments(arguments)?;
    if let Ok(endpoint) = std::env::var("AHU_DECISION_URL") {
        return call_endpoint(arguments, &endpoint);
    }
    let api_key = typesafe_api_key(repo)?;
    call_typesafe(arguments, &api_key)
}

/// Read only the TypeSafe credential needed by this tool. Process environment
/// wins; otherwise read the project checkout's ignored `.env` without loading
/// any values into the process environment or logging them.
fn typesafe_api_key(repo: &crate::git::Repo) -> Result<String> {
    if let Some(value) = process_api_key(std::env::var("TYPESAFE_API_KEY").ok())? {
        return Ok(value);
    }

    let root = repo.primary_root()?;
    dotenv_api_key(&root)
}

fn process_api_key(value: Option<String>) -> Result<Option<String>> {
    match value {
        Some(value) if !value.trim().is_empty() => checked_api_key(value).map(Some),
        _ => Ok(None),
    }
}

fn dotenv_api_key(root: &Path) -> Result<String> {
    const KEY_NAMES: [&str; 1] = ["TYPESAFE_API_KEY"];
    let path = match crate::util::resolve_existing_within(&root, ".env") {
        Ok(Some(path)) => path,
        Ok(None) => {
            return Err(Error::new(
                "ahu_typed_decide needs TYPESAFE_API_KEY in the MCP server environment or the primary checkout's .env",
            ));
        }
        Err(_) => {
            return Err(Error::new(
                "cannot safely read .env for ahu_typed_decide: the file or a parent is a symlink",
            ));
        }
    };
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| Error::new("cannot inspect .env for ahu_typed_decide"))?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err(Error::new(
            ".env for ahu_typed_decide must be a regular file no larger than 64 KiB",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::new(
                ".env permissions are too broad for an API credential; restrict access (for example, chmod 600 .env)",
            ));
        }
    }

    let vars = dotenvy::from_path_iter(&path).map_err(|_| {
        Error::new("cannot parse .env for ahu_typed_decide; values were not displayed")
    })?;
    let mut selected = Map::new();
    for item in vars {
        let (name, value) = item.map_err(|_| {
            Error::new("cannot parse .env for ahu_typed_decide; values were not displayed")
        })?;
        if KEY_NAMES.contains(&name.as_str()) && !value.trim().is_empty() {
            selected.entry(name).or_insert(Value::String(value));
        }
    }
    KEY_NAMES
        .iter()
        .find_map(|name| selected.get(*name).and_then(Value::as_str))
        .map(str::to_string)
        .map(checked_api_key)
        .transpose()?
        .ok_or_else(|| Error::new(".env does not define a nonempty TypeSafe API key"))
}

fn checked_api_key(value: String) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(Error::new("TypeSafe API key is empty or malformed"));
    }
    Ok(value.to_string())
}

const TYPESAFE_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

fn call_typesafe(arguments: &Value, api_key: &str) -> Result<Value> {
    call_typesafe_at(arguments, api_key, TYPESAFE_ENDPOINT)
}

/// `endpoint` is injectable for local HTTP tests; production always uses the
/// fixed HTTPS endpoint above and never accepts a user-provided remote URL.
fn call_typesafe_at(arguments: &Value, api_key: &str, endpoint: &str) -> Result<Value> {
    let body = typesafe_request(arguments)?;
    let started = std::time::Instant::now();
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| Error::new("cannot create TypeSafe decision client"))?;
    let response = client
        .post(endpoint)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .map_err(|_| Error::new("TypeSafe decision request failed"))?;
    if !response.status().is_success() {
        return Err(Error::new(format!(
            "TypeSafe decision API returned HTTP {}",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(Error::new("TypeSafe decision response exceeds 1 MiB"));
    }
    let mut bytes = Vec::new();
    response
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::new("cannot read TypeSafe decision response"))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(Error::new("TypeSafe decision response exceeds 1 MiB"));
    }
    let response: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::new("TypeSafe decision response is invalid JSON"))?;
    let mut result = typesafe_response(arguments, response)?;
    result["service"]["duration_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
    Ok(result)
}

fn typesafe_request(arguments: &Value) -> Result<Value> {
    let questions = arguments["questions"]
        .as_object()
        .ok_or_else(|| Error::new("questions must be an object"))?;
    let mut translated = Map::new();
    for (name, question) in questions {
        let instructions = question["instructions"].clone();
        let kind = question["type"].as_str().unwrap_or_default();
        let upstream = match kind {
            "choice" => json!({
                "type":"choice",
                "instructions":instructions,
                "criteria":question["options"]
            }),
            "score" => json!({
                "type":"score",
                "instructions":instructions,
                "criteria":[
                    format!("Minimum score ({})", question["min"]),
                    format!("Maximum score ({})", question["max"])
                ]
            }),
            "probability" => json!({
                "type":"noul",
                "instructions":instructions
            }),
            _ => return Err(Error::new(format!("unsupported question type {kind:?}"))),
        };
        translated.insert(name.clone(), upstream);
    }
    Ok(json!({
        "model":"jev-latest",
        "state":arguments["state"],
        "questions":translated
    }))
}

fn typesafe_response(arguments: &Value, response: Value) -> Result<Value> {
    let answers = response
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::new("TypeSafe response requires an answers object"))?;
    let questions = arguments["questions"]
        .as_object()
        .ok_or_else(|| Error::new("questions must be an object"))?;
    if answers.len() != questions.len() || questions.keys().any(|key| !answers.contains_key(key)) {
        return Err(Error::new(
            "TypeSafe answers must match the requested question names",
        ));
    }
    let mut normalized = Map::new();
    for (name, question) in questions {
        let answer = answers[name]
            .as_object()
            .ok_or_else(|| Error::new(format!("TypeSafe answer {name:?} must be an object")))?;
        let kind = question["type"].as_str().unwrap_or_default();
        let response_kind = answer
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut value = match kind {
            "choice" if response_kind == "choice" => {
                let choice = answer
                    .get("choice")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        Error::new(format!("TypeSafe choice answer {name:?} is missing"))
                    })?;
                if question["options"].get(choice).is_none() {
                    return Err(Error::new(format!(
                        "TypeSafe choice answer {name:?} is not a requested option"
                    )));
                }
                json!(choice)
            }
            "score" if response_kind == "score" => {
                let raw = answer.get("score").and_then(Value::as_f64).ok_or_else(|| {
                    Error::new(format!("TypeSafe score answer {name:?} is missing"))
                })?;
                if !raw.is_finite() || !(0.0..=1.0).contains(&raw) {
                    return Err(Error::new(format!(
                        "TypeSafe score answer {name:?} is outside its two-point rubric"
                    )));
                }
                let minimum = question["min"].as_f64().unwrap();
                let maximum = question["max"].as_f64().unwrap();
                json!(minimum + raw * (maximum - minimum))
            }
            "probability" if response_kind == "noul" => {
                let probability = answer.get("noul").and_then(Value::as_f64).ok_or_else(|| {
                    Error::new(format!("TypeSafe noul answer {name:?} is missing"))
                })?;
                if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
                    return Err(Error::new(format!(
                        "TypeSafe noul answer {name:?} is outside 0 to 1"
                    )));
                }
                json!(probability)
            }
            _ => {
                return Err(Error::new(format!(
                    "TypeSafe answer {name:?} has the wrong type"
                )));
            }
        };
        let mut item = Map::new();
        item.insert("value".into(), value.take());
        if let Some(confidence) = answer.get("confidence") {
            if !confidence
                .as_f64()
                .is_some_and(|n| (0.0..=1.0).contains(&n))
            {
                return Err(Error::new(format!(
                    "TypeSafe confidence for {name:?} must be between 0 and 1"
                )));
            }
            item.insert("confidence".into(), confidence.clone());
        }
        normalized.insert(name.clone(), Value::Object(item));
    }
    let usage = response.get("usage").unwrap_or(&Value::Null);
    let mut service = json!({
        "backend":"typesafe",
        "model":response.get("model").and_then(Value::as_str).unwrap_or("jev-latest")
    });
    if let Some(tokens) = usage.get("input_tokens") {
        service["prompt_tokens"] = tokens.clone();
    }
    if let Some(tokens) = usage.get("output_tokens") {
        service["generated_tokens"] = tokens.clone();
    }
    Ok(json!({"answers":normalized,"service":service}))
}

fn call_endpoint(arguments: &Value, endpoint: &str) -> Result<Value> {
    let url = url::Url::parse(endpoint)
        .map_err(|error| Error::new(format!("invalid AHU_DECISION_URL: {error}")))?;
    if url.scheme() != "http"
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.host_str().is_some_and(|host| {
            host.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        })
    {
        return Err(Error::new(
            "AHU_DECISION_URL must be a credential-free http:// URL with a loopback IP literal",
        ));
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|error| Error::new(format!("cannot create decision client: {error}")))?;
    let response = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(arguments)
        .send()
        .map_err(|error| Error::new(format!("decision service request failed: {error}")))?;
    if !response.status().is_success() {
        return Err(Error::new(format!(
            "decision service returned HTTP {}",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(Error::new("decision service response exceeds 1 MiB"));
    }
    let mut bytes = Vec::new();
    response
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::new(format!("cannot read decision response: {error}")))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(Error::new("decision service response exceeds 1 MiB"));
    }
    let result: Value = serde_json::from_slice(&bytes)
        .map_err(|error| Error::new(format!("decision service returned invalid JSON: {error}")))?;
    validate_response(arguments, result)
}

fn validate_response(arguments: &Value, result: Value) -> Result<Value> {
    let answers = result
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::new("decision service response requires an answers object"))?;
    let questions: &Map<String, Value> = arguments["questions"].as_object().unwrap();
    if answers.len() != questions.len() || questions.keys().any(|key| !answers.contains_key(key)) {
        return Err(Error::new(
            "decision service answers must match the requested question names",
        ));
    }
    for (name, question) in questions {
        let answer = answers[name]
            .as_object()
            .ok_or_else(|| Error::new(format!("answer {name:?} must be an object")))?;
        let value = answer
            .get("value")
            .ok_or_else(|| Error::new(format!("answer {name:?} requires value")))?;
        if let Some(confidence) = answer.get("confidence")
            && !confidence
                .as_f64()
                .is_some_and(|n| (0.0..=1.0).contains(&n))
        {
            return Err(Error::new(format!(
                "answer {name:?} confidence must be between 0 and 1"
            )));
        }
        let valid = match question["type"].as_str().unwrap() {
            "choice" => value
                .as_str()
                .is_some_and(|choice| question["options"].get(choice).is_some()),
            "score" => value.as_f64().is_some_and(|score| {
                score.is_finite()
                    && score >= question["min"].as_f64().unwrap()
                    && score <= question["max"].as_f64().unwrap()
            }),
            "probability" => value
                .as_f64()
                .is_some_and(|probability| (0.0..=1.0).contains(&probability)),
            _ => false,
        };
        if !valid {
            return Err(Error::new(format!(
                "answer {name:?} has a value outside its requested type or range"
            )));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{
        call_typesafe_at, checked_api_key, dotenv_api_key, process_api_key, typesafe_request,
        typesafe_response, validate_arguments, validate_response,
    };
    use serde_json::{Value, json};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;

    fn request(key: Value) -> Value {
        json!({
            "state": {},
            "questions": {
                "route": {
                    "type": "choice",
                    "instructions": "Pick a route",
                    "options": {"a":"Route A", "b":"Route B"},
                    "telemetry_key": key
                }
            }
        })
    }

    #[test]
    fn telemetry_key_accepts_only_bounded_lowercase_identifiers() {
        assert!(validate_arguments(&request(json!("routing.v1"))).is_ok());
        assert!(validate_arguments(&request(json!("Private label"))).is_err());
        assert!(validate_arguments(&request(json!("1route"))).is_err());
        assert!(validate_arguments(&request(json!("a".repeat(49)))).is_err());
        assert!(validate_arguments(&request(json!(17))).is_err());
    }

    #[test]
    fn telemetry_keys_must_be_unique_per_request() {
        let mut arguments = request(json!("route"));
        arguments["questions"]["risk"] = json!({
            "type":"probability",
            "instructions":"Estimate risk",
            "telemetry_key":"route"
        });
        assert!(validate_arguments(&arguments).is_err());
    }

    fn typed_request() -> Value {
        json!({
            "state": {"text":"A duplicate charge"},
            "questions": {
                "route": {
                    "type":"choice","instructions":"Choose a team",
                    "options":{"billing":"Payments","other":"Everything else"}
                },
                "urgency": {
                    "type":"score","instructions":"Estimate urgency",
                    "min":0,"max":2
                },
                "refund": {
                    "type":"probability","instructions":"Is a refund requested?"
                }
            }
        })
    }

    fn jev_response() -> Value {
        json!({
            "model":"jev-1.13.0",
            "answers":{
                "route":{"type":"choice","choice":"billing","confidence":0.9,
                    "probabilities":{"billing":0.9,"other":0.1}},
                "urgency":{"type":"score","score":0.75,"confidence":0.8,
                    "legend":{"0":"Minimum score (0)","1":"Maximum score (2)"}},
                "refund":{"type":"noul","noul":0.8}
            },
            "usage":{"input_tokens":123,"output_tokens":17}
        })
    }

    #[test]
    fn translates_neutral_questions_to_jev_primitives_and_maps_typed_answers_back() {
        let mut request = typed_request();
        request["questions"]["route"]["telemetry_key"] = json!("department");
        let upstream = typesafe_request(&request).unwrap();
        assert_eq!(upstream["model"], "jev-latest");
        assert_eq!(upstream["questions"]["route"]["type"], "choice");
        assert_eq!(
            upstream["questions"]["route"]["criteria"]["billing"],
            "Payments"
        );
        assert_eq!(upstream["questions"]["urgency"]["type"], "score");
        assert_eq!(
            upstream["questions"]["urgency"]["criteria"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(upstream["questions"]["refund"]["type"], "noul");
        assert!(upstream.to_string().find("telemetry_key").is_none());

        let result = typesafe_response(&request, jev_response()).unwrap();
        assert_eq!(result["answers"]["route"]["value"], "billing");
        assert_eq!(result["answers"]["route"]["confidence"], 0.9);
        assert_eq!(result["answers"]["urgency"]["value"], 1.5);
        assert_eq!(result["answers"]["refund"]["value"], 0.8);
        assert_eq!(result["service"]["backend"], "typesafe");
        assert_eq!(result["service"]["model"], "jev-1.13.0");
        assert_eq!(result["service"]["prompt_tokens"], 123);
        assert_eq!(result["service"]["generated_tokens"], 17);
    }

    #[test]
    fn rejects_malformed_jev_answer_types_and_values() {
        for response in [
            json!({"answers":{}}),
            json!({"answers":{"route":{"type":"choice","choice":"unknown"},"urgency":{"type":"score","score":0.5},"refund":{"type":"noul","noul":0.5}}}),
        ] {
            assert!(typesafe_response(&typed_request(), response).is_err());
        }
        let mut invalid = jev_response();
        invalid["answers"]["refund"]["noul"] = json!(1.2);
        assert!(typesafe_response(&typed_request(), invalid).is_err());
        let mut invalid = jev_response();
        invalid["answers"]["urgency"]["score"] = json!(1.2);
        assert!(typesafe_response(&typed_request(), invalid).is_err());

        for (name, value) in [
            ("route", json!({"type":"score","score":0.5})),
            ("urgency", json!({"type":"choice","choice":"billing"})),
            ("refund", json!({"type":"choice","choice":"billing"})),
        ] {
            let mut invalid = jev_response();
            invalid["answers"][name] = value;
            assert!(typesafe_response(&typed_request(), invalid).is_err());
        }

        let mut invalid = jev_response();
        invalid["answers"]["route"]["confidence"] = json!(1.1);
        assert!(typesafe_response(&typed_request(), invalid).is_err());
        let mut invalid = jev_response();
        invalid["answers"]["route"]["confidence"] = json!("high");
        assert!(typesafe_response(&typed_request(), invalid).is_err());
    }

    #[test]
    fn jev_translation_handles_optional_metadata_and_rejects_unvalidated_inputs() {
        let mut response = jev_response();
        response.as_object_mut().unwrap().remove("model");
        response.as_object_mut().unwrap().remove("usage");
        let mapped = typesafe_response(&typed_request(), response).unwrap();
        assert_eq!(mapped["service"]["model"], "jev-latest");
        assert!(mapped["service"].get("prompt_tokens").is_none());
        assert!(mapped["service"].get("generated_tokens").is_none());

        assert!(typesafe_request(&json!({"questions":[]})).is_err());
        assert!(typesafe_response(&typed_request(), json!({"answers":[]})).is_err());
        assert!(typesafe_response(&json!({"questions":[]}), jev_response()).is_err());

        let mut unsupported = typed_request();
        unsupported["questions"]["route"]["type"] = json!("unsupported");
        assert!(typesafe_request(&unsupported).is_err());
        assert!(typesafe_response(&unsupported, jev_response()).is_err());

        let mut missing = jev_response();
        missing["answers"]["route"] = json!(null);
        assert!(typesafe_response(&typed_request(), missing).is_err());
        let mut missing = jev_response();
        missing["answers"]["refund"]["noul"] = json!(null);
        assert!(typesafe_response(&typed_request(), missing).is_err());
    }

    #[test]
    fn sends_auth_only_to_the_injected_endpoint_and_keeps_payload_metadata_safe() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let headers = String::from_utf8_lossy(&request).to_ascii_lowercase();
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .trim()
                .parse::<usize>()
                .unwrap();
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            assert!(headers.contains("authorization: bearer test-secret"));
            let payload: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(payload["model"], "jev-latest");
            assert!(payload.to_string().find("telemetry_key").is_none());
            let response = serde_json::to_vec(&jev_response()).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            )
            .unwrap();
            stream.write_all(&response).unwrap();
        });
        let mut request = typed_request();
        request["questions"]["route"]["telemetry_key"] = json!("department");
        let result = call_typesafe_at(&request, "test-secret", &endpoint).unwrap();
        server.join().unwrap();
        assert_eq!(result["service"]["backend"], "typesafe");
        assert!(result["service"]["duration_ms"].as_f64().is_some());
    }

    #[test]
    fn dotenv_reads_only_credentials_without_exporting_values_and_requires_private_mode() {
        let root = tempfile::tempdir().unwrap();
        let env_file = root.path().join(".env");
        std::fs::write(
            &env_file,
            "OTHER_VALUE=do-not-export\nTYPESAFE_API_KEY=canonical-key\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&env_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(
            dotenv_api_key(Path::new(root.path())).unwrap(),
            "canonical-key"
        );
        assert!(std::env::var("OTHER_VALUE").is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&env_file, std::fs::Permissions::from_mode(0o644)).unwrap();
            let error = dotenv_api_key(root.path()).unwrap_err().to_string();
            assert!(error.contains("chmod 600"));
            assert!(!error.contains("canonical-key"));
        }
    }

    #[test]
    fn dotenv_refuses_missing_links_special_paths_oversize_and_malformed_files() {
        fn private(path: &Path) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }

        let missing = tempfile::tempdir().unwrap();
        let error = dotenv_api_key(missing.path()).unwrap_err().to_string();
        assert!(error.contains("needs TYPESAFE_API_KEY"));

        let no_key = tempfile::tempdir().unwrap();
        let no_key_file = no_key.path().join(".env");
        std::fs::write(&no_key_file, "OTHER=value\n").unwrap();
        private(&no_key_file);
        let error = dotenv_api_key(no_key.path()).unwrap_err().to_string();
        assert!(error.contains("does not define a nonempty"));

        let malformed = tempfile::tempdir().unwrap();
        std::fs::write(
            malformed.path().join(".env"),
            "TYPESAFE_API_KEY='unterminated\n",
        )
        .unwrap();
        private(&malformed.path().join(".env"));
        let error = dotenv_api_key(malformed.path()).unwrap_err().to_string();
        assert!(error.contains("cannot parse .env"));

        let special = tempfile::tempdir().unwrap();
        std::fs::create_dir(special.path().join(".env")).unwrap();
        let error = dotenv_api_key(special.path()).unwrap_err().to_string();
        assert!(error.contains("regular file"));

        let oversized = tempfile::tempdir().unwrap();
        std::fs::write(oversized.path().join(".env"), vec![b'x'; 65 * 1024]).unwrap();
        let error = dotenv_api_key(oversized.path()).unwrap_err().to_string();
        assert!(error.contains("64 KiB"));

        #[cfg(unix)]
        {
            let linked = tempfile::tempdir().unwrap();
            let outside = linked.path().join("outside");
            std::fs::write(&outside, "TYPESAFE_API_KEY=synthetic\n").unwrap();
            std::os::unix::fs::symlink(outside, linked.path().join(".env")).unwrap();
            let error = dotenv_api_key(linked.path()).unwrap_err().to_string();
            assert!(error.contains("symlink"));
        }
    }

    #[test]
    fn api_key_validation_trims_values_and_rejects_empty_or_control_characters() {
        assert_eq!(
            checked_api_key("  synthetic-key  ".into()).unwrap(),
            "synthetic-key"
        );
        assert!(checked_api_key(" \t ".into()).is_err());
        assert!(checked_api_key("synthetic\nkey".into()).is_err());
        assert_eq!(process_api_key(None).unwrap(), None);
        assert_eq!(process_api_key(Some("  ".into())).unwrap(), None);
        assert_eq!(
            process_api_key(Some(" process-key ".into())).unwrap(),
            Some("process-key".into())
        );
        assert!(process_api_key(Some("invalid\nkey".into())).is_err());
    }

    fn valid_response() -> Value {
        json!({
            "answers": {
                "route":{"value":"billing","confidence":0.9},
                "urgency":{"value":1.5},
                "refund":{"value":0.8}
            },
            "service":{"backend":"ollama","model":"fixture"}
        })
    }

    #[test]
    fn validates_each_typed_answer_and_keeps_service_metadata() {
        let result = validate_response(&typed_request(), valid_response()).expect("valid response");
        assert_eq!(result["answers"]["route"]["value"], "billing");
        assert_eq!(result["service"]["model"], "fixture");
    }

    #[test]
    fn rejects_incomplete_mismatched_and_out_of_range_decisions() {
        let invalid = [
            json!({"service":{"model":"fixture"}}),
            json!({"answers":{"route":{"value":"billing"}}}),
            json!({"answers":{
                "route":{"value":"billing"},
                "urgency":{"value":1.0},
                "refund":{"value":0.5},
                "extra":{"value":true}
            }}),
            json!({"answers":{
                "route":{"value":"unknown"},
                "urgency":{"value":1.0},"refund":{"value":0.5}
            }}),
            json!({"answers":{
                "route":{"value":"billing"},
                "urgency":{"value":2.1},"refund":{"value":0.5}
            }}),
            json!({"answers":{
                "route":{"value":"billing"},
                "urgency":{"value":1.0},"refund":{"value":1.1}
            }}),
            json!({"answers":{
                "route":{"value":"billing","confidence":1.1},
                "urgency":{"value":1.0},"refund":{"value":0.5}
            }}),
            json!({"answers":{
                "route":{"value":"billing","confidence":"high"},
                "urgency":{"value":1.0},"refund":{"value":0.5}
            }}),
        ];
        for response in invalid {
            assert!(
                validate_response(&typed_request(), response).is_err(),
                "invalid service response was accepted"
            );
        }
    }

    #[test]
    fn refuses_invalid_argument_shapes_and_numeric_bounds() {
        let mut arguments = typed_request();
        arguments["state"] = Value::Null;
        assert!(validate_arguments(&arguments).is_err());

        let mut arguments = typed_request();
        arguments["state"] = json!(["supported", "structured", "state"]);
        assert!(validate_arguments(&arguments).is_ok());

        let mut arguments = typed_request();
        arguments["questions"]["urgency"]["min"] = json!(2);
        assert!(validate_arguments(&arguments).is_err());

        let mut arguments = typed_request();
        arguments["questions"]["refund"]["min"] = json!(0);
        assert!(validate_arguments(&arguments).is_err());

        let mut arguments = typed_request();
        arguments["unexpected"] = json!(true);
        assert!(validate_arguments(&arguments).is_err());

        let mut arguments = typed_request();
        arguments["state"] = json!("x".repeat(64 * 1024));
        assert!(validate_arguments(&arguments).is_err());

        let mut arguments = typed_request();
        let options = arguments["questions"]["route"]["options"]
            .as_object_mut()
            .expect("choice options");
        for index in 2..=32 {
            options.insert(format!("extra_{index}"), json!("Extra route"));
        }
        assert!(validate_arguments(&arguments).is_err());
    }

    #[test]
    fn argument_validation_rejects_each_question_shape_before_dispatch() {
        let mut request = typed_request();
        request["questions"]["bad\nname"] = request["questions"]["route"].clone();
        assert!(validate_arguments(&request).is_err());

        let mut request = typed_request();
        request["questions"]["route"]["unexpected"] = json!(true);
        assert!(validate_arguments(&request).is_err());

        let mut request = typed_request();
        request["questions"]["route"]["type"] = json!("freeform");
        assert!(validate_arguments(&request).is_err());

        let mut request = typed_request();
        request["questions"]["route"]["instructions"] = json!("  ");
        assert!(validate_arguments(&request).is_err());

        for options in [json!({}), json!({"a":"A"}), json!({"a":"A","b":" "})] {
            let mut request = typed_request();
            request["questions"]["route"]["options"] = options;
            assert!(validate_arguments(&request).is_err());
        }

        let mut request = typed_request();
        request["questions"]["route"]["min"] = json!(0);
        assert!(validate_arguments(&request).is_err());

        let mut request = typed_request();
        request["questions"]["urgency"]["options"] = json!({"a":"A"});
        assert!(validate_arguments(&request).is_err());

        let mut request = typed_request();
        request["questions"]["refund"]["max"] = json!(1);
        assert!(validate_arguments(&request).is_err());
    }

    #[test]
    fn response_validation_requires_object_answers_and_typed_values() {
        for response in [json!({}), json!({"answers": []})] {
            assert!(validate_response(&typed_request(), response).is_err());
        }

        for answer in [
            json!(null),
            json!({}),
            json!({"value":"billing","confidence":-0.1}),
        ] {
            let mut response = valid_response();
            response["answers"]["route"] = answer;
            assert!(validate_response(&typed_request(), response).is_err());
        }

        let mut response = valid_response();
        response["answers"]["route"]["value"] = json!(false);
        assert!(validate_response(&typed_request(), response).is_err());
    }

    fn serve_once(response: &'static [u8]) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = format!("http://{}/decide", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let (status, body) = if response == b"oversized" {
                ("200 OK", Vec::new())
            } else if response == b"bad-status" {
                ("503 Service Unavailable", b"{}".to_vec())
            } else {
                ("200 OK", response.to_vec())
            };
            let length = if response == b"oversized" {
                1_048_577
            } else {
                body.len()
            };
            let mut output = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n").into_bytes();
            output.extend(body);
            stream.write_all(&output).unwrap();
        });
        address
    }

    #[test]
    fn decision_http_boundary_accepts_valid_results_and_rejects_bad_status_and_size() {
        for (endpoint, needle) in [
            ("not a URL", "invalid AHU_DECISION_URL"),
            ("ftp://127.0.0.1", "loopback IP literal"),
            ("http://localhost", "loopback IP literal"),
            ("http://user@127.0.0.1", "loopback IP literal"),
        ] {
            assert!(
                super::call_endpoint(&typed_request(), endpoint)
                    .unwrap_err()
                    .to_string()
                    .contains(needle)
            );
        }
        for (body, expected_error) in [
            (br#"{"answers":{"route":{"value":"billing"},"urgency":{"value":1.5},"refund":{"value":0.8}}}"#.as_slice(), None),
            (b"bad-status".as_slice(), Some("HTTP 503")),
            (b"invalid-json".as_slice(), Some("invalid JSON")),
            (b"oversized".as_slice(), Some("exceeds 1 MiB")),
        ] {
            let url = serve_once(body);
            let result = super::call_endpoint(&typed_request(), &url);
            if let Some(needle) = expected_error {
                assert!(result.unwrap_err().to_string().contains(needle));
            } else {
                assert_eq!(result.unwrap()["answers"]["route"]["value"], "billing");
            }
        }
    }
}
