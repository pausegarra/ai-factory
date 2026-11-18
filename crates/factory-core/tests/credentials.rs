mod common;
use factory_core::context::factory::infrastructure::{
    repositories::store::Store, security::native_credentials::NativeCredentials,
};
use factory_core::*;
use rusqlite::Connection;
use std::{
    path::Path,
    sync::{atomic::Ordering, Arc},
};

fn with_token(token: &str) -> Settings {
    let mut settings = Settings::default();
    settings.source_connectors.push(SourceConnector {
        kind: factory_core::SourceKind::GithubIssues,
        id: "alice".into(),
        label: "Alice".into(),
        account: "alice".into(),
        owners: vec!["acme".into()],
        authors: vec!["alice".into()],
        reviewers: vec!["alice".into()],
        token: Some(token.into()),
        has_token: false,
    });
    settings
}
fn raw(root: &Path) -> String {
    Connection::open(root.join("factory.sqlite3"))
        .unwrap()
        .query_row("SELECT json FROM settings", [], |r| r.get(0))
        .unwrap()
}
fn seed_plaintext(root: &Path, token: &str) {
    let db = Connection::open(root.join("factory.sqlite3")).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE settings (id INTEGER PRIMARY KEY,json TEXT NOT NULL);").unwrap();
    db.execute(
        "INSERT INTO settings VALUES(1,?1)",
        [serde_json::json!({"token_profiles":[{"id":"alice","account":"alice","label":"Old","token":token}],"owners":["old"]}).to_string()],
    )
    .unwrap();
}

#[test]
fn github_tokens_are_not_written_to_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.source_connectors.push(SourceConnector {
        kind: factory_core::SourceKind::GithubIssues,
        id: "alice".into(),
        label: "Alice".into(),
        account: "alice".into(),
        owners: vec!["acme".into()],
        authors: vec!["alice".into()],
        reviewers: vec!["alice".into()],
        token: Some("fixture-token-must-not-reach-sqlite".into()),
        has_token: false,
    });
    factory.save_settings(settings).unwrap();
    let db = Connection::open(dir.path().join("factory.sqlite3")).unwrap();
    let raw: String = db
        .query_row("SELECT json FROM settings", [], |r| r.get(0))
        .unwrap();
    assert!(
        !raw.contains("fixture-token-must-not-reach-sqlite"),
        "Token persisted in plaintext"
    );
    assert!(factory.settings().unwrap().source_connectors[0].has_token);
}

#[test]
fn legacy_tokens_are_discarded_and_live_database_files_are_scrubbed() {
    let dir = tempfile::tempdir().unwrap();
    let token = "legacy-fixture-secret-uniquely-searchable";
    seed_plaintext(dir.path(), token);
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    assert!(store.settings().unwrap().source_connectors.is_empty());
    assert!(credentials.gets.lock().unwrap().is_empty());
    assert!(credentials.values.lock().unwrap().is_empty());
    assert!(!raw(dir.path()).contains(token));
    for name in [
        "factory.sqlite3",
        "factory.sqlite3-wal",
        "factory.sqlite3-shm",
    ] {
        if let Ok(bytes) = std::fs::read(dir.path().join(name)) {
            assert!(
                !bytes
                    .windows(token.len())
                    .any(|window| window == token.as_bytes()),
                "Secret remains in {name}"
            );
        }
    }
    drop(store);
    let reopened = Store::open_with_credentials(dir.path(), credentials).unwrap();
    assert!(reopened.settings().unwrap().source_connectors.is_empty());
}

