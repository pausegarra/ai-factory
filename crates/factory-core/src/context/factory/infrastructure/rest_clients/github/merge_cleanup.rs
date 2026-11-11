use super::Github;
use crate::{valid_repository, Result, TriggerEvent};
use anyhow::{bail, ensure, Context};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use std::fmt::Write;

#[derive(Debug, PartialEq, Eq)]
struct MergeSource {
    repo: String,
    head: String,
    base: String,
    sha: String,
}

fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && branch != "@"
        && branch != "HEAD"
        && !branch.starts_with('-')
        && !branch.ends_with('.')
        && !branch.contains("..")
        && !branch.contains("@{")
        && !branch.bytes().any(|b| {
            b <= b' ' || b == 127 || matches!(b, b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
        && branch
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

fn ref_path(repo: &str, branch: &str, delete: bool) -> String {
    let mut encoded = String::new();
    for byte in branch.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            write!(&mut encoded, "%{byte:02X}").expect("Writing to String cannot fail");
        }
    }
    let endpoint = if delete { "refs" } else { "ref" };
    format!("/repos/{repo}/git/{endpoint}/heads/{encoded}")
}

fn parse_source(pr: &Value, event: &TriggerEvent, default: &str) -> Result<MergeSource> {
    for side in ["head", "base"] {
        ensure!(
            pr[side]["repo"]["full_name"]
                .as_str()
                .is_some_and(|repo| repo.eq_ignore_ascii_case(&event.repository)),
            "PR {side} repository mismatch; fork cleanup is unsupported"
        );
    }
    let head = pr["head"]["ref"]
        .as_str()
        .context("Missing source branch")?;
    let base = pr["base"]["ref"]
        .as_str()
        .context("Missing target branch")?;
    ensure!(
        valid_branch(head) && valid_branch(base),
        "Invalid PR branch reference"
    );
    ensure!(head != base, "Source branch is target branch");
    ensure!(head != default, "Source branch is default branch");
    ensure!(
        pr["head"]["sha"] == event.head_sha,
        "PR changed since approval"
    );
    Ok(MergeSource {
        repo: event.repository.clone(),
        head: head.into(),
        base: base.into(),
        sha: event.head_sha.clone(),
    })
}

fn reference_sha(value: &Value, branch: &str) -> Result<String> {
    ensure!(
        value["ref"] == format!("refs/heads/{branch}"),
        "Reference identity mismatch"
    );
    ensure!(
        value["object"]["type"] == "commit",
        "Reference is not a commit"
    );
    let sha = value["object"]["sha"]
        .as_str()
        .context("Missing reference SHA")?;
    ensure!(
        sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid reference SHA"
    );
    Ok(sha.into())
}

impl Github {
    async fn repository_default(&self, repo: &str) -> Result<String> {
        ensure!(valid_repository(repo), "Invalid repository identity");
        let value = self
            .request(Method::GET, &format!("/repos/{repo}"), None)
            .await?;
        ensure!(
            value["full_name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(repo)),
            "Repository identity mismatch"
        );
        let default = value["default_branch"]
            .as_str()
            .context("Missing default branch")?;
        ensure!(valid_branch(default), "Invalid default branch");
        Ok(default.into())
    }

    async fn confirmed_source(&self, event: &TriggerEvent) -> Result<MergeSource> {
        let current = self.pr(&event.repository, event.number).await?;
        ensure!(current["merged"] == true, "PR merge is not confirmed");
        let default = self.repository_default(&event.repository).await?;
        parse_source(&current, event, &default)
    }

    async fn source_sha(&self, source: &MergeSource) -> Result<Option<String>> {
        let path = ref_path(&source.repo, &source.head, false);
        let response = self.send(Method::GET, &path, None).await?;
        match response.status() {
            StatusCode::OK => Ok(Some(reference_sha(
                &response.json::<Value>().await?,
                &source.head,
            )?)),
            StatusCode::NOT_FOUND => {
                // Repo metadata alone does not demonstrate access to Git references.
                let default = self.repository_default(&source.repo).await?;
                ensure!(source.head != default, "Source branch is default branch");
                let witness = self
                    .request(Method::GET, &ref_path(&source.repo, &default, false), None)
                    .await?;
                reference_sha(&witness, &default)?;
                Ok(None)
            }
            status => bail!("GitHub API {} for {path}", status.as_u16()),
        }
    }

    async fn delete_or_reconcile(&self, event: &TriggerEvent, source: &MergeSource) -> Result<()> {
        let path = ref_path(&source.repo, &source.head, true);
        let deletion_error = match self.send(Method::DELETE, &path, None).await {
            Ok(response) if response.status() == StatusCode::NO_CONTENT => return Ok(()),
            Ok(response) => anyhow::anyhow!("GitHub API {} for {path}", response.status().as_u16()),
            Err(error) => error,
        };
        // Never issue a second DELETE within this attempt.
        let reconciliation = async {
            ensure!(
                &self.confirmed_source(event).await? == source,
                "PR branch identity changed after merge"
            );
            ensure!(
                self.source_sha(source).await?.is_none(),
                "Source branch still exists after DELETE"
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        match reconciliation {
            Ok(()) => Ok(()),
            Err(error) => {
                bail!("Delete failed: {deletion_error:#}; absence not verified: {error:#}")
            }
        }
    }

    pub(super) async fn merge_and_cleanup(
        &self,
        event: &TriggerEvent,
        check_active: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<()> {
        let current = self
            .pr(&event.repository, event.number)
            .await
            .context("Merge not confirmed; no branch deletion attempted")?;
        let already_merged = current["merged"]
            .as_bool()
            .context("Missing PR merge state")?;
        let prepared = async {
            let default = self.repository_default(&event.repository).await?;
            parse_source(&current, event, &default)
        }
        .await;
        let source = prepared.map_err(|error| {
            let phase = if already_merged {
                "merged; source cleanup pending"
            } else {
                "merge not confirmed; preflight failed"
            };
            anyhow::anyhow!(
                "PR {}#{} {phase}: {error:#}",
                event.repository,
                event.number
            )
        })?;
        if !already_merged {
            ensure!(
                current["state"] == "open",
                "PR is not open; merge not confirmed"
            );
            check_active()?;
            let response = self
                .request(
                    Method::PUT,
                    &format!("/repos/{}/pulls/{}/merge", event.repository, event.number),
                    Some(&json!({"merge_method":"squash", "sha":event.head_sha})),
                )
                .await
                .context("Merge not confirmed; no branch deletion attempted")?;
            ensure!(
                response["merged"] == true,
                "GitHub did not merge PR; merge not confirmed"
            );
        }
        let cleanup = async {
            check_active()?;
            ensure!(
                self.confirmed_source(event).await? == source,
                "PR branch identity changed after merge"
            );
            let Some(sha) = self.source_sha(&source).await? else {
                return Ok(());
            };
            ensure!(
                sha == source.sha,
                "Source branch moved since approval; preserving branch"
            );
            check_active()?;
            self.delete_or_reconcile(event, &source).await
        }
        .await;
        cleanup.map_err(|error| {
            anyhow::anyhow!(
                "PR {}#{} merged; cleanup pending for source branch {}: {error:#}",
                source.repo,
                event.number,
                source.head
            )
        })
    }
}
