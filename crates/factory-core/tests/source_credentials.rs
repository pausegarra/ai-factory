mod common;
use factory_core::context::factory::infrastructure::repositories::store::Store;
use factory_core::*;
use rusqlite::Connection;
use std::sync::Arc;

#[tokio::test]
async fn source_secret_is_private_and_resolved_only_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials).unwrap();
    let mut settings = Settings::default();
    settings.source_connectors.push(SourceConnector {
        id: "src-work".into(),
        label: "Work".into(),
        account: "alice".into(),
        token: Some("fixture-source-secret".into()),
        ..Default::default()
    });
    store.save_settings(&settings).unwrap();
    assert_eq!(
        store.token_for_source("src-work").await.unwrap(),
        "fixture-source-secret"
    );
    assert!(store.token_for_source("alice").await.is_err());
    let public = store.public_settings().unwrap();
    assert!(public.source_connectors[0].has_token);
    assert!(public.source_connectors[0].token.is_none());
    let db = Connection::open(dir.path().join("factory.sqlite3")).unwrap();
    let raw: String = db
        .query_row("SELECT json FROM settings", [], |r| r.get(0))
        .unwrap();
    assert!(!raw.contains("fixture-source-secret"));
}

#[tokio::test]
async fn legacy_metadata_and_secret_refs_are_retired_without_hydration_or_id_reuse() {
    let dir = tempfile::tempdir().unwrap();
    let c = Arc::new(common::MemoryCredentials::default());
    c.values
        .lock()
        .unwrap()
        .insert("legacy-reference".into(), "old-keychain-fixture".into());
    let db = Connection::open(dir.path().join("factory.sqlite3")).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE settings(id INTEGER PRIMARY KEY,json TEXT NOT NULL); CREATE TABLE credential_refs(profile_id TEXT PRIMARY KEY,account TEXT NOT NULL,reference TEXT NOT NULL UNIQUE);").unwrap();
    let raw = serde_json::json!({"max_concurrent":7,"token_profiles":[{"id":"same","label":false,"token":"old-plaintext-fixture"}],"owners":null,"authors":true,"reviewers":42});
    db.execute("INSERT INTO settings VALUES(1,?1)", [raw.to_string()])
        .unwrap();
    db.execute(
        "INSERT INTO credential_refs VALUES('same','alice','legacy-reference')",
        [],
    )
    .unwrap();
    drop(db);
    c.fail_get.store(true, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..2 {
        let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
        assert_eq!(store.public_settings().unwrap().max_concurrent, 7);
        assert!(store
            .public_settings()
            .unwrap()
            .source_connectors
            .is_empty());
        assert!(store.token_for_source("same").await.is_err());
    }
    assert!(c.gets.lock().unwrap().is_empty());
    assert!(c.values.lock().unwrap().is_empty());
    let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
    let mut s = store.public_settings().unwrap();
    s.source_connectors.push(SourceConnector {
        id: "same".into(),
        label: "New".into(),
        account: "alice".into(),
        has_token: true,
        ..Default::default()
    });
    assert!(store.save_settings(&s).is_err());
    assert!(store
        .public_settings()
        .unwrap()
        .source_connectors
        .is_empty());
    for name in [
        "factory.sqlite3",
        "factory.sqlite3-wal",
        "factory.sqlite3-shm",
    ] {
        if let Ok(bytes) = std::fs::read(dir.path().join(name)) {
            assert!(
                !bytes
                    .windows("old-plaintext-fixture".len())
                    .any(|v| v == b"old-plaintext-fixture"),
                "{name}"
            );
        }
    }
}
#[tokio::test]
async fn same_login_tokens_rotate_independently_and_deleted_sources_cannot_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let c = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
    let mut s = Settings::default();
    for (id, token) in [("a", "fixture-a"), ("b", "fixture-b")] {
        s.source_connectors.push(SourceConnector {
            id: id.into(),
            label: id.into(),
            account: "alice".into(),
            token: Some(token.into()),
            ..Default::default()
        });
    }
    store.save_settings(&s).unwrap();
    assert_eq!(store.token_for_source("b").await.unwrap(), "fixture-b");
    let mut public = store.public_settings().unwrap();
    public.source_connectors[0].label = "Renamed".into();
    store.save_settings(&public).unwrap();
    assert_eq!(store.token_for_source("a").await.unwrap(), "fixture-a");
    public.source_connectors[0].account = "bob".into();
    assert!(store.save_settings(&public).is_err());
    public.source_connectors[0].token = Some("fixture-rotated".into());
    store.save_settings(&public).unwrap();
    assert_eq!(
        store.token_for_source("a").await.unwrap(),
        "fixture-rotated"
    );
    assert_eq!(store.token_for_source("b").await.unwrap(), "fixture-b");
    public.source_connectors.retain(|s| s.id != "a");
    store.save_settings(&public).unwrap();
    assert!(store.token_for_source("a").await.is_err());
}

#[test]
fn diagnostic_formatting_never_exposes_source_tokens() {
    let source = SourceConnector {
        id: "src-a".into(),
        label: "Work".into(),
        account: "alice".into(),
        token: Some("PRIVATE_DEBUG_SECRET".into()),
        ..Default::default()
    };
    assert!(!format!("{source:?}").contains("PRIVATE_DEBUG_SECRET"));
}

#[tokio::test]
async fn review_scope_survives_reload_and_changes_without_a_new_token() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    {
        let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
        let mut settings = Settings::default();
        settings.source_connectors.push(SourceConnector {
            id: "work".into(),
            label: "Work".into(),
            account: "alice".into(),
            owners: vec!["acme".into()],
            reviewers: vec!["alice".into()],
            token: Some("fixture-secret".into()),
            ..Default::default()
        });
        store.save_settings(&settings).unwrap();
    }
    for value in [true, false, true] {
        {
            let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
            let mut public = store.public_settings().unwrap();
            assert!(public.source_connectors[0].token.is_none());
            assert!(public.source_connectors[0].has_token);
            public.source_connectors[0].review_all_open_pull_requests = value;
            store.save_settings(&public).unwrap();
        }
        let reopened = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
        let public = reopened.public_settings().unwrap();
        assert_eq!(
            public.source_connectors[0].review_all_open_pull_requests,
            value
        );
        assert!(public.source_connectors[0].token.is_none());
        assert_eq!(
            reopened.token_for_source("work").await.unwrap(),
            "fixture-secret"
        );
    }
}
