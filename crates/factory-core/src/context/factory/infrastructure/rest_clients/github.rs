use crate::*;
use anyhow::{bail, ensure, Context};
use reqwest::{Client, Method};
use serde_json::{json, Value};

mod merge_cleanup;
mod node_comments;
mod pr_updates;
mod version_bumps;

pub struct Github {
    client: Client,
    base: String,
    token: String,
}
impl Github {
    pub fn new(token: &str) -> Result<Self> {
        Self::with_base(token, "https://api.github.com")
    }
    pub fn with_base(token: &str, base: &str) -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .user_agent("AI-Factory/0.1")
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            base: base.trim_end_matches('/').into(),
            token: token.into(),
        })
    }
    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<reqwest::Response> {
        ensure!(
            path.starts_with('/') && !path.starts_with("//"),
            "Invalid API path"
        );
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(b) = body {
            req = req.json(b)
        }
        Ok(req.send().await?)
    }
    pub async fn request(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let r = self.send(method, path, body).await?;
        let status = r.status();
        if !status.is_success() {
            bail!(
                "GitHub API {} for {}",
                status.as_u16(),
                path.split('?').next().unwrap_or(path)
            );
        }
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        Ok(r.json().await?)
    }
    pub async fn verify_actor(&self, actor: &str) -> Result<Value> {
        let v = self.request(Method::GET, "/user", None).await?;
        ensure!(
            v["login"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(actor)),
            "API token account does not match actor {actor}"
        );
        Ok(v)
    }
    async fn pages(&self, path: &str) -> Result<Vec<Value>> {
        self.pages_guarded(path, &|| true).await
    }
    async fn pages_guarded(
        &self,
        path: &str,
        can_read: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<Value>> {
        let mut all = vec![];
        for page in 1..=100 {
            ensure!(can_read(), "GitHub request no longer allowed");
            let sep = if path.contains('?') { "&" } else { "?" };
            let v = self
                .request(
                    Method::GET,
                    &format!("{path}{sep}per_page=100&page={page}"),
                    None,
                )
                .await?;
            ensure!(can_read(), "GitHub request no longer allowed");
            let rows = v.as_array().context("Malformed GitHub list response")?;
            all.extend(rows.iter().cloned());
            if rows.len() < 100 {
                return Ok(all);
            }
        }
        bail!("GitHub pagination limit reached; incomplete response")
    }
    async fn search(&self, query: &str) -> Result<Vec<Value>> {
        let mut pending = vec![(0i64, chrono::Utc::now().timestamp())];
        let mut rows = vec![];
        let mut queries = 0;
        while let Some((lo, hi)) = pending.pop() {
            queries += 1;
            ensure!(queries <= 128, "GitHub search partition limit reached");
            let q = format!("{query} created:{lo}..{hi}"); // Search accepts ISO dates, not Unix timestamps.
            let stamp = |n| {
                chrono::DateTime::from_timestamp(n, 0)
                    .unwrap()
                    .format("%Y-%m-%dT%H:%M:%SZ")
                    .to_string()
            };
            let q = q.replace(
                &format!("created:{lo}..{hi}"),
                &format!("created:{}..{}", stamp(lo), stamp(hi)),
            );
            let url = reqwest::Url::parse_with_params(
                &format!("{}/search/issues", self.base),
                &[
                    ("q", q.as_str()),
                    ("per_page", "100"),
                    ("page", "1"),
                    ("sort", "created"),
                    ("order", "asc"),
                ],
            )?;
            let path = format!("/search/issues?{}", url.query().unwrap());
            let first = self.request(Method::GET, &path, None).await?;
            ensure!(
                first["incomplete_results"] == false,
                "GitHub returned incomplete search"
            );
            let count = first["total_count"]
                .as_u64()
                .context("Malformed GitHub search")?;
            if count >= 1000 {
                ensure!(lo < hi, "GitHub search limit reached within one second");
                let mid = (lo + hi) / 2;
                pending.push((lo, mid));
                pending.push((mid + 1, hi));
                continue;
            }
            let items = first["items"]
                .as_array()
                .context("Malformed search items")?;
            rows.extend(items.iter().cloned());
            for page in 2..=count.div_ceil(100) {
                let p = path.replace("page=1&", &format!("page={page}&"));
                let v = self.request(Method::GET, &p, None).await?;
                ensure!(v["incomplete_results"] == false, "Incomplete search page");
                rows.extend(
                    v["items"]
                        .as_array()
                        .context("Malformed search page")?
                        .iter()
                        .cloned(),
                );
            }
        }
        Ok(rows)
    }
    pub(crate) async fn poll(factory: &Factory) -> Result<()> {
        let s = factory.store.public_settings()?;
        let mut failures = vec![];
        for profile in &s.token_profiles {
            if !profile.allows_actor("new_issue") {
                continue;
            }
            let author = &profile.account;
            for owner in &profile.owners {
                let result = async {
                    if !factory.workflow_enabled_for("new_issue", author)? {
                        return Ok(());
                    }
                    let api = factory.github(&factory.store.token_for(author).await?)?;
                    api.verify_actor(author).await?;
                    for row in api
                        .search(&format!("is:issue is:open user:{owner} author:{author}"))
                        .await?
                    {
                        if row.get("pull_request").is_some() {
                            continue;
                        }
                        let repo = repository(&row)?;
                        if !repo.split('/').next().unwrap().eq_ignore_ascii_case(owner) {
                            continue;
                        }
                        let n = row["number"].as_u64().context("Invalid issue number")?;
                        let fresh = api
                            .request(Method::GET, &format!("/repos/{repo}/issues/{n}"), None)
                            .await?;
                        if fresh.get("pull_request").is_some()
                            || fresh["state"] != "open"
                            || !fresh["title"].as_str().unwrap_or("").starts_with("[AI]")
                            || !fresh["user"]["login"]
                                .as_str()
                                .is_some_and(|x| x.eq_ignore_ascii_case(author))
                        {
                            continue;
                        }
                        let mut event = TriggerEvent::issue(
                            &repo,
                            n,
                            fresh["title"].as_str().unwrap(),
                            author,
                            fresh["body"].as_str().unwrap_or(""),
                        );
                        ensure!(
                            fresh["html_url"] == event.url,
                            "Issue URL identity mismatch"
                        );
                        event.payload = json!({"issue":fresh});
                        factory.enqueue(event)?;
                    }
                    Ok::<(), anyhow::Error>(())
                }
                .await;
                if let Err(e) = result {
                    failures.push(format!("Issues {owner}/{author}: {e}"));
                }
            }
        }
        {
            for origin in factory.store.tracked_prs()? {
                let repo = origin.repository.clone();
                let n = origin.number;
                let result = async {
                    if !factory.workflow_enabled_for("pr_approved", &origin.actor)? {
                        return Ok(());
                    }
                    let profile = s
                        .profile_for(&origin.actor)
                        .context("Tracked source no longer authorized")?;
                    ensure!(
                        profile.allows_actor("pr_approved")
                            && profile.allows_owner(repo.split('/').next().unwrap()),
                        "Tracked source no longer authorized"
                    );
                    let api = factory.github(&factory.store.token_for(&origin.actor).await?)?;
                    api.verify_actor(&origin.actor).await?;
                    let pr = api.pr(&repo, n).await?;
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
                    let reviews = api.pages(&format!("/repos/{repo}/pulls/{n}/reviews")).await;
                    let comments = api.pages(&format!("/repos/{repo}/issues/{n}/comments")).await;
                    let approved = pr["state"] == "open"
                        && reviews.as_ref().is_ok_and(|rows| {
                            rows.iter().any(|review| {
                                review["state"] == "APPROVED"
                                    && review["commit_id"]
                                        .as_str()
                                        .is_some_and(|commit| commit.eq_ignore_ascii_case(sha))
                            })
                        });
                    let mut fresh_comments = Vec::new();
                    let mut signal = None;
                    if let Ok(rows) = &comments {
                        for comment in rows {
                            if !comment["user"]["login"]
                                .as_str()
                                .is_some_and(|actor| actor.eq_ignore_ascii_case(&origin.actor))
                            {
                                continue;
                            }
                            let body = comment["body"].as_str().unwrap_or("").trim();
                            let command = if body.eq_ignore_ascii_case("ok") {
                                "ok"
                            } else {
                                continue;
                            };
                            let Some(id) = comment["id"].as_u64() else {
                                continue;
                            };
                            if factory
                                .store
                                .observation(&format!("approval-comment:{repo}#{n}:{id}"))?
                                .is_some()
                            {
                                continue;
                            }
                            fresh_comments.push(id);
                            if signal.is_none() {
                                signal = Some((command, id));
                            }
                        }
                    }
                    if approved || signal.is_some() {
                        let command = signal.map_or("review", |(command, _)| command);
                        let source = json!({"event_id":origin.event_id,"trigger_id":"new_issue","repository":origin.issue_repository,"number":origin.issue_number,"actor":origin.actor,"url":format!("https://github.com/{}/issues/{}",origin.issue_repository,origin.issue_number)});
                        let mut approval = json!({"command":command,"actor":origin.actor,"head_sha":sha});
                        if let Some((_, id)) = signal {
                            approval["comment_id"] = json!(id);
                        }
                        let event = pr_event(
                            "pr_approved",
                            format!("approval:{repo}#{n}@{sha}"),
                            &repo,
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
                    Ok::<(), anyhow::Error>(())
                }
                .await;
                if let Err(e) = result {
                    failures.push(format!("Approval {repo}#{n}: {e}"));
                }
            }
        }
        {
            for profile in &s.token_profiles {
                if !profile.allows_actor("pr_review") {
                    continue;
                }
                let reviewer = &profile.account;
                for owner in &profile.owners {
                    let result = async {
                        if !factory.workflow_enabled_for("pr_review", reviewer)? {
                            return Ok(());
                        }
                        let api = factory.github(&factory.store.token_for(reviewer).await?)?;
                        api.verify_actor(reviewer).await?;
                        let rows = api
                            .search(&format!(
                                "is:pr is:open user:{owner} review-requested:{reviewer}"
                            ))
                            .await?;
                        let mut candidates = Vec::new();
                        for row in rows {
                            let repo = repository(&row)?;
                            if !repo.split('/').next().unwrap().eq_ignore_ascii_case(owner) {
                                continue;
                            }
                            let n = row["number"].as_u64().context("Invalid PR number")?;
                            let pr = api.pr(&repo, n).await?;
                            if pr["state"] != "open"
                                || !pr["requested_reviewers"].as_array().is_some_and(|rs| {
                                    rs.iter().any(|r| {
                                        r["login"]
                                            .as_str()
                                            .is_some_and(|v| v.eq_ignore_ascii_case(reviewer))
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
                        factory.enqueue_review_scan(owner, reviewer, &candidates)?;
                        Ok::<(), anyhow::Error>(())
                    }
                    .await;
                    if let Err(e) = result {
                        failures.push(format!("Review {owner}/{reviewer}: {e}"));
                    }
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            bail!("{}", failures.join("; "))
        }
    }
    pub async fn pr(&self, repo: &str, n: u64) -> Result<Value> {
        ensure!(valid_repository(repo) && n > 0, "Invalid PR identity");
        let pr = self
            .request(Method::GET, &format!("/repos/{repo}/pulls/{n}"), None)
            .await?;
        ensure!(
            pr["number"] == n
                && pr["base"]["repo"]["full_name"]
                    .as_str()
                    .is_some_and(|r| r.eq_ignore_ascii_case(repo))
                && pr["html_url"].as_str().is_some_and(
                    |u| u.eq_ignore_ascii_case(&format!("https://github.com/{repo}/pull/{n}"))
                ),
            "PR identity mismatch"
        );
        let sha = pr["head"]["sha"].as_str().context("Missing PR head")?;
        ensure!(
            sha.len() == 40 && sha.bytes().all(|c| c.is_ascii_hexdigit()),
            "Invalid PR head"
        );
        Ok(pr)
    }
    pub async fn publish_actions(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
        node_id: &str,
        workspace: &std::path::Path,
        result: &Value,
    ) -> Result<()> {
        let data = &result["data"];
        let action = |name| data.get(name).filter(|v| !v.is_null());
        ensure!(
            action("release").is_none(),
            "Release actions are no longer supported"
        );
        if action("bump_version").is_some() {
            ensure!(
                event.trigger_id == "pr_approved",
                "Version bump requires approved PR flow"
            );
            ensure!(
                node_id == VERSION_BUMP_NODE_ID,
                "Only the assigned version node can publish a bump"
            );
            ensure!(
                ["publish", "update_pr", "merge", "review", "comment"]
                    .iter()
                    .all(|key| action(key).is_none()),
                "bump_version cannot be combined with other actions"
            );
            ensure!(
                result["status"] == "succeeded",
                "Only succeeded results can publish a version bump"
            );
        }
        // Validate the entire request before any externally visible mutation.
        if let Some(update) = action("update_pr") {
            ensure!(
                event.trigger_id == "pr_approved",
                "Only approved flow can update PR"
            );
            ensure!(
                result["status"] == "succeeded",
                "Only succeeded results can update PR"
            );
            ensure!(
                update.as_object().is_some_and(|v| v.is_empty()),
                "update_pr must be an empty object; Factory selects the PR branch"
            );
            ensure!(
                [
                    "publish",
                    "merge",
                    "review",
                    "comment",
                    "release",
                    "bump_version"
                ]
                .iter()
                .all(|k| action(k).is_none()),
                "update_pr cannot be combined with other actions"
            );
        }
        if let Some(p) = action("publish") {
            ensure!(
                event.trigger_id == "new_issue",
                "Only issue flow can publish PR"
            );
            ensure!(
                result["status"] == "succeeded",
                "Only succeeded results can publish PR"
            );
            ensure!(
                p["title"].as_str().is_some_and(|s| !s.trim().is_empty()),
                "PR title required"
            );
            ensure!(
                closes_issue(
                    p["body"].as_str().context("PR body required")?,
                    event.number
                ),
                "PR body must close source issue"
            );
            ensure!(
                p.get("base").is_none_or(|v| v
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && !s.starts_with('-'))),
                "Invalid PR base"
            );
        }
        if let Some(m) = action("merge") {
            ensure!(
                event.trigger_id == "pr_approved",
                "Only approved flow can merge"
            );
            ensure!(
                result["status"] == "succeeded",
                "Only succeeded results can merge"
            );
            ensure!(
                m["method"].as_str().unwrap_or("squash") == "squash",
                "Only squash merge is supported"
            );
        }
        if let Some(r) = action("review") {
            ensure!(
                event.trigger_id == "pr_review",
                "Only review flow submits reviews"
            );
            let kind = r["event"].as_str().context("Review event required")?;
            ensure!(
                ["APPROVE", "REQUEST_CHANGES", "COMMENT"].contains(&kind),
                "Invalid review event"
            );
            ensure!(
                result["status"] == "succeeded"
                    || (result["status"] == "blocked" && kind != "APPROVE"),
                "Blocked result cannot approve PR"
            );
            ensure!(
                r["body"].as_str().is_some_and(|s| !s.trim().is_empty()),
                "Review body required"
            );
        }
        if let Some(c) = action("comment") {
            ensure!(
                c.as_str().is_some_and(|s| !s.trim().is_empty()),
                "Comment body required"
            );
            ensure!(
                result["status"] == "succeeded" || result["status"] == "blocked",
                "Failed result cannot comment"
            );
        }
        let check_active = || -> Result<()> {
            ensure!(!factory.stop.is_cancelled(), "Factory is stopping");
            let detail = factory.store.detail(run)?;
            ensure!(
                ["queued", "running"].contains(&detail.run.status.as_str()),
                "Run is already terminal"
            );
            Ok(())
        };
        check_active()?;
        self.verify_actor(&event.actor).await?;
        check_active()?;
        if action("update_pr").is_some() {
            self.update_pr(factory, event, run, workspace, &check_active)
                .await?;
        }
        if let Some(p) = action("publish") {
            let branch = super::super::processes::agents::git_output(
                factory,
                workspace,
                &["branch", "--show-current"],
                Some(&event.actor),
            )
            .await?;
            ensure!(
                branch == super::super::processes::agents::event_branch(event),
                "Cannot publish outside event branch"
            );
            let sha = super::super::processes::agents::git_output(
                factory,
                workspace,
                &["rev-parse", "HEAD"],
                Some(&event.actor),
            )
            .await?;
            let body = p["body"].as_str().unwrap();
            let rows = self
                .pages(&format!(
                    "/repos/{}/pulls?state=open&head={}:{}",
                    event.repository,
                    event.repository.split('/').next().unwrap(),
                    branch
                ))
                .await?;
            // A previous attempt may have created the PR but lost its response.
            // Adopt verified provenance before pushing a newly rebuilt retry workspace.
            let n = if let Some(pr) = rows.first() {
                let n = pr["number"].as_u64().context("No existing PR number")?;
                let pr = self.pr(&event.repository, n).await?;
                verify_published_pr(&pr, event, &branch)?;
                n
            } else {
                // Agents may return a guessed or stale base (often "main").
                // Resolve the repository's actual principal branch before pushing.
                let base = self.repository_default(&event.repository).await?;
                check_active()?;
                super::super::processes::agents::git_output(
                    factory,
                    workspace,
                    &["push", "origin", &format!("HEAD:refs/heads/{branch}")],
                    Some(&event.actor),
                )
                .await?;
                check_active()?;
                let pr = self.request(Method::POST, &format!("/repos/{}/pulls", event.repository), Some(&json!({"title":p["title"],"body":body,"head":branch,"base":base,"draft":false}))).await?;
                let n = pr["number"].as_u64().context("No created PR number")?;
                let pr = self.pr(&event.repository, n).await?;
                verify_published_pr(&pr, event, &branch)?;
                ensure!(pr["head"]["sha"] == sha, "Published PR head mismatch");
                n
            };
            factory.store.track_pr(&event.repository, n, run)?;
        }
        if action("merge").is_some() {
            ensure!(
                factory
                    .store
                    .observation(&format!("pr-update:{run}"))?
                    .is_none(),
                "PR updated during this run; new approval required before merge"
            );
            self.merge_and_cleanup(event, &check_active).await?;
            factory.store.observe(
                &format!("pr-merge:{run}"),
                &json!({"repository":event.repository,"number":event.number,"head_sha":event.head_sha}),
            )?;
        }
        if let Some(bump) = action("bump_version") {
            self.publish_version_bump(factory, event, run, workspace, bump, &check_active)
                .await?;
        }
        for kind in ["review", "comment"] {
            let Some(value) = action(kind) else { continue };
            use sha2::{Digest, Sha256};
            let key = format!(
                "action:{:x}",
                Sha256::digest(serde_json::to_vec(&json!([
                    event.event_id,
                    event.actor.to_lowercase(),
                    node_id,
                    kind,
                    value
                ]))?)
            );
            let marker = format!("<!-- ai-factory:{key} -->");
            if factory.store.observation(&key)?.is_some() {
                continue;
            }
            let path = if kind == "review" {
                let current = self.pr(&event.repository, event.number).await?;
                ensure!(
                    current["head"]["sha"] == event.head_sha && current["state"] == "open",
                    "PR changed since review request"
                );
                format!("/repos/{}/pulls/{}/reviews", event.repository, event.number)
            } else {
                format!(
                    "/repos/{}/issues/{}/comments",
                    event.repository, event.number
                )
            };
            let wanted_state = match value["event"].as_str().unwrap_or("") {
                "APPROVE" => "APPROVED",
                "REQUEST_CHANGES" => "CHANGES_REQUESTED",
                _ => "COMMENTED",
            };
            let existing = self.pages(&path).await?.into_iter().find(|row| {
                row["body"]
                    .as_str()
                    .is_some_and(|body| body.contains(&marker))
                    && row["user"]["login"]
                        .as_str()
                        .is_some_and(|a| a.eq_ignore_ascii_case(&event.actor))
                    && (kind != "review"
                        || (row["commit_id"] == event.head_sha && row["state"] == wanted_state))
            });
            if let Some(receipt) = existing {
                factory.store.record_action_receipt(&key, &receipt)?;
                continue;
            }
            check_active()?;
            let body = if kind == "review" {
                value["body"].as_str().unwrap()
            } else {
                value.as_str().unwrap()
            };
            let body = format!("{body}\n\n{marker}");
            let payload = if kind == "review" {
                json!({"commit_id":event.head_sha,"event":value["event"],"body":body})
            } else {
                json!({"body":body})
            };
            let receipt = self.request(Method::POST, &path, Some(&payload)).await?;
            factory.store.record_action_receipt(&key, &receipt)?;
        }
        Ok(())
    }
}
fn repository(row: &Value) -> Result<String> {
    let url = row["repository_url"]
        .as_str()
        .context("Missing repository URL")?;
    let repo = url
        .split("/repos/")
        .nth(1)
        .context("Invalid repository URL")?
        .to_lowercase();
    ensure!(valid_repository(&repo), "Invalid repository identity");
    Ok(repo)
}
fn pr_event(
    trigger: &str,
    event_id: String,
    repo: &str,
    n: u64,
    actor: &str,
    pr: &Value,
    payload: Value,
) -> Result<TriggerEvent> {
    Ok(TriggerEvent {
        event_id,
        trigger_id: trigger.into(),
        repository: repo.into(),
        number: n,
        title: pr["title"].as_str().context("PR title missing")?.into(),
        body: pr["body"].as_str().unwrap_or("").into(),
        actor: actor.into(),
        url: format!("https://github.com/{repo}/pull/{n}"),
        head_sha: pr["head"]["sha"]
            .as_str()
            .context("PR head missing")?
            .into(),
        payload,
    })
}

fn closes_issue(body: &str, number: u64) -> bool {
    body.to_lowercase().split("closes #").skip(1).any(|tail| {
        let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<u64>().ok() == Some(number)
    })
}
fn verify_published_pr(pr: &Value, event: &TriggerEvent, branch: &str) -> Result<()> {
    ensure!(
        pr["state"] == "open"
            && pr["user"]["login"]
                .as_str()
                .is_some_and(|a| a.eq_ignore_ascii_case(&event.actor))
            && pr["head"]["ref"] == branch
            && closes_issue(pr["body"].as_str().unwrap_or(""), event.number),
        "Published PR provenance mismatch"
    );
    Ok(())
}
