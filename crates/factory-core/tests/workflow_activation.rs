mod common;
use factory_core::*;
use serde_json::json;

fn configured() -> Settings {
    let mut value = serde_json::to_value(Settings::default()).unwrap();
    value["token_profiles"] = json!([
        {"id":"work","label":"Work","account":"alice","token":"fixture-a"},
        {"id":"personal","label":"Personal","account":"bob","token":"fixture-b"}
    ]);
    for flow in value["flows"].as_array_mut().unwrap() {
        flow["profile_activation"] = json!({"work":false});
    }
    serde_json::from_value(value).unwrap()
}

#[test]
fn activation_metadata_roundtrips_instead_of_being_ignored() {
    let value = serde_json::to_value(configured()).unwrap();
    assert_eq!(
        value["flows"][0]["profile_activation"],
        json!({"work":false})
    );
}

#[test]
fn activation_profiles_are_independent_for_all_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    factory.save_settings(configured()).unwrap();
    for trigger in ["new_issue", "pr_approved", "pr_review"] {
        let mut event = TriggerEvent::issue("acme/repo", 11, "[AI] fixture", "ALICE", "");
        event.trigger_id = trigger.into();
        event.event_id = format!("{trigger}:alice");
        assert!(factory.enqueue(event.clone()).unwrap().is_none());
        event.actor = "bob".into();
        event.event_id = format!("{trigger}:bob");
        assert!(factory.enqueue(event).unwrap().is_some());
    }
}

#[test]
fn activation_precedence_defaults_validation_and_stable_ids() {
    let mut s = configured();
    for global in [false, true] {
        for preference in [false, true] {
            s.flows[0].enabled = global;
            s.flows[0]
                .profile_activation
                .insert("work".into(), preference);
            let expected = if !global {
                ActivationState::DisabledGlobally
            } else if !preference {
                ActivationState::DisabledForProfile
            } else {
                ActivationState::Enabled
            };
            assert_eq!(
                s.flows[0].activation_for("ALICE", &s.token_profiles),
                expected
            );
        }
    }
    s.flows[0].profile_activation.clear();
    assert_eq!(
        s.flows[0].activation_for("alice", &s.token_profiles),
        ActivationState::Enabled
    );
    s.flows[0]
        .profile_activation
        .insert("removed".into(), false);
    assert_eq!(
        s.flows[0].activation_for("carol", &s.token_profiles),
        ActivationState::Enabled
    );
    assert!(s.token_for("carol").is_err());
    for invalid in [
        json!(null),
        json!([]),
        json!(true),
        json!({"work":"false"}),
        json!({"work":0}),
    ] {
        let mut value = serde_json::to_value(configured()).unwrap();
        value["flows"][0]["profile_activation"] = invalid;
        assert!(serde_json::from_value::<Settings>(value).is_err());
    }
    for id in ["", ".", "..", "../work", "work/account", &"x".repeat(129)] {
        let mut s = configured();
        s.flows[0].profile_activation.insert(id.into(), false);
        assert!(s.validate().is_err());
    }
    let mut s = configured();
    s.flows[0].nodes.clear();
    assert!(s.validate().is_err());
    let mut value = serde_json::to_value(configured()).unwrap();
    for flow in value["flows"].as_array_mut().unwrap() {
        flow.as_object_mut().unwrap().remove("profile_activation");
    }
    value["flows"][0]["enabled"] = json!(false);
    let old: Settings = serde_json::from_value(value).unwrap();
    assert_eq!(
        old.flows[0].activation_for("alice", &old.token_profiles),
        ActivationState::DisabledGlobally
    );
    assert_eq!(
        old.flows[1].activation_for("alice", &old.token_profiles),
        ActivationState::Enabled
    );
}

