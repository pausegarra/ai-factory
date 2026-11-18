use super::store::Store;
use crate::context::factory::application::{
    dto::backup_format::*, interfaces::backup_repository::*,
};
use crate::*;
use rusqlite::{params, Connection, OptionalExtension};
type Result<T> = std::result::Result<T, BackupFailure>;
fn db_error<T>(v: rusqlite::Result<T>) -> Result<T> {
    v.map_err(|_| BackupFailure::persistence_failed())
}
pub(super) fn migrate(db: &Connection) -> crate::Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS backup_import_operations(operation_id TEXT PRIMARY KEY CHECK(length(operation_id)=36),archive_sha256 TEXT NOT NULL CHECK(length(archive_sha256)=64),components_json TEXT NOT NULL,summary_json TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('staged','committed')));CREATE TABLE IF NOT EXISTS backup_staged_credentials(operation_id TEXT NOT NULL REFERENCES backup_import_operations(operation_id),reference TEXT PRIMARY KEY CHECK(length(reference)=36));")?;
    Ok(())
}
impl Store {
    pub(crate) fn backup_pending(&self) -> crate::Result<bool> {
        Ok(self.db.lock().unwrap().query_row(
            "SELECT EXISTS(SELECT 1 FROM backup_import_operations WHERE state='staged')",
            [],
            |r| r.get(0),
        )?)
    }
}
impl BackupRestoreRepository for Store {
    fn recover(&self) -> Result<()> {
        if !self.db.lock().unwrap().is_autocommit() {
            return Err(BackupFailure::new(BackupErrorCode::RecoveryRequired));
        }
        let staged = {
            let db = self.db.lock().unwrap();
            let mut q=db_error(db.prepare("SELECT s.operation_id,s.reference,o.state FROM backup_staged_credentials s JOIN backup_import_operations o USING(operation_id) ORDER BY s.reference"))?;
            let rows = db_error(q.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            }))?;
            db_error(rows.collect::<rusqlite::Result<Vec<_>>>())?
        };
        for (operation, reference, state) in staged {
            let active:bool=db_error(self.db.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM credential_refs WHERE reference=?1 UNION ALL SELECT 1 FROM notification_credential_refs WHERE reference=?1)",[&reference],|r|r.get(0)))?;
            if state != "committed" && !active {
                self.credentials
                    .delete(&reference)
                    .map_err(|_| BackupFailure::new(BackupErrorCode::RecoveryRequired))?;
            }
            db_error(self.db.lock().unwrap().execute(
                "DELETE FROM backup_staged_credentials WHERE operation_id=?1 AND reference=?2",
                params![operation, reference],
            ))?;
        }
        db_error(self.db.lock().unwrap().execute("DELETE FROM backup_import_operations WHERE state='staged' AND NOT EXISTS(SELECT 1 FROM backup_staged_credentials WHERE operation_id=backup_import_operations.operation_id)",[]))?;
        Ok(())
    }
    fn apply(&self, mut plan: RestorePlan) -> Result<BackupOutcome> {
        let _settings = self.settings_lock.lock().unwrap();
        let operation = uuid::Uuid::new_v4().to_string();
        let mut new_refs = Vec::new();
        for secret in &plan.credential_updates.records {
            let reference = uuid::Uuid::new_v4().to_string();
            let config = plan
                .effective_configuration
                .as_mut()
                .ok_or_else(BackupFailure::invalid_archive)?;
            let descriptor = config
                .credentials
                .iter_mut()
                .find(|d| d.identity == secret.identity)
                .ok_or_else(BackupFailure::invalid_archive)?;
            descriptor.source_reference = Some(reference.clone());
            descriptor.available = true;
            new_refs.push(reference);
        }
        if let Some(config) = &mut plan.effective_configuration {
            super::backup::annotate(config);
        }
        let outcome = BackupOutcome {
            committed: true,
            components: plan.selection.components.clone(),
            path: None,
            counts: crate::context::factory::application::dto::backup_format::counts(
                plan.effective_configuration
                    .as_ref()
                    .filter(|_| plan.selection.contains(BackupComponent::Configuration)),
                plan.normalized_history.as_ref(),
                plan.selection
                    .contains(BackupComponent::Credentials)
                    .then_some(&plan.credential_updates),
            ),
            warnings: plan.warnings,
        };
        let summary =
            serde_json::to_string(&outcome).map_err(|_| BackupFailure::persistence_failed())?;
        #[cfg(test)]
        tests::inject(tests::Point::BeforeJournal)
            .map_err(|_| BackupFailure::persistence_failed())?;
        {
            let mut db = self.db.lock().unwrap();
            let tx = db_error(db.transaction())?;
            db_error(tx.execute(
                "INSERT INTO backup_import_operations VALUES(?1,?2,?3,?4,'staged')",
                params![
                        operation,
                        plan.archive_sha256,
                        serde_json::to_string(&plan.selection.components)
                            .map_err(|_| BackupFailure::persistence_failed())?,
                        summary
                    ],
            ))?;
            for reference in &new_refs {
                db_error(tx.execute(
                    "INSERT INTO backup_staged_credentials VALUES(?1,?2)",
                    params![operation, reference],
                ))?;
            }
            db_error(tx.commit())?;
        }
        let staging = (|| -> Result<()> {
            #[cfg(test)]
            tests::inject(tests::Point::AfterJournal)
                .map_err(|_| BackupFailure::persistence_failed())?;
            for (record, reference) in plan.credential_updates.records.iter().zip(&new_refs) {
                self.credentials
                    .set(reference, &record.value)
                    .map_err(|_| BackupFailure::new(BackupErrorCode::CredentialUnavailable))?;
                let value = self
                    .credentials
                    .get(reference)
                    .map_err(|_| BackupFailure::new(BackupErrorCode::CredentialUnavailable))?
                    .map(zeroize::Zeroizing::new);
                if value.as_deref() != Some(&*record.value) {
                    return Err(BackupFailure::new(BackupErrorCode::CredentialUnavailable));
                }
                #[cfg(test)]
                tests::inject(tests::Point::AfterFirstSet)
                    .map_err(|_| BackupFailure::persistence_failed())?;
            }
            Ok(())
        })();
        if let Err(error) = staging {
            return match self.recover() {
                Ok(()) => Err(error),
                Err(_) => Err(BackupFailure::new(BackupErrorCode::RecoveryRequired)),
            };
        }
        let committed = (|| -> crate::Result<()> {
            #[cfg(test)]
            tests::inject(tests::Point::BeforeSql)?;
            let mut db = self.db.lock().unwrap();
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if let Some(history) = &plan.normalized_history {
                super::backup::replace_history(&tx, history)?;
            }
            if let Some(configuration) = &plan.effective_configuration {
                super::backup::replace_configuration(
                    &tx,
                    configuration,
                    plan.selection.contains(BackupComponent::Configuration),
                )?;
            }
            tx.execute(
                "UPDATE backup_import_operations SET state='committed' WHERE operation_id=?1",
                [&operation],
            )?;
            tx.execute(
                "DELETE FROM backup_staged_credentials WHERE operation_id=?1",
                [&operation],
            )?;
            tx.execute(
                "DELETE FROM backup_import_operations WHERE state='committed' AND operation_id!=?1",
                [&operation],
            )?;
            #[cfg(test)]
            tests::inject(tests::Point::BeforeCommit)?;
            tx.commit()?;
            #[cfg(test)]
            tests::inject(tests::Point::AfterCommit)?;
            Ok(())
        })();
        if committed.is_err() {
            // Resolve an ambiguous commit from a fresh reader, never from an
            // unresolved transaction which could still expose uncommitted pointers.
            let state = (|| -> rusqlite::Result<Option<String>> {
                let path = {
                    let db = self.db.lock().unwrap();
                    if !db.is_autocommit() {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    db.path()
                        .map(str::to_owned)
                        .ok_or(rusqlite::Error::InvalidQuery)?
                };
                let reader =
                    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                reader
                    .query_row(
                        "SELECT state FROM backup_import_operations WHERE operation_id=?1",
                        [&operation],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
            })();
            match state {
                Ok(Some(s)) if s == "committed" => (),
                Ok(Some(s)) if s == "staged" => {
                    return match self.recover() {
                        Ok(()) => Err(BackupFailure::persistence_failed()),
                        Err(_) => Err(BackupFailure::new(BackupErrorCode::RecoveryRequired)),
                    };
                }
                _ => return Err(BackupFailure::new(BackupErrorCode::RecoveryRequired)),
            }
        }
        self.secret_cache.lock().unwrap().clear();
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        collections::HashMap,
        sync::{Arc, Mutex},
    };
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub(super) enum Point {
        BeforeJournal,
        AfterJournal,
        AfterFirstSet,
        BeforeSql,
        BeforeCommit,
        AfterCommit,
    }
    thread_local! {static POINT:Cell<Option<Point>>=const {Cell::new(None)};}
    pub(super) fn inject(point: Point) -> crate::Result<()> {
        anyhow::ensure!(
            !POINT.with(|p| p.get() == Some(point)),
            "fixture persistence failure"
        );
        Ok(())
    }
    struct Fault;
    impl Drop for Fault {
        fn drop(&mut self) {
            POINT.with(|p| p.set(None));
        }
    }
    #[derive(Default)]
    struct Memory(Mutex<HashMap<String, String>>);
    impl Credentials for Memory {
        fn get(&self, key: &str) -> crate::Result<Option<String>> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        fn set(&self, key: &str, value: &str) -> crate::Result<()> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        fn delete(&self, key: &str) -> crate::Result<()> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }
    fn seed(store: &Store, number: u64) {
        let mut settings = Settings {
            poll_interval_seconds: 30 + number,
            ..Settings::default()
        };
        settings.token_profiles.push(TokenProfile {
            id: "profile".into(),
            account: "alice".into(),
            label: "Fixture".into(),
            token: Some(format!("fixture-secret-{number}")),
            ..Default::default()
        });
        store.save_settings(&settings).unwrap();
        store
            .insert(
                &TriggerEvent::issue("acme/repo", number, "[AI] fixture", "alice", ""),
                &settings.flows[0],
                1,
                true,
            )
            .unwrap();
    }
    #[test]
    fn backup_recovery_resolves_every_commit_boundary_including_without_secrets() {
        for components in [
            vec![BackupComponent::Configuration],
            vec![BackupComponent::History],
            vec![
                BackupComponent::Configuration,
                BackupComponent::History,
                BackupComponent::Credentials,
            ],
        ] {
            for point in [
                Point::BeforeJournal,
                Point::AfterJournal,
                Point::AfterFirstSet,
                Point::BeforeSql,
                Point::BeforeCommit,
                Point::AfterCommit,
            ] {
                if point == Point::AfterFirstSet
                    && !components.contains(&BackupComponent::Credentials)
                {
                    continue;
                }
                let src = tempfile::tempdir().unwrap();
                let dst = tempfile::tempdir().unwrap();
                let source =
                    Store::open_with_credentials(src.path(), Arc::new(Memory::default())).unwrap();
                let backend = Arc::new(Memory::default());
                let target = Store::open_with_credentials(dst.path(), backend.clone()).unwrap();
                seed(&source, 1);
                seed(&target, 2);
                let selection = BackupSelection::new(components.clone()).unwrap();
                let snapshot = source.capture(&selection).unwrap();
                let before = target.fingerprint().unwrap();
                let old_secrets = backend.0.lock().unwrap().clone();
                let archive = ValidatedBackup {
                    inspection: BackupInspection {
                        archive_id: uuid::Uuid::new_v4().to_string(),
                        archive_sha256: "a".repeat(64),
                        components: components.clone(),
                        encrypted_credentials: components.contains(&BackupComponent::Credentials),
                    },
                    manifest: ManifestV1 {
                        format: "ai-factory-backup".into(),
                        version: 1,
                        archive_id: uuid::Uuid::new_v4().to_string(),
                        created_at: now(),
                        components: components.clone(),
                        entries: Default::default(),
                        encryption: None,
                    },
                    configuration: snapshot.configuration,
                    history: snapshot.history,
                    credentials: snapshot.credentials,
                    warnings: vec![],
                };
                let plan = target.prepare(archive, selection).unwrap();
                POINT.with(|p| p.set(Some(point)));
                let _fault = Fault;
                let outcome = target.apply(plan);
                POINT.with(|p| p.set(None));
                if point == Point::AfterCommit {
                    assert!(outcome.unwrap().committed);
                    assert_ne!(target.fingerprint().unwrap(), before);
                } else {
                    assert!(outcome.is_err(), "{point:?} {components:?}");
                    assert_eq!(target.fingerprint().unwrap(), before, "{point:?}");
                    assert_eq!(*backend.0.lock().unwrap(), old_secrets);
                }
                assert!(!target.backup_pending().unwrap());
                drop(target);
                let reopened = Store::open_with_credentials(dst.path(), backend).unwrap();
                assert!(!reopened.backup_pending().unwrap());
            }
        }
    }
    #[test]
    fn backup_recovery_never_compensates_an_unresolved_sqlite_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = Arc::new(Memory::default());
        let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
        let reference = uuid::Uuid::new_v4().to_string();
        credentials.set(&reference, "fixture-staged").unwrap();
        {
            let db = store.db.lock().unwrap();
            db.execute(
                "INSERT INTO backup_import_operations VALUES(?1,?2,'[]','{}','staged')",
                params![reference, "a".repeat(64)],
            )
            .unwrap();
            db.execute(
                "INSERT INTO backup_staged_credentials VALUES(?1,?1)",
                [&reference],
            )
            .unwrap();
            db.execute_batch("BEGIN IMMEDIATE").unwrap();
        }
        assert!(store.recover().is_err());
        assert!(credentials.get(&reference).unwrap().is_some());
        store.db.lock().unwrap().execute_batch("ROLLBACK").unwrap();
        store.recover().unwrap();
        assert!(credentials.get(&reference).unwrap().is_none());
    }
}
