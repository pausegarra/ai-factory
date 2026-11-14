use super::store::Store;
use crate::context::factory::application::dto::connector_notification::{
    ConnectorTarget, ConnectorTargetConfig, ConnectorWorkerKey,
};
use crate::context::factory::application::services::connector_notifications::targets_for_event;
use crate::*;
use anyhow::ensure;
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::HashMap;

pub(super) struct PreparedNotificationSettings {
    pub connectors: Vec<NotificationConnector>,
    pub references: HashMap<String, String>,
}
impl Store {
    pub(crate) fn notification_targets(
        &self,
        event: NotificationEvent,
        run: &Run,
    ) -> Result<Vec<ConnectorTarget>> {
        let db = self.db.lock().unwrap();
        let raw: Option<String> = db
            .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
            .optional()?;
        let settings: Settings = raw
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default();
        let references = read_refs(&db)?;
        Ok(targets_for_event(
            &settings.notification_connectors,
            event,
            run,
            &settings.token_profiles,
        )
        .into_iter()
        .filter_map(|c| {
            let config = match &c.provider {
                ConnectorProvider::Telegram(config) => ConnectorTargetConfig::Telegram {
                    chat_id: config.chat_id.clone(),
                },
                ConnectorProvider::GoogleChat(_) => ConnectorTargetConfig::GoogleChat,
                _ => return None,
            };
            Some(ConnectorTarget {
                connector_id: c.id.clone(),
                credential_ref: references.get(&c.id)?.clone(),
                config,
            })
        })
        .collect())
    }
    pub(crate) fn notification_worker_keys(&self) -> Result<HashMap<String, ConnectorWorkerKey>> {
        let db = self.db.lock().unwrap();
        let raw: Option<String> = db
            .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
            .optional()?;
        let settings: Settings = raw
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default();
        let refs = read_refs(&db)?;
        Ok(settings
            .notification_connectors
            .iter()
            .filter_map(|c| {
                let key = match &c.provider {
                    ConnectorProvider::Telegram(_) => ConnectorWorkerKey::Telegram,
                    ConnectorProvider::GoogleChat(_) if c.enabled => {
                        ConnectorWorkerKey::GoogleChat {
                            credential_ref: refs.get(&c.id)?.clone(),
                        }
                    }
                    _ => return None,
                };
                Some((c.id.clone(), key))
            })
            .collect())
    }
    pub(super) fn notification_refs(&self) -> Result<HashMap<String, String>> {
        let db = self.db.lock().unwrap();
        read_refs(&db)
    }
    pub(super) fn prepare_notification_settings(
        &self,
        settings: &Settings,
    ) -> Result<PreparedNotificationSettings> {
        use crate::context::factory::infrastructure::rest_clients::google_chat::parse_webhook_url;
        const URL_ERROR: &str =
            "Enter a valid HTTPS webhook URL without credentials or a fragment.";
        let old = self.notification_refs()?;
        let previous = self.raw_settings()?;
        let mut connectors = settings.notification_connectors.clone();
        let mut references = HashMap::new();
        for c in &mut connectors {
            let same_provider = previous.notification_connectors.iter().any(|p| {
                p.id == c.id
                    && matches!(
                        (&p.provider, &c.provider),
                        (
                            ConnectorProvider::Telegram(_),
                            ConnectorProvider::Telegram(_)
                        ) | (
                            ConnectorProvider::GoogleChat(_),
                            ConnectorProvider::GoogleChat(_)
                        )
                    )
            });
            let (replacement, google_chat) = match &mut c.provider {
                ConnectorProvider::Telegram(config) => {
                    config.chat_id = config.chat_id.trim().into();
                    (
                        config
                            .bot_token
                            .take()
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty()),
                        false,
                    )
                }
                ConnectorProvider::GoogleChat(config) => {
                    let replacement = config.webhook_url.take().filter(|s| !s.trim().is_empty());
                    if let Some(url) = &replacement {
                        parse_webhook_url(url).map_err(|_| anyhow::anyhow!(URL_ERROR))?;
                    }
                    (replacement.map(|s| s.trim().to_owned()), true)
                }
                ConnectorProvider::Unsupported { .. } => continue,
            };
            let reference = match replacement {
                Some(secret) => Some(self.stage_secret(&secret)?),
                None if same_provider => old.get(&c.id).cloned(),
                None => None,
            };
            if c.enabled {
                let key = reference.as_ref().ok_or_else(|| {
                    anyhow::anyhow!(if google_chat {
                        "Enabled Google Chat connector requires a webhook URL"
                    } else {
                        "Enabled Telegram connector requires a Bot token"
                    })
                })?;
                let secret=self.credentials.get(key).map_err(|_| anyhow::anyhow!("Cannot read notification credential; unlock the OS credential store and retry"))?
                    .filter(|s| !s.is_empty()).ok_or_else(|| anyhow::anyhow!("Stored notification credential is missing; replace the credential"))?;
                if google_chat {
                    parse_webhook_url(&secret).map_err(|_| anyhow::anyhow!(URL_ERROR))?;
                }
                self.secret_cache
                    .lock()
                    .unwrap()
                    .insert(key.clone(), secret);
            }
            match &mut c.provider {
                ConnectorProvider::Telegram(config) => config.has_token = reference.is_some(),
                ConnectorProvider::GoogleChat(config) => {
                    config.has_webhook_url = reference.is_some()
                }
                _ => {}
            }
            if let Some(reference) = reference {
                references.insert(c.id.clone(), reference);
            }
        }
        Ok(PreparedNotificationSettings {
            connectors,
            references,
        })
    }
    pub(crate) fn notification_token_for(&self, id: &str, reference: &str) -> Result<String> {
        let live = || -> Result<bool> {
            Ok(self
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT reference FROM notification_credential_refs WHERE connector_id=?1",
                    [id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .as_deref()
                == Some(reference))
        };
        ensure!(live()?, "Notification credential unavailable");
        let token = self
            .credentials
            .get(reference)
            .map_err(|_| anyhow::anyhow!("Notification credential unavailable"))?
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Notification credential unavailable"))?;
        ensure!(live()?, "Notification credential unavailable");
        self.secret_cache
            .lock()
            .unwrap()
            .insert(reference.into(), token.clone());
        Ok(token)
    }
}
pub(super) fn read_refs(db: &rusqlite::Connection) -> Result<HashMap<String, String>> {
    let mut query =
        db.prepare("SELECT connector_id,reference FROM notification_credential_refs")?;
    let rows = query.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
pub(super) fn annotate(settings: &mut Settings, refs: &HashMap<String, String>) {
    for c in &mut settings.notification_connectors {
        if let ConnectorProvider::Telegram(config) = &mut c.provider {
            config.bot_token = None;
            config.has_token = refs.contains_key(&c.id);
        }
        if let ConnectorProvider::GoogleChat(config) = &mut c.provider {
            config.webhook_url = None;
            config.has_webhook_url = refs.contains_key(&c.id);
        }
    }
}
pub(super) fn write_notification_refs(
    tx: &Transaction<'_>,
    prepared: &PreparedNotificationSettings,
) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO credential_gc SELECT reference FROM notification_credential_refs",
        [],
    )?;
    tx.execute("DELETE FROM notification_credential_refs", [])?;
    for (id, reference) in &prepared.references {
        tx.execute(
            "INSERT INTO notification_credential_refs VALUES(?1,?2)",
            params![id, reference],
        )?;
        tx.execute("DELETE FROM credential_gc WHERE reference=?1", [reference])?;
    }
    Ok(())
}
