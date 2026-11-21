use super::models::{
    issue_title_matches_trigger, normalize_issue_title_prefix,
    normalize_release_issue_title_prefix, safe_id, valid_login, DEFAULT_ISSUE_TITLE_PREFIX,
    DEFAULT_RELEASE_ISSUE_TITLE_PREFIX,
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    #[default]
    GithubIssues,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SourceConnector {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: SourceKind,
    pub account: String,
    #[serde(default = "default_issue_title_prefix")]
    pub issue_title_prefix: String,
    #[serde(default = "default_release_issue_title_prefix")]
    pub release_issue_title_prefix: String,
    #[serde(default)]
    pub owners: Vec<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub reviewers: Vec<String>,
    #[serde(default)]
    pub review_all_open_pull_requests: bool,
    #[serde(default)]
    pub pull_request_authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default)]
    pub has_token: bool,
}

fn default_issue_title_prefix() -> String {
    DEFAULT_ISSUE_TITLE_PREFIX.into()
}

fn default_release_issue_title_prefix() -> String {
    DEFAULT_RELEASE_ISSUE_TITLE_PREFIX.into()
}

impl Default for SourceConnector {
    fn default() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            kind: SourceKind::default(),
            account: String::new(),
            issue_title_prefix: default_issue_title_prefix(),
            release_issue_title_prefix: default_release_issue_title_prefix(),
            owners: vec![],
            authors: vec![],
            reviewers: vec![],
            review_all_open_pull_requests: false,
            pull_request_authors: vec![],
            token: None,
            has_token: false,
        }
    }
}

impl SourceConnector {
    pub(crate) fn matches_issue_title(&self, title: &str, trigger: &str) -> bool {
        issue_title_matches_trigger(
            title,
            trigger,
            &self.issue_title_prefix,
            &self.release_issue_title_prefix,
        )
    }

    pub(crate) fn normalize_title_prefixes(&mut self) -> Result<()> {
        self.issue_title_prefix = normalize_issue_title_prefix(&self.issue_title_prefix)?;
        self.release_issue_title_prefix =
            normalize_release_issue_title_prefix(&self.release_issue_title_prefix)?;
        Ok(())
    }

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

    pub fn allows_pull_request_author(&self, author: Option<&str>) -> bool {
        !self.review_all_open_pull_requests
            || self.pull_request_authors.is_empty()
            || author.is_some_and(|author| {
                valid_login(author)
                    && self
                        .pull_request_authors
                        .iter()
                        .any(|value| value.trim().eq_ignore_ascii_case(author))
            })
    }

    pub fn validate(&self) -> Result<()> {
        normalize_issue_title_prefix(&self.issue_title_prefix)?;
        normalize_release_issue_title_prefix(&self.release_issue_title_prefix)?;
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
            .chain(&self.pull_request_authors)
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
