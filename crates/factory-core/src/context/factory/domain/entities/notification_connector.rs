use super::models::safe_id;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationEvent {
    RunStarted,
    RunSucceeded,
    RunFailed,
    RunBlocked,
}
impl NotificationEvent {
    pub fn from_run_status(status: &str) -> Option<Self> {
        match status {
            "running" => Some(Self::RunStarted),
            "succeeded" => Some(Self::RunSucceeded),
            "failed" => Some(Self::RunFailed),
            "blocked" => Some(Self::RunBlocked),
            _ => None,
        }
    }
}
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct TelegramConnectorConfig {
    pub chat_id: String,
    #[serde(default)]
    pub has_token: bool,
    #[serde(default, skip_serializing)]
    pub bot_token: Option<String>,
}
impl std::fmt::Debug for TelegramConnectorConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramConnectorConfig")
            .field("has_token", &self.has_token)
            .finish_non_exhaustive()
    }
}
#[derive(Clone)]
pub enum ConnectorProvider {
    Telegram(TelegramConnectorConfig),
    Unsupported { kind: String, config: Value },
}
impl std::fmt::Debug for ConnectorProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectorProvider { config: [REDACTED] }")
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "WireConnector", into = "WireConnector")]
pub struct NotificationConnector {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub events: Vec<NotificationEvent>,
    pub provider: ConnectorProvider,
}
#[derive(Serialize, Deserialize)]
struct WireConnector {
    id: String,
    label: String,
    enabled: bool,
    events: Vec<NotificationEvent>,
    #[serde(rename = "type")]
    kind: String,
    config: Value,
}
impl TryFrom<WireConnector> for NotificationConnector {
    type Error = &'static str;
    fn try_from(w: WireConnector) -> Result<Self, Self::Error> {
        if !safe_id(&w.kind) {
            return Err("Invalid notification connector type");
        }
        let provider = if w.kind == "telegram" {
            ConnectorProvider::Telegram(
                serde_json::from_value(w.config).map_err(|_| "Invalid Telegram configuration")?,
            )
        } else {
            ConnectorProvider::Unsupported {
                kind: w.kind,
                config: w.config,
            }
        };
        Ok(Self {
            id: w.id,
            label: w.label,
            enabled: w.enabled,
            events: w.events,
            provider,
        })
    }
}
impl From<NotificationConnector> for WireConnector {
    fn from(c: NotificationConnector) -> Self {
        let (kind, config) = match c.provider {
            ConnectorProvider::Telegram(config) => (
                "telegram".into(),
                serde_json::to_value(config).expect("Telegram config serializes"),
            ),
            ConnectorProvider::Unsupported { kind, config } => (kind, config),
        };
        Self {
            id: c.id,
            label: c.label,
            enabled: c.enabled,
            events: c.events,
            kind,
            config,
        }
    }
}
pub fn validate_connectors(connectors: &[NotificationConnector]) -> Result<()> {
    let mut ids = HashSet::new();
    for c in connectors {
        ensure!(
            safe_id(&c.id) && ids.insert(&c.id),
            "Notification connector IDs must be valid and unique"
        );
        ensure!(
            !c.label.trim().is_empty(),
            "Notification connector name is required"
        );
        let mut events = HashSet::new();
        ensure!(
            c.events.iter().all(|e| events.insert(e)),
            "Duplicate notification events"
        );
        if let ConnectorProvider::Telegram(config) = &c.provider {
            ensure!(
                !c.enabled || !config.chat_id.trim().is_empty(),
                "Enabled Telegram connector requires a Chat ID"
            );
            ensure!(
                !c.enabled || !c.events.is_empty(),
                "Enabled Telegram connector requires a run event"
            );
        }
    }
    Ok(())
}
