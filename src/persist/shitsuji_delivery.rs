use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::shitsuji_agent::delivery::{PersistedShitsujiDelivery, ShitsujiDeliveryState};

fn delivery_path() -> PathBuf {
    crate::session::data_dir().join("shitsuji-delivery.json")
}

/// Pre-rename delivery file. Sessions written before the Shitsuji Agent rename
/// still carry this file, so it is read once and rewritten under the new name.
fn legacy_delivery_path() -> PathBuf {
    crate::session::data_dir().join("review-delivery.json") // pre-rename
}

pub(crate) fn save(state: &ShitsujiDeliveryState) -> std::io::Result<()> {
    save_to_path(&delivery_path(), state)
}

fn save_to_path(path: &Path, state: &ShitsujiDeliveryState) -> std::io::Result<()> {
    super::io::save_json_to_path(path, &state.persisted()).inspect_err(|error| {
        warn!(path = %path.display(), error = %error, "failed to persist shitsuji delivery state");
    })
}

pub(crate) fn clear() -> std::io::Result<()> {
    clear_paths(&delivery_path(), &legacy_delivery_path())
}

/// Clears the legacy file too. Disabling the runtime invalidates the delivery
/// state, and a surviving legacy file would be migrated back in on the next
/// start.
fn clear_paths(path: &Path, legacy_path: &Path) -> std::io::Result<()> {
    let result = super::io::clear_path(path);
    let legacy_result = super::io::clear_path(legacy_path);
    result.and(legacy_result)
}

pub(crate) fn load() -> ShitsujiDeliveryState {
    load_with_legacy_migration(&delivery_path(), &legacy_delivery_path())
}

fn load_with_legacy_migration(path: &Path, legacy_path: &Path) -> ShitsujiDeliveryState {
    if path.exists() || !legacy_path.exists() {
        return load_from_path(path);
    }
    // Only a legacy file that actually parsed is migrated. Writing the default
    // for an unreadable one would make the new path present from then on, so the
    // legacy file would never be retried.
    let Some(state) = stored_state(legacy_path) else {
        return ShitsujiDeliveryState::default();
    };
    match save_to_path(path, &state) {
        Ok(()) => info!(
            path = %path.display(),
            legacy_path = %legacy_path.display(),
            "migrated the pre-rename delivery state to the shitsuji delivery state"
        ),
        Err(error) => warn!(
            path = %path.display(),
            legacy_path = %legacy_path.display(),
            error = %error,
            "failed to write the migrated shitsuji delivery state"
        ),
    }
    state
}

fn load_from_path(path: &Path) -> ShitsujiDeliveryState {
    stored_state(path).unwrap_or_default()
}

