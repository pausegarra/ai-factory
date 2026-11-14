mod common;
use factory_core::*;
use serde_json::json;

const TRIGGERS: [&str; 3] = ["new_issue", "pr_review", "pr_approved"];

#[test]
fn legacy_capacity_inherits_and_explicit_override_stays_independent() {
    let mut s: Settings = serde_json::from_value(json!({"max_concurrent": 8})).unwrap();
    assert!(s.trigger_max_concurrent.is_empty());
    for trigger in TRIGGERS {
        assert_eq!(s.concurrency_for(trigger).unwrap(), 8);
        assert_eq!(Settings::default().concurrency_for(trigger).unwrap(), 4);
    }
    s.trigger_max_concurrent.insert("new_issue".into(), 8);
    s.max_concurrent = 4;
    let reread: Settings = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(reread.concurrency_for("new_issue").unwrap(), 8);
    assert_eq!(reread.concurrency_for("pr_review").unwrap(), 4);
    assert_eq!(reread.trigger_max_concurrent.len(), 1);
    assert!(s.concurrency_for("unknown").is_err());
    assert_eq!(
        serde_json::to_value(Settings::default()).unwrap()["trigger_max_concurrent"],
        json!({})
    );
}

#[test]
fn capacity_rejects_invalid_input_instead_of_normalizing_it() {
    for invalid in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(65),
        json!("4"),
        json!(null),
    ] {
        let mut raw = serde_json::to_value(Settings::default()).unwrap();
        raw["max_concurrent"] = invalid.clone();
        assert!(
            serde_json::from_value::<Settings>(raw).is_err(),
            "global: {invalid}"
        );
        for trigger in TRIGGERS {
            let mut raw = serde_json::to_value(Settings::default()).unwrap();
            raw["trigger_max_concurrent"] = json!({trigger: invalid.clone()});
            assert!(
                serde_json::from_value::<Settings>(raw).is_err(),
                "{trigger}: {invalid}"
            );
        }
    }
    for overrides in [json!(null), json!([]), json!({"unknown": 4})] {
        let mut raw = serde_json::to_value(Settings::default()).unwrap();
        raw["trigger_max_concurrent"] = overrides;
        assert!(serde_json::from_value::<Settings>(raw).is_err());
    }
    for limit in [1, 64] {
        let raw = json!({"max_concurrent":limit,"trigger_max_concurrent": {"new_issue":limit,"pr_review":limit,"pr_approved":limit}});
        let s: Settings = serde_json::from_value(raw).unwrap();
        s.validate().unwrap();
        for trigger in TRIGGERS {
            assert_eq!(s.concurrency_for(trigger).unwrap(), limit as usize);
        }
    }
}

#[test]
fn capacity_roundtrip_persists_overrides_and_public_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    let mut s = f.settings().unwrap();
    s.max_concurrent = 8;
    s.trigger_max_concurrent = [
        ("new_issue".into(), 4),
        ("pr_review".into(), 4),
        ("pr_approved".into(), 2),
    ]
    .into();
    s.token_profiles.push(TokenProfile {
        id: "fixture".into(),
        label: "Fixture".into(),
        account: "alice".into(),
        token: Some("capacity-fixture-secret".into()),
        ..Default::default()
    });
    let saved = f.save_settings(s).unwrap();
    let before = serde_json::to_value(&saved).unwrap();
    assert!(!before.to_string().contains("capacity-fixture-secret"));
    assert_eq!(
        serde_json::to_value(saved.redacted()).unwrap()["trigger_max_concurrent"],
        json!({"new_issue":4,"pr_review":4,"pr_approved":2})
    );
    let mut invalid = saved.clone();
    invalid
        .trigger_max_concurrent
        .insert("pr_approved".into(), 0);
    assert!(f.save_settings(invalid).is_err());
    assert_eq!(serde_json::to_value(f.settings().unwrap()).unwrap(), before);
    drop(f);
    let reopened = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    let s = reopened.settings().unwrap();
    assert_eq!(s.max_concurrent, 8);
    assert_eq!(s.trigger_max_concurrent, saved.trigger_max_concurrent);
    assert!(s.token_profiles[0].has_token);
}

#[test]
fn step_parallelism_and_run_capacity_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    let mut s = f.settings().unwrap();
    s.max_concurrent = 8;
    s.trigger_max_concurrent = [
        ("new_issue".into(), 8),
        ("pr_review".into(), 8),
        ("pr_approved".into(), 8),
    ]
    .into();
    for parallel in [1, 64] {
        s.flows[0].max_parallel = parallel;
        s = f.save_settings(s).unwrap();
        assert_eq!(s.max_concurrent, 8);
        assert!(TRIGGERS.iter().all(|t| s.concurrency_for(t).unwrap() == 8));
    }
    s.max_concurrent = 1;
    s.trigger_max_concurrent.insert("pr_approved".into(), 2);
    let saved = f.save_settings(s).unwrap();
    assert_eq!(saved.flows[0].max_parallel, 64);
    assert_eq!(saved.concurrency_for("pr_approved").unwrap(), 2);
}
