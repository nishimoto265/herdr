use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::review_agent::ReviewAgentState;

const STORE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct ReviewAgentStore {
    version: u32,
    state: ReviewAgentState,
}

fn store_path() -> PathBuf {
    crate::session::data_dir().join("review-agent.json")
}

pub(crate) fn save(state: &ReviewAgentState) -> std::io::Result<()> {
    save_to_path(&store_path(), state)
}

pub(crate) fn save_to_path(path: &Path, state: &ReviewAgentState) -> std::io::Result<()> {
    super::io::save_replace_json_with_backup_to_path(
        path,
        &ReviewAgentStore {
            version: STORE_VERSION,
            state: state.clone(),
        },
    )
}

pub(crate) fn load() -> ReviewAgentState {
    load_from_path(&store_path())
}

pub(crate) fn load_from_path(path: &Path) -> ReviewAgentState {
    let content = match read_store_with_backup_recovery(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return ReviewAgentState::default();
        }
        Err(err) => {
            warn!(path = %path.display(), err = %err, "failed to read review agent store");
            return ReviewAgentState::default();
        }
    };
    let store = match serde_json::from_str::<ReviewAgentStore>(&content) {
        Ok(store) => store,
        Err(err) => {
            warn!(path = %path.display(), err = %err, "failed to parse review agent store");
            return ReviewAgentState::default();
        }
    };
    if store.version != STORE_VERSION {
        warn!(
            path = %path.display(),
            version = store.version,
            supported_version = STORE_VERSION,
            "unsupported review agent store version"
        );
        return ReviewAgentState::default();
    }
    store.state
}

fn read_store_with_backup_recovery(path: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let backup_path = super::io::replace_backup_path(path);
            let content = std::fs::read_to_string(&backup_path)?;
            match std::fs::rename(&backup_path, path) {
                Ok(()) => {
                    warn!(path = %path.display(), "restored review agent store from backup");
                }
                Err(restore_error) => {
                    warn!(
                        path = %path.display(),
                        backup = %backup_path.display(),
                        err = %restore_error,
                        "loaded review agent backup but could not restore its primary path"
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
    use crate::review_agent::{
        ReviewBackendProfileId, RuleProposalDecision, RuleProposalDecisionRequest,
        RuleProposalSubmitInput,
    };

    fn temp_store_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir()
            .join(format!(
                "herdr-review-agent-{name}-{}-{nanos}",
                std::process::id()
            ))
            .join("review-agent.json")
    }

    fn input(event: &str) -> RuleProposalSubmitInput {
        RuleProposalSubmitInput {
            rule_text: "Check affected callers.".into(),
            target_profile_id: ReviewBackendProfileId::new("review-agent"),
            fingerprint: "check-callers".into(),
            source_event_id: event.into(),
        }
    }

    #[test]
    fn roundtrip_preserves_proposals_and_active_rules() {
        let path = temp_store_path("roundtrip");
        let mut state = ReviewAgentState::default();
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
        assert_eq!(load_from_path(&corrupt_path), ReviewAgentState::default());

        let old_path = temp_store_path("old");
        std::fs::create_dir_all(old_path.parent().unwrap()).unwrap();
        std::fs::write(
            &old_path,
            serde_json::json!({
                "version": 0,
                "state": ReviewAgentState::default(),
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(load_from_path(&old_path), ReviewAgentState::default());
    }

    #[test]
    fn two_updates_replace_the_atomic_store() {
        let path = temp_store_path("updates");
        let mut state = ReviewAgentState::default();
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
        let mut state = ReviewAgentState::default();
        state.submit(input("event-1")).unwrap();
        save_to_path(&path, &state).unwrap();

        let mut loaded = load_from_path(&path);
        let duplicate = loaded.submit(input("event-1")).unwrap();

        assert!(!duplicate.changed);
        assert_eq!(loaded, state);
    }

    #[test]
    fn missing_primary_recovers_the_last_backup() {
        let path = temp_store_path("backup-recovery");
        let mut state = ReviewAgentState::default();
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