#[test]
fn failed_legacy_scrub_is_atomic_and_can_be_retried() {
    let dir = tempfile::tempdir().unwrap();
    seed_plaintext(dir.path(), "migration-fixture-secret");
    let c = Arc::new(common::MemoryCredentials::default());
    let db = Connection::open(dir.path().join("factory.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER deny_scrub BEFORE UPDATE ON settings BEGIN SELECT RAISE(ABORT,'fixture scrub denied'); END;").unwrap();
    assert!(Store::open_with_credentials(dir.path(), c.clone()).is_err());
    assert!(raw(dir.path()).contains("migration-fixture-secret"));
    assert!(c.values.lock().unwrap().is_empty());
    db.execute_batch("DROP TRIGGER deny_scrub").unwrap();
    drop(db);
    let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
    assert!(store
        .public_settings()
        .unwrap()
        .source_connectors
        .is_empty());
    assert!(c.gets.lock().unwrap().is_empty());
    assert!(!raw(dir.path()).contains("migration-fixture-secret"));
}

#[test]
fn rotation_and_profile_deletion_remove_obsolete_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    store
        .save_settings(&with_token("old-fixture-token"))
        .unwrap();
    store
        .save_settings(&with_token("new-fixture-token"))
        .unwrap();
    let values = credentials.values.lock().unwrap();
    assert_eq!(values.len(), 1);
    assert_eq!(values.values().next().unwrap(), "new-fixture-token");
    drop(values);
    drop(store);
    let reopened = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    assert_eq!(
        reopened
            .settings()
            .unwrap()
            .source_by_id("alice")
            .unwrap()
            .token
            .as_deref()
            .unwrap(),
        "new-fixture-token"
    );
    reopened.save_settings(&Settings::default()).unwrap();
    assert!(credentials.values.lock().unwrap().is_empty());
}

#[test]
fn blank_replacement_preserves_token_but_account_change_cannot_reuse_it() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    store
        .save_settings(&with_token("alice-fixture-token"))
        .unwrap();
    let mut settings = store.public_settings().unwrap();
    settings.source_connectors[0].token = Some("  ".into());
    store.save_settings(&settings).unwrap();
    assert_eq!(
        store
            .settings()
            .unwrap()
            .source_by_id("alice")
            .unwrap()
            .token
            .as_deref()
            .unwrap(),
        "alice-fixture-token"
    );
    settings.source_connectors[0].account = "bob".into();
    assert!(store.save_settings(&settings).is_err());
    assert_eq!(
        store.public_settings().unwrap().source_connectors[0].account,
        "alice"
    );
    assert_eq!(credentials.values.lock().unwrap().len(), 1);
}

#[test]
fn failed_native_write_or_verification_never_changes_saved_settings() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    store
        .save_settings(&with_token("previous-fixture-token"))
        .unwrap();
    let original = raw(dir.path());
    credentials.fail_set.store(true, Ordering::SeqCst);
    let error = store
        .save_settings(&with_token("replacement-fixture-token"))
        .unwrap_err()
        .to_string();
    assert!(!error.contains("fixture backend"));
    assert_eq!(raw(dir.path()), original);
    credentials.fail_set.store(false, Ordering::SeqCst);
    credentials.corrupt_write.store(true, Ordering::SeqCst);
    assert!(store
        .save_settings(&with_token("replacement-fixture-token"))
        .is_err());
    assert_eq!(raw(dir.path()), original);
    assert_eq!(credentials.values.lock().unwrap().len(), 1);
}

#[test]
fn sqlite_failure_rolls_back_references_and_keeps_previous_token() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    store
        .save_settings(&with_token("previous-fixture-token"))
        .unwrap();
    store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_settings BEFORE UPDATE ON settings BEGIN SELECT RAISE(ABORT,'save denied'); END;").unwrap();
    assert!(store
        .save_settings(&with_token("replacement-fixture-token"))
        .is_err());
    assert_eq!(
        store
            .settings()
            .unwrap()
            .source_by_id("alice")
            .unwrap()
            .token
            .as_deref()
            .unwrap(),
        "previous-fixture-token"
    );
    assert_eq!(credentials.values.lock().unwrap().len(), 1);
}

