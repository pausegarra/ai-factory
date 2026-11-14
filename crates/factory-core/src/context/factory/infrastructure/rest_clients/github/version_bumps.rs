use super::{
    merge_cleanup::{ref_path, reference_sha, valid_branch},
    Github,
};
use crate::{
    context::factory::infrastructure::processes::agents::git_output, Factory, Result, TriggerEvent,
};
use anyhow::{ensure, Context};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VersionWorkspace {
    repository: String,
    number: u64,
    head_sha: String,
    base_ref: String,
    base_sha: String,
    merge_sha: String,
    version_branch: String,
}

impl VersionWorkspace {
    fn matches(&self, event: &TriggerEvent) -> Result<()> {
        ensure!(
            self.repository == event.repository
                && self.number == event.number
                && self.head_sha == event.head_sha,
            "Version workspace identity mismatch"
        );
        ensure!(
            valid_branch(&self.base_ref) && valid_branch(&self.version_branch),
            "Invalid version branch"
        );
        validate_sha(&self.base_sha)?;
        validate_sha(&self.merge_sha)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VersionAction {
    version: String,
    bump: String,
    reason: String,
}

impl VersionAction {
    fn parse(value: &Value) -> Result<Self> {
        let action: Self =
            serde_json::from_value(value.clone()).context("Invalid version action")?;
        let version = &action.version;
        let components: Vec<_> = version.split('.').collect();
        ensure!(
            components.len() == 3
                && components.iter().all(|part| !part.is_empty()
                    && part.bytes().all(|b| b.is_ascii_digit())
                    && (part.len() == 1 || !part.starts_with('0'))
                    && part.parse::<u64>().is_ok()),
            "Version must be a stable semantic version"
        );
        ensure!(
            matches!(action.bump.as_str(), "major" | "minor" | "patch"),
            "Invalid version bump classification"
        );
        ensure!(
            !action.reason.trim().is_empty() && action.reason.len() <= 16_384,
            "Version bump reason required"
        );
        Ok(action)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VersionIntent {
    context: VersionWorkspace,
    action: VersionAction,
    target_sha: String,
}

#[derive(Debug, PartialEq, Eq)]
enum PushState {
    Unused,
    Applied,
}

fn version_active(factory: &Factory, run: &str) -> Result<()> {
    ensure!(!factory.stop.is_cancelled(), "Factory stopped");
    ensure!(
        ["queued", "running"].contains(&factory.store.detail(run)?.run.status.as_str()),
        "Version run no longer active"
    );
    Ok(())
}

fn validate_sha(sha: &str) -> Result<()> {
    ensure!(
        sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid version commit SHA"
    );
    Ok(())
}

impl Github {
    async fn version_authorized(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
    ) -> Result<()> {
        ensure!(
            event.trigger_id == "pr_approved",
            "Version bump requires approved PR flow"
        );
        ensure!(
            crate::valid_repository(&event.repository),
            "Invalid repository identity"
        );
        validate_sha(&event.head_sha)?;
        version_active(factory, run)?;
        let current = factory.store.detail(run)?;
        ensure!(
            current.event.trigger_id == "pr_approved"
                && current.event.payload["approval"] == event.payload["approval"],
            "Stored run lacks matching approval"
        );
        ensure!(
            current.event.event_id == event.event_id
                && current.event.repository == event.repository
                && current.event.number == event.number
                && current.event.head_sha == event.head_sha
                && current.event.actor.eq_ignore_ascii_case(&event.actor),
            "Version run event identity mismatch"
        );
        let settings = factory.store.public_settings()?;
        let profile = settings
            .profile_for(&event.actor)
            .context("Version actor no longer authorized")?;
        ensure!(
            profile.allows_actor("pr_approved")
                && profile.allows_owner(event.repository.split('/').next().unwrap()),
            "Version actor or repository no longer authorized"
        );
        let receipt = factory
            .store
            .observation(&format!("pr-merge:{run}"))?
            .context("Current run merge receipt required for version")?;
        ensure!(
            receipt["repository"] == event.repository
                && receipt["number"] == event.number
                && receipt["head_sha"] == event.head_sha,
            "Version merge receipt identity mismatch"
        );
        self.verify_actor(&event.actor).await?;
        Ok(())
    }

    async fn merged_version_context(
        &self,
        event: &TriggerEvent,
    ) -> Result<(String, String, String)> {
        let pr = self.pr(&event.repository, event.number).await?;
        ensure!(pr["merged"] == true, "Version requires confirmed merged PR");
        ensure!(
            pr["head"]["sha"] == event.head_sha,
            "PR head changed before version"
        );
        ensure!(
            pr["head"]["repo"]["full_name"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(&event.repository)),
            "Version requires same-repository PR head"
        );
        let default = self.repository_default(&event.repository).await?;
        ensure!(
            pr["base"]["ref"] == default
                && pr["base"]["repo"]["full_name"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&event.repository)),
            "Version requires same-repository default base"
        );
        let merge = pr["merge_commit_sha"]
            .as_str()
            .context("Missing PR merge commit SHA")?
            .to_owned();
        validate_sha(&merge)?;
        let reference = self
            .request(
                Method::GET,
                &ref_path(&event.repository, &default, false),
                None,
            )
            .await?;
        let base = reference_sha(&reference, &default)?;
        Ok((default, base, merge))
    }

    async fn version_ancestor(&self, repo: &str, base: &str, head: &str) -> Result<bool> {
        if base == head {
            return Ok(true);
        }
        let response = self
            .send(
                Method::GET,
                &format!("/repos/{repo}/compare/{base}...{head}"),
                None,
            )
            .await?;
        // A rejected push may leave its local commit unavailable remotely.
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(false);
        }
        ensure!(
            response.status() == StatusCode::OK,
            "Cannot verify version commit ancestry"
        );
        let comparison: Value = response.json().await?;
        ensure!(
            comparison["base_commit"]["sha"] == base,
            "Version comparison identity mismatch"
        );
        Ok(comparison["merge_base_commit"]["sha"] == base
            && matches!(comparison["status"].as_str(), Some("ahead" | "identical")))
    }

    async fn version_push_state(&self, intent: &VersionIntent) -> Result<PushState> {
        let context = &intent.context;
        let branch = self
            .request(
                Method::GET,
                &ref_path(&context.repository, &context.base_ref, false),
                None,
            )
            .await?;
        let sha = reference_sha(&branch, &context.base_ref)?;
        if self
            .version_ancestor(&context.repository, &intent.target_sha, &sha)
            .await?
        {
            return Ok(PushState::Applied);
        }
        ensure!(
            self.version_ancestor(&context.repository, &context.base_sha, &sha)
                .await?,
            "Default branch diverged from prepared version base"
        );
        // Default advanced without this commit: retry must recompute from its new version.
        Ok(PushState::Unused)
    }

    async fn finish_version(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
        intent: &VersionIntent,
        check_active: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Value> {
        check_active()?;
        ensure!(
            self.version_push_state(intent).await? == PushState::Applied,
            "Version push not confirmed"
        );
        let receipt = json!({"repository":event.repository,"number":event.number,"head_sha":event.head_sha,
            "merge_sha":intent.context.merge_sha,"base_ref":intent.context.base_ref,"commit_sha":intent.target_sha,
            "version":intent.action.version,"bump":intent.action.bump,"reason":intent.action.reason});
        factory
            .store
            .observe(&format!("version-result:{}", event.event_id), &receipt)?;
        factory
            .store
            .observe(&format!("pr-version:{run}"), &receipt)?;
        Ok(receipt)
    }

    fn recorded_version(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
    ) -> Result<Option<Value>> {
        let Some(receipt) = factory
            .store
            .observation(&format!("version-result:{}", event.event_id))?
        else {
            return Ok(None);
        };
        ensure!(
            receipt["repository"] == event.repository
                && receipt["number"] == event.number
                && receipt["head_sha"] == event.head_sha,
            "Recorded version identity mismatch"
        );
        factory
            .store
            .observe(&format!("pr-version:{run}"), &receipt)?;
        Ok(Some(receipt))
    }

    pub async fn prepare_version_workspace(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
        workspace: &Path,
    ) -> Result<Value> {
        self.version_authorized(factory, event, run).await?;
        let (base_ref, base_sha, merge_sha) = self.merged_version_context(event).await?;
        if let Some(receipt) = self.recorded_version(factory, event, run)? {
            return Ok(json!({"completed":true,"receipt":receipt}));
        }
        let key = format!("version-intent:{}", event.event_id);
        if let Some(value) = factory.store.observation(&key)?.filter(|v| !v.is_null()) {
            let intent: VersionIntent =
                serde_json::from_value(value).context("Invalid durable version intent")?;
            intent.context.matches(event)?;
            ensure!(
                intent.context.base_ref == base_ref && intent.context.merge_sha == merge_sha,
                "Version PR identity changed since intent"
            );
            match self.version_push_state(&intent).await? {
                PushState::Applied => {
                    let active = || version_active(factory, run);
                    let receipt = self
                        .finish_version(factory, event, run, &intent, &active)
                        .await?;
                    return Ok(json!({"completed":true,"receipt":receipt}));
                }
                PushState::Unused => {
                    factory.store.observe(&key, &Value::Null)?;
                }
            }
        }
        ensure!(
            self.version_ancestor(&event.repository, &merge_sha, &base_sha)
                .await?,
            "Default branch does not contain approved PR merge"
        );
        ensure!(
            git_output(
                factory,
                workspace,
                &["status", "--porcelain", "--untracked-files=all"],
                None
            )
            .await?
            .is_empty(),
            "Version workspace must be clean"
        );
        let remote = format!("https://github.com/{}.git", event.repository);
        let tracking = format!("refs/remotes/origin/{base_ref}");
        git_output(
            factory,
            workspace,
            &[
                "fetch",
                &remote,
                &format!("+refs/heads/{base_ref}:{tracking}"),
            ],
            Some(&event.actor),
        )
        .await?;
        ensure!(
            git_output(
                factory,
                workspace,
                &["rev-parse", "--verify", &format!("{tracking}^{{commit}}")],
                None
            )
            .await?
                == base_sha,
            "Fetched version base differs from verified GitHub ref"
        );
        let version_branch = format!("ai-factory/version/{}", uuid::Uuid::new_v4());
        git_output(
            factory,
            workspace,
            &["checkout", "-b", &version_branch, &base_sha],
            None,
        )
        .await?;
        let context = VersionWorkspace {
            repository: event.repository.clone(),
            number: event.number,
            head_sha: event.head_sha.clone(),
            base_ref,
            base_sha,
            merge_sha,
            version_branch,
        };
        let mut value = serde_json::to_value(&context)?;
        value["completed"] = false.into();
        factory
            .store
            .observe(&format!("version-workspace:{run}"), &value)?;
        Ok(value)
    }

    pub async fn publish_version_bump(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
        workspace: &Path,
        data: &Value,
        check_active: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<()> {
        self.version_authorized(factory, event, run).await?;
        let (base_ref, base_sha, merge_sha) = self.merged_version_context(event).await?;
        if self.recorded_version(factory, event, run)?.is_some() {
            return Ok(());
        }
        let action = VersionAction::parse(data)?;
        let context: VersionWorkspace = serde_json::from_value(
            factory
                .store
                .observation(&format!("version-workspace:{run}"))?
                .context("Prepared version workspace required")?,
        )?;
        context.matches(event)?;
        ensure!(
            base_ref == context.base_ref && merge_sha == context.merge_sha,
            "PR merge identity changed before version"
        );
        let intent_key = format!("version-intent:{}", event.event_id);
        if let Some(value) = factory
            .store
            .observation(&intent_key)?
            .filter(|v| !v.is_null())
        {
            let intent: VersionIntent = serde_json::from_value(value)?;
            intent.context.matches(event)?;
            ensure!(
                intent.context.base_ref == base_ref && intent.context.merge_sha == merge_sha,
                "Version intent identity mismatch"
            );
            ensure!(
                self.version_push_state(&intent).await? == PushState::Applied,
                "Prepare version retry before publishing unused intent"
            );
            self.finish_version(factory, event, run, &intent, check_active)
                .await?;
            return Ok(());
        }
        ensure!(
            base_sha == context.base_sha,
            "Default branch moved before version"
        );
        ensure!(
            git_output(factory, workspace, &["branch", "--show-current"], None).await?
                == context.version_branch,
            "Version must use prepared isolated branch"
        );
        ensure!(
            git_output(
                factory,
                workspace,
                &["status", "--porcelain", "--untracked-files=all"],
                None
            )
            .await?
            .is_empty(),
            "Commit version files before publishing version"
        );
        let target_sha = git_output(
            factory,
            workspace,
            &["rev-parse", "--verify", "HEAD^{commit}"],
            None,
        )
        .await?;
        validate_sha(&target_sha)?;
        if target_sha != context.base_sha {
            let parents = git_output(
                factory,
                workspace,
                &["rev-list", "--parents", "-n", "1", "HEAD"],
                None,
            )
            .await?;
            let parents: Vec<_> = parents.split_whitespace().collect();
            ensure!(
                parents == [target_sha.as_str(), context.base_sha.as_str()],
                "Version requires one version commit directly after prepared base"
            );
        }
        let intent = VersionIntent {
            context,
            action,
            target_sha,
        };
        factory
            .store
            .observe(&intent_key, &serde_json::to_value(&intent)?)?;
        check_active()?;
        if intent.target_sha == intent.context.base_sha {
            self.finish_version(factory, event, run, &intent, check_active)
                .await?;
            return Ok(());
        }
        let remote = format!("https://github.com/{}.git", event.repository);
        // Exact lease and direct-parent validation prevent overwriting concurrent work.
        let push = git_output(
            factory,
            workspace,
            &[
                "push",
                "--no-follow-tags",
                &format!(
                    "--force-with-lease=refs/heads/{}:{}",
                    intent.context.base_ref, intent.context.base_sha
                ),
                &remote,
                &format!(
                    "{}:refs/heads/{}",
                    intent.target_sha, intent.context.base_ref
                ),
            ],
            Some(&event.actor),
        )
        .await;
        match self.version_push_state(&intent).await {
            Ok(PushState::Applied) => {
                self.finish_version(factory, event, run, &intent, check_active)
                    .await?;
                Ok(())
            }
            state => {
                let error = match state {
                    Err(error) => error,
                    _ => anyhow::anyhow!("Version push not applied"),
                };
                Err(match push {
                    Err(push) => error.context(format!("Version push failed: {push:#}")),
                    Ok(_) => error,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_action_requires_stable_semver_bump_evidence_without_release_fields() {
        let good = json!({"version":"1.2.3","bump":"patch","reason":"Fixes retry bug"});
        assert!(VersionAction::parse(&good).is_ok());
        for (key, value) in [
            ("version", "v1.2.3"),
            ("version", "1.2.3-rc.1"),
            ("version", "01.2.3"),
            ("bump", "auto"),
            ("reason", " "),
            ("tag", "v1.2.3"),
        ] {
            let mut bad = good.clone();
            bad[key] = value.into();
            assert!(VersionAction::parse(&bad).is_err(), "accepted {bad}");
        }
    }
}
