use super::{
    merge_cleanup::{ref_path, reference_sha, valid_branch},
    Github,
};
use crate::{
    context::factory::{
        application::dto::pr_workspace::PrWorkspace,
        infrastructure::processes::agents::{event_branch, git_output},
    },
    Factory, Result, TriggerEvent,
};
use anyhow::{ensure, Context};
use reqwest::Method;
use serde_json::json;
use std::path::Path;

impl Github {
    pub(crate) async fn pr_workspace(&self, event: &TriggerEvent) -> Result<PrWorkspace> {
        let pr = self.pr(&event.repository, event.number).await?;
        let merged = pr["merged"] == true;
        ensure!(
            (pr["state"] == "open" && !merged) || (event.trigger_id == "pr_approved" && merged),
            "PR is not open"
        );
        ensure!(
            pr["head"]["sha"] == event.head_sha,
            "PR head changed before execution"
        );
        let base_ref = pr["base"]["ref"].as_str().context("Missing PR base ref")?;
        let head_ref = pr["head"]["ref"].as_str().context("Missing PR head ref")?;
        ensure!(
            valid_branch(base_ref) && valid_branch(head_ref),
            "Invalid PR branch reference"
        );
        let head_repository = pr["head"]["repo"]["full_name"]
            .as_str()
            .context("Missing PR head repository")?;
        let reference = self
            .request(
                Method::GET,
                &ref_path(&event.repository, base_ref, false),
                None,
            )
            .await?;
        Ok(PrWorkspace {
            repository: event.repository.clone(),
            number: event.number,
            merged,
            head_repository: head_repository.into(),
            head_ref: head_ref.into(),
            head_sha: event.head_sha.clone(),
            base_ref: base_ref.into(),
            base_sha: reference_sha(&reference, base_ref)?,
        })
    }

    pub(super) async fn update_pr(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
        workspace: &Path,
        check_active: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<()> {
        ensure!(
            factory
                .store
                .observation(&format!("pr-update:{run}"))?
                .is_none(),
            "PR already updated; new approval required"
        );
        let snapshot: PrWorkspace = serde_json::from_value(
            factory
                .store
                .observation(&format!("pr-workspace:{run}"))?
                .context("Authenticated PR workspace snapshot missing")?,
        )?;
        ensure!(!snapshot.merged, "Cannot update an already merged PR");
        ensure!(
            snapshot.repository == event.repository
                && snapshot.number == event.number
                && snapshot.head_sha == event.head_sha,
            "PR workspace identity mismatch"
        );
        ensure!(
            snapshot
                .head_repository
                .eq_ignore_ascii_case(&event.repository),
            "Updating fork PR branches is unsupported"
        );
        let default = self.repository_default(&event.repository).await?;
        ensure!(
            snapshot.head_ref != snapshot.base_ref && snapshot.head_ref != default,
            "Cannot update target or default branch"
        );
        let branch = git_output(factory, workspace, &["branch", "--show-current"], None).await?;
        ensure!(
            branch == event_branch(event),
            "Cannot update PR outside event branch"
        );
        let dirty = git_output(
            factory,
            workspace,
            &["status", "--porcelain", "--untracked-files=all"],
            None,
        )
        .await?;
        ensure!(
            dirty.is_empty(),
            "Commit PR corrections before requesting update_pr; workspace is dirty"
        );
        for operation in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-merge",
            "rebase-apply",
        ] {
            let path = git_output(
                factory,
                workspace,
                &["rev-parse", "--git-path", operation],
                None,
            )
            .await?;
            ensure!(
                !workspace.join(path).try_exists()?,
                "Finish local Git operation before requesting update_pr"
            );
        }
        let sha = git_output(
            factory,
            workspace,
            &["rev-parse", "--verify", "HEAD^{commit}"],
            None,
        )
        .await?;
        ensure!(
            sha != event.head_sha,
            "No committed PR corrections to publish"
        );
        for ancestor in [&event.head_sha, &snapshot.base_sha] {
            git_output(factory, workspace, &["merge-base", "--is-ancestor", ancestor, &sha], None).await.context("PR correction must include both original head and checked base; history rewrites are forbidden")?;
        }
        let current = self.pr_workspace(event).await?;
        ensure!(!current.merged, "Cannot update an already merged PR");
        ensure!(
            current.head_repository == snapshot.head_repository
                && current.head_ref == snapshot.head_ref
                && current.base_ref == snapshot.base_ref,
            "PR branch identity changed before update"
        );
        ensure!(
            current.base_sha == snapshot.base_sha,
            "PR base changed since conflict check; repeat resolution"
        );
        let head_path = ref_path(&event.repository, &snapshot.head_ref, false);
        let remote = self.request(Method::GET, &head_path, None).await?;
        ensure!(
            reference_sha(&remote, &snapshot.head_ref)? == event.head_sha,
            "PR source branch changed before update"
        );
        check_active()?;
        // Lineage was checked above. The exact lease adds compare-and-swap
        // semantics; it never permits rewriting the approved head's history.
        let lease = format!(
            "--force-with-lease=refs/heads/{}:{}",
            snapshot.head_ref, event.head_sha
        );
        let remote = format!("https://github.com/{}.git", event.repository);
        let push = git_output(
            factory,
            workspace,
            &[
                "push",
                &lease,
                &remote,
                &format!("{sha}:refs/heads/{}", snapshot.head_ref),
            ],
            Some(&event.actor),
        )
        .await;
        // A transport failure can occur after GitHub accepted the push. Read
        // authoritative refs instead of issuing another push.
        let confirmed = async {
            let reference = self.request(Method::GET, &head_path, None).await?;
            ensure!(
                reference_sha(&reference, &snapshot.head_ref)? == sha,
                "PR correction push not confirmed; source branch differs"
            );
            let pr = self.pr(&event.repository, event.number).await?;
            ensure!(
                pr["head"]["ref"] == snapshot.head_ref
                    && pr["head"]["repo"]["full_name"] == snapshot.head_repository
                    && pr["base"]["ref"] == snapshot.base_ref
                    && pr["state"] == "open",
                "PR identity changed after correction push"
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = confirmed {
            return Err(match push {
                Err(push) => error.context(format!("PR correction push failed: {push:#}")),
                Ok(_) => error,
            });
        }
        factory.store.observe(
            &format!("pr-update:{run}"),
            &json!({
                "repository":event.repository, "number":event.number,
                "head_ref":snapshot.head_ref, "previous_head_sha":event.head_sha,
                "head_sha":sha, "base_sha":snapshot.base_sha, "requires_approval":true
            }),
        )?;
        Ok(())
    }
}
