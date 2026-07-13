use std::path::PathBuf;

use tracing::warn;

use crate::review_agent::delivery::{PersistedReviewDelivery, ReviewDeliveryState};

fn delivery_path() -> PathBuf {
    crate::session::data_dir().join("review-delivery.json")
}

pub(crate) fn save(state: &ReviewDeliveryState) -> std::io::Result<()> {
    let path = delivery_path();
    super::io::save_json_to_path(&path, &state.persisted()).inspect_err(|error| {
        warn!(path = %path.display(), error = %error, "failed to persist review delivery state");
    })
}

pub(crate) fn clear() -> std::io::Result<()> {
    super::io::clear_path(&delivery_path())
}

pub(crate) fn load() -> ReviewDeliveryState {
    let path = delivery_path();
    let Ok(content) = std::fs::read_to_string(&path) else {
        return ReviewDeliveryState::default();
    };
    match serde_json::from_str::<PersistedReviewDelivery>(&content) {
        Ok(snapshot) => ReviewDeliveryState::restore(snapshot),
        Err(error) => {
            warn!(path = %path.display(), error = %error, "failed to restore review delivery state");
            ReviewDeliveryState::default()
        }
    }
}
