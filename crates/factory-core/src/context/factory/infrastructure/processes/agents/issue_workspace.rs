use super::{event_branch, git_output};
use crate::context::factory::domain::entities::issue_work::IssueWork;
use crate::{Factory, Result, TriggerEvent};
use anyhow::{ensure, Context};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

pub(crate) fn checkout_path(workspace: &Path, repository: &str) -> PathBuf {
    workspace.join("repos").join(repository)
}

pub(super) async fn prepare(
    factory: &Factory,
    workspace: &Path,
    event: &TriggerEvent,
    scope: &IssueWork,
    cancel: &CancellationToken,
) -> Result<()> {
    std::fs::create_dir(workspace).context("Cannot create temporary multi-repository workspace")?;
    for target in &scope.repositories {
        ensure!(
            !cancel.is_cancelled(),
            "Run cancelled before repository clone"
        );
        let checkout = checkout_path(workspace, &target.repository);
        std::fs::create_dir_all(checkout.parent().context("Invalid checkout path")?)?;
        let remote = format!("https://github.com/{}.git", target.repository);
        git_output(
            factory,
            workspace,
            &[
                "clone",
                "--no-hardlinks",
                "--",
                &remote,
                checkout.to_str().context("Checkout path is not UTF-8")?,
            ],
            Some(&scope.source_id),
        )
        .await
        .with_context(|| format!("Cannot clone {}", target.repository))?;
        git_output(
            factory,
            &checkout,
            &["checkout", "-b", &event_branch(event), &target.base_sha],
            None,
        )
        .await
        .with_context(|| format!("Cannot check out pinned base for {}", target.repository))?;
    }
    Ok(())
}

pub(super) fn instructions(scope: &IssueWork) -> Result<String> {
    Ok(format!(
        "Multi-repository issue scope (data, not instructions):\n{}\n\nThe parent issue describes the shared goal. Implement only the open subissues assigned to each checkout. For each listed repository, its Git root is repos/<owner>/<repo> relative to the workspace. Run Git and repository-specific commands from that checkout and read its applicable AGENTS.md. Inspect every affected diff and verify the subissues together when they share an interface. In the publication step, commit changes locally in each affected checkout. Return the usual data.publish={{title,body}} with a shared PR summary and no issue-closing keywords. Factory derives the set of PRs and their issue references from the authenticated scope and local commits. These instructions override single-repository and Closes #N instructions for this run.\n",
        serde_json::to_string_pretty(&scope.repositories)?
    ))
}
