use super::{Commands, Flow, Project, Settings, TokenProfile};
use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;

// Legacy lists are accepted only at this input boundary. Canonical settings
// always serialize scopes on each profile, including explicit empty lists.
#[derive(Deserialize)]
#[serde(default)]
pub(super) struct SettingsInput {
    notification_connectors: Vec<crate::NotificationConnector>,
    owners: Vec<String>,
    authors: Vec<String>,
    reviewers: Vec<String>,
    token_profiles: Vec<TokenProfileInput>,
    codex_bindings: BTreeMap<String, String>,
    projects: Vec<Project>,
    commands: Commands,
    issue_node_comments_enabled: bool,
    codex_yolo: bool,
    poll_interval_seconds: u64,
    max_concurrent: usize,
    trigger_max_concurrent: BTreeMap<String, usize>,
    start_at_login: bool,
    flows: Vec<Flow>,
}

#[derive(Deserialize)]
struct TokenProfileInput {
    #[serde(flatten)]
    profile: TokenProfile,
    #[serde(default, deserialize_with = "present_scope")]
    owners: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present_scope")]
    authors: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present_scope")]
    reviewers: Option<Vec<String>>,
}

fn present_scope<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    // Missing fields may inherit legacy values; explicit null is invalid input.
    Vec::<String>::deserialize(deserializer).map(Some)
}

impl Default for SettingsInput {
    fn default() -> Self {
        let defaults = Settings::default();
        Self {
            owners: vec![],
            authors: vec![],
            reviewers: vec![],
            notification_connectors: vec![],
            token_profiles: vec![],
            codex_bindings: defaults.codex_bindings,
            projects: defaults.projects,
            commands: defaults.commands,
            issue_node_comments_enabled: defaults.issue_node_comments_enabled,
            codex_yolo: defaults.codex_yolo,
            poll_interval_seconds: defaults.poll_interval_seconds,
            max_concurrent: defaults.max_concurrent,
            trigger_max_concurrent: defaults.trigger_max_concurrent,
            start_at_login: defaults.start_at_login,
            flows: defaults.flows,
        }
    }
}

impl TryFrom<SettingsInput> for Settings {
    type Error = anyhow::Error;

    fn try_from(input: SettingsInput) -> Result<Self, Self::Error> {
        let token_profiles = input
            .token_profiles
            .into_iter()
            .map(|token| {
                let mut profile = token.profile;
                profile.apply_legacy_scope(&input.owners, &input.authors, &input.reviewers);
                if let Some(owners) = token.owners {
                    profile.owners = owners;
                }
                if let Some(authors) = token.authors {
                    profile.authors = authors;
                }
                if let Some(reviewers) = token.reviewers {
                    profile.reviewers = reviewers;
                }
                profile
            })
            .collect();
        let settings = Self {
            notification_connectors: input.notification_connectors,
            token_profiles,
            codex_bindings: input.codex_bindings,
            projects: input.projects,
            commands: input.commands,
            issue_node_comments_enabled: input.issue_node_comments_enabled,
            codex_yolo: input.codex_yolo,
            poll_interval_seconds: input.poll_interval_seconds,
            max_concurrent: input.max_concurrent,
            trigger_max_concurrent: input.trigger_max_concurrent,
            start_at_login: input.start_at_login,
            flows: input.flows,
        };
        settings.validate_concurrency()?;
        Ok(settings)
    }
}
