use std::fs;
use std::time::SystemTime;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{ConversationProvider, TranscriptBinding, TranscriptCheckpoint};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileObservation {
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
}

impl FileObservation {
    pub(crate) fn is_stable_with(self, next: Self) -> bool {
        self == next
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CompletionReadiness {
    Ready { completed: TranscriptCheckpoint },
    BoundaryPending,
    PartialFinalLine,
    SessionChanged,
    Unreadable,
}

pub(super) fn read_completion_after(binding: &TranscriptBinding) -> CompletionReadiness {
    let Ok(bytes) = fs::read(&binding.absolute_path) else {
        return CompletionReadiness::Unreadable;
    };
    if transcript_identity(&bytes, binding.checkpoint.byte_offset)
        != Some(binding.checkpoint.identity)
    {
        return CompletionReadiness::SessionChanged;
    }
    completion_after(binding.provider, &bytes, &binding.checkpoint)
}

pub(crate) fn completion_after(
    provider: ConversationProvider,
    bytes: &[u8],
    checkpoint: &TranscriptCheckpoint,
) -> CompletionReadiness {
    let Ok(start) = usize::try_from(checkpoint.byte_offset) else {
        return CompletionReadiness::SessionChanged;
    };
    if start > bytes.len() {
        return CompletionReadiness::SessionChanged;
    }
    if bytes
        .get(start..)
        .is_some_and(|tail| !tail.is_empty() && tail.last() != Some(&b'\n'))
    {
        return CompletionReadiness::PartialFinalLine;
    }

    let Some(relative_end) = first_completion_end(provider, &bytes[start..]) else {
        return CompletionReadiness::BoundaryPending;
    };
    CompletionReadiness::Ready {
        completed: TranscriptCheckpoint {
            byte_offset: checkpoint.byte_offset + relative_end as u64,
            identity: checkpoint.identity,
        },
    }
}

pub(crate) fn last_completion_checkpoint(provider: ConversationProvider, bytes: &[u8]) -> u64 {
    let mut base = 0usize;
    let mut last = 0usize;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.last() != Some(&b'\n') {
            break;
        }
        base += line.len();
        let record = &line[..line.len() - 1];
        let Ok(value) = serde_json::from_slice::<Value>(record) else {
            continue;
        };
        if is_completion_record(provider, &value) {
            last = base;
        }
    }
    last as u64
}

/// Return the completion boundary immediately before the latest provider user
/// record. Resolution runs asynchronously, so the current turn's completion
/// may already be present by the time the transcript is inspected. Anchoring
/// at the last completion would otherwise skip that turn entirely.
pub(super) fn checkpoint_before_latest_user(
    provider: ConversationProvider,
    bytes: &[u8],
) -> Option<u64> {
    let mut base = 0usize;
    let mut previous_completion = 0usize;
    let mut checkpoint = None;

    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.last() != Some(&b'\n') {
            break;
        }
        base += line.len();
        let record = &line[..line.len() - 1];
        let Ok(value) = serde_json::from_slice::<Value>(record) else {
            continue;
        };
        if is_top_level_user_record(provider, &value) {
            checkpoint = Some(previous_completion as u64);
        }
        if is_completion_record(provider, &value) {
            previous_completion = base;
        }
    }

    checkpoint
}

pub(super) fn transcript_identity(bytes: &[u8], checkpoint: u64) -> Option<[u8; 32]> {
    let first_complete_end = bytes.iter().position(|byte| *byte == b'\n')? + 1;
    let checkpoint = usize::try_from(checkpoint).ok()?;
    let identity_end = checkpoint.max(first_complete_end);
    let prefix = bytes.get(..identity_end)?;
    serde_json::from_slice::<Value>(&prefix[..first_complete_end - 1]).ok()?;
    Some(Sha256::digest(prefix).into())
}

fn first_completion_end(provider: ConversationProvider, bytes: &[u8]) -> Option<usize> {
    let mut offset = 0usize;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.last() != Some(&b'\n') {
            return None;
        }
        offset += line.len();
        let record = &line[..line.len() - 1];
        let Ok(value) = serde_json::from_slice::<Value>(record) else {
            continue;
        };
        if is_completion_record(provider, &value) {
            return Some(offset);
        }
    }
    None
}

fn is_completion_record(provider: ConversationProvider, value: &Value) -> bool {
    match provider {
        ConversationProvider::Claude => super::claude::is_completion_record(value),
        ConversationProvider::Codex => super::codex::is_completion_record(value),
    }
}

