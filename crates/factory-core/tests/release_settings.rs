use factory_core::{Settings, TriggerEvent};
use serde_json::json;

#[test]
fn release_defaults_are_disabled_and_old_flows_stay_enabled() {
    let s = Settings::default();
    assert_eq!(s.flows.len(), 4);
    s.validate().unwrap();
    for id in ["new_issue", "pr_approved", "pr_review"] {
        assert!(s.flows.iter().find(|f| f.id == id).unwrap().enabled);
    }
    let f = s.flows.iter().find(|f| f.id == "release").unwrap();
    assert_eq!(f.name, "Release");
    assert!(!f.enabled);
    assert_eq!(f.nodes.len(), 1);
    assert_eq!(f.nodes[0].id, "publish_app");
    assert_eq!(f.nodes[0].access, "write");
    assert_eq!(f.nodes[0].skills, None);
    assert!(f.nodes[0].depends_on.is_empty());
}

#[test]
fn three_flow_settings_gain_only_the_disabled_release_default() {
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    wire["flows"]
        .as_array_mut()
        .unwrap()
        .retain(|f| f["id"] != "release");
    wire["flows"][0]["nodes"][0]["prompt"] = json!("custom source prompt");
    wire["flows"][1]["enabled"] = json!(false);
    let before = wire["flows"].clone();
    let s: Settings = serde_json::from_value(wire).unwrap();
    s.validate().unwrap();
    let after = serde_json::to_value(&s).unwrap();
    assert_eq!(
        &after["flows"].as_array().unwrap()[..3],
        before.as_array().unwrap()
    );
    assert_eq!(after["flows"][3]["id"], "release");
    assert_eq!(after["flows"][3]["enabled"], false);
    let again: Settings = serde_json::from_value(after.clone()).unwrap();
    assert_eq!(serde_json::to_value(again).unwrap(), after);
}

#[test]
fn release_identity_does_not_collide_with_issue_identity() {
    let release = TriggerEvent::release_issue("Acme/Repo", 80, "[Release]", "alice", "body");
    let issue = TriggerEvent::issue("Acme/Repo", 80, "[AI]", "alice", "body");
    assert_eq!(release.event_id, "release:acme/repo#80");
    assert_eq!(release.repository, "acme/repo");
    assert_eq!(release.trigger_id, "release");
    assert_eq!(release.url, issue.url);
    assert_ne!(release.event_id, issue.event_id);
}

#[test]
fn migration_preserves_reversed_legacy_and_custom_release_without_repairing_invalid_flows() {
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    wire["flows"]
        .as_array_mut()
        .unwrap()
        .retain(|f| f["id"] != "release");
    wire["flows"].as_array_mut().unwrap().reverse();
    let before = wire["flows"].clone();
    let migrated: Settings = serde_json::from_value(wire).unwrap();
    let stored = serde_json::to_value(&migrated).unwrap();
    assert_eq!(
        &stored["flows"].as_array().unwrap()[..3],
        before.as_array().unwrap()
    );
    let mut custom = stored.clone();
    custom["flows"][3]["enabled"] = json!(true);
    custom["flows"][3]["profile_activation"] = json!({"work":false});
    custom["flows"][3]["nodes"][0]["prompt"] = json!("custom release");
    custom["flows"][3]["nodes"][0]["skills"] = json!(["fixture-publish"]);
    let settings: Settings = serde_json::from_value(custom.clone()).unwrap();
    assert_eq!(serde_json::to_value(settings).unwrap(), custom);
    for ids in [
        vec![],
        vec!["new_issue", "pr_review"],
        vec!["new_issue", "new_issue", "pr_review"],
        vec!["new_issue", "pr_approved", "unknown"],
        vec!["new_issue", "pr_review", "release"],
    ] {
        let mut bad = stored.clone();
        bad["flows"] = json!(ids
            .iter()
            .map(|id| {
                let mut f = stored["flows"][0].clone();
                f["id"] = json!(id);
                f
            })
            .collect::<Vec<_>>());
        let bad: Settings = serde_json::from_value(bad).unwrap();
        assert!(bad.validate().is_err(), "{ids:?}");
    }
    let empty: Settings = serde_json::from_value(json!({})).unwrap();
    assert_eq!(empty.flows.len(), 4);
    let mut bad = stored;
    bad["trigger_max_concurrent"] = json!({"unknown":1});
    assert!(serde_json::from_value::<Settings>(bad).is_err());
}

#[test]
fn release_authorization_and_capacity_follow_issue_rules() {
    let mut profile = factory_core::TokenProfile {
        account: "alice".into(),
        authors: vec!["ALICE".into()],
        ..Default::default()
    };
    assert!(profile.allows_actor("release"));
    profile.authors.clear();
    assert!(!profile.allows_actor("release"));
    let mut settings = Settings::default();
    settings.trigger_max_concurrent.insert("release".into(), 2);
    assert_eq!(settings.concurrency_for("release").unwrap(), 2);
    settings.trigger_max_concurrent.clear();
    assert_eq!(settings.concurrency_for("release").unwrap(), 4);
}

mod common;
#[test]
fn legacy_stored_flows_are_migrated_on_read_and_persisted_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    let mut wire = serde_json::to_value(f.settings().unwrap()).unwrap();
    wire["flows"]
        .as_array_mut()
        .unwrap()
        .retain(|f| f["id"] != "release");
    wire["flows"][0]["nodes"][0]["prompt"] = json!("stored custom prompt");
    f.save_settings(f.settings().unwrap()).unwrap();
    let raw = wire.to_string();
    let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
    assert_eq!(
        db.execute("UPDATE settings SET json=?1", [&raw]).unwrap(),
        1
    );
    let loaded = f.settings().unwrap();
    assert_eq!(loaded.flows.len(), 4);
    assert!(!loaded.flows[3].enabled);
    assert_eq!(loaded.flows[0].nodes[0].prompt, "stored custom prompt");
    let unchanged: String = db
        .query_row("SELECT json FROM settings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(unchanged, raw);
    f.save_settings(loaded).unwrap();
    drop(db);
    drop(f);
    let reopened = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    let s = reopened.settings().unwrap();
    assert_eq!(s.flows.len(), 4);
    assert!(!s.flows[3].enabled);
    assert_eq!(s.flows[0].nodes[0].prompt, "stored custom prompt");
}
