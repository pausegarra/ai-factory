use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct IssueNodeCommentSnapshot {
    pub run_id: String,
    pub node_id: String,
    pub node_name: String,
    pub repository: String,
    pub number: u64,
    pub actor: String,
    pub run_attempt: u32,
    pub revision: u32,
    pub status: String,
    pub summary: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Debug)]
pub(crate) struct IssueNodeCommentJob {
    pub snapshot: IssueNodeCommentSnapshot,
    pub key: String,
    pub failures: u32,
}
#[derive(Clone, Debug)]
pub(crate) struct IssueNodeCommentRequest {
    pub repository: String,
    pub number: u64,
    pub actor: String,
    pub marker: String,
    pub body: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct IssueNodeCommentReceipt {
    pub comment_id: u64,
    pub html_url: Option<String>,
}
