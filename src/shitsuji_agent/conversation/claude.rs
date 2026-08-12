use std::path::PathBuf;

use serde_json::Value;

pub(super) fn transcript_cwd(bytes: &[u8]) -> Option<PathBuf> {
    super::read_complete_records(bytes).find_map(|record| {
        let value: Value = serde_json::from_slice(record).ok()?;
        value.get("cwd").and_then(Value::as_str).map(PathBuf::from)
    })
}

pub(super) fn session_id(bytes: &[u8]) -> Option<String> {
    super::read_complete_records(bytes).find_map(|record| {
        let value: Value = serde_json::from_slice(record).ok()?;
        value
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

pub(super) fn is_completion_record(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("assistant")
        && value.get("isSidechain").and_then(Value::as_bool) != Some(true)
        && value
            .get("message")
            .and_then(|message| message.get("stop_reason"))
            .and_then(Value::as_str)
            == Some("end_turn")
}

pub(super) fn is_top_level_user_record(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("user")
        && value.get("isSidechain").and_then(Value::as_bool) != Some(true)
}

/// A Claude subagent transcript identifies its conversational records as
/// sidechain records. Main transcripts can still contain unrelated metadata,
/// so only classify a candidate after seeing a conversational record.
pub(super) fn is_sidechain_transcript(bytes: &[u8]) -> bool {
    let mut saw_conversation = false;
    let mut saw_top_level = false;

    for record in super::read_complete_records(bytes) {
        let Ok(value) = serde_json::from_slice::<Value>(record) else {
            continue;
        };
        if !matches!(
            value.get("type").and_then(Value::as_str),
            Some("user" | "assistant")
        ) {
            continue;
        }
        saw_conversation = true;
        if value.get("isSidechain").and_then(Value::as_bool) != Some(true) {
            saw_top_level = true;
            break;
        }
    }

    saw_conversation && !saw_top_level
}
