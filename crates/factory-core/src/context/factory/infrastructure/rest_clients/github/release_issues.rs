use super::Github;
use crate::context::factory::domain::entities::models::issue_trigger_for_title;
use crate::{valid_repository, Factory, Result, TriggerEvent};
use anyhow::{ensure, Context};
use reqwest::Method;
use serde_json::{json, Value};

fn validate_issue(issue: &Value, event: &TriggerEvent) -> Result<()> {
    ensure!(
        issue["number"] == event.number
            && issue["html_url"]
                .as_str()
                .is_some_and(|url| url.eq_ignore_ascii_case(&format!(
                    "https://github.com/{}/issues/{}",
                    event.repository, event.number
                )))
            && issue.get("pull_request").is_none_or(Value::is_null)
            && issue["title"].as_str().and_then(issue_trigger_for_title) == Some("release")
            && issue["user"]["login"]
                .as_str()
                .is_some_and(|author| author.eq_ignore_ascii_case(&event.actor))
            && matches!(issue["state"].as_str(), Some("open" | "closed")),
        "Release issue identity or eligibility changed before closure"
    );
    Ok(())
}

impl Github {
    pub(crate) async fn close_release_issue(
        &self,
        factory: &Factory,
        event: &TriggerEvent,
        run: &str,
    ) -> Result<()> {
        ensure!(
            event.trigger_id == "release"
                && valid_repository(&event.repository)
                && event.number > 0,
            "Invalid release issue"
        );
        let check_active = || -> Result<()> {
            ensure!(!factory.stop.is_cancelled(), "Factory is stopping");
            let detail = factory.store.detail(run)?;
            ensure!(detail.run.status == "running", "Run is already terminal");
            ensure!(
                detail.event.event_id == event.event_id,
                "Release run identity mismatch"
            );
            let settings = factory.store.public_settings()?;
            let profile = settings.source_for_event(event)?;
            ensure!(
                profile.allows_actor("release")
                    && profile.allows_owner(event.repository.split('/').next().unwrap()),
                "Release actor or repository no longer authorized"
            );
            Ok(())
        };
        check_active()?;
        self.verify_actor(&event.actor).await?;
        check_active()?;
        let path = format!("/repos/{}/issues/{}", event.repository, event.number);
        let mut issue = self.request(Method::GET, &path, None).await?;
        validate_issue(&issue, event)?;
        if issue["state"] == "open" {
            check_active()?;
            let response = self
                .request(
                    Method::PATCH,
                    &path,
                    Some(&json!({"state":"closed","state_reason":"completed"})),
                )
                .await;
            // A lost/error response can follow an accepted write. Reconcile
            // remote state before reporting failure; never repeat the PATCH.
            check_active()?;
            issue = self
                .request(Method::GET, &path, None)
                .await
                .context("Cannot confirm release issue closure")?;
            validate_issue(&issue, event)?;
            if issue["state"] != "closed" {
                response?;
                anyhow::bail!("GitHub did not close the release issue");
            }
        }
        check_active()?;
        factory.store.observe(
            &format!("release-issue-close:{run}"),
            &json!({"repository":event.repository,"number":event.number,"actor":event.actor,"event_id":event.event_id,"publication_confirmed":true,"url":issue["html_url"],"state":"closed","state_reason":issue["state_reason"]}),
        )?;
        Ok(())
    }
}
