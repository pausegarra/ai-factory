#[path = "common/activation_http.rs"]
mod activation_http;
mod common;
use activation_http::Server;
use factory_core::*;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

fn pull(repo: &str, number: u64) -> Value {
    json!({
        "number": number, "title": "Review fixture", "body": "",
        "state": "open", "draft": true,
        "html_url": format!("https://github.com/{repo}/pull/{number}"),
        "base": {"repo": {"full_name": repo}},
        "head": {"sha": "a".repeat(40), "ref": "feature"},
        "user": {"login": "contributor"}, "requested_reviewers": []
    })
}
fn row(repo: &str, number: u64) -> Value {
    json!({"number":number,
        "repository_url":format!("https://api.github.com/repos/{repo}"),
        "pull_request":{}})
}
struct Discovery {
    rows: Vec<Value>,
    pulls: BTreeMap<String, Value>,
    status: u16,
    incomplete: bool,
    login: String,
    detail_gate: Option<(Arc<Notify>, Arc<Notify>)>,
}
impl Default for Discovery {
    fn default() -> Self {
        Self {
            rows: vec![row("acme/repo", 5)],
            pulls: BTreeMap::from([("/repos/acme/repo/pulls/5".into(), pull("acme/repo", 5))]),
            status: 200,
            incomplete: false,
            login: "alice".into(),
            detail_gate: None,
        }
    }
}
async fn fixture(
    all: bool,
) -> (
    tempfile::TempDir,
    Arc<Factory>,
    Server,
    Arc<Mutex<Discovery>>,
) {
    let state = Arc::new(Mutex::new(Discovery::default()));
    let captured = state.clone();
    let server = Server::start(move |request| {
        let state = captured.clone();
        async move {
            let gate = {
                let mut state = state.lock().unwrap();
                if request.path == "/repos/acme/repo/pulls/5" {
                    state.detail_gate.take()
                } else {
                    None
                }
            };
            if let Some((reached, release)) = gate {
                reached.notify_one();
                release.notified().await;
            }
            let state = state.lock().unwrap();
            if request.path == "/user" {
                return (200, json!({"login":state.login}));
            }
            if request.path.starts_with("/search/issues?") {
                return (
                    state.status,
                    json!({
                        "incomplete_results":state.incomplete,
                        "total_count":state.rows.len(), "items":state.rows
                    }),
                );
            }
            if let Some(pr) = state.pulls.get(&request.path) {
                return (200, pr.clone());
            }
            (500, json!({"unexpected":request.path}))
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.source_connectors = vec![SourceConnector {
        id: "work".into(),
        label: "Work".into(),
        account: "alice".into(),
        owners: vec!["acme".into()],
        reviewers: vec!["alice".into()],
        token: Some("fixture-token".into()),
        review_all_open_pull_requests: all,
        ..Default::default()
    }];
    for flow in &mut settings.flows {
        flow.enabled = flow.id == "pr_review";
    }
    factory.save_settings(settings).unwrap();
    (dir, factory, server, state)
}
fn events(factory: &Factory) -> Vec<TriggerEvent> {
    factory
        .snapshot()
        .unwrap()
        .runs
        .into_iter()
        .map(|run| {
            assert_eq!(run.trigger_id, "pr_review");
            factory.run_detail(&run.id).unwrap().event
        })
        .collect()
}
fn queries(server: &Server) -> Vec<String> {
    server
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.path.starts_with("/search/issues?"))
        .map(|r| {
            let url = reqwest::Url::parse(&format!("http://fixture{}", r.path)).unwrap();
            assert!(url
                .query_pairs()
                .any(|(k, v)| k == "advanced_search" && v == "true"));
            url.query_pairs()
                .find(|(k, _)| k == "q")
                .unwrap()
                .1
                .into_owned()
        })
        .collect()
}

#[tokio::test]
async fn review_scope_query_and_detail_matrix() {
    for all in [false, true] {
        for (state_name, requested, expected) in [
            ("open", false, usize::from(all)),
            ("open", true, 1),
            ("closed", false, 0),
            ("closed", true, 0),
        ] {
            let (_dir, factory, server, state) = fixture(all).await;
            {
                let mut state = state.lock().unwrap();
                let pr = state.pulls.get_mut("/repos/acme/repo/pulls/5").unwrap();
                pr["state"] = json!(state_name);
                pr["requested_reviewers"] = if requested {
                    json!([{"login":"ALICE"}])
                } else {
                    json!([])
                };
            }
            factory.poll_now().await.unwrap();
            let found = events(&factory);
            assert_eq!(found.len(), expected, "{all}/{state_name}/{requested}");
            let searches = queries(&server);
            assert!(!searches.is_empty());
            for query in searches {
                assert!(query.contains("is:pr") && query.contains("is:open"));
                assert!(query.contains("user:acme"));
                assert_eq!(query.contains("review-requested:alice"), !all);
                if all {
                    assert!(!query.contains("review-requested:"));
                }
            }
            for event in found {
                assert_eq!(event.actor, "alice");
                assert_eq!(event.origin_token_profile_id.as_deref(), Some("work"));
                assert_eq!(event.payload["reviewer"], "alice");
                assert_eq!(event.payload["pull_request"]["draft"], true);
                assert_eq!(event.repository, "acme/repo");
            }
            assert!(server
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|r| r.method == "GET" && r.authorization == "Bearer fixture-token"));
        }
    }
}

#[tokio::test]
async fn review_scope_uses_every_owner_and_existing_search_batches() {
    let (_dir, factory, server, state) = fixture(true).await;
    let mut settings = factory.settings().unwrap();
    settings.source_connectors[0].pull_request_authors = vec!["CONTRIBUTOR".into()];
    factory.save_settings(settings).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.source_connectors[0].owners = vec![
        "acme".into(),
        "other".into(),
        "team3".into(),
        "team4".into(),
        "team5".into(),
        "team6".into(),
        "team7".into(),
    ];
    factory.save_settings(settings).unwrap();
    {
        let mut state = state.lock().unwrap();
        state.rows.push(row("other/repo", 6));
        state
            .pulls
            .insert("/repos/other/repo/pulls/6".into(), pull("other/repo", 6));
    }
    factory.poll_now().await.unwrap();
    let searches = queries(&server);
    assert_eq!(searches.len(), 2);
    for owner in ["acme", "other", "team3", "team4", "team5", "team6", "team7"] {
        assert!(searches
            .iter()
            .any(|q| q.contains(&format!("user:{owner}"))));
    }
    assert!(searches.iter().all(|q| q.matches("user:").count() <= 6));
    let mut repos: Vec<_> = events(&factory).into_iter().map(|e| e.repository).collect();
    repos.sort();
    assert_eq!(repos, ["acme/repo", "other/repo"]);
}

#[tokio::test]
async fn review_scope_cannot_bypass_scope_identity_or_activation() {
    for all in [false, true] {
        for mode in [
            "outside_owner",
            "detail_identity",
            "wrong_token",
            "no_owners",
            "reviewer_revoked",
            "workflow_off",
            "source_off",
        ] {
            let (_dir, factory, server, state) = fixture(all).await;
            let mut settings = factory.settings().unwrap();
            settings.source_connectors[0].pull_request_authors = vec!["CONTRIBUTOR".into()];
            factory.save_settings(settings).unwrap();
            {
                let mut state = state.lock().unwrap();
                state.pulls.get_mut("/repos/acme/repo/pulls/5").unwrap()["requested_reviewers"] =
                    json!([{"login":"alice"}]);
                match mode {
                    "outside_owner" => state.rows.push(row("forbidden/repo", 6)),
                    "detail_identity" => {
                        state.pulls.get_mut("/repos/acme/repo/pulls/5").unwrap()["base"]["repo"]
                            ["full_name"] = json!("forbidden/repo");
                    }
                    "wrong_token" => state.login = "mallory".into(),
                    _ => {}
                }
            }
            let mut settings = factory.settings().unwrap();
            match mode {
                "no_owners" => settings.source_connectors[0].owners.clear(),
                "reviewer_revoked" => settings.source_connectors[0].reviewers.clear(),
                "workflow_off" => {
                    settings
                        .flows
                        .iter_mut()
                        .find(|f| f.id == "pr_review")
                        .unwrap()
                        .enabled = false
                }
                "source_off" => {
                    settings
                        .flows
                        .iter_mut()
                        .find(|f| f.id == "pr_review")
                        .unwrap()
                        .profile_activation
                        .insert("work".into(), false);
                }
                _ => {}
            }
            factory.save_settings(settings).unwrap();
            let result = factory.poll_now().await;
            assert_eq!(
                result.is_err(),
                matches!(mode, "outside_owner" | "detail_identity" | "wrong_token"),
                "{all}/{mode}: {result:?}"
            );
            assert!(events(&factory).is_empty(), "{all}/{mode}");
            let calls = server.calls.lock().unwrap();
            if matches!(
                mode,
                "no_owners" | "reviewer_revoked" | "workflow_off" | "source_off"
            ) {
                assert!(calls.is_empty(), "{all}/{mode}");
            }
            assert!(!calls
                .iter()
                .any(|r| r.path.starts_with("/repos/forbidden/")));
        }
    }
}

#[tokio::test]
async fn review_scope_preserves_scan_edges_and_deduplication() {
    let (_dir, factory, _server, state) = fixture(true).await;
    let mut settings = factory.settings().unwrap();
    settings.source_connectors[0].pull_request_authors = vec!["CONTRIBUTOR".into()];
    factory.save_settings(settings).unwrap();
    factory.poll_now().await.unwrap();
    factory.poll_now().await.unwrap();
    assert_eq!(events(&factory).len(), 1);

    // Cambiar reviewers no retira una PR presente en modo amplio.
    state
        .lock()
        .unwrap()
        .pulls
        .get_mut("/repos/acme/repo/pulls/5")
        .unwrap()["requested_reviewers"] = json!([{"login":"someone-else"}]);
    factory.poll_now().await.unwrap();
    assert_eq!(events(&factory).len(), 1);

    // Error de Search y resultado incompleto no deben marcar retirada.
    for (status, incomplete) in [(422, false), (200, true)] {
        {
            let mut state = state.lock().unwrap();
            state.status = status;
            state.incomplete = incomplete;
        }
        assert!(factory.poll_now().await.is_err());
        {
            let mut state = state.lock().unwrap();
            state.status = 200;
            state.incomplete = false;
        }
        factory.poll_now().await.unwrap();
        assert_eq!(events(&factory).len(), 1);
    }

    // Detalle erróneo tampoco debe retirar la observación.
    state
        .lock()
        .unwrap()
        .pulls
        .get_mut("/repos/acme/repo/pulls/5")
        .unwrap()["number"] = json!(99);
    assert!(factory.poll_now().await.is_err());
    state
        .lock()
        .unwrap()
        .pulls
        .get_mut("/repos/acme/repo/pulls/5")
        .unwrap()["number"] = json!(5);
    factory.poll_now().await.unwrap();
    assert_eq!(events(&factory).len(), 1);

    // Sigue presente en ambos modos: no crear otra identidad de dedup.
    state
        .lock()
        .unwrap()
        .pulls
        .get_mut("/repos/acme/repo/pulls/5")
        .unwrap()["requested_reviewers"] = json!([{"login":"alice"}]);
    let mut settings = factory.settings().unwrap();
    settings.source_connectors[0].review_all_open_pull_requests = false;
    factory.save_settings(settings).unwrap();
    factory.poll_now().await.unwrap();
    assert_eq!(events(&factory).len(), 1);
    let mut settings = factory.settings().unwrap();
    settings.source_connectors[0].review_all_open_pull_requests = true;
    factory.save_settings(settings).unwrap();
    factory.poll_now().await.unwrap();
    assert_eq!(events(&factory).len(), 1);

    // Retirada y reaparición reales reutilizan la siguiente generación existente.
    state.lock().unwrap().rows.clear();
    factory.poll_now().await.unwrap();
    state.lock().unwrap().rows.push(row("acme/repo", 5));
    factory.poll_now().await.unwrap();
    let found = events(&factory);
    assert_eq!(found.len(), 2);
    assert_ne!(found[0].event_id, found[1].event_id);
    assert!(found
        .iter()
        .all(|e| e.event_id.starts_with("review:acme/repo#5:alice:")));
}

#[tokio::test]
async fn pull_request_authors_polling_mode_and_login_matrix() {
    for all in [false, true] {
        for filtered in [false, true] {
            for requested in [false, true] {
                for (missing, login) in [
                    (false, json!("CCarneroMerida")),
                    (false, json!("next-dvalverde")),
                    (false, json!("outside")),
                    (false, json!(null)),
                    (true, json!(null)),
                    (false, json!(42)),
                    (false, json!("")),
                    (false, json!("bad login")),
                ] {
                    let (_dir, factory, server, state) = fixture(all).await;
                    let mut settings = factory.settings().unwrap();
                    settings.source_connectors[0].pull_request_authors = if filtered {
                        vec!["ccarneromerida".into(), "next-dvalverde".into()]
                    } else {
                        vec![]
                    };
                    factory.save_settings(settings).unwrap();
                    {
                        let mut state = state.lock().unwrap();
                        let pr = state.pulls.get_mut("/repos/acme/repo/pulls/5").unwrap();
                        if missing {
                            pr.as_object_mut().unwrap().remove("user");
                        } else {
                            pr["user"]["login"] = login.clone();
                        }
                        pr["requested_reviewers"] = if requested {
                            json!([{"login":"ALICE"}])
                        } else {
                            json!([])
                        };
                    }
                    factory.poll_now().await.unwrap();
                    let matches = login.as_str().is_some_and(|s| {
                        s.eq_ignore_ascii_case("ccarneromerida") || s == "next-dvalverde"
                    });
                    let want = if all { !filtered || matches } else { requested };
                    let found = events(&factory);
                    assert_eq!(
                        found.len(),
                        usize::from(want),
                        "{all}/{filtered}/{requested}/{missing}/{login}"
                    );
                    for event in found {
                        assert_eq!(event.actor, "alice");
                        assert_eq!(event.payload["reviewer"], "alice");
                        assert_eq!(event.origin_token_profile_id.as_deref(), Some("work"));
                    }
                    for query in queries(&server) {
                        assert!(query.contains("user:acme"));
                        assert_eq!(query.contains("review-requested:alice"), !all);
                        assert!(!query.contains("author:"));
                    }
                    factory.shutdown().await;
                }
            }
        }
    }
}

#[tokio::test]
async fn pull_request_authors_exclusion_does_not_consume_and_reuses_edges() {
    let (dir, factory, _server, _state) = fixture(true).await;
    let mut s = factory.settings().unwrap();
    s.source_connectors[0].pull_request_authors = vec!["different".into()];
    factory.save_settings(s).unwrap();
    factory.poll_now().await.unwrap();
    factory.poll_now().await.unwrap();
    assert!(events(&factory).is_empty());
    {
        let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
        let count: i64 = db
            .query_row(
                "SELECT count(*) FROM dedup WHERE event_id LIKE 'review:acme/repo#5:alice:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        let mut q = db.prepare("SELECT json FROM observations WHERE key IN ('review:acme:alice','review-source:work:acme:alice')").unwrap();
        for row in q.query_map([], |r| r.get::<_, String>(0)).unwrap() {
            let observation: Value = serde_json::from_str(&row.unwrap()).unwrap();
            assert!(observation.get("acme/repo#5").is_none());
        }
    }
    let mut s = factory.settings().unwrap();
    s.source_connectors[0].pull_request_authors = vec!["CONTRIBUTOR".into()];
    factory.save_settings(s).unwrap();
    factory.poll_now().await.unwrap();
    assert_eq!(events(&factory)[0].event_id, "review:acme/repo#5:alice:1");
    factory.poll_now().await.unwrap();
    assert_eq!(events(&factory).len(), 1);
    let mut s = factory.settings().unwrap();
    s.source_connectors[0].pull_request_authors = vec!["different".into()];
    factory.save_settings(s).unwrap();
    factory.poll_now().await.unwrap();
    assert_eq!(events(&factory).len(), 1);
    let mut s = factory.settings().unwrap();
    s.source_connectors[0].pull_request_authors.clear();
    factory.save_settings(s).unwrap();
    factory.poll_now().await.unwrap();
    let mut ids: Vec<_> = events(&factory).into_iter().map(|e| e.event_id).collect();
    ids.sort();
    assert_eq!(
        ids,
        ["review:acme/repo#5:alice:1", "review:acme/repo#5:alice:2"]
    );
    factory.shutdown().await;
}

// A saved exclusion must win over the Source captured before the HTTP await.
#[tokio::test]
async fn pull_request_authors_saved_during_detail_are_rechecked_before_admission() {
    for all in [true, false] {
        for initial in [vec![], vec!["contributor".to_string()]] {
            let (dir, factory, _server, state) = fixture(all).await;
            let mut settings = factory.settings().unwrap();
            settings.source_connectors[0].pull_request_authors = initial;
            factory.save_settings(settings).unwrap();
            let reached = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            {
                let mut state = state.lock().unwrap();
                state.detail_gate = Some((reached.clone(), release.clone()));
                state.pulls.get_mut("/repos/acme/repo/pulls/5").unwrap()["requested_reviewers"] =
                    if all {
                        json!([])
                    } else {
                        json!([{"login":"alice"}])
                    };
            }
            let pending = {
                let factory = factory.clone();
                tokio::spawn(async move { factory.poll_now().await })
            };
            tokio::time::timeout(std::time::Duration::from_secs(5), reached.notified())
                .await
                .unwrap();
            let mut settings = factory.settings().unwrap();
            settings.source_connectors[0].pull_request_authors = vec!["different".into()];
            factory.save_settings(settings).unwrap();
            release.notify_one();
            tokio::time::timeout(std::time::Duration::from_secs(5), pending)
                .await
                .unwrap()
                .unwrap()
                .unwrap();

            if all {
                assert!(
                    events(&factory).is_empty(),
                    "saved author exclusion must prevent a run"
                );
                let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
                let consumed: i64 = db.query_row(
                    "SELECT count(*) FROM dedup WHERE event_id LIKE 'review:acme/repo#5:alice:%'",
                    [], |r| r.get(0),
                ).unwrap();
                assert_eq!(consumed, 0, "excluded review must not consume dedup");
                let mut q = db.prepare(
                    "SELECT json FROM observations WHERE key IN ('review:acme:alice','review-source:work:acme:alice')"
                ).unwrap();
                for row in q.query_map([], |r| r.get::<_, String>(0)).unwrap() {
                    let observation: Value = serde_json::from_str(&row.unwrap()).unwrap();
                    assert!(
                        observation.get("acme/repo#5").is_none(),
                        "excluded review must not be observed as present"
                    );
                }
            } else {
                assert_eq!(
                    events(&factory).len(),
                    1,
                    "requested reviews ignore the author list"
                );
            }

            // A later inclusion must still get its first generation, exactly once.
            let mut settings = factory.settings().unwrap();
            settings.source_connectors[0].pull_request_authors = vec!["CONTRIBUTOR".into()];
            factory.save_settings(settings).unwrap();
            factory.poll_now().await.unwrap();
            factory.poll_now().await.unwrap();
            let found = events(&factory);
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].event_id, "review:acme/repo#5:alice:1");
            assert_eq!(found[0].origin_token_profile_id.as_deref(), Some("work"));
            factory.shutdown().await;
        }
    }
}

