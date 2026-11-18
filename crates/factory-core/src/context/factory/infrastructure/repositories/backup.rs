use super::store::Store;
use crate::context::factory::application::{
    dto::backup_format::*, interfaces::backup_repository::*,
};
use crate::context::factory::infrastructure::filesystem::backup_archive::{
    digest, portable_observation, portable_observation_owner, validate_configuration,
    validate_history,
};
use crate::*;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};
use zeroize::Zeroizing;
type Result<T> = std::result::Result<T, BackupFailure>;
fn persisted<T>(value: crate::Result<T>) -> Result<T> {
    value.map_err(|_| BackupFailure::persistence_failed())
}
fn json<T: serde::de::DeserializeOwned>(s: String) -> crate::Result<T> {
    Ok(serde_json::from_str(&s)?)
}
pub(super) fn settings_db(db: &Connection) -> crate::Result<Settings> {
    Ok(db
        .query_row("SELECT json FROM settings WHERE id=1", [], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .map(json)
        .transpose()?
        .unwrap_or_default())
}
pub(super) fn descriptors_db(
    db: &Connection,
    s: &Settings,
) -> crate::Result<Vec<CredentialDescriptor>> {
    let mut out = Vec::new();
    for p in &s.token_profiles {
        let reference: Option<String> = db
            .query_row(
                "SELECT reference FROM credential_refs WHERE profile_id=?1 AND lower(account)=?2",
                params![p.id, p.account.to_lowercase()],
                |r| r.get(0),
            )
            .optional()?;
        out.push(CredentialDescriptor {
            identity: CredentialIdentity::Github {
                profile_id: p.id.clone(),
                account: p.account.to_lowercase(),
            },
            available: reference.is_some(),
            source_reference: reference,
        });
    }
    for c in &s.notification_connectors {
        let identity = match c.provider {
            ConnectorProvider::Telegram(_) => CredentialIdentity::Telegram {
                connector_id: c.id.clone(),
            },
            ConnectorProvider::GoogleChat(_) => CredentialIdentity::GoogleChat {
                connector_id: c.id.clone(),
            },
            _ => continue,
        };
        let reference = db
            .query_row(
                "SELECT reference FROM notification_credential_refs WHERE connector_id=?1",
                [&c.id],
                |r| r.get(0),
            )
            .optional()?;
        out.push(CredentialDescriptor {
            identity,
            available: reference.is_some(),
            source_reference: reference,
        });
    }
    Ok(out)
}
fn query<T>(
    db: &Connection,
    sql: &str,
    mapper: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> crate::Result<Vec<T>> {
    let mut q = db.prepare(sql)?;
    let rows = q.query_map([], mapper)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
fn scrub_files(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(o) => {
            for k in [
                "artifacts",
                "workspace",
                "workspace_path",
                "session_id",
                "event_path",
                "result_path",
                "prompt_path",
            ] {
                o.remove(k);
            }
            for v in o.values_mut() {
                scrub_files(v);
            }
        }
        serde_json::Value::Array(a) => {
            for v in a {
                scrub_files(v)
            }
        }
        _ => (),
    }
}
fn portable_value(key: &str, value: serde_json::Value) -> serde_json::Value {
    if key.starts_with("action:")
        || [
            "pr-update:",
            "pr-merge:",
            "pr-version:",
            "release-issue-close:",
            "version-result:",
        ]
        .iter()
        .any(|p| key.starts_with(p))
    {
        super::pr_origins::compact_receipt(&value)
    } else if key.starts_with("review:") {
        let mut value = value;
        if let Some(map) = value.as_object_mut() {
            for row in map.values_mut() {
                if let Some(object) = row.as_object_mut() {
                    object.retain(|key, _| matches!(key.as_str(), "requested" | "generation"));
                }
            }
        }
        value
    } else {
        value
    }
}
fn capture_history(db: &Connection) -> crate::Result<HistoryV1> {
    let ids=query(db,"SELECT id FROM runs WHERE NOT EXISTS(SELECT 1 FROM run_deletions WHERE run_id=runs.id) ORDER BY id",|r|r.get::<_,String>(0))?;
    let mut runs = Vec::new();
    let included = ids.iter().cloned().collect::<BTreeSet<_>>();
    for id in ids {
        let mut detail = super::run_deletions::detail_db(db, &id)?;
        detail.run.workspace = None;
        for n in &mut detail.nodes {
            n.session_id = None;
            if let Some(v) = &mut n.result {
                scrub_files(v);
            }
        }
        for n in &mut detail.flow.nodes {
            n.codex_account_id = None;
        }
        scrub_files(&mut detail.event.payload);
        runs.push(RunRowV1 { detail });
    }
    let mut dedup = query(
        db,
        "SELECT event_id,run_id FROM dedup ORDER BY event_id",
        |r| {
            Ok(DedupRowV1 {
                event_id: r.get(0)?,
                run_id: r.get(1)?,
            })
        },
    )?;
    for row in &mut dedup {
        if !included.contains(&row.run_id)
            && !matches!(row.run_id.as_str(), "consumed" | "legacy-consumed")
        {
            row.run_id = "consumed".into();
        }
    }
    let observations = query(db, "SELECT key,json FROM observations ORDER BY key", |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?
    .into_iter()
    .filter(|(k, _)| {
        portable_observation(k)
            && portable_observation_owner(k).is_none_or(|owner| included.contains(owner))
    })
    .map(|(key, raw)| {
        let value = portable_value(&key, serde_json::from_str(&raw)?);
        Ok(ObservationRowV1 { key, value })
    })
    .collect::<crate::Result<Vec<_>>>()?;
    let pr_origins=query(db,"SELECT repository,number,origin_key,event_id,issue_repository,issue_number,actor,trigger_id,trusted FROM tracked_pr_origins ORDER BY repository,number,origin_key",|r|Ok(PrOriginRowV1 {repository:r.get(0)?,number:r.get::<_,i64>(1)?.try_into().map_err(|_|rusqlite::Error::InvalidQuery)?,origin_key:r.get(2)?,event_id:r.get(3)?,issue_repository:r.get(4)?,issue_number:r.get::<_,Option<i64>>(5)?.map(u64::try_from).transpose().map_err(|_|rusqlite::Error::InvalidQuery)?,actor:r.get(6)?,trigger_id:r.get(7)?,trusted:r.get(8)?}))?;
    let comments=query(db,"SELECT key,run_id,node_id,revision,snapshot,state,failures,next_at,last_error,receipt FROM issue_node_comments WHERE NOT EXISTS(SELECT 1 FROM run_deletions WHERE run_id=issue_node_comments.run_id) ORDER BY key",|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get(6)?,r.get::<_,String>(7)?,r.get::<_,Option<String>>(8)?,r.get::<_,Option<String>>(9)?)))?.into_iter().filter(|r|included.contains(&r.1)).map(|r|Ok(CommentRowV1 {key:r.0,run_id:r.1,node_id:r.2,revision:r.3,snapshot:serde_json::from_str(&r.4)?,state:r.5,failures:r.6,next_at:r.7,last_error:r.8,receipt:r.9.map(|s|serde_json::from_str(&s)).transpose()?})).collect::<crate::Result<_>>()?;
    Ok(HistoryV1 {
        runs,
        dedup,
        observations,
        pr_origins,
        comments,
    })
}
impl BackupRepository for Store {
    fn capture(&self, selection: &BackupSelection) -> Result<BackupSnapshot> {
        let _settings = self.settings_lock.lock().unwrap();
        let (settings, mut descriptors, history, all_refs) = persisted((|| {
            let mut db = self.db.lock().unwrap();
            let tx = db.transaction()?;
            let settings = settings_db(&tx)?;
            let descriptors = descriptors_db(&tx, &settings)?;
            let history = if selection.contains(BackupComponent::History) {
                Some(capture_history(&tx)?)
            } else {
                None
            };
            let refs=query(&tx,"SELECT reference FROM credential_refs UNION SELECT reference FROM notification_credential_refs ORDER BY reference",|r|r.get::<_,String>(0))?;
            tx.commit()?;
            Ok((settings, descriptors, history, refs))
        })())?;
        let mut secrets = BTreeMap::new();
        let mut redaction_secrets = Vec::new();
        for reference in all_refs {
            let secret = self
                .credentials
                .get(&reference)
                .map_err(|_| BackupFailure::new(BackupErrorCode::CredentialUnavailable))?
                .map(Zeroizing::new);
            if let Some(value) = &secret {
                redaction_secrets.push(Zeroizing::new(value.to_string()));
            }
            secrets.insert(reference, secret);
        }
        let mut credentials = SecretBundle::default();
        let mut warnings = vec![];
        for descriptor in &mut descriptors {
            let value = descriptor
                .source_reference
                .as_ref()
                .and_then(|r| secrets.get(r))
                .and_then(Option::as_ref)
                .filter(|s| !s.is_empty());
            descriptor.available = value.is_some();
            if let Some(value) = value {
                credentials.records.push(SecretRecord {
                    identity: descriptor.identity.clone(),
                    source_reference: descriptor.source_reference.clone(),
                    value: Zeroizing::new(value.to_string()),
                });
            } else {
                credentials.missing.push(descriptor.identity.clone());
                warnings.push(crate::context::factory::application::dto::backup::warning(
                    "credential_missing",
                    BackupComponent::Credentials,
                ));
            }
        }
        let configuration = selection
            .contains(BackupComponent::Configuration)
            .then_some(ConfigurationV1 {
                settings,
                credentials: descriptors,
            });
        if history.is_some() {
            warnings.push(crate::context::factory::application::dto::backup::warning(
                "external_files_omitted",
                BackupComponent::History,
            ));
        }
        if !selection.contains(BackupComponent::Credentials) && !credentials.records.is_empty() {
            warnings.push(crate::context::factory::application::dto::backup::warning(
                "credential_omitted",
                BackupComponent::Credentials,
            ));
        }
        Ok(BackupSnapshot {
            configuration,
            history,
            credentials: selection
                .contains(BackupComponent::Credentials)
                .then_some(credentials),
            redaction_secrets,
            warnings,
        })
    }
    fn fingerprint(&self) -> Result<String> {
        persisted((|| {
            let mut db = self.db.lock().unwrap();
            let tx = db.transaction()?;
            let mut bytes = Vec::new();
            for table in [
                "settings",
                "runs",
                "nodes",
                "dedup",
                "observations",
                "credential_refs",
                "notification_credential_refs",
                "agent_accounts",
                "run_file_origins",
                "run_deletions",
                "run_deletion_targets",
                "tracked_pr_origins",
                "issue_node_comments",
            ] {
                bytes.extend_from_slice(table.as_bytes());
                let mut q = tx.prepare(&format!("SELECT * FROM {table}"))?;
                let count = q.column_count();
                let mut rows = q
                    .query_map([], |r| {
                        (0..count)
                            .map(|i| r.get_ref(i).map(|v| format!("{v:?}")))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows.sort();
                bytes.extend(serde_json::to_vec(&rows)?);
            }
            tx.commit()?;
            Ok(digest(&bytes))
        })())
    }
    fn prepare(
        &self,
        mut archive: ValidatedBackup,
        selection: BackupSelection,
    ) -> Result<RestorePlan> {
        let _settings = self.settings_lock.lock().unwrap();
        selection.validate_present(&archive.inspection.components)?;
        if selection.contains(BackupComponent::History)
            && !persisted(self.pending_deletions())?.is_empty()
        {
            return Err(BackupFailure::new(BackupErrorCode::CleanupBusy));
        }
        let mut warnings = archive.warnings;
        let mut credentials = archive.credentials.take().unwrap_or_default();
        let effective_configuration = if selection.contains(BackupComponent::Configuration)
            || selection.contains(BackupComponent::Credentials)
        {
            let (local, local_descriptors, accounts) = persisted((|| {
                let db = self.db.lock().unwrap();
                let settings = settings_db(&db)?;
                let descriptors = descriptors_db(&db, &settings)?;
                let accounts=query(&db,"SELECT id FROM agent_accounts WHERE json_extract(json,'$.status')='ready' ORDER BY id",|r|r.get::<_,String>(0))?;
                let accounts = if selection.contains(BackupComponent::Configuration) {
                    let root = db
                        .path()
                        .and_then(|path| std::path::Path::new(path).parent())
                        .ok_or_else(|| anyhow::anyhow!("Invalid local account root"))?;
                    accounts.into_iter().filter(|id|crate::context::factory::infrastructure::filesystem::agent_home::existing_home(root,id).is_ok()).collect()
                } else {
                    accounts
                };
                Ok((settings, descriptors, accounts))
            })())?;
            let mut config = archive.configuration.take().unwrap_or(ConfigurationV1 {
                settings: local.clone(),
                credentials: local_descriptors.clone(),
            });
            if selection.contains(BackupComponent::Configuration) {
                validate_configuration(&config)?;
            }
            let expected = persisted({
                let db = self.db.lock().unwrap();
                descriptors_db(&db, &config.settings)
            })?
            .into_iter()
            .map(|d| d.identity)
            .collect::<BTreeSet<_>>();
            if config
                .credentials
                .iter()
                .any(|d| !expected.contains(&d.identity))
                || config.credentials.len() != expected.len()
                || credentials
                    .records
                    .iter()
                    .any(|r| !expected.contains(&r.identity))
                || (selection.contains(BackupComponent::Configuration)
                    && credentials.missing.iter().any(|r| !expected.contains(r)))
            {
                return Err(BackupFailure::new(BackupErrorCode::CredentialTargetMissing));
            }
            if selection.contains(BackupComponent::Configuration) {
                for record in &credentials.records {
                    let descriptor = config
                        .credentials
                        .iter()
                        .find(|d| d.identity == record.identity)
                        .ok_or_else(BackupFailure::invalid_archive)?;
                    if descriptor.source_reference != record.source_reference
                        || !descriptor.available
                    {
                        return Err(BackupFailure::invalid_archive());
                    }
                }
            }
            for descriptor in &mut config.credentials {
                let update = credentials
                    .records
                    .iter()
                    .find(|r| r.identity == descriptor.identity);
                let local = local_descriptors.iter().find(|d| {
                    d.identity == descriptor.identity
                        && (!selection.contains(BackupComponent::Configuration)
                            || d.source_reference == descriptor.source_reference)
                });
                descriptor.source_reference = local.and_then(|d| d.source_reference.clone());
                descriptor.available = update.is_some();
                if update.is_none() {
                    descriptor.available = if let Some(reference) = &descriptor.source_reference {
                        self.credentials
                            .get(reference)
                            .map_err(|_| {
                                BackupFailure::new(BackupErrorCode::CredentialUnavailable)
                            })?
                            .is_some_and(|s| !s.is_empty())
                    } else {
                        false
                    };
                    if !descriptor.available {
                        descriptor.source_reference = None;
                        warnings.push(crate::context::factory::application::dto::backup::warning(
                            "credential_missing",
                            BackupComponent::Credentials,
                        ));
                    }
                }
            }
            if selection.contains(BackupComponent::Configuration) {
                config.settings.start_at_login = local.start_at_login;
                let previous = config.settings.codex_bindings.len();
                config
                    .settings
                    .codex_bindings
                    .retain(|_, account| accounts.contains(account));
                if previous != config.settings.codex_bindings.len() {
                    warnings.push(crate::context::factory::application::dto::backup::warning(
                        "codex_binding_missing",
                        BackupComponent::Configuration,
                    ));
                }
                for flow in &mut config.settings.flows {
                    for node in &mut flow.nodes {
                        if node
                            .codex_account_id
                            .as_ref()
                            .is_some_and(|a| !accounts.contains(a))
                        {
                            node.codex_account_id = None;
                            warnings.push(
                                crate::context::factory::application::dto::backup::warning(
                                    "codex_binding_missing",
                                    BackupComponent::Configuration,
                                ),
                            );
                        }
                    }
                }
            }
            annotate(&mut config);
            Some(config)
        } else {
            None
        };
        let history = archive.history.take();
        let mut history = history;
        if let Some(h) = &mut history {
            validate_history(h)?;
            for row in &mut h.runs {
                let d = &mut row.detail;
                let active = matches!(d.run.status.as_str(), "queued" | "running");
                if active {
                    d.run.status = "interrupted".into();
                    d.run.current_step = None;
                    d.run
                        .finished_at
                        .get_or_insert(archive.manifest.created_at.clone());
                    d.run.error = Some("Imported history; retry explicitly to run again".into());
                }
                d.run.workspace = None;
                for n in &mut d.nodes {
                    n.session_id = None;
                    if active && matches!(n.status.as_str(), "pending" | "running") {
                        n.status = "interrupted".into();
                        n.finished_at
                            .get_or_insert(archive.manifest.created_at.clone());
                    }
                    if let Some(v) = &mut n.result {
                        scrub_files(v);
                    }
                }
                for n in &mut d.flow.nodes {
                    n.codex_account_id = None;
                }
                scrub_files(&mut d.event.payload);
            }
            for row in &mut h.comments {
                if row.state == "pending" {
                    row.state = "exhausted".into();
                    row.last_error = Some("imported_history".into());
                }
            }
            for row in &mut h.pr_origins {
                row.trusted = false;
            }
            for row in &mut h.observations {
                row.value = portable_value(&row.key, row.value.take());
            }
            warnings.push(crate::context::factory::application::dto::backup::warning(
                "external_files_omitted",
                BackupComponent::History,
            ));
        }
        credentials.missing.clear();
        Ok(RestorePlan {
            archive_sha256: archive.inspection.archive_sha256,
            effective_configuration,
            normalized_history: history,
            credential_updates: credentials,
            selection,
            warnings,
        })
    }
}
pub(super) fn annotate(config: &mut ConfigurationV1) {
    for p in &mut config.settings.token_profiles {
        p.token = None;
        p.has_token = config.credentials.iter().any(|d| {
            d.available
                && d.identity
                    == CredentialIdentity::Github {
                        profile_id: p.id.clone(),
                        account: p.account.to_lowercase(),
                    }
        });
    }
    for c in &mut config.settings.notification_connectors {
        let available = config.credentials.iter().any(|d| {
            d.available
                && match &d.identity {
                    CredentialIdentity::Telegram { connector_id }
                    | CredentialIdentity::GoogleChat { connector_id } => connector_id == &c.id,
                    _ => false,
                }
        });
        match &mut c.provider {
            ConnectorProvider::Telegram(v) => {
                v.bot_token = None;
                v.has_token = available;
            }
            ConnectorProvider::GoogleChat(v) => {
                v.webhook_url = None;
                v.has_webhook_url = available;
            }
            _ => (),
        };
        if !available {
            c.enabled = false;
        }
    }
}
pub(super) fn replace_history(tx: &rusqlite::Transaction<'_>, h: &HistoryV1) -> crate::Result<()> {
    tx.execute_batch("DELETE FROM nodes;DELETE FROM issue_node_comments;DELETE FROM runs;DELETE FROM dedup;DELETE FROM tracked_pr_origins;")?;
    let keys = query(tx, "SELECT key FROM observations ORDER BY key", |r| {
        r.get::<_, String>(0)
    })?;
    for key in keys {
        if portable_observation(&key) {
            tx.execute("DELETE FROM observations WHERE key=?1", [key])?;
        }
    }
    for row in &h.runs {
        let d = &row.detail;
        let r = &d.run;
        tx.execute(
            "INSERT INTO runs VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                r.id,
                r.event_id,
                serde_json::to_string(r)?,
                serde_json::to_string(&d.event)?,
                serde_json::to_string(&d.flow)?,
                r.created_at
            ],
        )?;
        tx.execute(
            "INSERT INTO run_file_origins VALUES(?1,'imported')",
            [&r.id],
        )?;
        for n in &d.nodes {
            tx.execute(
                "INSERT INTO nodes VALUES(?1,?2,?3)",
                params![r.id, n.id, serde_json::to_string(n)?],
            )?;
        }
    }
    for row in &h.dedup {
        tx.execute(
            "INSERT INTO dedup VALUES(?1,?2)",
            params![row.event_id, row.run_id],
        )?;
    }
    for row in &h.observations {
        tx.execute(
            "INSERT INTO observations VALUES(?1,?2)",
            params![row.key, serde_json::to_string(&row.value)?],
        )?;
    }
    for p in &h.pr_origins {
        tx.execute(
            "INSERT INTO tracked_pr_origins VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0)",
            params![
                p.repository,
                i64::try_from(p.number)?,
                p.origin_key,
                p.event_id,
                p.issue_repository,
                p.issue_number.map(i64::try_from).transpose()?,
                p.actor,
                p.trigger_id
            ],
        )?;
    }
    for c in &h.comments {
        tx.execute(
            "INSERT INTO issue_node_comments VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                c.key,
                c.run_id,
                c.node_id,
                c.revision,
                serde_json::to_string(&c.snapshot)?,
                c.state,
                c.failures,
                c.next_at,
                c.last_error,
                c.receipt.as_ref().map(serde_json::to_string).transpose()?
            ],
        )?;
    }
    Ok(())
}
pub(super) fn replace_configuration(
    tx: &rusqlite::Transaction<'_>,
    c: &ConfigurationV1,
    replace_all: bool,
) -> crate::Result<()> {
    tx.execute(
        "INSERT INTO settings VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json",
        [serde_json::to_string(&c.settings)?],
    )?;
    if replace_all {
        tx.execute_batch("DELETE FROM credential_refs;DELETE FROM notification_credential_refs;")?;
    }
    for d in &c.credentials {
        if let Some(reference) = &d.source_reference {
            match &d.identity {
                CredentialIdentity::Github {
                    profile_id,
                    account,
                } => {
                    tx.execute(
                        "INSERT INTO credential_refs VALUES(?1,?2,?3) ON CONFLICT(profile_id) DO UPDATE SET account=excluded.account,reference=excluded.reference",
                        params![profile_id, account, reference],
                    )?;
                }
                CredentialIdentity::Telegram { connector_id }
                | CredentialIdentity::GoogleChat { connector_id } => {
                    tx.execute(
                        "INSERT INTO notification_credential_refs VALUES(?1,?2) ON CONFLICT(connector_id) DO UPDATE SET reference=excluded.reference",
                        params![connector_id, reference],
                    )?;
                }
            }
        }
    }
    Ok(())
}
