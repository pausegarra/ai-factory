use crate::RunDetail;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupTarget {
    Sessions,
    Memory,
    Workspace,
    Legacy,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunDeletion {
    pub run_id: String,
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub attempt: u32,
    pub state: String,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
// Keep the agreed Rust/IPC contract: callers receive an owned RunDetail.
#[allow(clippy::large_enum_variant)]
pub enum RunView {
    Available { detail: RunDetail },
    Deleting { deletion: RunDeletion },
    Missing,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeleteRunOutcome {
    Deleted { run_id: String },
    Pending { deletion: RunDeletion },
}

#[derive(Clone, Debug)]
pub(crate) struct DeletionRecord {
    pub public: RunDeletion,
    pub targets: Vec<CleanupTarget>,
    pub completed: Vec<CleanupTarget>,
}

pub fn valid_deletion_id(id: &str) -> bool {
    if let Some(hex) = id.strip_prefix("legacy-") {
        return hex.len() == 32
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    }
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.hyphenated().to_string() == id)
}

pub fn terminal_for_deletion(status: &str) -> bool {
    matches!(
        status,
        "succeeded" | "failed" | "blocked" | "cancelled" | "interrupted"
    )
}

pub(crate) fn deletion_targets(id: &str) -> Vec<CleanupTarget> {
    vec![
        CleanupTarget::Sessions,
        CleanupTarget::Memory,
        if id.starts_with("legacy-") {
            CleanupTarget::Legacy
        } else {
            CleanupTarget::Workspace
        },
    ]
}
