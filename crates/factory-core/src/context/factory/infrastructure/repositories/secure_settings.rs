use super::store::Store;
use crate::context::factory::domain::entities::models::{
    normalize_issue_title_prefix, normalize_release_issue_title_prefix,
};
use crate::*;
use anyhow::ensure;
use rusqlite::OptionalExtension;
use rusqlite::{params, Transaction};
use std::collections::HashMap;

struct Reference {
    account: String,
    key: String,
}

impl Store {
    fn metadata_with_refs(&self) -> Result<(Settings, HashMap<String, Reference>)> {
        // One short SQLite snapshot; no keychain I/O and no writer lock on read paths.
        let db = self.db.lock().unwrap();
        let raw: Option<String> = db
            .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
            .optional()?;
        let mut settings = raw
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default();
        let mut query =
            db.prepare("SELECT connector_id,account,reference FROM source_credential_refs")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                Reference {
                    account: row.get(1)?,
                    key: row.get(2)?,
                },
            ))
        })?;
        let refs = rows.collect::<rusqlite::Result<HashMap<_, _>>>()?;
        super::secure_notifications::annotate(
            &mut settings,
            &super::secure_notifications::read_refs(&db)?,
        );
        Ok((settings, refs))
    }
    fn source_credential_refs(&self) -> Result<HashMap<String, Reference>> {
        let db = self.db.lock().unwrap();
        let mut query =
            db.prepare("SELECT connector_id,account,reference FROM source_credential_refs")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                Reference {
                    account: row.get(1)?,
                    key: row.get(2)?,
                },
            ))
        })?;
        rows.map(|row| Ok(row?)).collect()
    }

    fn secret(&self, reference: &str) -> Result<Option<String>> {
        if let Some(secret) = self.secret_cache.lock().unwrap().get(reference).cloned() {
            return Ok(Some(secret));
        }
        let secret = self
            .credentials
            .get(reference)
            .map_err(|_| anyhow::anyhow!("Cannot read OS credential store; unlock it and retry"))?;
        if let Some(secret) = &secret {
            self.secret_cache
                .lock()
                .unwrap()
                .insert(reference.into(), secret.clone());
        }
        Ok(secret)
    }

    /// Resolve the exact current connector, without consulting legacy references or logins.
    pub async fn token_for_source(&self, source_id: &str) -> Result<String> {
        let reference = {
            let (settings, mut refs) = self.metadata_with_refs()?;
            let source = settings
                .source_by_id(source_id)
                .ok_or_else(|| anyhow::anyhow!("Source connector unavailable"))?;
            refs.remove(source_id)
                .filter(|r| r.account.eq_ignore_ascii_case(&source.account))
                .ok_or_else(|| anyhow::anyhow!("No API token configured for source connector"))?
                .key
        };
        let credentials = self.credentials.clone();
        let cache = self.secret_cache.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(secret) = cache.lock().unwrap().get(&reference).cloned() {
                return Ok(secret);
            }
            let secret = credentials
                .get(&reference)
                .map_err(|_| {
                    anyhow::anyhow!("Cannot read OS credential store; unlock it and retry")
                })?
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Stored GitHub credential is missing; replace the token in Sources"
                    )
                })?;
            cache.lock().unwrap().insert(reference, secret.clone());
            Ok(secret)
        })
        .await
        .map_err(|_| anyhow::anyhow!("OS credential reader stopped"))?
    }

    /// Used before displaying persisted logs on a blocking caller; fail closed if secrets cannot be loaded.
    pub fn load_redaction_secrets(&self) -> Result<()> {
        for reference in self.source_credential_refs()?.values() {
            ensure!(
                self.secret(&reference.key)?.is_some(),
                "Cannot display historical logs: a stored GitHub credential is missing"
            );
        }
        for reference in self.notification_refs()?.values() {
            ensure!(
                self.secret(reference)?.is_some(),
                "Cannot display historical logs: a stored notification credential is missing"
            );
        }
        Ok(())
    }

    /// Every token used by an agent/GitHub call is cached before use. Redaction does no keychain I/O.
    pub fn redact_cached(&self, text: &str) -> String {
        self.redact_window(text.as_bytes(), 0..text.len())
    }

    pub fn redaction_overlap(&self) -> usize {
        self.secret_cache
            .lock()
            .unwrap()
            .values()
            .flat_map(|s| redaction_patterns(s))
            .map(|s| s.len().saturating_sub(1))
            .max()
            .unwrap_or(0)
    }

    pub fn redact_window(&self, bytes: &[u8], range: std::ops::Range<usize>) -> String {
        let cache = self.secret_cache.lock().unwrap();
        let mut spans = Vec::new();
        for secret in cache
            .values()
            .flat_map(|s| redaction_patterns(s))
            .filter(|s| !s.is_empty())
        {
            for (match_start, window) in bytes.windows(secret.len()).enumerate() {
                if window == secret.as_bytes() {
                    let start = match_start.max(range.start);
                    let end = (match_start + secret.len()).min(range.end);
                    if start < end {
                        spans.push((start, end));
                    }
                }
            }
        }
        spans.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (start, end) in spans {
            if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        let mut out = String::new();
        let mut position = range.start;
        for (start, end) in merged {
            out.push_str(&String::from_utf8_lossy(&bytes[position..start]));
            out.push_str("[REDACTED]");
            position = end;
        }
        out.push_str(&String::from_utf8_lossy(&bytes[position..range.end]));
        out
    }

    /// Public configuration contains only metadata, even while the keychain is locked.
    pub fn public_settings(&self) -> Result<Settings> {
        let (mut settings, refs) = self.metadata_with_refs()?;
        for profile in &mut settings.source_connectors {
            profile.token = None;
            profile.has_token = refs
                .get(&profile.id)
                .is_some_and(|r| r.account.eq_ignore_ascii_case(&profile.account));
        }
        Ok(settings)
    }

    /// Hydrated secrets stay inside the engine and are never serialized to SQLite.
    pub fn settings(&self) -> Result<Settings> {
        let (mut settings, refs) = self.metadata_with_refs()?;
        for profile in &mut settings.source_connectors {
            profile.token = match refs
                .get(&profile.id)
                .filter(|r| r.account.eq_ignore_ascii_case(&profile.account))
            {
                Some(reference) => Some(
                    self.secret(&reference.key)?
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Stored GitHub credential is missing; replace the token in Sources"
                            )
                        })?,
                ),
                None => None,
            };
            profile.has_token = profile.token.is_some();
        }
        Ok(settings)
    }

    fn prepare_settings(
        &self,
        settings: &Settings,
    ) -> Result<(Settings, HashMap<String, Reference>)> {
        let mut clean = settings.clone();
        clean.issue_title_prefix = normalize_issue_title_prefix(&clean.issue_title_prefix)?;
        clean.release_issue_title_prefix =
            normalize_release_issue_title_prefix(&clean.release_issue_title_prefix)?;
        clean.validate()?;
        let old = self.raw_settings()?;
        let old_refs = self.source_credential_refs()?;
        let mut refs = HashMap::new();
        for profile in &mut clean.source_connectors {
            let existing = old_refs
                .get(&profile.id)
                .filter(|r| r.account.eq_ignore_ascii_case(&profile.account));
            let previous = old
                .source_connectors
                .iter()
                .find(|p| p.id == profile.id && p.account.eq_ignore_ascii_case(&profile.account));
            let token = profile
                .token
                .take()
                .filter(|t| !t.trim().is_empty())
                .or_else(|| {
                    previous
                        .and_then(|p| p.token.clone())
                        .filter(|t| !t.trim().is_empty())
                });
            let key = if let Some(token) = token {
                if let Some(existing) = existing
                    .filter(|r| self.secret_cache.lock().unwrap().get(&r.key) == Some(&token))
                {
                    Some(existing.key.clone())
                } else if let Some(existing) =
                    existing.filter(|r| self.secret(&r.key).ok().flatten().as_ref() == Some(&token))
                {
                    Some(existing.key.clone())
                } else {
                    let key = self.stage_secret(&token)?;
                    Some(key)
                }
            } else {
                existing.map(|r| r.key.clone())
            };
            ensure!(key.is_some(), "Source connector token is required");
            profile.has_token = key.is_some();
            if let Some(key) = key {
                refs.insert(
                    profile.id.clone(),
                    Reference {
                        account: profile.account.clone(),
                        key,
                    },
                );
            }
        }
        Ok((clean, refs))
    }

    pub(super) fn stage_secret(&self, token: &str) -> Result<String> {
        let key = uuid::Uuid::new_v4().to_string();
        self.db
            .lock()
            .unwrap()
            .execute("INSERT INTO credential_gc VALUES(?1)", [&key])?;
        self.credentials.set(&key, token).map_err(|_| anyhow::anyhow!("Cannot save token in OS credential store; unlock it and retry. Settings were not changed"))?;
        let verified = self.credentials.get(&key).map_err(|_| {
            anyhow::anyhow!("Cannot verify saved OS credential. Settings were not changed")
        })?;
        ensure!(
            verified.as_deref() == Some(token),
            "OS credential verification failed. Settings were not changed"
        );
        self.secret_cache
            .lock()
            .unwrap()
            .insert(key.clone(), token.into());
        Ok(key)
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.write_with_settings(Some(settings), |_| Ok(()))
    }

    pub(super) fn write_with_settings<T>(
        &self,
        settings: Option<&Settings>,
        write: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let _guard = self.settings_lock.lock().unwrap();
        let result = (|| {
            let prepared = settings.map(|s| self.prepare_settings(s)).transpose()?;
            let notifications = settings
                .map(|s| self.prepare_notification_settings(s))
                .transpose()?;
            let mut db = self.db.lock().unwrap();
            let tx = db.transaction()?;
            if let Some((mut clean, refs)) = prepared {
                if let Some(notifications) = notifications {
                    super::secure_notifications::write_notification_refs(&tx, &notifications)?;
                    clean.notification_connectors = notifications.connectors;
                }
                tx.execute(
                    "INSERT OR IGNORE INTO credential_gc SELECT reference FROM source_credential_refs",
                    [],
                )?;
                tx.execute("DELETE FROM source_credential_refs", [])?;
                for (id, reference) in refs {
                    tx.execute(
                        "INSERT INTO source_credential_refs VALUES(?1,?2,?3)",
                        params![id, reference.account, reference.key],
                    )?;
                    tx.execute(
                        "DELETE FROM credential_gc WHERE reference=?1",
                        [&reference.key],
                    )?;
                }
                tx.execute("INSERT INTO settings(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json", [serde_json::to_string(&clean)?])?;
            }
            let value = write(&tx)?;
            tx.commit()?;
            Ok(value)
        })();
        // Failed deletions remain queued for retry; never report a committed save as rolled back.
        self.collect_credentials();
        result
    }

    fn collect_credentials(&self) {
        let pending = (|| -> Result<Vec<String>> {
            let db = self.db.lock().unwrap();
            let mut query = db.prepare("SELECT reference FROM credential_gc WHERE reference NOT IN (SELECT reference FROM source_credential_refs UNION SELECT reference FROM notification_credential_refs)")?;
            let rows = query.query_map([], |row| row.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })();
        for key in pending.unwrap_or_default() {
            if self.credentials.delete(&key).is_ok() {
                let _ = self
                    .db
                    .lock()
                    .unwrap()
                    .execute("DELETE FROM credential_gc WHERE reference=?1", [&key]);
            }
        }
    }

    pub(super) fn migrate_credentials(&self) -> Result<()> {
        // Scrub the original JSON before decoding: legacy scopes/profiles can be malformed.
        // Never hydrate, copy or stage a legacy secret, including matching connector IDs.
        {
            let mut db = self.db.lock().unwrap();
            let raw: Option<String> = db
                .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
                .optional()?;
            let tx = db.transaction()?;
            if let Some(raw) = raw {
                let mut value: serde_json::Value = serde_json::from_str(&raw)?;
                if let Some(map) = value.as_object_mut() {
                    let mut removed = false;
                    for key in ["token_profiles", "owners", "authors", "reviewers"] {
                        removed |= map.remove(key).is_some();
                    }
                    if removed {
                        tx.execute("INSERT OR REPLACE INTO observations VALUES('credential-scrub-pending','true')", [])?;
                        tx.execute(
                            "UPDATE settings SET json=?1 WHERE id=1",
                            [serde_json::to_string(&value)?],
                        )?;
                    }
                }
            }
            tx.execute(
                "INSERT OR IGNORE INTO credential_gc SELECT reference FROM credential_refs",
                [],
            )?;
            tx.execute("DELETE FROM credential_refs", [])?;
            tx.commit()?;
        }
        let pending: bool = self.db.lock().unwrap().query_row(
            "SELECT EXISTS(SELECT 1 FROM observations WHERE key='credential-scrub-pending')",
            [],
            |row| row.get(0),
        )?;
        if pending {
            let db = self.db.lock().unwrap();
            db.execute_batch("VACUUM;")?;
            let (busy, _, _): (i64, i64, i64) =
                db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?;
            ensure!(busy == 0, "Credential migration needs exclusive database access; close other database readers and retry");
            db.execute(
                "DELETE FROM observations WHERE key='credential-scrub-pending'",
                [],
            )?;
        }
        self.collect_credentials();
        Ok(())
    }
}

fn redaction_patterns(secret: &str) -> Vec<String> {
    let mut patterns = vec![secret.to_owned()];
    if let Ok(url) = reqwest::Url::parse(secret) {
        for (raw, (key, value)) in url
            .query()
            .unwrap_or("")
            .split('&')
            .filter(|p| !p.is_empty())
            .zip(url.query_pairs())
        {
            if matches!(key.as_ref(), "key" | "token") {
                if !value.is_empty() {
                    patterns.push(value.into_owned());
                }
                if let Some((_, raw_value)) = raw.split_once('=') {
                    if !raw_value.is_empty() {
                        patterns.push(raw_value.into());
                    }
                }
            }
        }
    }
    patterns
}