/// `None` when the file is missing, unreadable, or unparsable.
fn stored_state(path: &Path) -> Option<ShitsujiDeliveryState> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return None;
    };
    match serde_json::from_str::<PersistedShitsujiDelivery>(&content) {
        Ok(snapshot) => Some(ShitsujiDeliveryState::restore(snapshot)),
        Err(error) => {
            warn!(path = %path.display(), error = %error, "failed to restore shitsuji delivery state");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a session written before the rename. Pins the on-disk
    /// field names and nesting so the type renames cannot silently change the
    /// serde representation.
    const LEGACY_DELIVERY_FIXTURE: &str = r#"{
  "version": 1,
  "fronts": [
    {
      "front_pane_id": 1,
      "backside_pane_id": 2,
      "generation": 1,
      "armed": true,
      "phase": {
        "kind": "idle"
      },
      "pending_phases": [],
      "acknowledged_checkpoint": null
    }
  ],
  "backends": [
    {
      "backside_pane_id": 2,
      "queue": [],
      "in_flight": null
    }
  ]
}"#;

    fn temp_delivery_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "herdr-shitsuji-delivery-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn write_delivery(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn fixture_with_backside(backside_pane_id: u64) -> String {
        LEGACY_DELIVERY_FIXTURE.replace(
            "\"backside_pane_id\": 2",
            &format!("\"backside_pane_id\": {backside_pane_id}"),
        )
    }

    fn backside_pane_ids(state: &ShitsujiDeliveryState) -> Vec<u64> {
        serde_json::to_value(state.persisted()).unwrap()["backends"]
            .as_array()
            .unwrap()
            .iter()
            .map(|backend| backend["backside_pane_id"].as_u64().unwrap())
            .collect()
    }

    #[test]
    fn legacy_delivery_fixture_loads_without_representation_drift() {
        let path = temp_delivery_dir("fixture").join("fixture.json");
        write_delivery(&path, LEGACY_DELIVERY_FIXTURE);

        let state = load_from_path(&path);

        let reserialized = serde_json::to_string_pretty(&state.persisted()).unwrap();
        assert_eq!(reserialized, LEGACY_DELIVERY_FIXTURE);
    }

    #[test]
    fn legacy_delivery_is_migrated_when_only_the_legacy_file_exists() {
        let dir = temp_delivery_dir("migrate-legacy-only");
        let path = dir.join("shitsuji-delivery.json");
        let legacy_path = dir.join("review-delivery.json"); // pre-rename
        write_delivery(&legacy_path, &fixture_with_backside(7));

        let state = load_with_legacy_migration(&path, &legacy_path);

        assert_eq!(backside_pane_ids(&state), vec![7]);
        assert!(
            path.exists(),
            "the legacy delivery state should be rewritten"
        );
        assert!(
            legacy_path.exists(),
            "the legacy delivery state should be kept"
        );
        assert_eq!(backside_pane_ids(&load_from_path(&path)), vec![7]);
    }

    #[test]
    fn new_delivery_is_used_when_only_the_new_file_exists() {
        let dir = temp_delivery_dir("migrate-new-only");
        let path = dir.join("shitsuji-delivery.json");
        let legacy_path = dir.join("review-delivery.json"); // pre-rename
        write_delivery(&path, &fixture_with_backside(9));

        let state = load_with_legacy_migration(&path, &legacy_path);

        assert_eq!(backside_pane_ids(&state), vec![9]);
        assert!(
            !legacy_path.exists(),
            "no legacy delivery state should be created"
        );
    }

    #[test]
    fn an_unreadable_legacy_delivery_file_is_not_migrated() {
        let dir = temp_delivery_dir("migrate-corrupt");
        let path = dir.join("shitsuji-delivery.json");
        let legacy_path = dir.join("review-delivery.json"); // pre-rename
        write_delivery(&legacy_path, "not json");

        let state = load_with_legacy_migration(&path, &legacy_path);

        assert_eq!(backside_pane_ids(&state), Vec::<u64>::new());
        assert!(
            !path.exists(),
            "an unreadable legacy file must not be migrated, or it would never be retried"
        );
        assert!(legacy_path.exists());
    }

    #[test]
    fn clearing_removes_the_legacy_file_so_it_cannot_be_migrated_back() {
        let dir = temp_delivery_dir("clear-both");
        let path = dir.join("shitsuji-delivery.json");
        let legacy_path = dir.join("review-delivery.json"); // pre-rename
        write_delivery(&path, &fixture_with_backside(9));
        write_delivery(&legacy_path, &fixture_with_backside(7));

        clear_paths(&path, &legacy_path).unwrap();

        assert!(!path.exists());
        assert!(!legacy_path.exists());
        assert_eq!(
            backside_pane_ids(&load_with_legacy_migration(&path, &legacy_path)),
            Vec::<u64>::new()
        );
    }

    #[test]
    fn new_delivery_wins_when_both_files_exist() {
        let dir = temp_delivery_dir("migrate-both");
        let path = dir.join("shitsuji-delivery.json");
        let legacy_path = dir.join("review-delivery.json"); // pre-rename
        write_delivery(&path, &fixture_with_backside(9));
        write_delivery(&legacy_path, &fixture_with_backside(7));

        let state = load_with_legacy_migration(&path, &legacy_path);

        assert_eq!(backside_pane_ids(&state), vec![9]);
        assert_eq!(
            std::fs::read_to_string(&legacy_path).unwrap(),
            fixture_with_backside(7),
            "the legacy delivery state should be left untouched"
        );
    }
}
