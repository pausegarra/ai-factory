mod common;
use factory_core::Settings;
use serde_json::{json, Value};

#[test]
fn release_prefix_defaults_and_legacy_json_preserve_other_values() {
    let mut wire = serde_json::to_value(common::configured_settings()).unwrap();
    assert_eq!(
        wire["source_connectors"][0]["release_issue_title_prefix"],
        json!("[RELEASE]")
    );
    wire["source_connectors"][0]
        .as_object_mut()
        .unwrap()
        .remove("release_issue_title_prefix");
    wire["source_connectors"][0]["issue_title_prefix"] = json!("task:");
    wire["poll_interval_seconds"] = json!(37);
    wire["source_connectors"][0]
        .as_object_mut()
        .unwrap()
        .remove("token");
    let loaded: Settings = serde_json::from_value(wire).unwrap();
    let loaded = serde_json::to_value(loaded).unwrap();
    assert_eq!(
        loaded["source_connectors"][0]["release_issue_title_prefix"],
        json!("[RELEASE]")
    );
    assert_eq!(
        loaded["source_connectors"][0]["issue_title_prefix"],
        json!("task:")
    );
    assert_eq!(loaded["poll_interval_seconds"], json!(37));
}

#[test]
fn release_prefix_json_normalizes_literal_values_and_rejects_invalid_input() {
    for (input, expected) in [
        (" \t[DEPLOY] \n", "[DEPLOY]"),
        (" Team Release: ", "Team Release:"),
        (" [.*] ", "[.*]"),
        (" [Release] ", "[Release]"),
        ("\u{2003}ship:\u{2003}", "ship:"),
    ] {
        let mut wire = serde_json::to_value(common::configured_settings()).unwrap();
        wire["source_connectors"][0]["release_issue_title_prefix"] = json!(input);
        let loaded: Settings = serde_json::from_value(wire).unwrap();
        assert_eq!(
            serde_json::to_value(loaded).unwrap()["source_connectors"][0]
                ["release_issue_title_prefix"],
            json!(expected)
        );
    }
    for invalid in [
        json!(""),
        json!(" \t\n"),
        Value::Null,
        json!(0),
        json!(false),
        json!([]),
        json!({}),
    ] {
        let mut wire = serde_json::to_value(common::configured_settings()).unwrap();
        wire["source_connectors"][0]["release_issue_title_prefix"] = invalid;
        assert!(serde_json::from_value::<Settings>(wire).is_err());
    }
}

#[test]
fn release_prefix_store_persists_normalized_value_and_rejects_without_replacing() {
    use factory_core::context::factory::infrastructure::repositories::store::Store;
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    let mut input = common::configured_settings();
    input.source_connectors[0].issue_title_prefix = "task:".into();
    input.source_connectors[0].release_issue_title_prefix = " \t[DEPLOY] \n".into();
    store.save_settings(&input).unwrap();
    let raw: String = store
        .db
        .lock()
        .unwrap()
        .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&raw).unwrap()["source_connectors"][0]
            ["release_issue_title_prefix"],
        json!("[DEPLOY]")
    );
    for invalid in ["", " \t\n"] {
        input.source_connectors[0].release_issue_title_prefix = invalid.into();
        assert!(input
            .validate()
            .unwrap_err()
            .to_string()
            .contains("Release issue title prefix cannot be empty."));
        assert!(store
            .save_settings(&input)
            .unwrap_err()
            .to_string()
            .contains("Release issue title prefix cannot be empty."));
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
        reopened.public_settings().unwrap().source_connectors[0].release_issue_title_prefix,
        "[DEPLOY]"
    );
    assert_eq!(
        reopened.public_settings().unwrap().source_connectors[0].issue_title_prefix,
        "task:"
    );
}

#[test]
fn release_prefix_old_database_defaults_without_rewrite() {
    use factory_core::context::factory::infrastructure::repositories::store::Store;
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    let mut wire = serde_json::to_value(common::configured_settings()).unwrap();
    wire["source_connectors"][0]
        .as_object_mut()
        .unwrap()
        .remove("release_issue_title_prefix");
    wire["source_connectors"][0]
        .as_object_mut()
        .unwrap()
        .remove("issue_title_prefix");
    wire["issue_title_prefix"] = json!("task:");
    wire["poll_interval_seconds"] = json!(37);
    wire["source_connectors"][0]
        .as_object_mut()
        .unwrap()
        .remove("token");
    let raw = wire.to_string();
    store
        .db
        .lock()
        .unwrap()
        .execute("INSERT INTO settings(id,json) VALUES(1,?1)", [&raw])
        .unwrap();
    drop(store);
    let reopened = Store::open_with_credentials(dir.path(), credentials).unwrap();
    let settings = reopened.public_settings().unwrap();
    assert_eq!(
        settings.source_connectors[0].release_issue_title_prefix,
        "[RELEASE]"
    );
    assert_eq!(settings.source_connectors[0].issue_title_prefix, "task:");
    assert_eq!(settings.poll_interval_seconds, 37);
    let unchanged: String = reopened
        .db
        .lock()
        .unwrap()
        .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(unchanged, raw);
}

#[test]
fn release_prefix_factory_returns_canonical_value_and_keeps_previous_on_error() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), "http://127.0.0.1:1").unwrap();
    let mut settings = common::configured_settings();
    settings.source_connectors[0].issue_title_prefix = "task:".into();
    settings.source_connectors[0].release_issue_title_prefix = " \tTeam Release: \n".into();
    let stored = factory.save_settings(settings.clone()).unwrap();
    assert_eq!(
        stored.source_connectors[0].release_issue_title_prefix,
        "Team Release:"
    );
    assert_eq!(stored.source_connectors[0].issue_title_prefix, "task:");
    settings.source_connectors[0].release_issue_title_prefix = " \t".into();
    assert!(factory.save_settings(settings).is_err());
    assert_eq!(
        factory.settings().unwrap().source_connectors[0].release_issue_title_prefix,
        "Team Release:"
    );
    drop(factory);
    let reopened = common::open_with_api(dir.path().into(), "http://127.0.0.1:1").unwrap();
    assert_eq!(
        reopened.settings().unwrap().source_connectors[0].release_issue_title_prefix,
        "Team Release:"
    );
}
