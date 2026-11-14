mod common;
use factory_core::context::factory::infrastructure::repositories::store::Store;
use factory_core::*;
use serde_json::json;
use std::sync::Arc;
#[test]
fn notification_origin_survives_restart_and_stale_status_without_actor_inference() {
    let dir = tempfile::tempdir().unwrap();
    let c = Arc::new(common::MemoryCredentials::default());
    let store = Store::open_with_credentials(dir.path(), c.clone()).unwrap();
    let mut value =
        serde_json::to_value(TriggerEvent::issue("acme/repo", 61, "fixture", "alice", "")).unwrap();
    value["origin_token_profile_id"] = json!("work-profile");
    let event: TriggerEvent = serde_json::from_value(value).unwrap();
    let id = store
        .insert(&event, &Settings::default().flows[0], 1, true)
        .unwrap()
        .unwrap();
    let mut stale = serde_json::to_value(store.detail(&id).unwrap().run).unwrap();
    stale["origin_token_profile_id"] = json!("wrong-profile");
    stale["status"] = json!("running");
    store
        .save_run(&serde_json::from_value(stale).unwrap())
        .unwrap();
    drop(store);
    let store = Store::open_with_credentials(dir.path(), c).unwrap();
    let detail = store.detail(&id).unwrap();
    assert_eq!(
        serde_json::to_value(&detail.run).unwrap()["origin_token_profile_id"],
        "work-profile"
    );
    assert_eq!(
        serde_json::to_value(&detail.event).unwrap()["origin_token_profile_id"],
        "work-profile"
    );
    assert_eq!(detail.run.actor, "alice");
    let legacy = TriggerEvent::issue("acme/repo", 62, "fixture", "alice", "");
    let id = store
        .insert(&legacy, &Settings::default().flows[0], 1, true)
        .unwrap()
        .unwrap();
    assert!(serde_json::to_value(store.detail(&id).unwrap().run)
        .unwrap()
        .get("origin_token_profile_id")
        .is_none());
}

#[test]
fn notification_retry_and_dedup_keep_origin_and_stale_admission_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let f = Factory::open_with_notification_transport(
        dir.path().into(),
        "http://127.0.0.1:9",
        Arc::new(common::MemoryCredentials::default()),
        Arc::new(common::notifications::RecordingTransport::default()),
    )
    .unwrap();
    let mut s = f.settings().unwrap();
    s.token_profiles.push(TokenProfile {
        id: "work".into(),
        account: "alice".into(),
        label: "Work".into(),
        ..Default::default()
    });
    s.token_profiles.push(TokenProfile {
        id: "other".into(),
        account: "bob".into(),
        label: "Other".into(),
        ..Default::default()
    });
    f.save_settings(s).unwrap();
    let mut e = TriggerEvent::issue("acme/repo", 1, "Fixture", "alice", "");
    e.origin_token_profile_id = Some("work".into());
    let id = f.enqueue(e.clone()).unwrap().unwrap();
    let mut duplicate = e.clone();
    duplicate.origin_token_profile_id = Some("other".into());
    assert!(f.enqueue(duplicate).unwrap().is_none());
    assert_eq!(
        f.run_detail(&id)
            .unwrap()
            .run
            .origin_token_profile_id
            .as_deref(),
        Some("work")
    );
    f.set_run_status(&id, "failed", None).unwrap();
    let mut s = f.settings().unwrap();
    s.token_profiles[0].label = "Renamed".into();
    f.save_settings(s).unwrap();
    let retry = f.retry(&id).unwrap();
    let detail = f.run_detail(&retry).unwrap();
    assert_eq!(
        detail.event.origin_token_profile_id.as_deref(),
        Some("work")
    );
    assert_eq!(detail.run.origin_token_profile_id.as_deref(), Some("work"));
    let mut s = f.settings().unwrap();
    s.token_profiles.retain(|p| p.id != "work");
    f.save_settings(s).unwrap();
    e.number = 2;
    e.event_id = "issue:acme/repo#2".into();
    assert!(f.enqueue(e).unwrap().is_none());
}
