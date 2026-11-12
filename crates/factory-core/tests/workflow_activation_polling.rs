#[path = "common/activation_http.rs"]
mod activation_http;
mod common;
use activation_http::Server;
use factory_core::context::factory::infrastructure::repositories::store::Store;
use factory_core::*;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Notify;

fn settings() -> Settings {
    Settings {
        token_profiles: vec![
            TokenProfile {
                id: "work".into(),
                label: "Work".into(),
                account: "alice".into(),
                token: Some("fixture-a".into()),
                has_token: false,
                owners: vec!["acme".into()],
                authors: vec!["alice".into()],
                reviewers: vec!["alice".into()],
            },
            TokenProfile {
                id: "personal".into(),
                label: "Personal".into(),
                account: "bob".into(),
                token: Some("fixture-b".into()),
                has_token: false,
                owners: vec!["acme".into()],
                ..Default::default()
            },
        ],
        ..Settings::default()
    }
}
fn pr() -> Value {
    json!({"number":5,"title":"PR","body":"Closes #1","state":"open","html_url":"https://github.com/acme/repo/pull/5","base":{"repo":{"full_name":"acme/repo"}},"head":{"sha":"a".repeat(40),"ref":"feature"},"user":{"login":"alice"},"requested_reviewers":[{"login":"alice"}]})
}
fn search(present: bool, number: u64) -> Value {
    json!({"incomplete_results":false,"total_count":if present {1}else{0},"items":if present {vec![json!({"number":number,"repository_url":"https://api.github.com/repos/acme/repo"})]}else{vec![]}})
}
fn db(root: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(root.join("factory.sqlite3")).unwrap()
}
fn count(root: &std::path::Path, table: &str) -> i64 {
    db(root)
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
async fn poll(f: &Factory) {
    tokio::time::timeout(Duration::from_secs(5), f.poll_now())
        .await
        .unwrap()
        .unwrap();
}

// All final admission paths are exercised with an HTTP response in flight while save commits first.
#[tokio::test]
async fn issue_approval_and_review_save_races_do_not_consume_events() {
    for trigger in ["new_issue", "pr_approved", "pr_review"] {
        for global in [false, true] {
            for approved_signal in if trigger == "pr_approved" {
                vec![false, true]
            } else {
                vec![false]
            } {
                let reached = Arc::new(Notify::new());
                let release = Arc::new(Notify::new());
                let blocked = Arc::new(AtomicBool::new(false));
                let server = Server::start({
                    let reached = reached.clone(); let release = release.clone(); let blocked = blocked.clone();
                    move |r| {
                        let reached = reached.clone(); let release = release.clone(); let blocked = blocked.clone();
                        async move {
                            let gate = match trigger { "new_issue" => r.path == "/repos/acme/repo/issues/1", "pr_review" => r.path == "/repos/acme/repo/pulls/5", _ => r.path.contains("/comments?") };
                            if gate && !blocked.swap(true,Ordering::SeqCst) { reached.notify_one(); release.notified().await; }
                            if r.method != "GET" { return (500,json!({})); }
                            if r.path == "/user" { return (200,json!({"login":"alice"})); }
                            if r.path.starts_with("/search/") { return (200, search(true, if trigger == "new_issue" {1}else{5})); }
                            if r.path == "/repos/acme/repo/issues/1" { return (200,json!({"state":"open","title":"[AI] fixture","user":{"login":"alice"},"html_url":"https://github.com/acme/repo/issues/1"})); }
                            if r.path == "/repos/acme/repo/pulls/5" { return (200,pr()); }
                            if r.path.contains("/reviews?") { return (200,if approved_signal { json!([{"id":1,"state":"APPROVED","commit_id":"a".repeat(40),"user":{"login":"bob"}}]) } else {json!([])}); }
                            if r.path.contains("/comments?") { return (200,if approved_signal {json!([])}else{json!([{"id":7,"body":"OK","user":{"login":"alice"}}])}); }
                            (500,json!({"unexpected":r.path}))
                        }
                    }
                }).await;
                let dir = tempfile::tempdir().unwrap();
                let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
                let mut s = settings();
                if trigger != "new_issue" {
                    s.token_profiles[0].authors.clear();
                }
                if trigger != "pr_review" {
                    s.token_profiles[0].reviewers.clear();
                }
                f.save_settings(s).unwrap();
                if trigger == "pr_approved" {
                    let source = f
                        .enqueue(TriggerEvent::issue(
                            "acme/repo",
                            1,
                            "[AI] source",
                            "alice",
                            "",
                        ))
                        .unwrap()
                        .unwrap();
                    Store::open(dir.path())
                        .unwrap()
                        .track_pr("acme/repo", 5, &source)
                        .unwrap();
                    let mut s = f.settings().unwrap();
                    s.token_profiles[0].authors = vec!["alice".into()];
                    s.flows[0].enabled = false;
                    // Bob's preference must not replace the source author's preference.
                    s.flows[1]
                        .profile_activation
                        .insert("personal".into(), false);
                    f.save_settings(s).unwrap();
                }
                let baseline = count(dir.path(), "runs");
                let baseline_dedup = count(dir.path(), "dedup");
                let task = {
                    let f = f.clone();
                    tokio::spawn(async move { f.poll_now().await })
                };
                tokio::time::timeout(Duration::from_secs(5), reached.notified())
                    .await
                    .unwrap();
                let index = if trigger == "new_issue" {
                    0
                } else if trigger == "pr_approved" {
                    1
                } else {
                    2
                };
                let mut s = f.settings().unwrap();
                if global {
                    s.flows[index].enabled = false;
                } else {
                    s.flows[index]
                        .profile_activation
                        .insert("work".into(), false);
                }
                f.save_settings(s).unwrap();
                release.notify_one();
                tokio::time::timeout(Duration::from_secs(5), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert_eq!(count(dir.path(), "runs"), baseline, "{trigger}");
                assert_eq!(count(dir.path(), "dedup"), baseline_dedup, "{trigger}");
                assert_eq!(count(dir.path(), "observations"), 0, "{trigger}");
                let calls = server.calls.lock().unwrap().len();
                poll(&f).await;
                assert_eq!(
                    server.calls.lock().unwrap().len(),
                    calls,
                    "disabled preflight must be silent"
                );
                let mut s = f.settings().unwrap();
                s.flows[index].enabled = true;
                s.flows[index].profile_activation.remove("work");
                f.save_settings(s).unwrap();
                poll(&f).await;
                poll(&f).await;
                assert_eq!(count(dir.path(), "runs"), baseline + 1);
                drop(f);
                let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
                poll(&f).await;
                assert_eq!(count(dir.path(), "runs"), baseline + 1);
                assert!(server
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|r| r.method == "GET" && r.authorization == "Bearer fixture-a"));
            }
        }
    }
}

#[tokio::test]
async fn disabled_pairs_skip_keychain_and_http_for_each_trigger() {
    for trigger in ["new_issue", "pr_approved", "pr_review"] {
        let server = Server::start(move |r| async move {
            if r.method != "GET" || r.authorization != "Bearer fixture-b" { return (500,json!({})); }
            if r.path == "/user" { return (200,json!({"login":"bob"})); }
            if r.path.starts_with("/search/") { return (200,search(true,if trigger=="new_issue" {2}else{5})); }
            if r.path == "/repos/acme/repo/issues/2" { return (200,json!({"state":"open","title":"[AI] Bob","user":{"login":"bob"},"html_url":"https://github.com/acme/repo/issues/2"})); }
            if r.path == "/repos/acme/repo/pulls/5" { let mut p=pr(); p["user"]["login"]=json!("bob"); p["requested_reviewers"]=json!([{"login":"bob"}]); return (200,p); }
            if r.path.contains("/reviews?") { return (200,json!([{"state":"APPROVED","commit_id":"a".repeat(40),"user":{"login":"alice"}}])); }
            if r.path.contains("/comments?") { return (200,json!([])); }
            (500,json!({"unexpected":r.path}))
        }).await;
        let dir = tempfile::tempdir().unwrap();
        let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
        let mut s = settings();
        for profile in &mut s.token_profiles {
            profile.authors = if trigger == "new_issue" {
                vec![profile.account.clone()]
            } else {
                vec![]
            };
            profile.reviewers = if trigger == "pr_review" {
                vec![profile.account.clone()]
            } else {
                vec![]
            };
        }
        for flow in &mut s.flows {
            flow.profile_activation.insert("work".into(), false);
        }
        f.save_settings(s).unwrap();
        let mut baseline = 0;
        if trigger == "pr_approved" {
            // Establish sources with exclusions temporarily removed, then disable only Alice.
            let mut s = f.settings().unwrap();
            s.flows[0].profile_activation.clear();
            f.save_settings(s).unwrap();
            for (n, actor) in [(1, "alice"), (2, "bob")] {
                let id = f
                    .enqueue(TriggerEvent::issue(
                        "acme/repo",
                        n,
                        "[AI] source",
                        actor,
                        "",
                    ))
                    .unwrap()
                    .unwrap();
                Store::open(dir.path())
                    .unwrap()
                    .track_pr("acme/repo", if actor == "alice" { 6 } else { 5 }, &id)
                    .unwrap();
            }
            let mut s = f.settings().unwrap();
            for profile in &mut s.token_profiles {
                profile.authors = vec![profile.account.clone()];
            }
            s.flows[0].enabled = false;
            f.save_settings(s).unwrap();
            baseline = 2;
        }
        drop(f);
        let credentials = common::backend(dir.path());
        let alice_ref: String = db(dir.path())
            .query_row(
                "SELECT reference FROM credential_refs WHERE profile_id='work'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        credentials.values.lock().unwrap().remove(&alice_ref);
        let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
        credentials.gets.lock().unwrap().clear();
        poll(&f).await;
        assert_eq!(count(dir.path(), "runs"), baseline + 1, "{trigger}");
        assert_eq!(
            credentials
                .gets
                .lock()
                .unwrap()
                .get(&alice_ref)
                .copied()
                .unwrap_or(0),
            0
        );
        assert!(!server.calls.lock().unwrap().is_empty());
        assert!(server
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.method == "GET" && r.authorization == "Bearer fixture-b"));
    }
}

#[tokio::test]
async fn review_withdrawal_race_and_unobserved_edges_preserve_generation() {
    let mode = Arc::new(AtomicU8::new(0));
    let reached = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let server = Server::start({
        let mode = mode.clone();
        let reached = reached.clone();
        let release = release.clone();
        move |r| {
            let mode = mode.clone();
            let reached = reached.clone();
            let release = release.clone();
            async move {
                if r.path == "/user" {
                    return (200, json!({"login":"alice"}));
                }
                if r.path.starts_with("/search/") {
                    let m = mode.load(Ordering::SeqCst);
                    if m == 2 {
                        reached.notify_one();
                        release.notified().await;
                    }
                    if m == 3 {
                        return (503, json!({}));
                    }
                    return (200, search(m == 0 || m == 4, 5));
                }
                if r.path == "/repos/acme/repo/pulls/5" {
                    if mode.load(Ordering::SeqCst) == 4 {
                        return (503, json!({}));
                    }
                    let mut p = pr();
                    p["user"]["login"] = json!("bob");
                    return (200, p);
                }
                (500, json!({}))
            }
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut s = settings();
    s.token_profiles[0].authors.clear();
    s.flows[2]
        .profile_activation
        .insert("personal".into(), false);
    f.save_settings(s).unwrap();
    poll(&f).await;
    let before: String = db(dir.path())
        .query_row(
            "SELECT json FROM observations WHERE key='review:acme:alice'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    mode.store(2, Ordering::SeqCst);
    let task = {
        let f = f.clone();
        tokio::spawn(async move { f.poll_now().await })
    };
    tokio::time::timeout(Duration::from_secs(5), reached.notified())
        .await
        .unwrap();
    let mut s = f.settings().unwrap();
    s.flows[2].enabled = false;
    f.save_settings(s).unwrap();
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        db(dir.path())
            .query_row::<String, _, _>(
                "SELECT json FROM observations WHERE key='review:acme:alice'",
                [],
                |r| r.get(0)
            )
            .unwrap(),
        before
    );
    mode.store(1, Ordering::SeqCst);
    poll(&f).await;
    mode.store(0, Ordering::SeqCst);
    poll(&f).await;
    let mut s = f.settings().unwrap();
    s.flows[2].enabled = true;
    f.save_settings(s).unwrap();
    poll(&f).await;
    assert_eq!(count(dir.path(), "runs"), 1);
    for failure in [3, 4] {
        mode.store(failure, Ordering::SeqCst);
        assert!(f.poll_now().await.is_err());
        assert_eq!(
            db(dir.path())
                .query_row::<String, _, _>(
                    "SELECT json FROM observations WHERE key='review:acme:alice'",
                    [],
                    |r| r.get(0)
                )
                .unwrap(),
            before
        );
    }
    mode.store(1, Ordering::SeqCst);
    poll(&f).await;
    mode.store(0, Ordering::SeqCst);
    poll(&f).await;
    assert_eq!(count(dir.path(), "runs"), 2);
    assert!(server
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|r| r.method == "GET"));
}

#[tokio::test]
async fn periodic_poll_and_sync_now_skip_all_disabled_pairs_without_secrets() {
    let server = Server::start(|_| async { (500, json!({})) }).await;
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut s = settings();
    s.poll_interval_seconds = 1;
    for flow in &mut s.flows {
        flow.profile_activation.insert("work".into(), false);
    }
    f.save_settings(s).unwrap();
    drop(f);
    let credentials = common::backend(dir.path());
    credentials.fail_get.store(true, Ordering::SeqCst);
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    credentials.gets.lock().unwrap().clear();
    let mut events = f.subscribe();
    f.start();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            events.recv().await.unwrap();
            let service = f.snapshot().unwrap().service;
            if service.last_poll_at.is_some() && !service.polling {
                break;
            }
        }
    })
    .await
    .unwrap();
    f.shutdown().await;
    poll(&f).await;
    assert!(server.calls.lock().unwrap().is_empty());
    assert!(credentials.gets.lock().unwrap().is_empty());
    assert_eq!(count(dir.path(), "runs"), 0);
}

#[tokio::test]
async fn active_other_trigger_may_use_same_token() {
    let server = Server::start(|r| async move {
        if r.path == "/user" {
            return (200, json!({"login":"alice"}));
        }
        if r.path.starts_with("/search/") && r.path.contains("review-requested") {
            return (200, search(true, 5));
        }
        if r.path == "/repos/acme/repo/pulls/5" {
            return (200, pr());
        }
        (500, json!({}))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut s = settings();
    s.flows[0].profile_activation.insert("work".into(), false);
    f.save_settings(s).unwrap();
    poll(&f).await;
    let runs = f.snapshot().unwrap().runs;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].trigger_id, "pr_review");
    assert!(server
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|r| r.method == "GET" && r.authorization == "Bearer fixture-a"));
}
