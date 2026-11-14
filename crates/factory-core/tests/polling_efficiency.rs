#[path = "common/activation_http.rs"]
mod activation_http;
mod common;
use activation_http::Server;
use factory_core::context::factory::infrastructure::repositories::store::Store;
use factory_core::*;
use serde_json::{json, Value};

fn settings(factory: &Factory) -> Settings {
    let mut s = factory.settings().unwrap();
    s.token_profiles = vec![TokenProfile {
        id: "alice".into(),
        account: "alice".into(),
        label: "Alice".into(),
        owners: vec!["acme".into()],
        authors: vec!["alice".into()],
        reviewers: vec!["alice".into()],
        token: Some("fixture-token".into()),
        ..Default::default()
    }];
    s
}
fn search(items: Vec<Value>) -> Value {
    json!({"incomplete_results":false,"total_count":items.len(),"items":items})
}

#[tokio::test]
async fn historical_closed_prs_need_no_individual_requests_on_any_cycle() {
    let server = Server::start(|r| async move {
        if r.path == "/user" {
            return (200, json!({"login":"alice"}));
        }
        if r.path.starts_with("/search/issues?") {
            return (200, search(vec![]));
        }
        (500, json!({"unexpected":r.path}))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    factory.save_settings(settings(&factory)).unwrap();
    let store = Store::open(dir.path()).unwrap();
    for number in 1..=20 {
        let id = factory
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                number,
                "[AI] old",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        store.track_pr("acme/repo", number + 100, &id).unwrap();
    }
    let mut s = factory.settings().unwrap();
    s.flows[0].enabled = false;
    s.flows[2].enabled = false;
    factory.save_settings(s).unwrap();
    factory.poll_now().await.unwrap();
    factory.poll_now().await.unwrap();
    let calls = server.calls.lock().unwrap();
    assert!(calls.iter().all(|r| r.method == "GET"
        && r.authorization == "Bearer fixture-token"
        && !r.path.starts_with("/repos/")));
    assert_eq!(calls.iter().filter(|r| r.path == "/user").count(), 1);
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.path.starts_with("/search/"))
            .count(),
        2
    );
}

#[tokio::test]
async fn processed_issues_are_filtered_before_detail_fetch() {
    let server = Server::start(|r| async move {
        if r.path == "/user" {return (200,json!({"login":"alice"}));}
        if r.path.starts_with("/search/") {return (200,search(vec![json!({"number":1,"title":"[AI] task","repository_url":"https://api.github.com/repos/acme/repo"})]));}
        if r.path == "/repos/acme/repo/issues/1" {return (200,json!({"number":1,"title":"[AI] task","state":"open","user":{"login":"alice"},"html_url":"https://github.com/acme/repo/issues/1"}));}
        (500,json!({"unexpected":r.path}))
    }).await;
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut s = settings(&factory);
    s.flows[1].enabled = false;
    s.flows[2].enabled = false;
    factory.save_settings(s).unwrap();
    factory.poll_now().await.unwrap();
    factory.poll_now().await.unwrap();
    assert_eq!(factory.snapshot().unwrap().runs.len(), 1);
    assert_eq!(
        server
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == "/repos/acme/repo/issues/1")
            .count(),
        1
    );
}

