mod common;
use factory_core::{context::factory::infrastructure::repositories::store::Store, Settings};
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc};
const TOKEN: &str = "123456:fixture_telegram_secret_abcdefghijklmnopqrstuvwxyz";
fn input(enabled: bool, token: Option<&str>) -> Settings {
    let mut value = serde_json::to_value(Settings::default()).unwrap();
    value["token_profiles"] = json!([{"id":"work","label":"Work","account":"alice"}]);
    value["notification_connectors"] = json!([{"id":"team","label":"Team","enabled":enabled,"events":["run_failed"],"type":"telegram","config":{"chat_id":" -1001 ","has_token":true,"bot_token":token,"triggers":["new_issue"],"token_profile_ids":["work"]}}]);
    serde_json::from_value(value).unwrap()
}
fn public(store: &Store) -> Value {
    serde_json::to_value(store.public_settings().unwrap()).unwrap()
}
#[test]
fn notification_secret_is_saved_outside_sqlite_and_public_settings() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    store.save_settings(&input(true, Some(TOKEN))).unwrap();
    let value = public(&store);
    assert_eq!(
        value["notification_connectors"][0]["config"]["has_token"],
        true
    );
    assert_eq!(
        value["notification_connectors"][0]["config"]["chat_id"],
        "-1001"
    );
    assert!(!value.to_string().contains(TOKEN));
    let db = store.db.lock().unwrap();
    let raw: String = db
        .query_row("SELECT json FROM settings", [], |r| r.get(0))
        .unwrap();
    assert!(!raw.contains(TOKEN));
    let reference: String = db
        .query_row(
            "SELECT reference FROM notification_credential_refs WHERE connector_id='team'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!value.to_string().contains(&reference));
    assert_eq!(
        credentials
            .values
            .lock()
            .unwrap()
            .get(&reference)
            .map(String::as_str),
        Some(TOKEN)
    );
}
#[test]
fn notification_activation_requires_real_secret_not_claimed_has_token() {
    let dir = tempfile::tempdir().unwrap();
    let c = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
    assert!(store.save_settings(&input(true, None)).is_err());
    store.save_settings(&input(false, None)).unwrap();
    store.save_settings(&input(true, Some(TOKEN))).unwrap();
    c.values.lock().unwrap().clear();
    assert!(store.save_settings(&input(true, None)).is_err());
}
#[test]
fn notification_blank_preserves_rotation_and_deletion_collect_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let c = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
    store.save_settings(&input(true, Some(TOKEN))).unwrap();
    let old = c.values.lock().unwrap().keys().next().unwrap().clone();
    store.save_settings(&input(false, Some(" "))).unwrap();
    assert!(c.values.lock().unwrap().contains_key(&old));
    store.save_settings(&input(true, None)).unwrap();
    store
        .save_settings(&input(true, Some("123456:replacement_fixture")))
        .unwrap();
    assert!(!c.values.lock().unwrap().contains_key(&old));
    store.save_settings(&Settings::default()).unwrap();
    assert!(c.values.lock().unwrap().is_empty());
}
#[test]
fn notification_failed_staging_and_sqlite_commit_preserve_previous_settings() {
    for failure in ["set", "get", "verify", "sql"] {
        let dir = tempfile::tempdir().unwrap();
        let c = Arc::new(common::MemoryCredentials::default());
        let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
        store.save_settings(&input(true, Some(TOKEN))).unwrap();
        let before = public(&store);
        match failure {
            "set" => c.fail_set.store(true, Ordering::SeqCst),
            "get" => c.fail_get.store(true, Ordering::SeqCst),
            "verify" => c.corrupt_write.store(true, Ordering::SeqCst),
            _ => {
                store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_settings BEFORE UPDATE ON settings BEGIN SELECT RAISE(ABORT,'fixture rejection'); END;").unwrap();
            }
        }
        let err = store
            .save_settings(&input(true, Some("654321:new_fixture_secret")))
            .unwrap_err();
        assert!(!err.to_string().contains("fixture backend"));
        assert_eq!(public(&store), before);
        assert!(c.values.lock().unwrap().values().any(|v| v == TOKEN));
    }
}
#[test]
fn notification_gc_retries_after_restart_and_never_collects_live_github_secret() {
    let dir = tempfile::tempdir().unwrap();
    let c = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
    let mut settings = input(true, Some(TOKEN));
    settings.token_profiles[0] = factory_core::TokenProfile {
        id: "work".into(),
        label: "GitHub".into(),
        account: "alice".into(),
        token: Some("ghp_fixture_private".into()),
        ..Default::default()
    };
    store.save_settings(&settings).unwrap();
    assert_eq!(c.values.lock().unwrap().len(), 2);
    c.fail_delete.store(true, Ordering::SeqCst);
    let mut settings = store.public_settings().unwrap();
    settings.notification_connectors.clear();
    store.save_settings(&settings).unwrap();
    assert_eq!(c.values.lock().unwrap().len(), 2);
    let abandoned = "abandoned-fixture-reference";
    store
        .db
        .lock()
        .unwrap()
        .execute("INSERT INTO credential_gc VALUES(?1)", [abandoned])
        .unwrap();
    c.values
        .lock()
        .unwrap()
        .insert(abandoned.into(), "fixture abandoned secret".into());
    drop(store);
    c.fail_delete.store(false, Ordering::SeqCst);
    let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
    assert_eq!(
        c.values
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["ghp_fixture_private"]
    );
    assert!(store.public_settings().unwrap().token_profiles[0].has_token);
}
#[test]
fn notification_public_reads_and_disabled_incomplete_saves_do_not_access_locked_keychain() {
    let dir = tempfile::tempdir().unwrap();
    let c = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
    store.save_settings(&input(false, Some(TOKEN))).unwrap();
    c.fail_get.store(true, Ordering::SeqCst);
    c.gets.lock().unwrap().clear();
    assert_eq!(
        public(&store)["notification_connectors"][0]["config"]["has_token"],
        true
    );
    store.save_settings(&input(false, None)).unwrap();
    assert!(c.gets.lock().unwrap().is_empty());
    assert!(store
        .settings()
        .unwrap()
        .notification_connectors
        .iter()
        .all(|v| match &v.provider {
            factory_core::ConnectorProvider::Telegram(c) => c.bot_token.is_none(),
            _ => true,
        }));
}
#[test]
fn notification_secrets_stay_out_of_database_files_and_redact_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let c = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
    store.save_settings(&input(true, Some(TOKEN))).unwrap();
    for path in std::fs::read_dir(dir.path()).unwrap() {
        let bytes = std::fs::read(path.unwrap().path()).unwrap();
        assert!(!bytes.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()));
    }
    drop(store);
    let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
    store.load_redaction_secrets().unwrap();
    assert_eq!(
        store.redact_cached(&format!("before {TOKEN} after")),
        "before [REDACTED] after"
    );
    c.fail_get.store(true, Ordering::SeqCst);
    drop(store);
    let store = Store::open_with_credentials(dir.path(), c).unwrap();
    assert!(store.load_redaction_secrets().is_err());
}
#[test]
fn notification_two_destinations_and_provider_change_have_independent_secret_identity() {
    let dir = tempfile::tempdir().unwrap();
    let c = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
    let mut settings = input(false, Some(TOKEN));
    let mut second = settings.notification_connectors[0].clone();
    second.id = "second".into();
    settings.notification_connectors.push(second);
    store.save_settings(&settings).unwrap();
    assert_eq!(c.values.lock().unwrap().len(), 2);
    let mut settings = store.public_settings().unwrap();
    settings.notification_connectors[0].provider = factory_core::ConnectorProvider::Unsupported {
        kind: "future".into(),
        config: json!({"option":7}),
    };
    store.save_settings(&settings).unwrap();
    assert_eq!(c.values.lock().unwrap().len(), 1);
    settings.notification_connectors[0].provider =
        factory_core::ConnectorProvider::Telegram(Default::default());
    settings.notification_connectors[0].enabled = true;
    assert!(store.save_settings(&settings).is_err());
}