#[test]
fn failed_deletion_and_abandoned_staging_are_retried_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    store
        .save_settings(&with_token("previous-fixture-token"))
        .unwrap();
    credentials.fail_delete.store(true, Ordering::SeqCst);
    store.save_settings(&Settings::default()).unwrap();
    credentials
        .set("abandoned", "abandoned-fixture-token")
        .unwrap();
    store
        .db
        .lock()
        .unwrap()
        .execute("INSERT INTO credential_gc VALUES('abandoned')", [])
        .unwrap();
    assert_eq!(credentials.values.lock().unwrap().len(), 2);
    drop(store);
    credentials.fail_delete.store(false, Ordering::SeqCst);
    let _reopened = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    assert!(credentials.values.lock().unwrap().is_empty());
}

#[test]
fn locked_keychain_allows_metadata_edits_but_backend_secret_reads_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    store.save_settings(&with_token("fixture-token")).unwrap();
    drop(store);
    credentials.fail_get.store(true, Ordering::SeqCst);
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    let mut settings = store.public_settings().unwrap();
    assert!(settings.source_connectors[0].has_token);
    settings.poll_interval_seconds = 30;
    credentials.gets.lock().unwrap().clear();
    settings.flows[0]
        .profile_activation
        .insert("alice".into(), false);
    store.save_settings(&settings).unwrap();
    assert!(credentials.gets.lock().unwrap().is_empty());
    assert_eq!(
        store.public_settings().unwrap().flows[0]
            .profile_activation
            .get("alice"),
        Some(&false)
    );
    assert!(store.settings().is_err());
    assert!(!raw(dir.path()).contains("fixture-token"));
}

#[test]
fn config_import_keeps_credentials_out_of_settings_json() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().join("app")).unwrap();
    factory
        .save_settings(with_token("import-fixture-token"))
        .unwrap();
    let config = dir.path().join("config.yaml");
    std::fs::write(&config, "github:\n  owners: [acme]\n").unwrap();
    factory.import_legacy(Some(config), None).unwrap();
    let imported = factory.settings().unwrap();
    assert!(imported.source_connectors[0].has_token);
    assert_eq!(imported.source_connectors[0].owners, vec!["acme"]);
    assert_eq!(imported.source_connectors[0].authors, vec!["alice"]);
    assert_eq!(imported.source_connectors[0].reviewers, vec!["alice"]);
    assert!(!raw(&dir.path().join("app")).contains("import-fixture-token"));
}

#[tokio::test]
async fn locked_keychain_does_not_stop_scheduler_or_block_queue_operations() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    factory
        .save_settings(with_token("scheduler-fixture-token"))
        .unwrap();
    drop(factory);
    common::backend(dir.path())
        .fail_get
        .store(true, Ordering::SeqCst);
    let factory = common::open(dir.path().into()).unwrap();
    let id = factory
        .enqueue(common::from_source(
            TriggerEvent::issue("acme/repo", 1, "[AI] fixture", "alice", ""),
            "alice",
        ))
        .unwrap()
        .unwrap();
    factory.cancel(&id).unwrap();
    let retry = factory.retry(&id).unwrap();
    factory.cancel(&retry).unwrap();
    factory.start();
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let running = factory.snapshot().unwrap().service.running;
    factory.shutdown().await;
    assert!(running, "Keychain failure stopped the scheduler");
}

#[test]
fn missing_secondary_credential_cannot_expose_tokens_in_historical_logs() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    let mut settings = with_token("alpha-fixture-token");
    let mut bob = settings.source_connectors[0].clone();
    bob.id = "bob".into();
    bob.account = "bob".into();
    bob.token = Some("beta-fixture-token".into());
    settings.source_connectors.push(bob);
    factory.save_settings(settings).unwrap();
    let id = factory
        .enqueue(common::from_source(
            TriggerEvent::issue("acme/repo", 1, "[AI] fixture", "alice", ""),
            "alice",
        ))
        .unwrap()
        .unwrap();
    let session = dir.path().join("sessions").join(&id);
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("spec.log"), "output alpha-fixture-token").unwrap();
    drop(factory);
    common::backend(dir.path())
        .values
        .lock()
        .unwrap()
        .retain(|_, value| value != "beta-fixture-token");
    let factory = common::open(dir.path().into()).unwrap();
    match factory.logs(&id, "spec", 0) {
        Ok(log) => assert!(!log.text.contains("alpha-fixture-token")),
        Err(error) => assert!(!error.to_string().contains("alpha-fixture-token")),
    }
}

