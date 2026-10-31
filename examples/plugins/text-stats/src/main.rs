//! Independent one-shot plugin: no ahu implementation dependency.
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{Read, Write};

const MAX_INPUT: u64 = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    protocol_version: u32,
    plugin_id: String,
    grant: Grant,
    context: Context,
    input: Input,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    operation: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    invocation_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    text: String,
}

fn response(bytes: &[u8]) -> Value {
    let invalid = || json!({"protocol_version":1,"status":"error","code":"invalid_input"});
    if bytes.len() as u64 > MAX_INPUT {
        return invalid();
    }
    let Ok(request) = serde_json::from_slice::<Request>(bytes) else {
        return invalid();
    };
    if request.protocol_version != 1
        || request.plugin_id != "text-stats"
        || request.grant.operation != "invoke"
        || request.context.invocation_id.is_empty()
        || request.context.invocation_id.len() > 64
        || !request.context.invocation_id.is_ascii()
    {
        return invalid();
    }
    let text = request.input.text;
    // Preserve the original example's whitespace behavior, including the four
    // information separators recognized by Python but not Unicode White_Space.
    let whitespace = |c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c);
    json!({"protocol_version":1,"status":"ok","result":{
        "characters":text.chars().count(),
        "words":text.split(whitespace).filter(|word| !word.is_empty()).count()
    }})
}

fn main() {
    let result = if std::env::args().skip(1).collect::<Vec<_>>() != ["--ahu-plugin-protocol=1"] {
        json!({"protocol_version":1,"status":"error","code":"unsupported"})
    } else {
        let mut input = Vec::new();
        match std::io::stdin().take(MAX_INPUT + 1).read_to_end(&mut input) {
            Ok(_) => response(&input),
            Err(_) => json!({"protocol_version":1,"status":"error","code":"invalid_input"}),
        }
    };
    let mut output = std::io::stdout().lock();
    if serde_json::to_writer(&mut output, &result).is_err() || output.flush().is_err() {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(text: &str) -> Value {
        json!({"protocol_version":1,"plugin_id":"text-stats","grant":{"operation":"invoke"},
            "context":{"invocation_id":"test"},"input":{"text":text}})
    }

    #[test]
    fn unicode_and_empty_input() {
        for (text, characters, words) in [("", 0, 0), ("café\n🦀", 6, 2), ("a\u{1c}b", 3, 2)] {
            assert_eq!(
                response(request(text).to_string().as_bytes())["result"],
                json!({"characters":characters,"words":words})
            );
        }
    }

    #[test]
    fn rejects_duplicate_unknown_and_oversized_input() {
        let good = request("x").to_string();
        let duplicate = good.replace("\"text\":\"x\"", "\"text\":\"x\",\"text\":\"y\"");
        let mut unknown = request("x");
        unknown["input"]["extra"] = json!(true);
        for bad in [
            duplicate.into_bytes(),
            unknown.to_string().into_bytes(),
            vec![b' '; MAX_INPUT as usize + 1],
        ] {
            assert_eq!(response(&bad)["code"], "invalid_input");
        }
    }
}
