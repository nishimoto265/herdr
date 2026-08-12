use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::shitsuji_agent::ShitsujiAgentState;

const STORE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct ShitsujiAgentStore {
    version: u32,
    state: ShitsujiAgentState,
}

fn store_path() -> PathBuf {
    crate::session::data_dir().join("shitsuji-agent.json")
}

/// Pre-rename store name. Sessions written before the Shitsuji Agent rename
/// still carry this file, so it is read once and rewritten under the new name.
fn legacy_store_path() -> PathBuf {
    crate::session::data_dir().join("review-agent.json") // pre-rename
}

pub(crate) fn save(state: &ShitsujiAgentState) -> std::io::Result<()> {
    save_to_path(&store_path(), state)
}

pub(crate) fn save_to_path(path: &Path, state: &ShitsujiAgentState) -> std::io::Result<()> {
    super::io::save_replace_json_with_backup_to_path(
        path,
        &ShitsujiAgentStore {
            version: STORE_VERSION,
            state: state.clone(),
        },
    )
}

pub(crate) fn load() -> ShitsujiAgentState {
    load_with_legacy_migration(&store_path(), &legacy_store_path())
}

fn load_with_legacy_migration(path: &Path, legacy_path: &Path) -> ShitsujiAgentState {
    if store_present(path) || !store_present(legacy_path) {
        return load_from_path(path);
    }
    // Only a legacy store that actually parsed is migrated. Writing the default
    // for an unreadable one would make the new path present from then on, so the
    // legacy store would never be retried.
    let Some(state) = stored_state(legacy_path) else {
        return ShitsujiAgentState::default();
    };
    match save_to_path(path, &state) {
        Ok(()) => info!(
            path = %path.display(),
            legacy_path = %legacy_path.display(),
            "migrated the pre-rename agent store to the shitsuji agent store"
        ),
        Err(err) => warn!(
            path = %path.display(),
            legacy_path = %legacy_path.display(),
            err = %err,
            "failed to write the migrated shitsuji agent store"
        ),
    }
    state
}

/// A store counts as present even when only its replace backup survives, so a
/// crash between rename steps does not look like a missing store and trigger a
/// migration that would overwrite it with older legacy data.
fn store_present(path: &Path) -> bool {
    path.exists() || super::io::replace_backup_path(path).exists()
}

pub(crate) fn load_from_path(path: &Path) -> ShitsujiAgentState {
    stored_state(path).unwrap_or_default()
}

/// `None` when the store is missing, unreadable, unparsable, or written by an
/// unsupported version.
fn stored_state(path: &Path) -> Option<ShitsujiAgentState> {
    let content = match read_store_with_backup_recovery(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            warn!(path = %path.display(), err = %err, "failed to read shitsuji agent store");
            return None;
        }
    };
    let store = match serde_json::from_str::<ShitsujiAgentStore>(&content) {
        Ok(store) => store,
        Err(err) => {
            warn!(path = %path.display(), err = %err, "failed to parse shitsuji agent store");
            return None;
        }
    };
    if store.version != STORE_VERSION {
        warn!(
            path = %path.display(),
            version = store.version,
            supported_version = STORE_VERSION,
            "unsupported shitsuji agent store version"
        );
        return None;
    }
    Some(store.state)
}

