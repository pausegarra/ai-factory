use factory_core::*;

#[test]
fn effective_flow_uses_source_id_even_when_logins_match() {
    let mut settings = Settings::default();
    for id in ["src-a", "src-b"] {
        settings.source_connectors.push(SourceConnector {
            id: id.into(),
            label: id.into(),
            account: "alice".into(),
            owners: vec!["team".into()],
            authors: vec!["alice".into()],
            ..Default::default()
        });
    }
    let mut template = settings.flows[0].clone();
    template.enabled = true;
    template.profile_activation.insert("src-a".into(), false);
    template.profile_activation.insert("src-b".into(), true);
    assert_eq!(
        template.activation_for_source("src-a", &settings.source_connectors),
        ActivationState::DisabledForProfile
    );
    assert_eq!(
        template.activation_for_source("src-b", &settings.source_connectors),
        ActivationState::Enabled
    );
    assert_eq!(
        template.activation_for_source("missing", &settings.source_connectors),
        ActivationState::UnavailableSource
    );
    let mut event = TriggerEvent::issue("team/repo", 87, "[AI] fixture", "alice", "");
    event.origin_token_profile_id = Some("src-b".into());
    assert_eq!(settings.source_for_event(&event).unwrap().id, "src-b");
    event.origin_token_profile_id = None;
    assert!(settings.source_for_event(&event).is_err());
}

#[path = "common/activation_http.rs"]
mod activation_http;
mod common;
use serde_json::json;

fn configured() -> Settings {
    let mut s = Settings::default();
    for (id, token) in [("src-a", "fixture-a"), ("src-b", "fixture-b")] {
        s.source_connectors.push(SourceConnector {
            id: id.into(),
            label: id.into(),
            account: "alice".into(),
            owners: vec!["team".into()],
            authors: vec!["alice".into()],
            reviewers: vec!["alice".into()],
            token: Some(token.into()),
            ..Default::default()
        });
    }
    s
}
#[test]
fn event_authorization_rejects_missing_mismatched_revoked_and_malformed_origins() {
    let mut s = configured();
    let mut e = common::from_source(
        TriggerEvent::issue("team/repo", 1, "[AI] fixture", "alice", ""),
        "src-b",
    );
    assert_eq!(s.source_for_event(&e).unwrap().id, "src-b");
    for (repo, actor) in [
        ("other/repo", "alice"),
        ("team/repo", "bob"),
        ("team", "alice"),
    ] {
        e.repository = repo.into();
        e.actor = actor.into();
        assert!(s.source_for_event(&e).is_err(), "{repo}, {actor}");
    }
    e.repository = "team/repo".into();
    e.actor = "alice".into();
    s.source_connectors[1].authors.clear();
    assert!(s.source_for_event(&e).is_err());
    s.source_connectors[1].authors = vec!["alice".into()];
    e.origin_token_profile_id = None;
    assert!(s.source_for_event(&e).is_err());
}
#[test]
fn frozen_agent_preferences_and_bindings_use_source_ids_and_orphan_retry_fails() {
    let mut s = configured();
    s.codex_bindings.insert("src-b".into(), "b-account".into());
    s.flows[0].nodes[0].token_overrides.insert(
        "src-b".into(),
        AgentConfig {
            agent: "codex".into(),
            model: "b-model".into(),
            effort: "high".into(),
        },
    );
    assert_eq!(
        s.flow_for_source(&s.flows[0], "src-b").unwrap().nodes[0].model,
        "b-model"
    );
    assert_eq!(
        s.flow_for_source(&s.flows[0], "src-b").unwrap().nodes[0]
            .codex_account_id
            .as_deref(),
        Some("b-account")
    );
    assert!(s.flow_for_source(&s.flows[0], "src-a").unwrap().nodes[0]
        .model
        .is_empty());
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    s.codex_bindings.clear();
    f.save_settings(s).unwrap();
    let e = common::from_source(
        TriggerEvent::issue("team/repo", 1, "[AI] fixture", "alice", ""),
        "src-b",
    );
    let id = f.enqueue(e.clone()).unwrap().unwrap();
    f.cancel(&id).unwrap();
    let mut s = f.settings().unwrap();
    s.source_connectors.retain(|s| s.id != "src-b");
    f.save_settings(s).unwrap();
    assert!(f.retry(&id).is_err());
    assert!(f.enqueue(e).unwrap().is_none());
    assert_eq!(
        f.run_detail(&id)
            .unwrap()
            .event
            .origin_token_profile_id
            .as_deref(),
        Some("src-b")
    );
}
#[tokio::test]
async fn polling_same_account_tokens_respects_each_sources_activation_and_owners() {
    let server=activation_http::Server::start(|r|async move{
  if r.path=="/user" {return(200,json!({"login":"alice"}));}
  if r.path.starts_with("/search/") {return(200,json!({"incomplete_results":false,"total_count":1,"items":[{"number":1,"repository_url":"https://api.github.com/repos/team/repo"}]}));}
  if r.path=="/repos/team/repo/issues/1" {return(200,json!({"state":"open","title":"[AI] work","user":{"login":"alice"},"html_url":"https://github.com/team/repo/issues/1"}));}
  (500,json!({}))
 }).await;
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut s = configured();
    s.source_connectors[0].owners = vec!["other".into()];
    for flow in &mut s.flows {
        flow.profile_activation.insert("src-a".into(), false);
        if flow.id != "new_issue" {
            flow.enabled = false;
        }
    }
    f.save_settings(s).unwrap();
    f.poll_now().await.unwrap();
    let calls = server.calls.lock().unwrap();
    assert!(!calls.is_empty());
    assert!(calls
        .iter()
        .all(|r| r.method == "GET" && r.authorization == "Bearer fixture-b"));
    assert!(calls
        .iter()
        .filter(|r| r.path.starts_with("/search/"))
        .all(|r| r.path.contains("user:team") || r.path.contains("user%3Ateam")));
    let runs = f.snapshot().unwrap().runs;
    assert_eq!(runs.len(), 1);
    assert_eq!(
        f.run_detail(&runs[0].id)
            .unwrap()
            .event
            .origin_token_profile_id
            .as_deref(),
        Some("src-b")
    );
}
#[test]
fn source_deletion_keeps_notification_orphans_inactive_and_rejects_new_active_selections() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    let mut s = configured();
    s.notification_connectors=serde_json::from_value(json!([{"id":"notice","label":"Notice","type":"telegram","enabled":true,"events":["run_failed"],"config":{"bot_token":"fixture-notice-token","chat_id":"1","triggers":["new_issue"],"token_profile_ids":["src-b"]}}])).unwrap();
    f.save_settings(s).unwrap();
    let mut s = f.settings().unwrap();
    s.source_connectors.clear();
    let saved = f.save_settings(s).unwrap();
    assert_eq!(
        serde_json::to_value(&saved.notification_connectors).unwrap()[0]["config"]
            ["token_profile_ids"],
        json!(["src-b"])
    );
    let mut changed = saved.clone();
    changed.notification_connectors[0].events = vec![NotificationEvent::RunStarted];
    assert!(f.save_settings(changed).is_err());
    let mut orphan_binding = saved;
    orphan_binding.codex_bindings.insert(
        "removed".into(),
        "9ab14479-4d99-41c7-b347-e5b3acffb316".into(),
    );
    f.save_settings(orphan_binding).unwrap();
}
#[tokio::test]
async fn unavailable_source_cannot_load_models_even_for_opencode() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    let mut s = f.settings().unwrap();
    s.commands.opencode = dir
        .path()
        .join("never-launch-unavailable-source")
        .to_string_lossy()
        .into_owned();
    f.save_settings(s).unwrap();
    let error = f
        .agent_models_for_source("opencode", Some("missing"))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("Source connector unavailable"),
        "{error}"
    );
}

