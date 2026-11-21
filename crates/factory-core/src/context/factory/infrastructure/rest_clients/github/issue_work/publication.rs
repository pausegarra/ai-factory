use super::super::merge_cleanup::{ref_path, reference_sha};
use super::Github;
use crate::context::factory::domain::entities::issue_work::{
    IssuePublication, IssueWork, PublicationTarget, WorkRepository,
};
use crate::context::factory::infrastructure::processes::agents::{
    event_branch, git_output, issue_workspace::checkout_path,
};
use crate::{Factory, Result, RunDetail, SourceConnector, TriggerEvent};
use anyhow::{bail, ensure, Context};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn source_marker(scope: &IssueWork) -> String {
    let key = Sha256::digest(scope.event_id.as_bytes());
    format!("<!-- ai-factory:issue-work:{key:x} -->")
}

fn has_closing_reference(text: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
    ];
    text.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        let mut words = lower.split_ascii_whitespace().peekable();
        while let Some(word) = words.next() {
            let (keyword, reference) = word.split_once(':').unwrap_or((word, ""));
            if KEYWORDS.contains(&keyword.trim_start_matches(['*', '-', '>', '(', '_']))
                && (reference.contains('#')
                    || words.peek().is_some_and(|next| {
                        next.contains('#')
                            || next.contains("github.com/") && next.contains("/issues/")
                    }))
            {
                return true;
            }
        }
        false
    })
}

impl Github {
    pub(in super::super) async fn stage_issue_publish(
        &self,
        factory: &Factory,
        detail: &RunDetail,
        scope: &IssueWork,
        workspace: &Path,
        node_id: &str,
        result: &Value,
    ) -> Result<()> {
        let request = &result["data"]["publish"];
        let title = request["title"].as_str().context("PR title required")?;
        let body = request["body"].as_str().context("PR body required")?;
        ensure!(
            !has_closing_reference(title)
                && !has_closing_reference(body)
                && !body.contains("<!-- ai-factory:issue-work:"),
            "Multi-repository PR descriptions must leave issue references to Factory"
        );
        let branch = event_branch(&detail.event);
        let mut targets = Vec::new();
        let mut bundles = Vec::new();
        let bundle_dir = workspace.join("publication");
        std::fs::create_dir(&bundle_dir)?;
        for (index, repo) in scope.repositories.iter().enumerate() {
            let checkout = checkout_path(workspace, &repo.repository);
            ensure!(
                checkout.canonicalize()?
                    == checkout_path(&workspace.canonicalize()?, &repo.repository),
                "Invalid checkout for {}",
                repo.repository
            );
            let git = async |args: &[&str]| git_output(factory, &checkout, args, None).await;
            ensure!(
                git(&["branch", "--show-current"]).await? == branch,
                "Unexpected branch in {}",
                repo.repository
            );
            ensure!(
                git(&["status", "--porcelain", "--untracked-files=all"])
                    .await?
                    .is_empty(),
                "Commit the changes in {} before publication",
                repo.repository
            );
            git(&["merge-base", "--is-ancestor", &repo.base_sha, "HEAD"])
                .await
                .with_context(|| {
                    format!("{} does not descend from its pinned base", repo.repository)
                })?;
            let head_sha = git(&["rev-parse", "HEAD"]).await?;
            if git(&["rev-parse", &format!("{}^{{tree}}", repo.base_sha)]).await?
                == git(&["rev-parse", "HEAD^{tree}"]).await?
            {
                continue;
            }
            let messages =
                git(&["log", "--format=%B", &format!("{}..HEAD", repo.base_sha)]).await?;
            ensure!(
                !has_closing_reference(&messages),
                "Commit messages in {} must leave issue closure to Factory",
                repo.repository
            );
            self.require_current_target(repo).await?;
            let bundle = bundle_dir.join(format!("{index}.bundle"));
            git(&[
                "bundle",
                "create",
                bundle.to_str().context("Bundle path is not UTF-8")?,
                &format!("refs/heads/{branch}"),
                &format!("^{}", repo.base_sha),
            ])
            .await?;
            let mut body = format!(
                "{body}\n\nParent issue: {}#{}",
                scope.parent_repository, scope.parent_number
            );
            for child in &repo.sub_issues {
                body.push_str(&format!("\nCloses #{}", child.number));
            }
            body.push_str(&format!("\n\n{}", source_marker(scope)));
            targets.push(PublicationTarget {
                repository: repo.repository.clone(),
                head_sha,
                title: title.into(),
                body,
                number: None,
            });
            bundles.push(bundle);
        }
        factory.store.stage_issue_publication(
            scope,
            &IssuePublication {
                node_id: node_id.into(),
                flow: Some(detail.flow.clone()),
                result: Some(json!({"status":result["status"],"data":result["data"]})),
                targets,
            },
            bundles,
            &detail.run.id,
        )
    }

