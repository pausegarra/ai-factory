mod common;
use factory_core::context::factory::infrastructure::repositories::store::Store;
use factory_core::Settings;
use serde_json::{json, Value};
use std::sync::Arc;

#[test]
fn issue_comment_settings_default_and_legacy_are_enabled() {
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    assert_eq!(wire["issue_node_comments_enabled"], json!(true));
    wire.as_object_mut()
        .unwrap()
        .remove("issue_node_comments_enabled");
    let legacy: Settings = serde_json::from_value(wire).unwrap();
    assert_eq!(
        serde_json::to_value(legacy).unwrap()["issue_node_comments_enabled"],
        json!(true)
    );
}

#[test]
fn issue_comment_settings_round_trip_both_values() {
    for enabled in [false, true] {
        let mut wire = serde_json::to_value(Settings::default()).unwrap();
        wire["issue_node_comments_enabled"] = json!(enabled);
        let settings: Settings = serde_json::from_value(wire).unwrap();
        assert_eq!(
            serde_json::to_value(settings).unwrap()["issue_node_comments_enabled"],
            json!(enabled)
        );
    }
}

#[test]
fn issue_comment_settings_reject_non_boolean_input() {
    for invalid in [Value::Null, json!("false"), json!(0), json!([])] {
        let mut wire = serde_json::to_value(Settings::default()).unwrap();
        wire["issue_node_comments_enabled"] = invalid;
        assert!(serde_json::from_value::<Settings>(wire).is_err());
    }
}

#[test]
fn issue_comment_settings_save_and_reopen_both_values() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    for enabled in [false, true, false] {
        let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
        let mut wire = serde_json::to_value(store.public_settings().unwrap()).unwrap();
        wire["issue_node_comments_enabled"] = json!(enabled);
        store
            .save_settings(&serde_json::from_value(wire).unwrap())
            .unwrap();
        let raw: String = store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
            .unwrap();
        let saved: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(saved["issue_node_comments_enabled"], json!(enabled));
        drop(store);
        let reopened = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(reopened.public_settings().unwrap()).unwrap()
                ["issue_node_comments_enabled"],
            json!(enabled)
        );
    }
}

#[test]
fn issue_comment_settings_old_database_defaults_to_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), credentials.clone()).unwrap();
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    wire.as_object_mut()
        .unwrap()
        .remove("issue_node_comments_enabled");
    store
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO settings(id,json) VALUES(1,?1)",
            [serde_json::to_string(&wire).unwrap()],
        )
        .unwrap();
    drop(store);
    let reopened = Store::open_with_credentials(dir.path(), credentials).unwrap();
    assert!(
        reopened
            .public_settings()
            .unwrap()
            .issue_node_comments_enabled
    );
}
