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
    assert!(serde_json::to_value(&f.nodes[0])
        .unwrap()
        .get("skills")
        .is_none());
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
    let release = common::from_source(
        TriggerEvent::release_issue("Acme/Repo", 80, "[Release]", "alice", "body"),
        "alice",
    );
    let issue = common::from_source(
        TriggerEvent::issue("Acme/Repo", 80, "[AI]", "alice", "body"),
        "alice",
    );
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
    custom["flows"][3]["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("skills");
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
    let mut profile = factory_core::SourceConnector {
        label: "Fixture source".into(),
        token: Some("fixture-source-only".into()),
        owners: vec!["acme".into()],
        reviewers: vec!["alice".into()],
        kind: factory_core::SourceKind::GithubIssues,
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
    wire["flows"][0]["nodes"][0]["skills"] = json!(["retired"]);
    wire["flows"][1]["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == "version_bump")
        .unwrap()["skills"] = json!([]);
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
    assert_no_node_skills(&serde_json::to_value(&loaded).unwrap());
    f.save_settings(loaded).unwrap();
    let saved: String = db
        .query_row("SELECT json FROM settings", [], |r| r.get(0))
        .unwrap();
    assert_no_node_skills(&serde_json::from_str(&saved).unwrap());
    drop(db);
    drop(f);
    let reopened = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    let s = reopened.settings().unwrap();
    assert_eq!(s.flows.len(), 4);
    assert!(!s.flows[3].enabled);
    assert_eq!(s.flows[0].nodes[0].prompt, "stored custom prompt");
}

fn assert_no_node_skills(settings: &serde_json::Value) {
    for flow in settings["flows"].as_array().unwrap() {
        for node in flow["nodes"].as_array().unwrap() {
            assert!(node.get("skills").is_none(), "{node}");
        }
    }
}

#[test]
fn saved_release_prompt_updates_only_the_exact_legacy_default() {
    let legacy =
        include_str!("../src/context/factory/domain/entities/models/release_prompt_legacy.txt");
    let custom = format!("{legacy}\nRepositorio Acme usa su procedimiento propio.\n");
    let new_default = Settings::default()
        .flows
        .into_iter()
        .find(|flow| flow.id == "release")
        .unwrap()
        .nodes
        .remove(0)
        .prompt;
    assert!(new_default.contains("a suitable installed publishing skill available to the agent"));
    assert!(!new_default.contains("skill selection"));

    for (stored, expected_prompt) in [(legacy, new_default.as_str()), (&custom, custom.as_str())] {
        let dir = tempfile::tempdir().unwrap();
        let open = || common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
        let factory = open();
        factory.save_settings(factory.settings().unwrap()).unwrap();
        let mut wire = serde_json::to_value(factory.settings().unwrap()).unwrap();
        let release = &mut wire["flows"][3];
        release["enabled"] = json!(true);
        release["profile_activation"] = json!({"fixture":true});
        let node = &mut release["nodes"][0];
        node["agent"] = json!("opencode");
        node["model"] = json!("fixture/test");
        node["position"] = json!({"x":80.0,"y":120.0});
        node["prompt"] = json!(stored);
        node["skills"] = json!(["fixture-publish"]);
        let raw = wire.to_string();
        let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
        assert_eq!(
            db.execute("UPDATE settings SET json=?1 WHERE id=1", [&raw])
                .unwrap(),
            1
        );
        let read = || {
            db.query_row("SELECT json FROM settings WHERE id=1", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
        };
        let loaded = factory.settings().unwrap();
        let node = &mut wire["flows"][3]["nodes"][0];
        node.as_object_mut().unwrap().remove("skills");
        node["prompt"] = json!(expected_prompt);
        assert_eq!(
            serde_json::to_value(&loaded).unwrap()["flows"],
            wire["flows"]
        );
        assert_eq!(read(), raw);
        factory.save_settings(loaded).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&read()).unwrap();
        assert_no_node_skills(&saved);
        assert_eq!(saved["flows"], wire["flows"]);
        drop(factory);
        assert_eq!(
            serde_json::to_value(open().settings().unwrap()).unwrap()["flows"],
            wire["flows"]
        );
    }
}
