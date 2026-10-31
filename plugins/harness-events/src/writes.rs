use serde_json::Value;
const MAX_EVENT_JSON_DEPTH: usize = 64;
const WRITE_LIKE_TOOLS: [&str; 10] = [
    "write",
    "edit",
    "multiedit",
    "notebookedit",
    "applypatch",
    "editfile",
    "writefile",
    "createfile",
    "strreplaceeditor",
    "strreplacebasededit",
];
const WRITE_PATH_KEYS: [&str; 6] = [
    "filepath",
    "file",
    "path",
    "notebookpath",
    "abspath",
    "targetfile",
];
const WRITE_NAME_KEYS: [&str; 3] = ["tool", "name", "toolname"];
const WRITE_INPUT_KEYS: [&str; 2] = ["input", "arguments"];

/// Lowercase ASCII alphanumerics only: `file_path` and `filePath` both
/// normalize to `filepath`, `_` separators vanish.
fn normalize_token(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn collect_write_paths_within(value: &Value, depth: usize, candidates: &mut Vec<String>) {
    if depth >= MAX_EVENT_JSON_DEPTH {
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter() {
                let key = normalize_token(key);
                if WRITE_PATH_KEYS.contains(&key.as_str())
                    && let Some(path) = child.as_str()
                {
                    candidates.push(path.to_string());
                }
                if WRITE_INPUT_KEYS.contains(&key.as_str())
                    && let Some(embedded) = child.as_str()
                    && let Ok(decoded) = serde_json::from_str::<Value>(embedded)
                {
                    collect_write_paths_within(&decoded, depth + 1, candidates);
                }
                collect_write_paths_within(child, depth + 1, candidates);
            }
        }
        Value::Array(items) => {
            for child in items.iter() {
                collect_write_paths_within(child, depth + 1, candidates);
            }
        }
        _ => (),
    }
}

/// Walk one event, collecting write-path candidates from any object whose
/// name-ish key names a write-like tool. The main walk does not decode
/// embedded JSON in string `input`/`arguments` values; the within-walk
/// triggered by a matching tool name does.
pub(crate) fn collect_write_paths(event: &Value) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut stack: Vec<(&Value, usize)> = vec![(event, 0)];
    while let Some((value, depth)) = stack.pop() {
        if depth >= MAX_EVENT_JSON_DEPTH {
            continue;
        }
        match value {
            Value::Object(map) => {
                let is_write_tool = map.iter().any(|(key, child)| {
                    WRITE_NAME_KEYS.contains(&normalize_token(key).as_str())
                        && child
                            .as_str()
                            .map(normalize_token)
                            .is_some_and(|tool| WRITE_LIKE_TOOLS.contains(&tool.as_str()))
                });
                if is_write_tool {
                    collect_write_paths_within(value, depth, &mut candidates);
                }
                for (_, child) in map.iter() {
                    stack.push((child, depth + 1));
                }
            }
            Value::Array(items) => {
                for child in items.iter() {
                    stack.push((child, depth + 1));
                }
            }
            _ => (),
        }
    }
    candidates
}
