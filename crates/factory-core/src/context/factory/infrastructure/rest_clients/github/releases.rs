use super::{
    merge_cleanup::{ref_path, reference_sha, valid_branch},
    Github,
};
use crate::{
    context::factory::infrastructure::processes::agents::git_output, Factory, Result, TriggerEvent,
};
use anyhow::{bail, ensure, Context};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ReleaseWorkspace {
    repository: String,
    number: u64,
    head_sha: String,
    base_ref: String,
    base_sha: String,
    merge_sha: String,
    release_branch: String,
}

impl ReleaseWorkspace {
    fn matches(&self, event: &TriggerEvent) -> Result<()> {
        ensure!(
            self.repository == event.repository
                && self.number == event.number
                && self.head_sha == event.head_sha,
            "Release workspace identity mismatch"
        );
        ensure!(
            valid_branch(&self.base_ref) && valid_branch(&self.release_branch),
            "Invalid release branch"
        );
        validate_sha(&self.base_sha)?;
        validate_sha(&self.merge_sha)?;
        ensure!(
            self.base_sha == self.merge_sha,
            "Release base differs from merge commit"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ReleaseAction {
    tag: String,
    title: String,
    body: String,
    bump: String,
    reason: String,
}

impl ReleaseAction {
    fn parse(value: &Value) -> Result<Self> {
        let action: Self =
            serde_json::from_value(value.clone()).context("Invalid release action")?;
        let version = action.tag.strip_prefix('v').unwrap_or(&action.tag);
        let components: Vec<_> = version.split('.').collect();
        ensure!(
            components.len() == 3
                && components.iter().all(|part| !part.is_empty()
                    && part.bytes().all(|b| b.is_ascii_digit())
                    && (part.len() == 1 || !part.starts_with('0'))
                    && part.parse::<u64>().is_ok()),
            "Release tag must be a stable semantic version"
        );
        ensure!(valid_branch(&action.tag), "Invalid release tag");
        ensure!(
            matches!(action.bump.as_str(), "major" | "minor" | "patch"),
            "Invalid release bump classification"
        );
        ensure!(
            !action.reason.trim().is_empty() && action.reason.len() <= 16_384,
            "Release bump reason required"
        );
        ensure!(
            !action.title.trim().is_empty() && action.title.len() <= 1024,
            "Release title required"
        );
        ensure!(action.body.len() <= 125_000, "Release body too large");
        Ok(action)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ReleaseIntent {
    context: ReleaseWorkspace,
    action: ReleaseAction,
    target_sha: String,
    tag_object_sha: String,
}

#[derive(Debug, PartialEq, Eq)]
enum PushState {
    Unused,
    Applied,
}

fn push_state(
    branch: &str,
    tag: Option<(&str, &str)>,
    base: &str,
    target: &str,
    tag_object: &str,
) -> Result<PushState> {
    if branch == target && tag == Some((tag_object, target)) {
        return Ok(PushState::Applied);
    }
    if branch == base && tag.is_none() {
        return Ok(PushState::Unused);
    }
    bail!("Release refs are partial or divergent; preserving remote refs")
}

fn release_active(factory: &Factory, run: &str) -> Result<()> {
    ensure!(!factory.stop.is_cancelled(), "Factory stopped");
    ensure!(
        ["queued", "running"].contains(&factory.store.detail(run)?.run.status.as_str()),
        "Release run no longer active"
    );
    Ok(())
}

fn validate_sha(sha: &str) -> Result<()> {
    ensure!(
        sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid release commit SHA"
    );
    Ok(())
}

impl Github {
    async fn release_authorized(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
    ) -> Result<()> {
        ensure!(
            event.release_requested(),
            "Explicit release authorization required"
        );
        ensure!(
            crate::valid_repository(&event.repository),
            "Invalid repository identity"
        );
        validate_sha(&event.head_sha)?;
        release_active(factory, run)?;
        let current = factory.store.detail(run)?;
        ensure!(
            current.event.release_requested()
                && current.event.payload["approval"] == event.payload["approval"],
            "Stored run lacks matching explicit release authorization"
        );
        ensure!(
            current.event.event_id == event.event_id
                && current.event.repository == event.repository
                && current.event.number == event.number
                && current.event.head_sha == event.head_sha
                && current.event.actor.eq_ignore_ascii_case(&event.actor),
            "Release run event identity mismatch"
        );
        let settings = factory.store.public_settings()?;
        let profile = settings
            .profile_for(&event.actor)
            .context("Release actor no longer authorized")?;
        ensure!(
            profile.allows_actor("pr_approved")
                && profile.allows_owner(event.repository.split('/').next().unwrap()),
            "Release actor or repository no longer authorized"
        );
        let receipt = factory
            .store
            .observation(&format!("pr-merge:{run}"))?
            .context("Current run merge receipt required for release")?;
        ensure!(
            receipt["repository"] == event.repository
                && receipt["number"] == event.number
                && receipt["head_sha"] == event.head_sha,
            "Release merge receipt identity mismatch"
        );
        self.verify_actor(&event.actor).await?;
        Ok(())
    }

    async fn merged_release_context(
        &self,
        event: &TriggerEvent,
    ) -> Result<(String, String, String)> {
        let pr = self.pr(&event.repository, event.number).await?;
        ensure!(pr["merged"] == true, "Release requires confirmed merged PR");
        ensure!(
            pr["head"]["sha"] == event.head_sha,
            "PR head changed before release"
        );
        ensure!(
            pr["head"]["repo"]["full_name"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(&event.repository)),
            "Release requires same-repository PR head"
        );
        let default = self.repository_default(&event.repository).await?;
        ensure!(
            pr["base"]["ref"] == default
                && pr["base"]["repo"]["full_name"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&event.repository)),
            "Release requires same-repository default base"
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

    async fn release_tag(&self, repo: &str, tag: &str) -> Result<Option<(String, String)>> {
        // ReleaseAction restricts tags to ASCII semver; no path encoding ambiguity.
        let path = format!("/repos/{repo}/git/ref/tags/{tag}");
        let response = self.send(Method::GET, &path, None).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        ensure!(
            response.status() == StatusCode::OK,
            "GitHub API {} for {path}",
            response.status().as_u16()
        );
        let value: Value = response.json().await?;
        ensure!(
            value["ref"] == format!("refs/tags/{tag}"),
            "Release tag identity mismatch"
        );
        let original = value["object"]["sha"]
            .as_str()
            .context("Missing release tag SHA")?
            .to_owned();
        validate_sha(&original)?;
        let mut object = value["object"].clone();
        for _ in 0..8 {
            let sha = object["sha"]
                .as_str()
                .context("Missing release tag object SHA")?;
            validate_sha(sha)?;
            match object["type"].as_str() {
                Some("commit") => return Ok(Some((original, sha.into()))),
                Some("tag") => {
                    let annotation = self
                        .request(Method::GET, &format!("/repos/{repo}/git/tags/{sha}"), None)
                        .await?;
                    ensure!(
                        annotation["sha"] == sha,
                        "Annotated release tag identity mismatch"
                    );
                    object = annotation["object"].clone();
                }
                _ => bail!("Release tag does not reference a commit"),
            }
        }
        bail!("Release tag nesting limit reached")
    }

    async fn release_push_state(&self, intent: &ReleaseIntent) -> Result<PushState> {
        let context = &intent.context;
        let branch = self
            .request(
                Method::GET,
                &ref_path(&context.repository, &context.base_ref, false),
                None,
            )
            .await?;
        let sha = reference_sha(&branch, &context.base_ref)?;
        let tag = self
            .release_tag(&context.repository, &intent.action.tag)
            .await?;
        let tag_pair = tag
            .as_ref()
            .map(|(object, commit)| (object.as_str(), commit.as_str()));
        if sha != intent.target_sha
            && tag_pair == Some((intent.tag_object_sha.as_str(), intent.target_sha.as_str()))
        {
            // A later PR may advance default after this atomic push. The exact
            // durable annotation still proves our publication, provided the
            // current default retains the release commit in its history.
            let comparison = self
                .request(
                    Method::GET,
                    &format!(
                        "/repos/{}/compare/{}...{}",
                        context.repository, intent.target_sha, sha
                    ),
                    None,
                )
                .await?;
            ensure!(
                comparison["base_commit"]["sha"] == intent.target_sha
                    && comparison["merge_base_commit"]["sha"] == intent.target_sha
                    && matches!(comparison["status"].as_str(), Some("ahead" | "identical")),
                "Default branch diverged from published release commit"
            );
            return Ok(PushState::Applied);
        }
        push_state(
            &sha,
            tag_pair,
            &context.base_sha,
            &intent.target_sha,
            &intent.tag_object_sha,
        )
    }

    async fn existing_release(&self, intent: &ReleaseIntent) -> Result<Option<Value>> {
        let path = format!(
            "/repos/{}/releases/tags/{}",
            intent.context.repository, intent.action.tag
        );
        let response = self.send(Method::GET, &path, None).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        ensure!(
            response.status() == StatusCode::OK,
            "GitHub API {} for {path}",
            response.status().as_u16()
        );
        let value: Value = response.json().await?;
        ensure!(
            value["tag_name"] == intent.action.tag
                && value["draft"] == false
                && value["prerelease"] == false,
            "Existing GitHub release identity or publication state mismatch"
        );
        Ok(Some(value))
    }

    async fn finish_release(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
        intent: &ReleaseIntent,
        check_active: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Value> {
        ensure!(
            self.release_push_state(intent).await? == PushState::Applied,
            "Release atomic push not confirmed"
        );
        let release = if let Some(value) = self.existing_release(intent).await? {
            value
        } else {
            check_active()?;
            let action = &intent.action;
            let creation = self.request(Method::POST, &format!("/repos/{}/releases", event.repository),
                Some(&json!({"tag_name":action.tag,"target_commitish":intent.target_sha,"name":action.title,
                    "body":action.body,"draft":false,"prerelease":false}))).await;
            // Even a successful POST is followed by a read: lost responses never
            // cause a second POST in this attempt.
            match self.existing_release(intent).await? {
                Some(value) => value,
                None => {
                    return Err(creation.err().unwrap_or_else(|| {
                        anyhow::anyhow!("GitHub release creation not confirmed")
                    }))
                }
            }
        };
        // Publication and refs must agree before the durable result is recorded.
        ensure!(
            self.release_push_state(intent).await? == PushState::Applied,
            "Release refs changed during publication"
        );
        let receipt = json!({"repository":event.repository,"number":event.number,"head_sha":event.head_sha,
            "merge_sha":intent.context.merge_sha,"base_ref":intent.context.base_ref,"commit_sha":intent.target_sha,
            "tag":intent.action.tag,"bump":intent.action.bump,"reason":intent.action.reason,
            "release_id":release["id"],"release_url":release["html_url"]});
        factory
            .store
            .observe(&format!("release-result:{}", event.event_id), &receipt)?;
        factory
            .store
            .observe(&format!("pr-release:{run}"), &receipt)?;
        Ok(receipt)
    }

    fn recorded_release(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
    ) -> Result<Option<Value>> {
        let Some(receipt) = factory
            .store
            .observation(&format!("release-result:{}", event.event_id))?
        else {
            return Ok(None);
        };
        ensure!(
            receipt["repository"] == event.repository
                && receipt["number"] == event.number
                && receipt["head_sha"] == event.head_sha,
            "Recorded release identity mismatch"
        );
        factory
            .store
            .observe(&format!("pr-release:{run}"), &receipt)?;
        Ok(Some(receipt))
    }

    pub async fn prepare_release_workspace(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
        workspace: &Path,
    ) -> Result<Value> {
        self.release_authorized(factory, event, run).await?;
        let (base_ref, base_sha, merge_sha) = self.merged_release_context(event).await?;
        if let Some(receipt) = self.recorded_release(factory, event, run)? {
            return Ok(json!({"completed":true,"receipt":receipt}));
        }
        let key = format!("release-intent:{}", event.event_id);
        if let Some(value) = factory.store.observation(&key)?.filter(|v| !v.is_null()) {
            let intent: ReleaseIntent =
                serde_json::from_value(value).context("Invalid durable release intent")?;
            intent.context.matches(event)?;
            ensure!(
                intent.context.base_ref == base_ref && intent.context.merge_sha == merge_sha,
                "Release PR identity changed since intent"
            );
            match self.release_push_state(&intent).await? {
                PushState::Applied => {
                    let active = || release_active(factory, run);
                    let receipt = self
                        .finish_release(factory, event, run, &intent, &active)
                        .await?;
                    return Ok(json!({"completed":true,"receipt":receipt}));
                }
                PushState::Unused => {
                    // A failed atomic push left neither ref behind. Remove only
                    // our exact local annotated tag before preparing another try.
                    let local = git_output(
                        factory,
                        workspace,
                        &[
                            "show-ref",
                            "--verify",
                            "--hash",
                            &format!("refs/tags/{}", intent.action.tag),
                        ],
                        None,
                    )
                    .await;
                    if let Ok(local) = local {
                        ensure!(
                            local == intent.tag_object_sha,
                            "Local release tag conflicts with unused intent"
                        );
                        git_output(factory, workspace, &["tag", "-d", &intent.action.tag], None)
                            .await?;
                    }
                    factory.store.observe(&key, &Value::Null)?;
                }
            }
        }
        ensure!(
            base_sha == merge_sha,
            "Default branch moved after merge; release preparation refused"
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
            "Release workspace must be clean"
        );
        let remote = format!("https://github.com/{}.git", event.repository);
        let tracking = format!("refs/remotes/origin/{base_ref}");
        git_output(
            factory,
            workspace,
            &[
                "fetch",
                "--tags",
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
            "Fetched release base differs from verified GitHub ref"
        );
        let release_branch = format!("ai-factory/release/{}", uuid::Uuid::new_v4());
        git_output(
            factory,
            workspace,
            &["checkout", "-b", &release_branch, &merge_sha],
            None,
        )
        .await?;
        let context = ReleaseWorkspace {
            repository: event.repository.clone(),
            number: event.number,
            head_sha: event.head_sha.clone(),
            base_ref,
            base_sha,
            merge_sha,
            release_branch,
        };
        let mut value = serde_json::to_value(&context)?;
        value["completed"] = false.into();
        factory
            .store
            .observe(&format!("release-workspace:{run}"), &value)?;
        Ok(value)
    }

    pub async fn publish_release(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
        workspace: &Path,
        data: &Value,
        check_active: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<()> {
        self.release_authorized(factory, event, run).await?;
        let (base_ref, base_sha, merge_sha) = self.merged_release_context(event).await?;
        if self.recorded_release(factory, event, run)?.is_some() {
            return Ok(());
        }
        let action = ReleaseAction::parse(data)?;
        let context: ReleaseWorkspace = serde_json::from_value(
            factory
                .store
                .observation(&format!("release-workspace:{run}"))?
                .context("Prepared release workspace required")?,
        )?;
        context.matches(event)?;
        ensure!(
            base_ref == context.base_ref && merge_sha == context.merge_sha,
            "PR merge identity changed before release"
        );
        let intent_key = format!("release-intent:{}", event.event_id);
        if let Some(value) = factory
            .store
            .observation(&intent_key)?
            .filter(|v| !v.is_null())
        {
            let intent: ReleaseIntent = serde_json::from_value(value)?;
            intent.context.matches(event)?;
            ensure!(
                intent.context.base_ref == base_ref && intent.context.merge_sha == merge_sha,
                "Release intent identity mismatch"
            );
            ensure!(
                self.release_push_state(&intent).await? == PushState::Applied,
                "Prepare release retry before publishing unused intent"
            );
            self.finish_release(factory, event, run, &intent, check_active)
                .await?;
            return Ok(());
        }
        ensure!(
            base_sha == context.base_sha,
            "Default branch moved before release"
        );
        ensure!(
            git_output(factory, workspace, &["branch", "--show-current"], None).await?
                == context.release_branch,
            "Release must use prepared isolated branch"
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
            "Commit version files before publishing release"
        );
        let target_sha = git_output(
            factory,
            workspace,
            &["rev-parse", "--verify", "HEAD^{commit}"],
            None,
        )
        .await?;
        validate_sha(&target_sha)?;
        if target_sha != context.merge_sha {
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
                "Release requires one version commit directly after merged base"
            );
        }
        ensure!(
            self.release_tag(&event.repository, &action.tag)
                .await?
                .is_none(),
            "Release tag already exists; preserving existing tag"
        );
        ensure!(
            git_output(factory, workspace, &["tag", "--list", &action.tag], None)
                .await?
                .is_empty(),
            "Release tag must be created by Factory"
        );
        check_active()?;
        git_output(
            factory,
            workspace,
            &[
                "tag",
                "--annotate",
                &action.tag,
                "--message",
                &action.title,
                &target_sha,
            ],
            Some(&event.actor),
        )
        .await?;
        let tag_object_sha = git_output(
            factory,
            workspace,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/tags/{}", action.tag),
            ],
            None,
        )
        .await?;
        validate_sha(&tag_object_sha)?;
        let intent = ReleaseIntent {
            context,
            action,
            target_sha,
            tag_object_sha,
        };
        factory
            .store
            .observe(&intent_key, &serde_json::to_value(&intent)?)?;
        check_active()?;
        let remote = format!("https://github.com/{}.git", event.repository);
        // Exact lease plus direct-parent validation provides compare-and-swap
        // without granting a history rewrite. Both refs update atomically.
        let push = git_output(
            factory,
            workspace,
            &[
                "push",
                "--atomic",
                &format!(
                    "--force-with-lease=refs/heads/{}:{}",
                    intent.context.base_ref, intent.context.base_sha
                ),
                &remote,
                &format!(
                    "{}:refs/heads/{}",
                    intent.target_sha, intent.context.base_ref
                ),
                &format!(
                    "refs/tags/{}:refs/tags/{}",
                    intent.action.tag, intent.action.tag
                ),
            ],
            Some(&event.actor),
        )
        .await;
        match self.release_push_state(&intent).await {
            Ok(PushState::Applied) => {
                self.finish_release(factory, event, run, &intent, check_active)
                    .await?;
                Ok(())
            }
            state => {
                let error = match state {
                    Err(error) => error,
                    _ => anyhow::anyhow!("Release atomic push not applied"),
                };
                Err(match push {
                    Err(push) => error.context(format!("Release push failed: {push:#}")),
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
    fn atomic_push_reconciliation_accepts_only_both_or_neither() {
        assert_eq!(
            push_state("base", None, "base", "next", "tag-object").unwrap(),
            PushState::Unused
        );
        assert_eq!(
            push_state(
                "next",
                Some(("tag-object", "next")),
                "base",
                "next",
                "tag-object"
            )
            .unwrap(),
            PushState::Applied
        );
        assert!(push_state("next", None, "base", "next", "tag-object").is_err());
        assert!(push_state(
            "base",
            Some(("tag-object", "next")),
            "base",
            "next",
            "tag-object"
        )
        .is_err());
        assert!(push_state(
            "next",
            Some(("other-tag", "next")),
            "base",
            "next",
            "tag-object"
        )
        .is_err());
        assert!(push_state("unrelated", None, "base", "next", "tag-object").is_err());
    }

    #[test]
    fn tag_only_release_reconciliation_requires_tag() {
        assert_eq!(
            push_state("base", None, "base", "base", "tag-object").unwrap(),
            PushState::Unused
        );
        assert_eq!(
            push_state(
                "base",
                Some(("tag-object", "base")),
                "base",
                "base",
                "tag-object"
            )
            .unwrap(),
            PushState::Applied
        );
    }

    #[test]
    fn release_action_requires_stable_semver_bump_and_reason() {
        let good = serde_json::json!({"tag":"v1.2.3","title":"1.2.3","body":"Fixed bug","bump":"patch","reason":"Fixes retry bug"});
        assert!(ReleaseAction::parse(&good).is_ok());
        for (key, value) in [
            ("tag", "../1.2.3"),
            ("tag", "v1.2.3-rc.1"),
            ("tag", "v01.2.3"),
            ("bump", "auto"),
            ("reason", " "),
        ] {
            let mut bad = good.clone();
            bad[key] = value.into();
            assert!(ReleaseAction::parse(&bad).is_err(), "accepted {bad}");
        }
    }
}
