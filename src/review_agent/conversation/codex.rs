use std::path::PathBuf;

use serde_json::Value;

pub(super) fn transcript_cwd(bytes: &[u8]) -> Option<PathBuf> {
    super::read_complete_records(bytes).find_map(|record| {
        let value: Value = serde_json::from_slice(record).ok()?;
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            return None;
        }
        value
            .get("payload")
            .and_then(|payload| payload.get("cwd"))
            .and_then(Value::as_str)
            .map(PathBuf::from)
    })
}

pub(super) fn session_id(bytes: &[u8]) -> Option<String> {
    super::read_complete_records(bytes).find_map(|record| {
        let value: Value = serde_json::from_slice(record).ok()?;
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            return None;
        }
        value
            .get("payload")
            .and_then(|payload| payload.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

pub(super) fn is_completion_record(value: &Value) -> bool {
    if value.get("type").and_then(Value::as_str) != Some("event_msg") {
        return false;
    }
    matches!(
        value
            .get("payload")
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str),
        Some("task_complete" | "turn_complete")
    )
}

pub(super) fn is_top_level_user_record(value: &Value) -> bool {
    match value.get("type").and_then(Value::as_str) {
        Some("event_msg") => {
            value
                .get("payload")
                .and_then(|payload| payload.get("type"))
                .and_then(Value::as_str)
                == Some("user_message")
        }
        Some("response_item") => {
            value
                .get("payload")
                .and_then(|payload| payload.get("role"))
                .and_then(Value::as_str)
                == Some("user")
        }
        _ => false,
    }
}