#[tokio::test]
async fn pull_request_authors_polling_isolated_for_same_account_sources() {
    let (_dir, factory, _server, state) = fixture(true).await;
    let mut s = factory.settings().unwrap();
    s.source_connectors[0].pull_request_authors = vec!["contributor".into()];
    let mut b = s.source_connectors[0].clone();
    b.id = "other-source".into();
    b.label = "Other".into();
    b.token = Some("fixture-second".into());
    b.pull_request_authors = vec!["second".into()];
    s.source_connectors.push(b);
    factory.save_settings(s).unwrap();
    {
        let mut state = state.lock().unwrap();
        state.rows.push(row("acme/repo", 6));
        let mut pr = pull("acme/repo", 6);
        pr["user"]["login"] = json!("second");
        state.pulls.insert("/repos/acme/repo/pulls/6".into(), pr);
    }
    factory.poll_now().await.unwrap();
    factory.poll_now().await.unwrap();
    let mut origins: Vec<_> = events(&factory)
        .into_iter()
        .map(|e| (e.number, e.origin_token_profile_id.unwrap()))
        .collect();
    origins.sort();
    assert_eq!(
        origins,
        [(5, "work".to_string()), (6, "other-source".to_string())]
    );
    let mut s = factory.settings().unwrap();
    let before = s.source_connectors[1].pull_request_authors.clone();
    s.source_connectors[0].pull_request_authors.clear();
    let saved = factory.save_settings(s).unwrap();
    assert_eq!(saved.source_connectors[1].pull_request_authors, before);
    factory.shutdown().await;
}