#[test]
fn retry_and_save_failures_preserve_history_and_current_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open(dir.path().into()).unwrap();
    let mut s = configured();
    s.flows[0].profile_activation.clear();
    f.save_settings(s).unwrap();
    let event = TriggerEvent::issue("acme/repo", 1, "[AI] fixture", "ALICE", "");
    let id = f.enqueue(event.clone()).unwrap().unwrap();
    let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
    let raw: String = db
        .query_row("SELECT flow FROM runs WHERE id=?1", [&id], |r| r.get(0))
        .unwrap();
    let queued = serde_json::to_value(f.run_detail(&id).unwrap()).unwrap();
    let mut s = f.settings().unwrap();
    s.flows[0].enabled = false;
    f.save_settings(s).unwrap();
    assert_eq!(
        serde_json::to_value(f.run_detail(&id).unwrap()).unwrap(),
        queued
    );
    f.set_run_status(&id, "failed", None).unwrap();
    let original = serde_json::to_value(f.run_detail(&id).unwrap()).unwrap();
    for global in [false, true] {
        let mut s = f.settings().unwrap();
        s.flows[0].enabled = global;
        s.flows[0].profile_activation.insert("work".into(), false);
        f.save_settings(s).unwrap();
        let error = f.retry(&id).unwrap_err().to_string();
        assert!(error.contains("new_issue") && error.contains("ALICE"));
        assert!(error.contains(if global {
            "disabled for this account"
        } else {
            "disabled globally"
        }));
        assert_eq!(f.snapshot().unwrap().metrics.total, 1);
        assert_eq!(
            serde_json::to_value(f.run_detail(&id).unwrap()).unwrap(),
            original
        );
    }
    db.execute_batch("CREATE TRIGGER deny_settings BEFORE UPDATE ON settings BEGIN SELECT RAISE(ABORT,'fixture save denied'); END;").unwrap();
    let mut next = f.settings().unwrap();
    next.flows[0].profile_activation.clear();
    assert!(f.save_settings(next.clone()).is_err());
    let other = TriggerEvent::issue("acme/repo", 2, "[AI] another", "alice", "");
    assert!(f.enqueue(other).unwrap().is_none());
    db.execute_batch("DROP TRIGGER deny_settings").unwrap();
    next.flows[0].nodes[0].token_overrides.insert(
        "work".into(),
        AgentConfig {
            agent: "opencode".into(),
            model: "provider/current".into(),
            effort: "high".into(),
        },
    );
    f.save_settings(next).unwrap();
    let retry = f.retry(&id).unwrap();
    assert_eq!(f.run_detail(&retry).unwrap().run.attempt, 2);
    assert_eq!(
        f.run_detail(&retry).unwrap().flow.nodes[0].model,
        "provider/current"
    );
    f.set_run_status(&retry, "failed", None).unwrap();
    let retry_before = serde_json::to_value(f.run_detail(&retry).unwrap()).unwrap();
    let total_before = f.snapshot().unwrap().metrics.total;
    db.execute_batch("CREATE TRIGGER deny_retry_nodes BEFORE INSERT ON nodes BEGIN SELECT RAISE(ABORT,'fixture retry denied'); END;").unwrap();
    assert!(f.retry(&retry).is_err());
    assert_eq!(f.snapshot().unwrap().metrics.total, total_before);
    assert_eq!(
        serde_json::to_value(f.run_detail(&retry).unwrap()).unwrap(),
        retry_before
    );
    db.execute_batch("DROP TRIGGER deny_retry_nodes").unwrap();
    let third = f.retry(&retry).unwrap();
    assert_eq!(f.run_detail(&third).unwrap().run.attempt, 3);
    assert_eq!(
        db.query_row::<String, _, _>("SELECT flow FROM runs WHERE id=?1", [&id], |r| r.get(0))
            .unwrap(),
        raw
    );
}

#[test]
fn activation_profile_lifecycle_and_restart_use_id_not_label_or_secret() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open(dir.path().into()).unwrap();
    f.save_settings(configured()).unwrap();
    let mut s = f.settings().unwrap();
    s.token_profiles[0].label = "Renamed".into();
    s.token_profiles[0].token = Some("rotated-fixture".into());
    f.save_settings(s).unwrap();
    let mut s = f.settings().unwrap();
    s.token_profiles[0].account = "carol".into();
    s.token_profiles[0].token = Some("replacement-fixture".into());
    f.save_settings(s).unwrap();
    assert_eq!(
        f.settings().unwrap().flows[0]
            .activation_for("CAROL", &f.settings().unwrap().token_profiles),
        ActivationState::DisabledForProfile
    );
    let mut s = f.settings().unwrap();
    s.token_profiles.remove(0);
    s.token_profiles.push(TokenProfile {
        id: "new-id".into(),
        label: "New".into(),
        account: "alice".into(),
        token: Some("new-fixture".into()),
        has_token: false,
    });
    f.save_settings(s).unwrap();
    drop(f);
    let backend = common::backend(dir.path());
    backend.gets.lock().unwrap().clear();
    let f = common::open(dir.path().into()).unwrap();
    let s = f.settings().unwrap();
    assert_eq!(s.flows[0].profile_activation.get("work"), Some(&false));
    assert_eq!(
        s.flows[0].activation_for("alice", &s.token_profiles),
        ActivationState::Enabled
    );
    assert!(backend.gets.lock().unwrap().is_empty());
    let mut s = s;
    s.flows[0].profile_activation.remove("work");
    f.save_settings(s).unwrap();
    assert!(f.settings().unwrap().flows[0].profile_activation.is_empty());
}
