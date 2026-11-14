use super::{Commands, Flow, Project, Settings, TokenProfile};
use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;

// Legacy lists are accepted only at this input boundary. Canonical settings
// always serialize scopes on each profile, including explicit empty lists.
#[derive(Deserialize)]
#[serde(default)]
pub(super) struct SettingsInput {
    notification_connectors: Vec<serde_json::Value>,
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
        let token_profiles: Vec<TokenProfile> = input
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
            notification_connectors: decode_notification_connectors(
                input.notification_connectors,
                &token_profiles,
            )?,
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

// Only missing legacy Telegram fields inherit defaults, never explicit empty lists.
fn decode_notification_connectors(
    mut connectors: Vec<serde_json::Value>,
    profiles: &[TokenProfile],
) -> anyhow::Result<Vec<crate::NotificationConnector>> {
    use crate::context::factory::domain::entities::notification_connector::NOTIFICATION_TRIGGER_IDS;
    for connector in &mut connectors {
        if connector.get("type").and_then(serde_json::Value::as_str) != Some("telegram") {
            continue;
        }
        if let Some(config) = connector
            .get_mut("config")
            .and_then(serde_json::Value::as_object_mut)
        {
            if !config.contains_key("triggers") {
                config.insert(
                    "triggers".into(),
                    serde_json::json!(NOTIFICATION_TRIGGER_IDS),
                );
            }
            if !config.contains_key("token_profile_ids") {
                config.insert(
                    "token_profile_ids".into(),
                    serde_json::json!(profiles.iter().map(|p| &p.id).collect::<Vec<_>>()),
                );
            }
        }
    }
    connectors
        .into_iter()
        .map(|connector| {
            serde_json::from_value(connector)
                .map_err(|_| anyhow::anyhow!("Invalid notification connector configuration"))
        })
        .collect()
}