#[test]
fn notification_legacy_filters_persist_on_save_without_read_mutation_or_secret_loss() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    store.save_settings(&input(true, Some(TOKEN))).unwrap();
    let raw = |store: &Store| -> String {
        store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT json FROM settings", [], |r| r.get(0))
            .unwrap()
    };
    let reference = |store: &Store| -> String {
        store
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT reference FROM notification_credential_refs WHERE connector_id='team'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let original_ref = reference(&store);
    let mut legacy: Value = serde_json::from_str(&raw(&store)).unwrap();
    let config = legacy["notification_connectors"][0]["config"]
        .as_object_mut()
        .unwrap();
    config.remove("triggers");
    config.remove("token_profile_ids");
    let legacy_json = legacy.to_string();
    store
        .db
        .lock()
        .unwrap()
        .execute("UPDATE settings SET json=?1", [&legacy_json])
        .unwrap();
    let mut loaded = store.public_settings().unwrap();
    assert_eq!(
        raw(&store),
        legacy_json,
        "reads must not rewrite legacy settings"
    );
    let canonical = serde_json::to_value(&loaded).unwrap();
    assert_eq!(
        canonical["notification_connectors"][0]["config"]["triggers"],
        json!(["new_issue", "pr_approved", "pr_review"])
    );
    assert_eq!(
        canonical["notification_connectors"][0]["config"]["token_profile_ids"],
        json!(["work"])
    );
    assert_eq!(
        canonical["notification_connectors"][0]["config"]["has_token"],
        true
    );
    loaded.token_profiles[0].label = "Renamed".into();
    loaded.token_profiles.push(factory_core::TokenProfile {
        id: "new".into(),
        account: "bob".into(),
        label: "New".into(),
        ..Default::default()
    });
    store.save_settings(&loaded).unwrap();
    assert_eq!(reference(&store), original_ref);
    drop(store);
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    let mut loaded = store.public_settings().unwrap();
    assert_eq!(
        serde_json::to_value(&loaded).unwrap()["notification_connectors"][0]["config"],
        canonical["notification_connectors"][0]["config"]
    );
    assert_eq!(reference(&store), original_ref);
    if let factory_core::ConnectorProvider::Telegram(c) =
        &mut loaded.notification_connectors[0].provider
    {
        c.bot_token = Some("123456:rotated_fixture".into());
    }
    store.save_settings(&loaded).unwrap();
    assert_ne!(reference(&store), original_ref);
    let rotated_ref = reference(&store);
    let mut loaded = store.public_settings().unwrap();
    if let factory_core::ConnectorProvider::Telegram(c) =
        &mut loaded.notification_connectors[0].provider
    {
        c.bot_token = Some(" ".into());
    }
    store.save_settings(&loaded).unwrap();
    assert_eq!(reference(&store), rotated_ref);
    let before = public(&store);
    store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_settings BEFORE UPDATE ON settings BEGIN SELECT RAISE(ABORT,'fixture rejection'); END;").unwrap();
    if let factory_core::ConnectorProvider::Telegram(c) =
        &mut loaded.notification_connectors[0].provider
    {
        c.bot_token = Some("123456:rollback_fixture".into());
        c.triggers = vec!["pr_review".into()];
    }
    assert!(store.save_settings(&loaded).is_err());
    assert_eq!(public(&store), before);
    assert_eq!(reference(&store), rotated_ref);
    for text in [
        raw(&store),
        public(&store).to_string(),
        format!("{:?}", store.public_settings().unwrap()),
    ] {
        for secret in [TOKEN, "123456:rotated_fixture", "123456:rollback_fixture"] {
            assert!(!text.contains(secret));
        }
    }
}
