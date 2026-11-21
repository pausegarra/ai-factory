use super::super::source_connector::SourceConnector;
use super::default_prompts::refresh_release_prompt;
use super::{
    Commands, Flow, Project, Settings, DEFAULT_ISSUE_TITLE_PREFIX,
    DEFAULT_RELEASE_ISSUE_TITLE_PREFIX,
};
use serde::Deserialize;
use std::collections::BTreeMap;

// Legacy GitHub configuration is deliberately ignored.
#[derive(Deserialize)]
#[serde(default)]
pub(super) struct SettingsInput {
    notification_connectors: Vec<serde_json::Value>,
    source_connectors: Vec<serde_json::Value>,
    codex_bindings: BTreeMap<String, String>,
    projects: Vec<Project>,
    commands: Commands,
    issue_title_prefix: String,
    release_issue_title_prefix: String,
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
            issue_title_prefix: DEFAULT_ISSUE_TITLE_PREFIX.into(),
            release_issue_title_prefix: DEFAULT_RELEASE_ISSUE_TITLE_PREFIX.into(),
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
        let issue_prefix = super::normalize_issue_title_prefix(&input.issue_title_prefix)?;
        let release_prefix =
            super::normalize_release_issue_title_prefix(&input.release_issue_title_prefix)?;
        let mut source_connectors =
            Vec::<SourceConnector>::with_capacity(input.source_connectors.len());
        let mut ids = std::collections::HashSet::new();
        for mut source in input.source_connectors {
            let fields = source
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("Invalid source connector configuration"))?;
            // Existing sources inherit a global prefix only if they lack a local setting.
            fields
                .entry("issue_title_prefix")
                .or_insert_with(|| serde_json::json!(issue_prefix));
            fields
                .entry("release_issue_title_prefix")
                .or_insert_with(|| serde_json::json!(release_prefix));
            let mut source: SourceConnector = serde_json::from_value(source)
                .map_err(|_| anyhow::anyhow!("Invalid source connector configuration"))?;
            source.normalize_title_prefixes()?;
            source.validate()?;
            anyhow::ensure!(
                ids.insert(source.id.clone()),
                "Source connector IDs must be unique"
            );
            source_connectors.push(source);
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
        for flow in &mut flows {
            refresh_release_prompt(flow);
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
