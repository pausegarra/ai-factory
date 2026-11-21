use crate::{Flow, SourceConnector, TriggerEvent};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

pub(crate) enum IssueScope {
    SingleRepository,
    SubIssues(IssueWork),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SubIssue {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WorkRepository {
    pub repository: String,
    pub base_branch: String,
    pub base_sha: String,
    pub sub_issues: Vec<SubIssue>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct IssueWork {
    pub event_id: String,
    pub source_id: String,
    pub parent_repository: String,
    pub parent_number: u64,
    pub actor: String,
    pub repositories: Vec<WorkRepository>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct IssuePublication {
    pub node_id: String,
    pub flow: Option<Flow>,
    pub result: Option<serde_json::Value>,
    pub targets: Vec<PublicationTarget>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PublicationTarget {
    pub repository: String,
    pub head_sha: String,
    pub title: String,
    pub body: String,
    pub number: Option<u64>,
}

impl IssueWork {
    pub fn matches(&self, event: &TriggerEvent) -> bool {
        event.trigger_id == "new_issue"
            && self.event_id == event.event_id
            && Some(self.source_id.as_str()) == event.origin_token_profile_id.as_deref()
            && self.parent_repository == event.repository
            && self.parent_number == event.number
            && self.actor.eq_ignore_ascii_case(&event.actor)
    }

    pub fn authorize(&self, profile: &SourceConnector) -> Result<()> {
        ensure!(
            self.source_id == profile.id
                && self.actor.eq_ignore_ascii_case(&profile.account)
                && profile.allows_actor("new_issue")
                && profile.allows_owner(self.parent_repository.split('/').next().unwrap_or("")),
            "Parent issue no longer authorized"
        );
        for target in &self.repositories {
            ensure!(
                crate::valid_repository(&target.repository)
                    && profile.allows_owner(target.repository.split('/').next().unwrap_or("")),
                "Subissue repository {} is outside the source scope",
                target.repository
            );
        }
        Ok(())
    }
}
