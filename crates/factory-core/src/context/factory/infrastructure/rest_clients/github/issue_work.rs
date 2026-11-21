use super::{
    merge_cleanup::{ref_path, reference_sha},
    repository, Github,
};
use crate::context::factory::domain::entities::issue_work::{IssueWork, SubIssue, WorkRepository};
use crate::{Result, SourceConnector, TriggerEvent};
use anyhow::{ensure, Context};
use reqwest::Method;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

mod publication;

impl Github {
    pub(crate) async fn resolve_issue_work(
        &self,
        event: &TriggerEvent,
        profile: &SourceConnector,
        parent: &Value,
    ) -> Result<Option<IssueWork>> {
        let path = format!(
            "/repos/{}/issues/{}/sub_issues",
            event.repository, event.number
        );
        let rows = self
            .pages(&path)
            .await
            .context("Could not list subissues")?;
        if let Some(total) = parent["sub_issues_summary"]["total"].as_u64() {
            ensure!(
                total == rows.len() as u64,
                "Subissue list is incomplete for this source"
            );
        }
        if rows.is_empty() {
            return Ok(None);
        }
        let mut grouped = BTreeMap::<String, Vec<SubIssue>>::new();
        let mut seen = BTreeSet::new();
        for row in rows {
            let state = row["state"].as_str().context("Subissue state missing")?;
            ensure!(matches!(state, "open" | "closed"), "Invalid subissue state");
            if state == "closed" {
                continue;
            }
            let repo = repository(&row)?;
            ensure!(
                profile.allows_owner(repo.split('/').next().unwrap_or("")),
                "Subissue repository {repo} is outside the source scope"
            );
            let number = row["number"]
                .as_u64()
                .filter(|number| *number > 0)
                .context("Invalid subissue number")?;
            let url = format!("https://github.com/{repo}/issues/{number}");
            ensure!(
                row["html_url"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(&url))
                    && row["pull_request"].is_null()
                    && !(repo == event.repository && number == event.number),
                "Subissue identity mismatch"
            );
            if seen.insert((repo.clone(), number)) {
                grouped.entry(repo).or_default().push(SubIssue {
                    number,
                    url,
                    title: row["title"]
                        .as_str()
                        .context("Subissue title missing")?
                        .into(),
                    body: row["body"].as_str().unwrap_or_default().into(),
                });
            }
        }

        let mut repositories = Vec::new();
        for (repo, mut sub_issues) in grouped {
            sub_issues.sort_by_key(|issue| issue.number);
            let base_branch = self
                .repository_default(&repo)
                .await
                .with_context(|| format!("Cannot access subissue repository {repo}"))?;
            let base = self
                .request(Method::GET, &ref_path(&repo, &base_branch, false), None)
                .await?;
            repositories.push(WorkRepository {
                repository: repo,
                base_sha: reference_sha(&base, &base_branch)?,
                base_branch,
                sub_issues,
            });
        }
        Ok(Some(IssueWork {
            event_id: event.event_id.clone(),
            source_id: profile.id.clone(),
            parent_repository: event.repository.clone(),
            parent_number: event.number,
            actor: event.actor.clone(),
            repositories,
        }))
    }
}
