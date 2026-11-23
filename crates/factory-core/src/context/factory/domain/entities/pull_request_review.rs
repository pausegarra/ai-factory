use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub(crate) struct ReviewHeadChanged;

impl std::fmt::Display for ReviewHeadChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PR head changed during review")
    }
}

impl std::error::Error for ReviewHeadChanged {}

/// Effective submitted decision of the account performing the review.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum PullRequestReviewState {
    #[default]
    Unreviewed,
    Approved,
    ChangesRequested {
        head_sha: String,
    },
}

impl PullRequestReviewState {
    pub(crate) fn needs_updated_review(&self, head: &str) -> bool {
        matches!(self, Self::ChangesRequested { head_sha } if !head_sha.eq_ignore_ascii_case(head))
    }

    pub(crate) fn allows_review(&self, head: &str, all: bool, requested: bool) -> bool {
        match self {
            Self::Approved => false,
            Self::ChangesRequested { .. } => self.needs_updated_review(head),
            Self::Unreviewed => all || requested,
        }
    }
}
