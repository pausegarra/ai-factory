use crate::context::factory::application::dto::issue_node_comment::{
    IssueNodeCommentReceipt, IssueNodeCommentRequest,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IssueNodeCommentFailure {
    CredentialUnavailable,
    RedactionUnavailable,
    ActorMismatch,
    Timeout,
    Transport,
    HttpRejected,
    InvalidResponse,
    PaginationIncomplete,
    Stopped,
    RunUnavailable,
}
impl IssueNodeCommentFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::CredentialUnavailable => "credential_unavailable",
            Self::RedactionUnavailable => "redaction_unavailable",
            Self::ActorMismatch => "actor_mismatch",
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::HttpRejected => "http_rejected",
            Self::InvalidResponse => "invalid_response",
            Self::PaginationIncomplete => "pagination_incomplete",
            Self::Stopped => "stopped",
            Self::RunUnavailable => "run_unavailable",
        }
    }
}
pub(crate) trait IssueNodeCommentTransport: Send + Sync {
    fn deliver<'a>(
        &'a self,
        request: &'a IssueNodeCommentRequest,
        can_send: &'a (dyn Fn() -> bool + Send + Sync),
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<IssueNodeCommentReceipt, IssueNodeCommentFailure>,
                > + Send
                + 'a,
        >,
    >;
}
