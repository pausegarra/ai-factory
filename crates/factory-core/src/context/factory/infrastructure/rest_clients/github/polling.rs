use super::*;
use crate::context::factory::infrastructure::repositories::pr_origins::TrackedPrOrigin;
use std::collections::{HashMap, HashSet};

impl Github {
    pub(crate) async fn poll(factory: &Factory) -> Result<()> {
        let settings = factory.store.public_settings()?;
        let origins = factory.store.tracked_prs()?;
        let mut failures = vec![];
        for profile in &settings.token_profiles {
            let actor = &profile.account;
            let issue = profile.allows_actor("new_issue")
                && factory.workflow_enabled_for("new_issue", actor)?;
            let approval = profile.allows_actor("pr_approved")
                && factory.workflow_enabled_for("pr_approved", actor)?;
            let review = profile.allows_actor("pr_review")
                && factory.workflow_enabled_for("pr_review", actor)?;
            let tracked: Vec<_> = origins
                .iter()
                .filter(|origin| {
                    origin.actor.eq_ignore_ascii_case(actor)
                        && profile.allows_owner(origin.repository.split('/').next().unwrap_or(""))
                })
                .collect();
            if profile.owners.is_empty() || !(issue || (approval && !tracked.is_empty()) || review)
            {
                continue;
            }
            let api = match async {
                let api = factory
                    .github(&factory.store.token_for(actor).await?)?
                    .for_polling();
                api.verify_poll_actor(actor).await?;
                Ok::<_, anyhow::Error>(api)
            }
            .await
            {
                Ok(api) => api,
                Err(error) => {
                    failures.push(format!("Account {actor}: {error}"));
                    continue;
                }
            };
            if issue {
                if let Err(error) = api.poll_issues(factory, profile).await {
                    failures.push(format!("Issues @{actor}: {error}"));
                }
            }
            if approval && !tracked.is_empty() {
                if let Err(error) = api.poll_approvals(factory, profile, &tracked).await {
                    failures.push(format!("Approvals @{actor}: {error}"));
                }
            }
            if review {
                if let Err(error) = api.poll_reviews(factory, profile).await {
                    failures.push(format!("Reviews @{actor}: {error}"));
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            bail!("{}", failures.join("; "))
        }
    }

    // Legacy search accepts at most five Boolean operators. Keep large owner
    // configurations bounded without broadening any token's authorized scope.
    async fn scoped_search(&self, profile: &TokenProfile, query: &str) -> Result<Vec<Value>> {
        let mut rows = vec![];
        let mut seen = HashSet::new();
        let mut owners = profile.owners.clone();
        owners.sort_by_key(|owner| owner.to_ascii_lowercase());
        owners.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        for batch in owners.chunks(6) {
            let scopes = batch
                .iter()
                .map(|owner| format!("user:{owner}"))
                .collect::<Vec<_>>();
            let scope = if scopes.len() == 1 {
                scopes[0].clone()
            } else {
                format!("({})", scopes.join(" OR "))
            };
            for row in self.search(&format!("{query} {scope}")).await? {
                let repo = repository(&row)?;
                ensure!(
                    profile.allows_owner(repo.split('/').next().unwrap_or("")),
                    "Search returned repository outside token scope"
                );
                let number = row["number"]
                    .as_u64()
                    .filter(|number| *number > 0)
                    .context("Invalid search item number")?;
                if seen.insert((repo, number)) {
                    rows.push(row);
                }
            }
        }
        Ok(rows)
    }

    async fn poll_issues(&self, factory: &Factory, profile: &TokenProfile) -> Result<()> {
        let actor = &profile.account;
        let rows = self
            .scoped_search(profile, &format!("is:issue is:open author:{actor}"))
            .await?;
        for row in rows {
            if row.get("pull_request").is_some() {
                continue;
            }
            let repo = repository(&row)?;
            let n = row["number"].as_u64().context("Invalid issue number")?;
            if factory.store.event_consumed(&format!("issue:{repo}#{n}"))?
                || row["title"]
                    .as_str()
                    .is_some_and(|title| !title.starts_with("[AI]"))
                || row["user"]["login"]
                    .as_str()
                    .is_some_and(|author| !author.eq_ignore_ascii_case(actor))
            {
                continue;
            }
            if !factory.workflow_enabled_for("new_issue", actor)? {
                return Ok(());
            }
            let fresh = self
                .request(Method::GET, &format!("/repos/{repo}/issues/{n}"), None)
                .await?;
            if fresh.get("pull_request").is_some()
                || fresh["state"] != "open"
                || !fresh["title"].as_str().unwrap_or("").starts_with("[AI]")
                || !fresh["user"]["login"]
                    .as_str()
                    .is_some_and(|author| author.eq_ignore_ascii_case(actor))
            {
                continue;
            }
            let mut event = TriggerEvent::issue(
                &repo,
                n,
                fresh["title"].as_str().unwrap(),
                actor,
                fresh["body"].as_str().unwrap_or(""),
            );
            ensure!(
                fresh["html_url"] == event.url,
                "Issue URL identity mismatch"
            );
            event.payload = json!({"issue":fresh});
            factory.enqueue(event)?;
        }
        Ok(())
    }

    async fn poll_approvals(
        &self,
        factory: &Factory,
        profile: &TokenProfile,
        tracked: &[&TrackedPrOrigin],
    ) -> Result<()> {
        let actor = &profile.account;
        let origins: HashMap<_, _> = tracked
            .iter()
            .map(|origin| ((origin.repository.clone(), origin.number), *origin))
            .collect();
        let rows = self
            .scoped_search(profile, &format!("is:pr is:open author:{actor}"))
            .await?;
        let mut failures = vec![];
        for row in rows {
            let repo = repository(&row)?;
            let n = row["number"].as_u64().context("Invalid PR number")?;
            let Some(origin) = origins.get(&(repo.clone(), n)) else {
                continue;
            };
            let result = self.poll_approval(factory, origin).await;
            if let Err(error) = result {
                failures.push(format!("{repo}#{n}: {error}"));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            bail!("{}", failures.join("; "))
        }
    }

    async fn poll_approval(&self, factory: &Factory, origin: &TrackedPrOrigin) -> Result<()> {
        if !factory.workflow_enabled_for("pr_approved", &origin.actor)? {
            return Ok(());
        }
        let repo = &origin.repository;
        let n = origin.number;
        let pr = self.pr(repo, n).await?;
        ensure!(
            pr["user"]["login"]
                .as_str()
                .is_some_and(|actor| actor.eq_ignore_ascii_case(&origin.actor)),
            "Tracked PR author mismatch"
        );
        if pr["state"] != "open" {
            return Ok(());
        }
        let sha = pr["head"]["sha"].as_str().context("PR head missing")?;
        let reviews = self
            .pages(&format!("/repos/{repo}/pulls/{n}/reviews"))
            .await;
        let comments = self
            .pages(&format!("/repos/{repo}/issues/{n}/comments"))
            .await;
        let approved = reviews.as_ref().is_ok_and(|rows| {
            rows.iter().any(|review| {
                review["state"] == "APPROVED"
                    && review["commit_id"]
                        .as_str()
                        .is_some_and(|commit| commit.eq_ignore_ascii_case(sha))
            })
        });
        let mut fresh_comments = vec![];
        if let Ok(rows) = &comments {
            for comment in rows {
                if !comment["user"]["login"]
                    .as_str()
                    .is_some_and(|actor| actor.eq_ignore_ascii_case(&origin.actor))
                    || !comment["body"]
                        .as_str()
                        .unwrap_or("")
                        .trim()
                        .eq_ignore_ascii_case("ok")
                {
                    continue;
                }
                let Some(id) = comment["id"].as_u64() else {
                    continue;
                };
                if factory
                    .store
                    .observation(&format!("approval-comment:{repo}#{n}:{id}"))?
                    .is_none()
                {
                    fresh_comments.push(id);
                }
            }
        }
        if approved || !fresh_comments.is_empty() {
            let source = json!({"event_id":origin.event_id,"trigger_id":"new_issue","repository":origin.issue_repository,"number":origin.issue_number,"actor":origin.actor,"url":format!("https://github.com/{}/issues/{}",origin.issue_repository,origin.issue_number)});
            let mut approval = json!({"command":if fresh_comments.is_empty(){"review"}else{"ok"},"actor":origin.actor,"head_sha":sha});
            if let Some(id) = fresh_comments.first() {
                approval["comment_id"] = json!(id);
            }
            let event = pr_event(
                "pr_approved",
                format!("approval:{repo}#{n}@{sha}"),
                repo,
                n,
                &origin.actor,
                &pr,
                json!({"pull_request":pr,"source_issue":source,"approval":approval}),
            )?;
            factory.enqueue_approval(event, &fresh_comments, approved)?;
        }
        if reviews.is_err() && comments.is_err() {
            bail!("Both approval endpoints failed");
        }
        Ok(())
    }

    async fn poll_reviews(&self, factory: &Factory, profile: &TokenProfile) -> Result<()> {
        let reviewer = &profile.account;
        let rows = self
            .scoped_search(
                profile,
                &format!("is:pr is:open review-requested:{reviewer}"),
            )
            .await?;
        let mut candidates = vec![];
        for row in rows {
            if !factory.workflow_enabled_for("pr_review", reviewer)? {
                return Ok(());
            }
            let repo = repository(&row)?;
            let n = row["number"].as_u64().context("Invalid PR number")?;
            let pr = self.pr(&repo, n).await?;
            if pr["state"] != "open"
                || !pr["requested_reviewers"]
                    .as_array()
                    .is_some_and(|reviewers| {
                        reviewers.iter().any(|r| {
                            r["login"]
                                .as_str()
                                .is_some_and(|actor| actor.eq_ignore_ascii_case(reviewer))
                        })
                    })
            {
                continue;
            }
            candidates.push(pr_event(
                "pr_review",
                String::new(),
                &repo,
                n,
                reviewer,
                &pr,
                json!({"pull_request":pr,"reviewer":reviewer}),
            )?);
        }
        // Clear withdrawal edges only after the complete grouped scan succeeds.
        // Preserve the existing owner-specific durable observation keys.
        for owner in &profile.owners {
            let scoped: Vec<_> = candidates
                .iter()
                .filter(|event| {
                    event
                        .repository
                        .split('/')
                        .next()
                        .is_some_and(|value| value.eq_ignore_ascii_case(owner))
                })
                .cloned()
                .collect();
            factory.enqueue_review_scan(owner, reviewer, &scoped)?;
        }
        Ok(())
    }
}
