mod common;
use factory_core::Settings;
use serde_json::{json, Value};

#[test]
fn issue_prefix_defaults_and_legacy_input() {
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    assert_eq!(wire["issue_title_prefix"], json!("[AI]"));
    wire.as_object_mut().unwrap().remove("issue_title_prefix");
    wire["poll_interval_seconds"] = json!(37);
    let loaded: Settings = serde_json::from_value(wire).unwrap();
    let loaded = serde_json::to_value(loaded).unwrap();
    assert_eq!(loaded["issue_title_prefix"], json!("[AI]"));
    assert_eq!(loaded["poll_interval_seconds"], json!(37));
}

#[test]
fn issue_prefix_json_normalizes_and_preserves_literal_content() {
    for (input, expected) in [
        (" \tbot: \n", "bot:"),
        (" Team Bot: ", "Team Bot:"),
        (" [.*] ", "[.*]"),
        (" [Release] ", "[Release]"),
    ] {
        let mut wire = serde_json::to_value(Settings::default()).unwrap();
        wire["issue_title_prefix"] = json!(input);
        let loaded: Settings = serde_json::from_value(wire).unwrap();
        assert_eq!(
            serde_json::to_value(loaded).unwrap()["issue_title_prefix"],
            json!(expected)
        );
    }
}

#[test]
fn issue_prefix_json_rejects_explicit_invalid_values() {
    for invalid in [
        json!(""),
        json!(" \t\n"),
        Value::Null,
        json!(0),
        json!([]),
        json!({}),
        json!(false),
    ] {
        let mut wire = serde_json::to_value(Settings::default()).unwrap();
        wire["issue_title_prefix"] = invalid;
        assert!(serde_json::from_value::<Settings>(wire).is_err());
    }
}

#[test]
fn issue_prefix_store_normalizes_and_preserves_last_valid_value() {
    use factory_core::context::factory::infrastructure::repositories::store::Store;
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    let mut input = Settings {
        issue_title_prefix: " \tBot: \n".into(),
        ..Default::default()
    };
    store.save_settings(&input).unwrap();
    let raw: String = store
        .db
        .lock()
        .unwrap()
        .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&raw).unwrap()["issue_title_prefix"],
        json!("Bot:")
    );
    for invalid in ["", " \t\n"] {
        input.issue_title_prefix = invalid.into();
        assert!(store
            .save_settings(&input)
            .unwrap_err()
            .to_string()
            .contains("Issue title prefix cannot be empty."));
        let unchanged: String = store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(unchanged, raw);
    }
    drop(store);
    let reopened = Store::open_with_credentials(dir.path(), credentials).unwrap();
    assert_eq!(
        reopened.public_settings().unwrap().issue_title_prefix,
        "Bot:"
    );
}

#[test]
fn issue_prefix_factory_returns_canonical_value_and_rejects_direct_empty_input() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), "http://127.0.0.1:1").unwrap();
    let mut settings = factory.settings().unwrap();
    settings.issue_title_prefix = " \tTeam Bot: \n".into();
    assert_eq!(
        factory
            .save_settings(settings.clone())
            .unwrap()
            .issue_title_prefix,
        "Team Bot:"
    );
    settings.issue_title_prefix = " \t".into();
    assert!(factory.save_settings(settings).is_err());
    assert_eq!(factory.settings().unwrap().issue_title_prefix, "Team Bot:");
    drop(factory);
    let reopened = common::open_with_api(dir.path().into(), "http://127.0.0.1:1").unwrap();
    assert_eq!(reopened.settings().unwrap().issue_title_prefix, "Team Bot:");
}

#[test]
fn issue_prefix_old_database_loads_default_without_rewriting_other_settings() {
    use factory_core::context::factory::infrastructure::repositories::store::Store;
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    wire.as_object_mut().unwrap().remove("issue_title_prefix");
    wire["poll_interval_seconds"] = json!(37);
    let original = serde_json::to_string(&wire).unwrap();
    store
        .db
        .lock()
        .unwrap()
        .execute("INSERT INTO settings(id,json) VALUES(1,?1)", [&original])
        .unwrap();
    drop(store);
    let reopened = Store::open_with_credentials(dir.path(), credentials).unwrap();
    let loaded = reopened.public_settings().unwrap();
    assert_eq!(loaded.issue_title_prefix, "[AI]");
    assert_eq!(loaded.poll_interval_seconds, 37);
    let raw: String = reopened
        .db
        .lock()
        .unwrap()
        .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(raw, original);
}
