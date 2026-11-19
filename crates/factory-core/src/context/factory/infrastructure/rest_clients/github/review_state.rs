use super::merge_cleanup::valid_branch;
use super::*;
use crate::context::factory::domain::entities::pull_request_review::PullRequestReviewState;

impl Github {
    pub(super) fn review_targets_branch(pr: &Value, default: &str) -> Result<bool> {
        let target = pr["base"]["ref"].as_str().context("Missing PR base ref")?;
        ensure!(valid_branch(target), "Invalid PR base ref");
        Ok(target == default)
    }

    pub(super) async fn account_review_state(
        &self,
        repo: &str,
        number: u64,
        actor: &str,
    ) -> Result<PullRequestReviewState> {
        let reviews = self
            .pages(&format!("/repos/{repo}/pulls/{number}/reviews"))
            .await?;
        let mut decision = PullRequestReviewState::Unreviewed;
        // GitHub returns submitted reviews chronologically, across all pages.
        // Comments and pending reviews do not replace a submitted decision.
        for review in reviews {
            if !review["user"]["login"]
                .as_str()
                .is_some_and(|login| login.eq_ignore_ascii_case(actor))
            {
                continue;
            }
            match review["state"].as_str() {
                Some("APPROVED") => decision = PullRequestReviewState::Approved,
                Some("CHANGES_REQUESTED") => {
                    let sha = review["commit_id"]
                        .as_str()
                        .context("Missing reviewed PR head")?;
                    ensure!(
                        sha.len() == 40 && sha.bytes().all(|c| c.is_ascii_hexdigit()),
                        "Invalid reviewed PR head"
                    );
                    decision = PullRequestReviewState::ChangesRequested {
                        head_sha: sha.to_ascii_lowercase(),
                    };
                }
                Some("DISMISSED") => decision = PullRequestReviewState::Unreviewed,
                Some("COMMENTED" | "PENDING") => {}
                _ => bail!("Invalid GitHub review state"),
            }
        }
        Ok(decision)
    }
}
