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
        Some(Value::String(_)) | Some(Value::Object(_)) => {}
        _ => return Err(Error::new("state must be a string or object")),
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

pub(super) fn call(arguments: &Value) -> Result<Value> {
    validate_arguments(arguments)?;
    let endpoint = std::env::var("AHU_DECISION_URL").map_err(|_| {
        Error::new("ahu_typed_decide requires AHU_DECISION_URL to name a local decision service")
    })?;
    call_endpoint(arguments, &endpoint)
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
    use super::{validate_arguments, validate_response};
    use serde_json::{Value, json};
    use std::io::{Read, Write};
    use std::net::TcpListener;

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
        arguments["state"] = json!(["not", "supported"]);
        assert!(validate_arguments(&arguments).is_err());

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