#[tokio::test]
async fn missing_secondary_credential_does_not_block_valid_actor() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    let mut settings = with_token("alpha-fixture-token");
    let mut bob = settings.source_connectors[0].clone();
    bob.id = "bob".into();
    bob.account = "bob".into();
    bob.token = Some("beta-fixture-token".into());
    settings.source_connectors.push(bob);
    store.save_settings(&settings).unwrap();
    drop(store);
    credentials
        .values
        .lock()
        .unwrap()
        .retain(|_, value| value != "beta-fixture-token");
    let store = Store::open_with_credentials(dir.path(), credentials).unwrap();
    assert_eq!(
        store.token_for_source("alice").await.unwrap(),
        "alpha-fixture-token"
    );
    assert!(store.token_for_source("bob").await.is_err());
    assert_eq!(store.redact_cached("alpha-fixture-token"), "[REDACTED]");
}

#[tokio::test(flavor = "current_thread")]
async fn slow_keychain_lookup_does_not_block_async_timers() {
    struct Slow;
    impl Credentials for Slow {
        fn get(&self, _: &str) -> Result<Option<String>> {
            std::thread::sleep(std::time::Duration::from_millis(150));
            Ok(Some("slow-fixture-token".into()))
        }
        fn set(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<()> {
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_with_credentials(dir.path(), Arc::new(Slow)).unwrap();
    let mut settings = with_token("");
    settings.source_connectors[0].token = None;
    {
        let db = store.db.lock().unwrap();
        db.execute(
            "INSERT INTO settings VALUES(1,?1)",
            [serde_json::to_string(&settings).unwrap()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO source_credential_refs VALUES('alice','alice','slow')",
            [],
        )
        .unwrap();
    }
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(30),
        store.token_for_source("alice"),
    )
    .await;
    assert!(
        result.is_err(),
        "Synchronous credential lookup starved the async timer"
    );
}

#[test]
fn historical_log_tokens_are_redacted_across_chunk_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let token = "ghp_boundary_fixture_secret";
    let factory = common::open(dir.path().into()).unwrap();
    factory.save_settings(with_token(token)).unwrap();
    let id = factory
        .enqueue(common::from_source(
            TriggerEvent::issue("acme/repo", 1, "[AI] fixture", "alice", ""),
            "alice",
        ))
        .unwrap()
        .unwrap();
    let session = dir.path().join("sessions").join(&id);
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(
        session.join("spec.log"),
        format!("{}{token} tail", "x".repeat(256 * 1024 - 8)),
    )
    .unwrap();
    let first = factory.logs(&id, "spec", 0).unwrap();
    assert!(first.text.ends_with("[REDACTED]"));
    assert_eq!(first.next_offset, 256 * 1024);
    let second = factory.logs(&id, "spec", first.next_offset).unwrap();
    assert_eq!(second.text, "[REDACTED] tail");
}

#[test]
#[ignore = "Opt-in dummy credential round trip against the native OS store"]
fn native_keychain_dummy_round_trip() {
    let native = NativeCredentials;
    let key = format!("test-{}", uuid::Uuid::new_v4());
    struct Cleanup<'a>(&'a NativeCredentials, &'a str);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = self.0.delete(self.1);
        }
    }
    let _cleanup = Cleanup(&native, &key);
    assert!(native.get(&key).unwrap().is_none());
    native.set(&key, "non-sensitive-fixture-value").unwrap();
    assert_eq!(
        native.get(&key).unwrap().as_deref(),
        Some("non-sensitive-fixture-value")
    );
    native.delete(&key).unwrap();
    assert!(native.get(&key).unwrap().is_none());
}