    async fn require_current_target(&self, repo: &WorkRepository) -> Result<()> {
        ensure!(
            self.repository_default(&repo.repository).await? == repo.base_branch,
            "Default branch changed for {} since the run was prepared",
            repo.repository
        );
        Ok(())
    }

    pub(crate) async fn resume_issue_publish(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        scope: &IssueWork,
        run: &str,
        workspace: &Path,
    ) -> Result<()> {
        let publication = factory
            .store
            .issue_publication(scope)?
            .context("Issue publication not prepared")?;
        let branch = event_branch(event);
        let check_active = || -> Result<()> {
            ensure!(
                !factory.stop.is_cancelled() && factory.store.detail(run)?.run.status == "running",
                "Issue publication run is no longer active"
            );
            scope.authorize(factory.store.public_settings()?.source_for_event(event)?)
        };
        check_active()?;
        self.verify_actor(&scope.actor).await?;
        if publication
            .targets
            .iter()
            .any(|target| target.number.is_none())
        {
            let parent = self
                .request(Method::GET, &parent_issue_path(scope), None)
                .await?;
            let settings = factory.store.public_settings()?;
            let profile = settings.source_for_event(event)?;
            ensure!(
                parent["state"] == "open"
                    && parent_matches(&parent, scope)
                    && parent["title"]
                        .as_str()
                        .is_some_and(|title| profile.matches_issue_title(title, "new_issue")),
                "Parent issue is no longer eligible for publication"
            );
        }
        for target in &publication.targets {
            if target.number.is_some() {
                continue;
            }
            check_active()?;
            let repo = scope
                .repositories
                .iter()
                .find(|repo| repo.repository == target.repository)
                .context("Publish target is outside the stored scope")?;
            let owner = repo
                .repository
                .split('/')
                .next()
                .context("Invalid repository owner")?;
            let path = format!(
                "/repos/{}/pulls?state=all&head={owner}:{branch}",
                repo.repository
            );
            let existing = self.pages(&path).await?;
            ensure!(
                existing.len() <= 1,
                "Multiple PRs exist for the branch in {}",
                repo.repository
            );
            let number = if let Some(row) = existing.first() {
                pull_number(row)?
            } else {
                self.require_current_target(repo).await?;
                self.push_issue_branch(
                    factory,
                    scope,
                    repo,
                    target,
                    workspace,
                    &branch,
                    &check_active,
                )
                .await?;
                check_active()?;
                pull_number(&self.request(
                    Method::POST,
                    &format!("/repos/{}/pulls", repo.repository),
                    Some(&json!({"title":target.title,"body":target.body,"head":branch,"base":repo.base_branch,"draft":false})),
                ).await?)?
            };
            let pr = self.pr(&repo.repository, number).await?;
            verify_issue_pr(&pr, scope, repo, number, &branch)?;
            ensure!(
                (pr["state"] == "open" || pr["merged"] == true)
                    && pr["head"]["sha"] == target.head_sha,
                "PR in {} does not match the staged changes",
                repo.repository
            );
            check_active()?;
            factory
                .store
                .record_issue_pr(scope, event, run, &repo.repository, number)?;
            factory.emit("run_changed", Some(run), None);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn push_issue_branch(
        &self,
        factory: &Factory,
        scope: &IssueWork,
        repo: &WorkRepository,
        target: &PublicationTarget,
        workspace: &Path,
        branch: &str,
        check_active: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<()> {
        let path = ref_path(&repo.repository, branch, false);
        let response = self.send(Method::GET, &path, None).await?;
        match response.status() {
            StatusCode::OK => {
                let sha = reference_sha(&response.json::<Value>().await?, branch)?;
                ensure!(
                    sha == target.head_sha,
                    "Publication branch in {} already exists at a different commit",
                    repo.repository
                );
                return Ok(());
            }
            StatusCode::NOT_FOUND => {}
            status => bail!("GitHub API {} for {path}", status.as_u16()),
        }
        check_active()?;
        let url = format!("https://github.com/{}.git", repo.repository);
        let source = restore_commit(factory, scope, repo, target, workspace, branch, &url).await?;
        check_active()?;
        let destination = format!("refs/heads/{branch}");
        git_output(
            factory,
            &source,
            &[
                "push",
                &format!("--force-with-lease={destination}:"),
                "--",
                &url,
                &format!("{}:{destination}", target.head_sha),
            ],
            Some(&scope.source_id),
        )
        .await
        .with_context(|| format!("Could not push {}", repo.repository))?;
        Ok(())
    }

    pub(in super::super) async fn close_completed_issue_work(
        &self,
        factory: &Factory,
        profile: &SourceConnector,
        scope: &IssueWork,
        publication: &IssuePublication,
    ) -> Result<()> {
        scope.authorize(profile)?;
        let mut event = TriggerEvent::issue(
            &scope.parent_repository,
            scope.parent_number,
            "",
            &scope.actor,
            "",
        );
        event.event_id = scope.event_id.clone();
        let branch = event_branch(&event);
        for repo in &scope.repositories {
            if let Some(target) = publication
                .targets
                .iter()
                .find(|target| target.repository == repo.repository)
            {
                let number = target.number.context("Publication pending")?;
                let pr = self.pr(&repo.repository, number).await?;
                verify_issue_pr(&pr, scope, repo, number, &branch)?;
                if pr["merged"] != true {
                    return Ok(());
                }
            }
        }
        let path = parent_issue_path(scope);
        let parent = self.request(Method::GET, &path, None).await?;
        ensure!(
            parent_matches(&parent, scope),
            "Parent issue identity changed"
        );
        if parent["state"] == "open" {
            let children = self.pages(&format!("{path}/sub_issues")).await?;
            if let Some(total) = parent["sub_issues_summary"]["total"].as_u64() {
                ensure!(
                    total == children.len() as u64,
                    "Subissue list is incomplete for this source"
                );
            }
            if children.iter().any(|child| child["state"] != "closed") {
                return Ok(());
            }
            ensure!(!factory.stop.is_cancelled(), "Factory is stopping");
            scope.authorize(
                factory
                    .store
                    .public_settings()?
                    .source_by_id(&profile.id)
                    .context("Source connector unavailable")?,
            )?;
            let closed = self
                .request(
                    Method::PATCH,
                    &path,
                    Some(&json!({"state":"closed","state_reason":"completed"})),
                )
                .await?;
            ensure!(
                closed["state"] == "closed" && parent_matches(&closed, scope),
                "GitHub did not confirm closure of the parent issue"
            );
        } else {
            ensure!(parent["state"] == "closed", "Unknown parent issue state");
        }
        for run in factory.store.mark_issue_work_closed(scope)? {
            factory.emit("run_changed", Some(&run), None);
        }
        Ok(())
    }
}

fn parent_issue_path(scope: &IssueWork) -> String {
    format!(
        "/repos/{}/issues/{}",
        scope.parent_repository, scope.parent_number
    )
}

fn parent_matches(value: &Value, scope: &IssueWork) -> bool {
    value["pull_request"].is_null()
        && value["number"] == scope.parent_number
        && value["html_url"].as_str().is_some_and(|url| {
            url.eq_ignore_ascii_case(&format!(
                "https://github.com/{}/issues/{}",
                scope.parent_repository, scope.parent_number
            ))
        })
        && value["user"]["login"]
            .as_str()
            .is_some_and(|actor| actor.eq_ignore_ascii_case(&scope.actor))
}

fn pull_number(pr: &Value) -> Result<u64> {
    pr["number"]
        .as_u64()
        .filter(|number| *number > 0)
        .context("Invalid PR number")
}

fn verify_issue_pr(
    pr: &Value,
    scope: &IssueWork,
    repo: &WorkRepository,
    number: u64,
    branch: &str,
) -> Result<()> {
    ensure!(
        pull_number(pr)? == number
            && pr["head"]["ref"] == branch
            && pr["base"]["ref"] == repo.base_branch
            && ["head", "base"]
                .iter()
                .all(|side| pr[*side]["repo"]["full_name"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(&repo.repository)))
            && pr["user"]["login"]
                .as_str()
                .is_some_and(|actor| actor.eq_ignore_ascii_case(&scope.actor))
            && pr["body"]
                .as_str()
                .is_some_and(|body| body.lines().any(|line| line == source_marker(scope))),
        "PR provenance mismatch for {}#{number}",
        repo.repository
    );
    Ok(())
}

async fn restore_commit(
    factory: &Factory,
    scope: &IssueWork,
    repo: &WorkRepository,
    target: &PublicationTarget,
    workspace: &Path,
    branch: &str,
    remote: &str,
) -> Result<PathBuf> {
    std::fs::create_dir_all(workspace)?;
    let checkout = workspace.join(format!("publish-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&checkout)?;
    let bundle = checkout.join("source.bundle");
    std::fs::write(
        &bundle,
        factory.store.issue_bundle(scope, &repo.repository)?,
    )?;
    // The bundle contains commits above the pinned base. Fetch that base before
    // importing the bundle, keeping the approved commits byte-for-byte intact.
    git_output(factory, &checkout, &["init", "--bare"], None).await?;
    git_output(
        factory,
        &checkout,
        &["fetch", "--no-tags", "--", remote, &repo.base_sha],
        Some(&scope.source_id),
    )
    .await?;
    let reference = format!("refs/heads/{branch}");
    git_output(
        factory,
        &checkout,
        &[
            "fetch",
            "--no-tags",
            "--",
            bundle.to_str().context("Bundle path is not UTF-8")?,
            &format!("{reference}:{reference}"),
        ],
        None,
    )
    .await?;
    ensure!(
        git_output(factory, &checkout, &["rev-parse", &reference], None).await? == target.head_sha,
        "Recovered commit for {} does not match the approved SHA",
        repo.repository
    );
    Ok(checkout)
}
