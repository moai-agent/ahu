//! Executable runtime implementation support; never linked into ahu core.
mod launch;
use ahu_harness_api::{
    Failure, MAX_FRAME_BYTES, Operation, PROTOCOL_VERSION, RemoteError, Request, Response, Success,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

/// Run one selected provider with bounded JSONL framing and per-process decoder state.
pub fn run(harness: &str) -> std::process::ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--ahu-harness-runtime-protocol=1"))
        || arguments.next().is_some()
    {
        return std::process::ExitCode::from(2);
    }
    match serve(harness, std::io::stdin().lock(), std::io::stdout().lock()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(()) => std::process::ExitCode::from(2),
    }
}
fn serve(harness: &str, mut input: impl BufRead, mut output: impl Write) -> Result<(), ()> {
    let mut state = State {
        harness,
        decoder: None,
    };
    let mut previous = 0;
    loop {
        let mut line = Vec::new();
        // take bounds allocation even when an adversarial peer never sends newline.
        let count = std::io::Read::take(input.by_ref(), MAX_FRAME_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| ())?;
        if count == 0 {
            return Ok(());
        }
        if count > MAX_FRAME_BYTES || line.last() != Some(&b'\n') {
            return Err(());
        }
        let request: Request = ahu_harness_api::json::from_slice(&line).map_err(|_| ())?;
        if request.protocol_version != PROTOCOL_VERSION || request.id <= previous {
            return Err(());
        }
        previous = request.id;
        let response = match state.call(request.operation, request.params) {
            Ok(result) => Response::Success(Success {
                protocol_version: PROTOCOL_VERSION,
                id: request.id,
                result,
            }),
            Err(error) => Response::Failure(Failure {
                protocol_version: PROTOCOL_VERSION,
                id: request.id,
                error,
            }),
        };
        let mut bytes = serde_json::to_vec(&response).map_err(|_| ())?;
        bytes.push(b'\n');
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(());
        }
        output
            .write_all(&bytes)
            .and_then(|()| output.flush())
            .map_err(|_| ())?;
    }
}
struct State<'a> {
    harness: &'a str,
    decoder: Option<ahu_harness_events::Decoder>,
}
impl State<'_> {
    fn call(&mut self, operation: Operation, params: Value) -> Result<Value, RemoteError> {
        match operation {
            Operation::Launch => {
                launch::plan(self.harness, params).map_err(|_| RemoteError::InvalidInput)
            }
            Operation::DecodeStart => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Start {
                    harness: String,
                }
                let start: Start =
                    serde_json::from_value(params).map_err(|_| RemoteError::InvalidInput)?;
                if start.harness != self.harness || self.decoder.is_some() {
                    return Err(RemoteError::InvalidInput);
                }
                let decoder = ahu_harness_events::Decoder::new(self.harness)
                    .map_err(|_| RemoteError::Unsupported)?;
                let snapshot = decoder.snapshot();
                self.decoder = Some(decoder);
                Ok(snapshot)
            }
            Operation::DecodeEvent => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Event {
                    stream: String,
                    line: String,
                }
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Batch {
                    events: Vec<Event>,
                }
                let events = if params.get("events").is_some() {
                    let batch: Batch =
                        serde_json::from_value(params).map_err(|_| RemoteError::InvalidInput)?;
                    if batch.events.is_empty()
                        || batch.events.len() > 64
                        || batch.events.iter().map(|e| e.line.len()).sum::<usize>() > 64 * 1024
                    {
                        return Err(RemoteError::InvalidInput);
                    }
                    batch.events
                } else {
                    vec![serde_json::from_value(params).map_err(|_| RemoteError::InvalidInput)?]
                };
                if events
                    .iter()
                    .any(|e| !matches!(e.stream.as_str(), "stdout" | "stderr"))
                {
                    return Err(RemoteError::InvalidInput);
                }
                let decoder = self.decoder.as_mut().ok_or(RemoteError::InvalidInput)?;
                for event in events {
                    match event.stream.as_str() {
                        "stdout" => decoder.ingest_stdout(event.line.as_bytes()),
                        _ => decoder.ingest_stderr(event.line.as_bytes()),
                    }
                    .map_err(|_| RemoteError::Failed)?;
                }
                Ok(decoder.snapshot())
            }
            Operation::DecodeFinish => {
                if params != json!({}) {
                    return Err(RemoteError::InvalidInput);
                }
                self.decoder
                    .as_mut()
                    .ok_or(RemoteError::InvalidInput)?
                    .finish()
                    .map_err(|_| RemoteError::Failed)
            }
            _ => {
                let name =
                    serde_json::to_value(operation).map_err(|_| RemoteError::InvalidInput)?;
                ahu_harness_native::call(
                    self.harness,
                    name.as_str().ok_or(RemoteError::InvalidInput)?,
                    &params,
                )
                .map_err(|error| match error.diagnostic {
                    Some(ahu_harness_native::Diagnostic::NeedsNativeTrust) => {
                        RemoteError::NeedsNativeTrust
                    }
                    Some(ahu_harness_native::Diagnostic::NotSignedIn) => RemoteError::NotSignedIn,
                    None if error.effects_unknown => RemoteError::OutcomeUnknown,
                    None => RemoteError::Failed,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(id: u64, operation: &str, params: Value) -> Vec<u8> {
        let mut line = serde_json::to_vec(
            &json!({"protocol_version":1,"id":id,"operation":operation,"params":params}),
        )
        .unwrap();
        line.push(b'\n');
        line
    }
    #[test]
    fn batches_match_individual_events_and_reject_invalid_bounds() {
        let mut individual = State {
            harness: "codex",
            decoder: None,
        };
        let mut batched = State {
            harness: "codex",
            decoder: None,
        };
        for state in [&mut individual, &mut batched] {
            state
                .call(Operation::DecodeStart, json!({"harness":"codex"}))
                .unwrap();
        }
        let events = vec![
            json!({"stream":"stdout","line":r#"{"type":"thread.started","thread_id":"fixture-session"}"#}),
            json!({"stream":"stderr","line":"diagnostic"}),
            json!({"stream":"stdout","line":r#"{"type":"item.completed","item":{"type":"tool_call","name":"write_file","arguments":{"path":"src/example.rs"}}}"#}),
        ];
        let mut expected = Value::Null;
        for event in &events {
            expected = individual
                .call(Operation::DecodeEvent, event.clone())
                .unwrap();
        }
        let actual = batched
            .call(Operation::DecodeEvent, json!({"events":events}))
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual["session"], "fixture-session");
        assert!(
            actual["write_target_candidates"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "src/example.rs")
        );
        for params in [
            json!({"events":[]}),
            json!({"events":vec![json!({"stream":"stderr","line":"x"});65]}),
            json!({"events":[{"stream":"stderr","line":"x".repeat(65537)}]}),
            json!({"events":[{"stream":"stdin","line":"x"}]}),
            json!({"events":[{"stream":"stderr","line":"x"}],"line":"unexpected"}),
        ] {
            assert!(batched.call(Operation::DecodeEvent, params).is_err());
        }
        assert!(
            batched
                .call(
                    Operation::DecodeEvent,
                    json!({"events":[{"stream":"stdout","line":"not json"}]})
                )
                .is_err()
        );
    }
    #[test]
    fn frames_are_ordered_strict_and_bounded() {
        let first = frame(1, "decode_start", json!({"harness":"codex"}));
        let mut input = first.clone();
        input.extend(first);
        assert!(serve("codex", &input[..], Vec::new()).is_err());
        for input in [
            b"{\"id\":1,\"id\":2}\n".to_vec(),
            frame(0, "decode_start", json!({"harness":"codex"})),
            vec![b'x'; MAX_FRAME_BYTES + 1],
        ] {
            assert!(serve("codex", &input[..], Vec::new()).is_err());
        }
        let mut input = frame(1, "decode_start", json!({"harness":"codex"}));
        input.pop();
        assert!(serve("codex", &input[..], Vec::new()).is_err());
    }
    #[test]
    fn decoder_is_bound_to_one_provider_and_attempt() {
        let mut state = State {
            harness: "codex",
            decoder: None,
        };
        assert!(
            state
                .call(
                    Operation::DecodeEvent,
                    json!({"stream":"stdout","line":"{}"})
                )
                .is_err()
        );
        assert!(
            state
                .call(Operation::DecodeStart, json!({"harness":"claude-code"}))
                .is_err()
        );
        assert!(
            state
                .call(Operation::DecodeStart, json!({"harness":"codex"}))
                .is_ok()
        );
        assert!(
            state
                .call(Operation::DecodeStart, json!({"harness":"codex"}))
                .is_err()
        );
        assert!(
            state
                .call(
                    Operation::DecodeEvent,
                    json!({"stream":"other","line":"{}"})
                )
                .is_err()
        );
    }
    #[test]
    fn wire_failure_contains_no_native_input() {
        let secret = "PRIVATE_NATIVE_PAYLOAD";
        let input = frame(1, "decode_event", json!({"stream":"stdout","line":secret}));
        let mut output = Vec::new();
        serve("codex", &input[..], &mut output).unwrap();
        assert!(!String::from_utf8(output.clone()).unwrap().contains(secret));
        let response: Response = serde_json::from_slice(&output).unwrap();
        assert!(matches!(response, Response::Failure(_)));
    }
}