#[tokio::test]
async fn owner_scopes_and_identity_are_grouped_per_token_across_triggers() {
    let server = Server::start(|r| async move {
        if r.path == "/user" {
            return (200, json!({"login":"alice"}));
        }
        if r.path.starts_with("/search/") {
            let url = reqwest::Url::parse(&format!("http://fixture{}", r.path)).unwrap();
            if !url
                .query_pairs()
                .any(|(key, value)| key == "advanced_search" && value == "true")
            {
                return (
                    422,
                    json!({"message":"Boolean owner search requires advanced_search=true"}),
                );
            }
            return (200, search(vec![]));
        }
        (500, json!({"unexpected":r.path}))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut s = settings(&factory);
    s.token_profiles[0].owners.push("other".into());
    factory.save_settings(s).unwrap();
    factory.poll_now().await.unwrap();
    let calls = server.calls.lock().unwrap();
    assert_eq!(calls.iter().filter(|r| r.path == "/user").count(), 1);
    let searches: Vec<_> = calls
        .iter()
        .filter(|r| r.path.starts_with("/search/"))
        .collect();
    assert_eq!(searches.len(), 2);
    for request in searches {
        let url = reqwest::Url::parse(&format!("http://fixture{}", request.path)).unwrap();
        let query = url
            .query_pairs()
            .find(|(key, _)| key == "q")
            .unwrap()
            .1
            .into_owned();
        assert!(
            query.contains("user:acme") && query.contains("user:other"),
            "{query}"
        );
    }
}

#[tokio::test]
async fn issue_with_published_pr_stays_consumed_after_local_history_deletion() {
    let server = Server::start(|r| async move {
        if r.path == "/user" {return (200,json!({"login":"alice"}));}
        if r.path.starts_with("/search/") {return (200,search(vec![json!({"number":1,"title":"[AI] task","repository_url":"https://api.github.com/repos/acme/repo"})]));}
        (500,json!({"unexpected":r.path}))
    }).await;
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    factory.save_settings(settings(&factory)).unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            1,
            "[AI] task",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    Store::open(dir.path())
        .unwrap()
        .track_pr("acme/repo", 101, &id)
        .unwrap();
    let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
    db.execute("DELETE FROM nodes WHERE run_id=?1", [&id])
        .unwrap();
    db.execute("DELETE FROM runs WHERE id=?1", [&id]).unwrap();
    db.execute("DELETE FROM dedup WHERE run_id=?1", [&id])
        .unwrap();
    let mut s = factory.settings().unwrap();
    s.flows[1].enabled = false;
    s.flows[2].enabled = false;
    factory.save_settings(s).unwrap();
    factory.poll_now().await.unwrap();
    assert_eq!(factory.snapshot().unwrap().runs.len(), 0);
    assert!(server
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|r| !r.path.starts_with("/repos/")));
}

fn pull(number: u64) -> Value {
    json!({"number":number,"title":"PR","body":"Closes #1","state":"open","html_url":format!("https://github.com/acme/repo/pull/{number}"),"base":{"repo":{"full_name":"acme/repo"}},"head":{"sha":"a".repeat(40),"ref":"feature"},"user":{"login":"alice"},"requested_reviewers":[]})
}
async fn tracked_factory(server: &Server) -> (tempfile::TempDir, std::sync::Arc<Factory>) {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    factory.save_settings(settings(&factory)).unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            1,
            "[AI] task",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    Store::open(dir.path())
        .unwrap()
        .track_pr("acme/repo", 105, &id)
        .unwrap();
    let mut s = factory.settings().unwrap();
    s.flows[0].enabled = false;
    s.flows[2].enabled = false;
    factory.save_settings(s).unwrap();
    (dir, factory)
}

#[tokio::test]
async fn approval_scan_intersects_paginated_open_prs_with_local_tracking() {
    let server = Server::start(|r|async move {
        if r.path == "/user" {return (200,json!({"login":"alice"}));}
        if r.path.starts_with("/search/") {
            let items:Vec<_> = if r.path.contains("page=2") {vec![json!({"number":105,"repository_url":"https://api.github.com/repos/acme/repo","pull_request":{}})]}
                else {(1000..1100).map(|number|json!({"number":number,"repository_url":"https://api.github.com/repos/acme/repo","pull_request":{}})).collect()};
            return (200,json!({"incomplete_results":false,"total_count":101,"items":items}));
        }
        if r.path == "/repos/acme/repo/pulls/105" {return(200,pull(105));}
        if r.path.contains("/reviews?") {return(200,json!([]));}
        if r.path.contains("/comments?") {return(200,json!([{"id":7,"body":"OK","user":{"login":"alice"}}]));}
        (500,json!({"unexpected":r.path}))
    }).await;
    let (_dir, factory) = tracked_factory(&server).await;
    factory.poll_now().await.unwrap();
    let approvals: Vec<_> = factory
        .snapshot()
        .unwrap()
        .runs
        .into_iter()
        .filter(|r| r.trigger_id == "pr_approved")
        .collect();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].number, 105);
    let calls = server.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.path.starts_with("/search/"))
            .count(),
        2
    );
    assert!(calls
        .iter()
        .filter(|r| r.path.starts_with("/repos/"))
        .all(|r| r.path.contains("/105")));
}

#[tokio::test]
async fn incomplete_discovery_cannot_approve_a_tracked_candidate() {
    let server = Server::start(|r|async move {
        if r.path == "/user" {return (200,json!({"login":"alice"}));}
        if r.path.starts_with("/search/") {return(200,json!({"incomplete_results":true,"total_count":1,"items":[{"number":105,"repository_url":"https://api.github.com/repos/acme/repo","pull_request":{}}]}));}
        (500,json!({"unexpected":r.path}))
    }).await;
    let (_dir, factory) = tracked_factory(&server).await;
    assert!(factory.poll_now().await.is_err());
    assert_eq!(factory.snapshot().unwrap().runs.len(), 1);
    assert!(server
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|r| !r.path.starts_with("/repos/")));
}