fn is_top_level_user_record(provider: ConversationProvider, value: &Value) -> bool {
    match provider {
        ConversationProvider::Claude => super::claude::is_top_level_user_record(value),
        ConversationProvider::Codex => super::codex::is_top_level_user_record(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint(bytes: &[u8], offset: u64) -> TranscriptCheckpoint {
        TranscriptCheckpoint {
            byte_offset: offset,
            identity: transcript_identity(bytes, offset).expect("identity"),
        }
    }

    #[test]
    fn codex_requires_persisted_completion_boundary() {
        let prefix = b"{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/work\"}}\n";
        let mut bytes = prefix.to_vec();
        bytes.extend_from_slice(b"{\"type\":\"response_item\",\"payload\":{}}\n");
        let checkpoint = checkpoint(&bytes, prefix.len() as u64);
        assert_eq!(
            completion_after(ConversationProvider::Codex, &bytes, &checkpoint),
            CompletionReadiness::BoundaryPending
        );
        bytes.extend_from_slice(
            b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n",
        );
        assert!(matches!(
            completion_after(ConversationProvider::Codex, &bytes, &checkpoint),
            CompletionReadiness::Ready { .. }
        ));
    }

    #[test]
    fn partial_final_record_is_not_ready() {
        let prefix = b"{\"type\":\"user\",\"cwd\":\"/work\"}\n";
        let mut bytes = prefix.to_vec();
        bytes.extend_from_slice(
            b"{\"type\":\"assistant\",\"message\":{\"stop_reason\":\"end_turn\"}}",
        );
        let checkpoint = checkpoint(&bytes, prefix.len() as u64);
        assert_eq!(
            completion_after(ConversationProvider::Claude, &bytes, &checkpoint),
            CompletionReadiness::PartialFinalLine
        );
    }

    #[test]
    fn unknown_records_are_ignored_before_boundary() {
        let prefix = b"{\"type\":\"user\",\"cwd\":\"/work\"}\n";
        let mut bytes = prefix.to_vec();
        bytes.extend_from_slice(b"{\"type\":\"future_record\",\"payload\":{}}\n");
        bytes.extend_from_slice(
            b"{\"type\":\"assistant\",\"message\":{\"stop_reason\":\"end_turn\"}}\n",
        );
        let checkpoint = checkpoint(&bytes, prefix.len() as u64);
        assert!(matches!(
            completion_after(ConversationProvider::Claude, &bytes, &checkpoint),
            CompletionReadiness::Ready { .. }
        ));
    }

    #[test]
    fn claude_sidechain_completion_is_not_a_front_turn_boundary() {
        let prefix = b"{\"type\":\"user\",\"cwd\":\"/work\"}\n";
        let mut bytes = prefix.to_vec();
        bytes.extend_from_slice(
            b"{\"type\":\"assistant\",\"isSidechain\":true,\"message\":{\"stop_reason\":\"end_turn\"}}\n",
        );
        let checkpoint = checkpoint(&bytes, prefix.len() as u64);
        assert_eq!(
            completion_after(ConversationProvider::Claude, &bytes, &checkpoint),
            CompletionReadiness::BoundaryPending
        );
    }

    #[test]
    fn claude_sidechain_user_does_not_replace_latest_front_user_boundary() {
        let previous = concat!(
            "{\"type\":\"user\",\"cwd\":\"/work\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"stop_reason\":\"end_turn\"}}\n"
        );
        let current = concat!(
            "{\"type\":\"user\",\"cwd\":\"/work\"}\n",
            "{\"type\":\"user\",\"cwd\":\"/work\",\"isSidechain\":true}\n",
            "{\"type\":\"assistant\",\"isSidechain\":true,\"message\":{\"stop_reason\":\"end_turn\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"stop_reason\":\"end_turn\"}}\n"
        );
        let bytes = format!("{previous}{current}");

        assert_eq!(
            checkpoint_before_latest_user(ConversationProvider::Claude, bytes.as_bytes()),
            Some(previous.len() as u64)
        );
    }

    #[test]
    fn file_replacement_is_detected_by_identity() {
        let original = b"{\"type\":\"user\",\"cwd\":\"/one\"}\n";
        let replacement = b"{\"type\":\"user\",\"cwd\":\"/two\"}\n";
        let checkpoint = checkpoint(original, 0);
        assert_ne!(
            transcript_identity(replacement, checkpoint.byte_offset),
            Some(checkpoint.identity)
        );
    }

    #[test]
    fn stability_comparison_has_no_clock_or_sleep_dependency() {
        let observation = FileObservation {
            len: 10,
            modified: Some(SystemTime::UNIX_EPOCH),
        };
        assert!(observation.is_stable_with(observation));
        assert!(!observation.is_stable_with(FileObservation {
            len: 11,
            modified: observation.modified,
        }));
    }
}