#[tokio::test]
async fn approval_uses_durable_source_after_run_deletion_and_never_a_same_login_connector() {
    let server = activation_http::Server::start(|r| async move {
        if r.path == "/user" { return (200, json!({"login":"alice"})); }
        if r.path.starts_with("/search/") { return (200, json!({"incomplete_results":false,"total_count":1,"items":[{"number":5,"repository_url":"https://api.github.com/repos/team/repo"}]})); }
        if r.path == "/repos/team/repo/pulls/5" { return (200, json!({"number":5,"title":"PR","body":"Closes #1","state":"open","html_url":"https://github.com/team/repo/pull/5","user":{"login":"alice"},"base":{"repo":{"full_name":"team/repo"}},"head":{"sha":"a".repeat(40),"ref":"feature"}})); }
        if r.path.contains("/reviews?") { return (200, json!([])); }
        if r.path.contains("/comments?") { return (200, json!([{"id":7,"body":"OK","user":{"login":"alice"}}])); }
        (500,json!({}))
    }).await;
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    f.save_settings(configured()).unwrap();
    let e = common::from_source(
        TriggerEvent::issue("team/repo", 1, "[AI] source", "alice", ""),
        "src-a",
    );
    let id = f.enqueue(e).unwrap().unwrap();
    let store=factory_core::context::factory::infrastructure::repositories::store::Store::open_with_credentials(dir.path(),common::backend(dir.path())).unwrap();
    store.track_pr("team/repo", 5, &id).unwrap();
    drop(store);
    f.cancel(&id).unwrap();
    f.delete_run(&id).unwrap();
    let mut s = f.settings().unwrap();
    for flow in &mut s.flows {
        flow.enabled = flow.id == "pr_approved";
        flow.profile_activation.insert("src-a".into(), false);
    }
    f.save_settings(s).unwrap();
    f.poll_now().await.unwrap();
    assert!(server.calls.lock().unwrap().is_empty());
    let mut s = f.settings().unwrap();
    s.flows[1].profile_activation.remove("src-a");
    f.save_settings(s).unwrap();
    f.poll_now().await.unwrap();
    let calls = server.calls.lock().unwrap();
    assert!(!calls.is_empty());
    assert!(calls
        .iter()
        .all(|r| r.method == "GET" && r.authorization == "Bearer fixture-a"));
    let runs = f.snapshot().unwrap().runs;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].trigger_id, "pr_approved");
    assert_eq!(
        f.run_detail(&runs[0].id)
            .unwrap()
            .event
            .origin_token_profile_id
            .as_deref(),
        Some("src-a")
    );
}