fn read_store_with_backup_recovery(path: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let backup_path = super::io::replace_backup_path(path);
            let content = std::fs::read_to_string(&backup_path)?;
            match std::fs::rename(&backup_path, path) {
                Ok(()) => {
                    warn!(path = %path.display(), "restored shitsuji agent store from backup");
                }
                Err(restore_error) => {
                    warn!(
                        path = %path.display(),
                        backup = %backup_path.display(),
                        err = %restore_error,
                        "loaded shitsuji agent backup but could not restore its primary path"
                    );
                }
            }
            Ok(content)
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shitsuji_agent::{
        RuleProposalDecision, RuleProposalDecisionRequest, RuleProposalSubmitInput,
        ShitsujiBackendProfileId,
    };

    fn temp_store_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "herdr-shitsuji-agent-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn temp_store_path(name: &str) -> PathBuf {
        temp_store_dir(name).join("shitsuji-agent.json")
    }

    fn input(event: &str) -> RuleProposalSubmitInput {
        RuleProposalSubmitInput {
            rule_text: "Check affected callers.".into(),
            target_profile_id: ShitsujiBackendProfileId::new("shitsuji-agent"),
            fingerprint: "check-callers".into(),
            source_event_id: event.into(),
        }
    }

    #[test]
    fn roundtrip_preserves_proposals_and_active_rules() {
        let path = temp_store_path("roundtrip");
        let mut state = ShitsujiAgentState::default();
        state.submit(input("event-1")).unwrap();
        let proposal = state
            .submit(input("event-2"))
            .unwrap()
            .submission
            .proposal
            .unwrap();
        state
            .decide(RuleProposalDecisionRequest {
                proposal_id: proposal.proposal_id,
                expected_revision: proposal.revision,
                decision: RuleProposalDecision::Approve,
            })
            .unwrap();

        save_to_path(&path, &state).unwrap();
        let loaded = load_from_path(&path);

        assert_eq!(loaded, state);
        assert_eq!(loaded.active_rules().count(), 1);
    }

    #[test]
    fn corrupt_and_old_versions_start_empty() {
        let corrupt_path = temp_store_path("corrupt");
        std::fs::create_dir_all(corrupt_path.parent().unwrap()).unwrap();
        std::fs::write(&corrupt_path, "not json").unwrap();
        assert_eq!(load_from_path(&corrupt_path), ShitsujiAgentState::default());

        let old_path = temp_store_path("old");
        std::fs::create_dir_all(old_path.parent().unwrap()).unwrap();
        std::fs::write(
            &old_path,
            serde_json::json!({
                "version": 0,
                "state": ShitsujiAgentState::default(),
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(load_from_path(&old_path), ShitsujiAgentState::default());
    }

    #[test]
    fn two_updates_replace_the_atomic_store() {
        let path = temp_store_path("updates");
        let mut state = ShitsujiAgentState::default();
        state.submit(input("event-1")).unwrap();
        save_to_path(&path, &state).unwrap();
        state.submit(input("event-2")).unwrap();
        save_to_path(&path, &state).unwrap();

        let loaded = load_from_path(&path);
        assert_eq!(loaded.proposals().count(), 1);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn restart_keeps_submission_idempotent() {
        let path = temp_store_path("restart");
        let mut state = ShitsujiAgentState::default();
        state.submit(input("event-1")).unwrap();
        save_to_path(&path, &state).unwrap();

        let mut loaded = load_from_path(&path);
        let duplicate = loaded.submit(input("event-1")).unwrap();

        assert!(!duplicate.changed);
        assert_eq!(loaded, state);
    }

    /// Captured from a session written before the rename, with the operator
    /// chosen profile id replaced. Pins the on-disk field names and nesting so
    /// the type renames cannot silently change the serde representation.
    const LEGACY_STORE_FIXTURE: &str = r#"{
  "version": 1,
  "state": {
    "proposals": {},
    "proposal_by_fingerprint": {},
    "evidence_by_fingerprint": {
      "shitsuji-agent": {
        "exact-reply-visible-verbatim": {
          "rule_text": "When the user requests an exact reply, emit the requested text verbatim as the entire visible final response.",
          "target_profile_id": "shitsuji-agent",
          "source_event_ids": [
            "front-1-generation-1-offset-36615"
          ]
        }
      }
    },
    "active_rules": {},
    "next_proposal_sequence": 1
  }
}"#;

    fn write_store(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn legacy_fixture_with_event(event: &str) -> String {
        LEGACY_STORE_FIXTURE.replace("front-1-generation-1-offset-36615", event)
    }

    fn source_event_ids(state: &ShitsujiAgentState) -> Vec<String> {
        let store = serde_json::to_value(ShitsujiAgentStore {
            version: STORE_VERSION,
            state: state.clone(),
        })
        .unwrap();
        store["state"]["evidence_by_fingerprint"]["shitsuji-agent"]["exact-reply-visible-verbatim"]
            ["source_event_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn legacy_store_fixture_loads_without_representation_drift() {
        let path = temp_store_dir("fixture").join("fixture.json");
        write_store(&path, LEGACY_STORE_FIXTURE);

        let state = load_from_path(&path);

        let reserialized = serde_json::to_string_pretty(&ShitsujiAgentStore {
            version: STORE_VERSION,
            state,
        })
        .unwrap();
        assert_eq!(reserialized, LEGACY_STORE_FIXTURE);
    }

    #[test]
    fn legacy_store_is_migrated_when_only_the_legacy_file_exists() {
        let dir = temp_store_dir("migrate-legacy-only");
        let path = dir.join("shitsuji-agent.json");
        let legacy_path = dir.join("review-agent.json"); // pre-rename
        write_store(&legacy_path, &legacy_fixture_with_event("legacy-event"));

        let state = load_with_legacy_migration(&path, &legacy_path);

        assert_eq!(source_event_ids(&state), vec!["legacy-event".to_string()]);
        assert!(path.exists(), "the legacy store should be rewritten");
        assert!(legacy_path.exists(), "the legacy store should be kept");
        assert_eq!(load_from_path(&path), state);
    }

    #[test]
    fn new_store_is_used_when_only_the_new_file_exists() {
        let dir = temp_store_dir("migrate-new-only");
        let path = dir.join("shitsuji-agent.json");
        let legacy_path = dir.join("review-agent.json"); // pre-rename
        write_store(&path, &legacy_fixture_with_event("new-event"));

        let state = load_with_legacy_migration(&path, &legacy_path);

        assert_eq!(source_event_ids(&state), vec!["new-event".to_string()]);
        assert!(!legacy_path.exists(), "no legacy store should be created");
    }

    #[test]
    fn new_store_wins_when_both_files_exist() {
        let dir = temp_store_dir("migrate-both");
        let path = dir.join("shitsuji-agent.json");
        let legacy_path = dir.join("review-agent.json"); // pre-rename
        write_store(&path, &legacy_fixture_with_event("new-event"));
        write_store(&legacy_path, &legacy_fixture_with_event("legacy-event"));

        let state = load_with_legacy_migration(&path, &legacy_path);

        assert_eq!(source_event_ids(&state), vec!["new-event".to_string()]);
        assert_eq!(
            std::fs::read_to_string(&legacy_path).unwrap(),
            legacy_fixture_with_event("legacy-event"),
            "the legacy store should be left untouched"
        );
    }

    #[test]
    fn an_unreadable_legacy_store_is_not_migrated() {
        let dir = temp_store_dir("migrate-corrupt");
        let path = dir.join("shitsuji-agent.json");
        let legacy_path = dir.join("review-agent.json"); // pre-rename
        write_store(&legacy_path, "not json");

        let state = load_with_legacy_migration(&path, &legacy_path);

        assert_eq!(state, ShitsujiAgentState::default());
        assert!(
            !path.exists(),
            "an unreadable legacy store must not be migrated, or it would never be retried"
        );
        assert!(legacy_path.exists());
    }

    #[test]
    fn a_surviving_backup_of_the_new_store_blocks_legacy_migration() {
        let dir = temp_store_dir("migrate-backup");
        let path = dir.join("shitsuji-agent.json");
        let legacy_path = dir.join("review-agent.json"); // pre-rename
        write_store(&path, &legacy_fixture_with_event("new-event"));
        std::fs::rename(&path, super::super::io::replace_backup_path(&path)).unwrap();
        write_store(&legacy_path, &legacy_fixture_with_event("legacy-event"));

        let state = load_with_legacy_migration(&path, &legacy_path);

        assert_eq!(source_event_ids(&state), vec!["new-event".to_string()]);
    }

    #[test]
    fn missing_primary_recovers_the_last_backup() {
        let path = temp_store_path("backup-recovery");
        let mut state = ShitsujiAgentState::default();
        state.submit(input("event-1")).unwrap();
        save_to_path(&path, &state).unwrap();
        let backup_path = super::super::io::replace_backup_path(&path);
        std::fs::rename(&path, &backup_path).unwrap();

        let loaded = load_from_path(&path);

        assert_eq!(loaded, state);
        assert!(path.exists());
        assert!(!backup_path.exists());
    }
}