#[tokio::test]
async fn pull_request_authors_do_not_filter_issue_or_release_polling() {
    for authors in [vec![], vec!["different".to_string()]] {
        let server = Server::start(|request| async move {
            if request.path == "/user" { return (200, json!({"login":"alice"})); }
            if request.path.starts_with("/search/issues?") {
                return (200, json!({"incomplete_results":false,"total_count":2,"items":[
                    {"number":80,"repository_url":"https://api.github.com/repos/acme/repo","title":"[AI] Work","user":{"login":"alice"}},
                    {"number":81,"repository_url":"https://api.github.com/repos/acme/repo","title":"[Release] Work","user":{"login":"alice"}}
                ]}));
            }
            for (number, title) in [(80, "[AI] Work"), (81, "[Release] Work")] {
                if request.path == format!("/repos/acme/repo/issues/{number}") {
                    return (200, json!({"number":number,"title":title,"body":"fixture",
                        "state":"open","user":{"login":"alice"},
                        "html_url":format!("https://github.com/acme/repo/issues/{number}")}));
                }
            }
            (500, json!({"unexpected":request.path}))
        }).await;
        let dir = tempfile::tempdir().unwrap();
        let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
        let mut s = factory.settings().unwrap();
        s.source_connectors = vec![SourceConnector {
            id: "work".into(),
            label: "Work".into(),
            account: "alice".into(),
            owners: vec!["acme".into()],
            authors: vec!["alice".into()],
            token: Some("fixture-token".into()),
            review_all_open_pull_requests: true,
            pull_request_authors: authors.clone(),
            ..Default::default()
        }];
        for flow in &mut s.flows {
            flow.enabled = ["new_issue", "release"].contains(&flow.id.as_str());
        }
        factory.save_settings(s).unwrap();
        factory.poll_now().await.unwrap();
        factory.poll_now().await.unwrap();
        let mut triggers: Vec<_> = factory
            .snapshot()
            .unwrap()
            .runs
            .into_iter()
            .map(|r| r.trigger_id)
            .collect();
        triggers.sort();
        assert_eq!(triggers, ["new_issue", "release"]);
        assert_eq!(
            factory.settings().unwrap().source_connectors[0].authors,
            ["alice"]
        );
        assert!(queries(&server).iter().all(|q| q.contains("author:alice")));
        factory.shutdown().await;
    }
}
