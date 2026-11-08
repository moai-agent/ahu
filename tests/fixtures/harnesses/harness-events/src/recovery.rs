const EXPORT_MESSAGE_LIMIT: usize = 4096;
pub(crate) fn valid_session_id(session: &str) -> bool {
    session.len() >= 8
        && session.len() <= 128
        && session.starts_with("ses_")
        && session[4..].chars().all(|c| c.is_ascii_alphanumeric())
}

/// Decide whether a sanitized `opencode session export` establishes that
/// `session` completed the one turn this attempt started.
///
/// Statically verified against the 2.0.32 binary: `session export` prints
/// `{"info": Session.Info, "messages": [Session.Message.Info]}`, the export
/// drops assistant messages that have no `time.completed`, the projection
/// appends an `idle` message carrying the execution outcome, and sanitization
/// rewrites only text-bearing fields — `type`, `finish`, `outcome`, `status`
/// and `time` survive it intact. Nothing here reads a transcript field.
///
/// Every unexpected shape is a failure. In particular this refuses any
/// recorded tool call that is not `completed`: a refusal and a recovered tool
/// error are the same `error` status in the projection, and ahu will not tell
/// them apart from redacted text. A run that recovered from a tool error is
/// therefore not rescued here; it is reported as missing completion evidence,
/// which is what it was before this check existed.
pub fn export_establishes_completion(
    export: &str,
    session: &str,
    attempt_started_ms: u64,
) -> std::result::Result<(), &'static str> {
    use serde_json::Value;
    if export.len() > 1024 * 1024 {
        return Err("session export exceeds byte inspection bound");
    }

    if !valid_session_id(session) {
        return Err("the attempt's session identifier is not an OpenCode session id");
    }
    let value: Value = serde_json::from_str(export).map_err(|_| "the export is not JSON")?;
    let info = value
        .get("info")
        .and_then(Value::as_object)
        .ok_or("the export carries no session info")?;
    if info.get("id").and_then(Value::as_str) != Some(session) {
        return Err("the export describes a different session");
    }
    if info.get("outcome").and_then(Value::as_str) != Some("succeeded") {
        return Err("the recorded session did not end with a succeeded outcome");
    }
    let time = info
        .get("time")
        .and_then(Value::as_object)
        .ok_or("the recorded session carries no timestamps")?;
    if !time.get("idle").is_some_and(Value::is_number) {
        return Err("the recorded session is not idle");
    }
    // The projection timestamps are epoch milliseconds. A session created
    // before this attempt spawned is a different run's session, whatever its
    // identifier says.
    let created = time
        .get("created")
        .and_then(Value::as_u64)
        .ok_or("the recorded session has no usable creation time")?;
    if created < attempt_started_ms {
        return Err("the recorded session predates this attempt");
    }
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("the export carries no messages")?;
    if messages.is_empty() || messages.len() > EXPORT_MESSAGE_LIMIT {
        return Err("the exported message count is empty or past the inspection bound");
    }

    let mut prompts = 0usize;
    let mut last_assistant = None;
    for (index, message) in messages.iter().enumerate() {
        let message = message
            .as_object()
            .ok_or("an exported message is not an object")?;
        match message
            .get("type")
            .and_then(Value::as_str)
            .ok_or("an exported message has no type")?
        {
            "user" => prompts += 1,
            "assistant" => {
                if message.get("error").is_some_and(|error| !error.is_null()) {
                    return Err("an assistant message recorded an error");
                }
                if !message
                    .get("time")
                    .and_then(|time| time.get("completed"))
                    .is_some_and(Value::is_number)
                {
                    return Err("an assistant message never completed");
                }
                for part in message
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or("an assistant message has no content")?
                {
                    if part.get("type").and_then(Value::as_str) == Some("tool")
                        && part.pointer("/state/status").and_then(Value::as_str)
                            != Some("completed")
                    {
                        return Err("a recorded tool call did not complete");
                    }
                }
                last_assistant = Some(index);
            }
            "idle" => {
                if index + 1 != messages.len() {
                    return Err("the recorded session went idle before its last message");
                }
                if message.get("outcome").and_then(Value::as_str) != Some("succeeded") {
                    return Err("the final idle message did not record success");
                }
                if message.get("error").is_some_and(|error| !error.is_null()) {
                    return Err("the final idle message recorded an error");
                }
            }
            // Turn bookkeeping that carries no completion claim. Everything
            // else — a shell message, a compaction, a type added by a later
            // OpenCode — leaves this check unable to describe the run.
            "system" | "synthetic" | "skill" | "agent-switched" | "model-switched"
            | "location-switched" | "compaction" => (),
            _ => return Err("the export holds a message type this check does not recognize"),
        }
    }
    // ahu passes no --session on the launch this fallback covers, so the
    // session holds exactly one prompt: its own. More than one would mean the
    // stop evidence could belong to an earlier turn.
    if prompts != 1 {
        return Err("the exported session does not hold exactly this attempt's one turn");
    }
    let last_assistant = last_assistant.ok_or("the exported session has no assistant message")?;
    // The execution outcome lives in the idle message the projection appends,
    // so a session that never recorded one never recorded an outcome.
    if messages[messages.len() - 1]
        .get("type")
        .and_then(Value::as_str)
        != Some("idle")
    {
        return Err("the exported session does not end with an idle message");
    }
    // That idle message is last, so the final assistant message must be the
    // one immediately before it: anything in between is work the stop reason
    // does not describe.
    if last_assistant + 2 != messages.len() {
        return Err("the exported session records work after its final assistant message");
    }
    if messages[last_assistant]
        .get("finish")
        .and_then(Value::as_str)
        != Some("stop")
    {
        return Err("the final assistant message did not finish with stop");
    }
    Ok(())
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
