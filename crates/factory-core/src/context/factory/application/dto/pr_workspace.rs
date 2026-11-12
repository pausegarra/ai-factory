use serde::{Deserialize, Serialize};

/// Authenticated refs captured for one PR execution. The base SHA comes from
/// the branch ref, not the potentially stale pull_request.base.sha field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PrWorkspace {
    pub repository: String,
    pub number: u64,
    pub merged: bool,
    pub head_repository: String,
    pub head_ref: String,
    pub head_sha: String,
    pub base_ref: String,
    pub base_sha: String,
}
