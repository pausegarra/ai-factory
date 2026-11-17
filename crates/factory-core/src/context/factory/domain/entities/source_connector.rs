use super::models::{safe_id, valid_login};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    #[default]
    GithubIssues,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct SourceConnector {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: SourceKind,
    pub account: String,
    #[serde(default)]
    pub owners: Vec<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub reviewers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default)]
    pub has_token: bool,
}

impl SourceConnector {
    pub fn allows_owner(&self, owner: &str) -> bool {
        self.owners
            .iter()
            .any(|value| value.eq_ignore_ascii_case(owner))
    }

    pub fn allows_actor(&self, trigger: &str) -> bool {
        let actors = match trigger {
            "new_issue" | "pr_approved" | "release" => &self.authors,
            "pr_review" => &self.reviewers,
            _ => return false,
        };
        actors
            .iter()
            .any(|value| value.eq_ignore_ascii_case(&self.account))
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(safe_id(&self.id), "Invalid source connector ID");
        ensure!(
            !self.label.trim().is_empty(),
            "Source connector name is required"
        );
        ensure!(valid_login(&self.account), "Invalid GitHub account");
        for value in self
            .owners
            .iter()
            .chain(&self.authors)
            .chain(&self.reviewers)
        {
            ensure!(valid_login(value), "Invalid GitHub login");
        }
        Ok(())
    }
}

impl std::fmt::Debug for SourceConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceConnector")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("kind", &self.kind)
            .field("account", &self.account)
            .field("has_token", &self.has_token)
            .finish_non_exhaustive()
    }
}
