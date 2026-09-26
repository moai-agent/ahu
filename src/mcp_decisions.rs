//! Optional model-neutral typed-decision service exposed through MCP.

use crate::util::{Error, Result};
use serde_json::{Map, Value, json};
use std::io::Read;

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub(super) fn tool_definition() -> Value {
    json!({
        "name":"ahu_typed_decide",
        "description":"Ask a configured local decision service for typed answers. Use for bounded classification, scoring, or probability estimates; treat the result as evidence and make the final decision yourself. Requires AHU_DECISION_URL.",
        "inputSchema":{
            "type":"object",
            "properties":{
                "state":{"description":"The text or structured data to evaluate.","oneOf":[{"type":"string"},{"type":"object"}]},
                "questions":{
                    "type":"object","minProperties":1,"maxProperties":20,
                    "description":"Named questions. Each question has a portable type and a clear instruction.",
                    "additionalProperties":{
                        "type":"object","properties":{
                            "type":{"type":"string","enum":["choice","score","probability"],"description":"choice selects one option; score estimates a number between min and max; probability estimates whether the instruction is true, from 0 to 1."},
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
        Some(Value::String(_)) | Some(Value::Object(_)) => {}
        _ => return Err(Error::new("state must be a string or object")),
    }
    let questions = object
        .get("questions")
        .and_then(Value::as_object)
        .filter(|questions| !questions.is_empty() && questions.len() <= 20)
        .ok_or_else(|| Error::new("questions must be a nonempty object with at most 20 entries"))?;
    for (name, question) in questions {
        if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
            return Err(Error::new("invalid typed decision question name"));
        }
        let q = question
            .as_object()
            .ok_or_else(|| Error::new(format!("question {name:?} must be an object")))?;
        if q.keys()
            .any(|key| !["type", "instructions", "options", "min", "max"].contains(&key.as_str()))
        {
            return Err(Error::new(format!(
                "question {name:?} has an unknown field"
            )));
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

pub(super) fn call(arguments: &Value) -> Result<Value> {
    validate_arguments(arguments)?;
    let endpoint = std::env::var("AHU_DECISION_URL").map_err(|_| {
        Error::new("ahu_typed_decide requires AHU_DECISION_URL to name a local decision service")
    })?;
    let url = url::Url::parse(&endpoint)
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
