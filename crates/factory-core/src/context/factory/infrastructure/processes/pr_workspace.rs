use super::agents::git_output;
use crate::{Factory, Result, TriggerEvent};
use anyhow::ensure;
use std::path::Path;

pub(crate) async fn prepare(
    factory: &Factory,
    run: &str,
    event: &TriggerEvent,
    workspace: &Path,
) -> Result<()> {
    let api = factory.github(&factory.store.token_for(&event.actor).await?)?;
    let snapshot = api.pr_workspace(event).await?;
    let remote = format!("https://github.com/{}.git", event.repository);
    for (source, target, expected) in [
        (
            format!("refs/heads/{}", snapshot.base_ref),
            "refs/ai-factory/base",
            &snapshot.base_sha,
        ),
        (
            format!("refs/pull/{}/head", event.number),
            "refs/ai-factory/head",
            &snapshot.head_sha,
        ),
    ] {
        git_output(
            factory,
            workspace,
            &["fetch", "--no-tags", &remote, &format!("{source}:{target}")],
            Some(&event.actor),
        )
        .await?;
        let sha = git_output(
            factory,
            workspace,
            &["rev-parse", "--verify", &format!("{target}^{{commit}}")],
            None,
        )
        .await?;
        ensure!(
            sha == *expected,
            "PR refs changed during workspace preparation"
        );
    }
    factory.store.observe(
        &format!("pr-workspace:{run}"),
        &serde_json::to_value(snapshot)?,
    )?;
    Ok(())
}
