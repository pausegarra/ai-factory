use super::store::Store;
use crate::context::factory::application::dto::connector_notification::ConnectorTarget;
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
        Ok(targets_for_event(&settings.notification_connectors, event)
            .into_iter()
            .filter_map(|c| {
                let ConnectorProvider::Telegram(config) = &c.provider else {
                    return None;
                };
                Some(ConnectorTarget {
                    connector_id: c.id.clone(),
                    chat_id: config.chat_id.clone(),
                    credential_ref: references.get(&c.id)?.clone(),
                })
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
        let old = self.notification_refs()?;
        let mut connectors = settings.notification_connectors.clone();
        let mut references = HashMap::new();
        for c in &mut connectors {
            if let ConnectorProvider::Telegram(config) = &mut c.provider {
                config.chat_id = config.chat_id.trim().into();
                let replacement = config
                    .bot_token
                    .take()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty());
                let reference = match replacement {
                    Some(token) => Some(self.stage_secret(&token)?),
                    None => old.get(&c.id).cloned(),
                };
                if c.enabled {
                    let key = reference.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("Enabled Telegram connector requires a Bot token")
                    })?;
                    // Verify the actual OS entry, not an input hint or a stale cache entry.
                    let token = self.credentials.get(key).map_err(|_| anyhow::anyhow!("Cannot read notification credential; unlock the OS credential store and retry"))?
                        .filter(|s| !s.is_empty()).ok_or_else(|| anyhow::anyhow!("Stored notification credential is missing; replace the Bot token"))?;
                    self.secret_cache.lock().unwrap().insert(key.clone(), token);
                }
                config.has_token = reference.is_some();
                if let Some(reference) = reference {
                    references.insert(c.id.clone(), reference);
                }
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
