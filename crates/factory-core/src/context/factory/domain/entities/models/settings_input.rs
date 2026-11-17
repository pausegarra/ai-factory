use super::super::source_connector::SourceConnector;
use super::{Commands, Flow, Project, Settings};
use serde::Deserialize;
use std::collections::BTreeMap;

// Legacy GitHub configuration is deliberately ignored.
#[derive(Deserialize)]
#[serde(default)]
pub(super) struct SettingsInput {
    notification_connectors: Vec<serde_json::Value>,
    source_connectors: Vec<SourceConnector>,
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

impl Default for SettingsInput {
    fn default() -> Self {
        let defaults = Settings::default();
        Self {
            notification_connectors: vec![],
            source_connectors: vec![],
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
        let source_connectors = input.source_connectors;
        let mut ids = std::collections::HashSet::new();
        for source in &source_connectors {
            source.validate()?;
            anyhow::ensure!(
                ids.insert(&source.id),
                "Source connector IDs must be unique"
            );
        }
        let mut flows = input.flows;
        let legacy_ids = ["new_issue", "pr_approved", "pr_review"];
        if flows.len() == legacy_ids.len()
            && legacy_ids
                .iter()
                .all(|id| flows.iter().filter(|f| f.id == *id).count() == 1)
        {
            flows.push(
                Settings::default()
                    .flows
                    .into_iter()
                    .find(|f| f.id == "release")
                    .unwrap(),
            );
        }
        let settings = Self {
            notification_connectors: decode_notification_connectors(input.notification_connectors)?,
            source_connectors,
            codex_bindings: input.codex_bindings,
            projects: input.projects,
            commands: input.commands,
            issue_node_comments_enabled: input.issue_node_comments_enabled,
            codex_yolo: input.codex_yolo,
            poll_interval_seconds: input.poll_interval_seconds,
            max_concurrent: input.max_concurrent,
            trigger_max_concurrent: input.trigger_max_concurrent,
            start_at_login: input.start_at_login,
            flows,
        };
        settings.validate_concurrency()?;
        Ok(settings)
    }
}

// Only missing legacy Telegram fields inherit defaults, never explicit empty lists.
fn decode_notification_connectors(
    mut connectors: Vec<serde_json::Value>,
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
                config.insert("token_profile_ids".into(), serde_json::json!([]));
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
