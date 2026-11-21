#[path = "common/activation_http.rs"]
mod activation_http;
mod common;
use activation_http::Server;
use factory_core::*;
use serde_json::json;

#[tokio::test]
async fn release_poll_is_durable_and_keeps_profile_and_frozen_flow() {
    let server = Server::start(|r| async move {
        if r.method != "GET" {
            return (500, json!({}));
        }
        if r.path == "/user" {
            return (200, json!({"login":"alice"}));
        }
        if r.path.starts_with("/search/") {
            return (
                200,
                json!({"incomplete_results":false,"total_count":1,"items":[{
                    "number":80,"repository_url":"https://api.github.com/repos/acme/repo",
                    "title":"[RELEASE] 2.0.0","user":{"login":"alice"}
                }]}),
            );
        }
        if r.path == "/repos/acme/repo/issues/80" {
            return (
                200,
                json!({"number":80,"state":"open","title":"[RELEASE] 2.0.0",
                "body":"Publish app","user":{"login":"alice"},
                "html_url":"https://github.com/acme/repo/issues/80"}),
            );
        }
        (500, json!({"unexpected":r.path}))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut s = f.settings().unwrap();
    s.source_connectors = vec![SourceConnector {
        reviewers: vec!["alice".into()],
        kind: factory_core::SourceKind::GithubIssues,
        id: "work".into(),
        label: "Work".into(),
        account: "alice".into(),
        owners: vec!["acme".into()],
        authors: vec!["alice".into()],
        token: Some("fixture-a".into()),
        ..Default::default()
    }];
    for flow in &mut s.flows {
        flow.enabled = flow.id == "release";
    }
    let legacy =
        include_str!("../src/context/factory/domain/entities/models/release_prompt_legacy.txt");
    let default = Settings::default()
        .flows
        .into_iter()
        .find(|flow| flow.id == "release")
        .unwrap()
        .nodes
        .remove(0)
        .prompt;
    f.save_settings(s).unwrap();
    let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
    let read_settings = || {
        db.query_row("SELECT json FROM settings WHERE id=1", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap()
    };
    let mut stored: serde_json::Value = serde_json::from_str(&read_settings()).unwrap();
    let release = stored["flows"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|flow| flow["id"] == "release")
        .unwrap();
    release["nodes"][0]["prompt"] = json!(legacy);
    release["nodes"][0]["skills"] = json!(["fixture-publish"]);
    db.execute(
        "UPDATE settings SET json=?1 WHERE id=1",
        [stored.to_string()],
    )
    .unwrap();
    f.poll_now().await.unwrap();
    f.poll_now().await.unwrap();
    let runs = f.snapshot().unwrap().runs;
    assert_eq!(runs.len(), 1);
    let d = f.run_detail(&runs[0].id).unwrap();
    assert_eq!(d.event.trigger_id, "release");
    assert_eq!(d.event.event_id, "release:acme/repo#80");
    assert_eq!(d.event.origin_token_profile_id.as_deref(), Some("work"));
    assert_eq!(
        serde_json::to_value(&d.run).unwrap()["origin_token_profile_id"],
        "work"
    );
    assert!(d.flow.nodes[0]
        .prompt
        .contains("a suitable installed publishing skill available to the agent"));
    assert_eq!(d.flow.nodes[0].prompt, default);
    let id = &runs[0].id;
    let snapshot_bytes = || {
        db.query_row("SELECT flow FROM runs WHERE id=?1", [id], |row| {
            row.get::<_, String>(0)
        })
        .unwrap()
    };
    let mut snapshot: serde_json::Value = serde_json::from_str(&snapshot_bytes()).unwrap();
    snapshot["nodes"][0]["prompt"] = json!(legacy);
    snapshot["nodes"][0]["skills"] = json!([]);
    let frozen = snapshot.to_string();
    db.execute("UPDATE runs SET flow=?1 WHERE id=?2", [&frozen, id])
        .unwrap();
    let assert_history = |factory: &Factory| {
        let flow = factory.run_detail(id).unwrap().flow;
        assert_eq!(flow.nodes[0].prompt.as_bytes(), legacy.as_bytes());
        assert!(serde_json::to_value(flow).unwrap()["nodes"][0]
            .get("skills")
            .is_none());
        assert_eq!(snapshot_bytes(), frozen);
    };
    assert_history(&f);
    drop(f);
    let reopened = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut changed = reopened.settings().unwrap();
    changed
        .flows
        .iter_mut()
        .find(|flow| flow.id == "release")
        .unwrap()
        .nodes[0]
        .prompt = "another custom prompt".into();
    reopened.save_settings(changed).unwrap();
    assert_history(&reopened);
    reopened.poll_now().await.unwrap();
    assert_eq!(reopened.snapshot().unwrap().runs.len(), 1);
    assert_eq!(
        server
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == "/repos/acme/repo/issues/80")
            .count(),
        1
    );
    assert!(server
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|r| r.method == "GET" && r.authorization == "Bearer fixture-a"));
}

fn release_settings() -> Settings {
    let mut s = Settings {
        source_connectors: vec![SourceConnector {
            reviewers: vec!["alice".into()],
            kind: factory_core::SourceKind::GithubIssues,
            id: "work".into(),
            label: "Work".into(),
            account: "alice".into(),
            owners: vec!["acme".into()],
            authors: vec!["alice".into()],
            token: Some("fixture-a".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    for flow in &mut s.flows {
        flow.enabled = flow.id == "release";
    }
    s
}
fn row(title: &str) -> serde_json::Value {
    json!({"number":80,"repository_url":"https://api.github.com/repos/acme/repo","title":title,"user":{"login":"alice"}})
}
fn detail(title: &str) -> serde_json::Value {
    json!({"number":80,"state":"open","title":title,"body":"Publish app","user":{"login":"alice"},"html_url":"https://github.com/acme/repo/issues/80"})
}

#[tokio::test]
async fn release_requires_exact_prefix_fresh_identity_state_and_author() {
    for (title, field, value, want, fetch, error) in [
        ("[RELEASE]", "title", json!("[RELEASE]"), 1, true, false),
        ("[RELEASE] 2", "title", json!("[RELEASE] 2"), 1, true, false),
        ("x [RELEASE]", "title", json!("[RELEASE]"), 0, false, false),
        ("[release]", "title", json!("[RELEASE]"), 0, false, false),
        (" [RELEASE]", "title", json!("[RELEASE]"), 1, true, false),
        ("[Release]", "title", json!("[RELEASE]"), 0, false, false),
        ("[RELEASE", "title", json!("[RELEASE]"), 0, false, false),
        ("[RELEASE]", "title", json!("normal"), 0, true, false),
        ("[RELEASE]", "title", json!("[AI] changed"), 0, true, false),
        ("[RELEASE]", "state", json!("closed"), 0, true, false),
        ("[RELEASE]", "pull_request", json!({}), 0, true, false),
        ("[RELEASE]", "user", json!({"login":"bob"}), 0, true, false),
        (
            "[RELEASE]",
            "html_url",
            json!("https://github.com/acme/other/issues/80"),
            0,
            true,
            true,
        ),
    ] {
        let mut fresh = detail(title);
        fresh[field] = value;
        let server = Server::start(move |r| {
            let fresh = fresh.clone();
            async move {
                if r.path == "/user" {
                    return (200, json!({"login":"alice"}));
                }
                if r.path.starts_with("/search/") {
                    return (
                        200,
                        json!({"total_count":1,"incomplete_results":false,"items":[row(title)]}),
                    );
                }
                if r.path == "/repos/acme/repo/issues/80" {
                    return (200, fresh);
                }
                (500, json!({}))
            }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
        let mut s = release_settings();
        s.flows[0].enabled = true;
        f.save_settings(s).unwrap();
        assert_eq!(f.poll_now().await.is_err(), error, "{title} {field}");
        assert_eq!(f.snapshot().unwrap().runs.len(), want, "{title} {field}");
        assert_eq!(
            server
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.path.ends_with("/issues/80")),
            fetch,
            "{title} {field}"
        );
        let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM dedup", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            want as i64
        );
    }
}

#[tokio::test]
async fn disabled_or_unauthorized_release_never_reads_credentials_or_http() {
    for mode in ["global", "profile", "author", "owner"] {
        let server = Server::start(|_| async { (500, json!({})) }).await;
        let dir = tempfile::tempdir().unwrap();
        let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
        let mut s = release_settings();
        match mode {
            "global" => {
                s.flows
                    .iter_mut()
                    .find(|f| f.id == "release")
                    .unwrap()
                    .enabled = false
            }
            "profile" => {
                s.flows
                    .iter_mut()
                    .find(|f| f.id == "release")
                    .unwrap()
                    .profile_activation
                    .insert("work".into(), false);
            }
            "author" => s.source_connectors[0].authors.clear(),
            _ => s.source_connectors[0].owners.clear(),
        }
        f.save_settings(s).unwrap();
        let credentials = common::backend(dir.path());
        credentials.gets.lock().unwrap().clear();
        f.poll_now().await.unwrap();
        assert!(server.calls.lock().unwrap().is_empty(), "{mode}");
        assert!(credentials.gets.lock().unwrap().is_empty(), "{mode}");
    }
}

#[tokio::test]
async fn activation_changes_during_release_detail_do_not_consume_event() {
    use std::sync::Arc;
    use tokio::sync::Notify;
    for mode in ["global", "profile", "removed"] {
        let reached = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let server=Server::start({let reached=reached.clone();let resume=resume.clone();move|r|{let reached=reached.clone();let resume=resume.clone();async move {
            if r.path=="/user" {return (200,json!({"login":"alice"}));}
            if r.path.starts_with("/search/") {return (200,json!({"incomplete_results":false,"total_count":1,"items":[row("[RELEASE]")]}));}
            if r.path.ends_with("/issues/80") {reached.notify_one();resume.notified().await;return (200,detail("[RELEASE]"));}
            (500,json!({}))
        }}}).await;
        let dir = tempfile::tempdir().unwrap();
        let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
        let original = release_settings();
        f.save_settings(original.clone()).unwrap();
        let polling = {
            let f = f.clone();
            tokio::spawn(async move { f.poll_now().await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), reached.notified())
            .await
            .unwrap();
        let mut changed = f.settings().unwrap();
        match mode {
            "global" => {
                changed
                    .flows
                    .iter_mut()
                    .find(|f| f.id == "release")
                    .unwrap()
                    .enabled = false
            }
            "profile" => {
                changed
                    .flows
                    .iter_mut()
                    .find(|f| f.id == "release")
                    .unwrap()
                    .profile_activation
                    .insert("work".into(), false);
            }
            _ => changed.source_connectors.clear(),
        }
        f.save_settings(changed).unwrap();
        resume.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(5), polling)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(f.snapshot().unwrap().runs.is_empty());
        f.save_settings(original).unwrap();
        resume.notify_one();
        f.poll_now().await.unwrap();
        assert_eq!(f.snapshot().unwrap().runs.len(), 1);
    }
}

#[tokio::test]
async fn shared_scan_and_distinct_issue_keys_preserve_frozen_release_configuration() {
    let title = std::sync::Arc::new(std::sync::Mutex::new("[AI]"));
    let server = Server::start({
        let title = title.clone();
        move |r| {
            let title = *title.lock().unwrap();
            async move {
                if r.path == "/user" {
                    return (200, json!({"login":"alice"}));
                }
                if r.path.starts_with("/search/") {
                    return (
                        200,
                        json!({"incomplete_results":false,"total_count":1,"items":[row(title)]}),
                    );
                }
                if r.path.ends_with("/issues/80") {
                    return (200, detail(title));
                }
                (500, json!({}))
            }
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut s = release_settings();
    s.flows[0].enabled = true;
    let node = &mut s
        .flows
        .iter_mut()
        .find(|f| f.id == "release")
        .unwrap()
        .nodes[0];
    node.token_overrides.insert(
        "work".into(),
        AgentConfig {
            agent: "opencode".into(),
            model: "fixture".into(),
            effort: "low".into(),
        },
    );
    f.save_settings(s).unwrap();
    f.poll_now().await.unwrap();
    *title.lock().unwrap() = "[RELEASE]";
    f.poll_now().await.unwrap();
    f.poll_now().await.unwrap();
    let runs = f.snapshot().unwrap().runs;
    assert_eq!(runs.len(), 2);
    let id = &runs.iter().find(|r| r.trigger_id == "release").unwrap().id;
    let frozen = f.run_detail(id).unwrap().flow;
    let mut s = f.settings().unwrap();
    s.flows
        .iter_mut()
        .find(|f| f.id == "release")
        .unwrap()
        .nodes[0]
        .prompt = "changed".into();
    f.save_settings(s).unwrap();
    assert_eq!(
        serde_json::to_value(f.run_detail(id).unwrap().flow).unwrap(),
        serde_json::to_value(&frozen).unwrap()
    );
    assert_eq!(frozen.nodes[0].agent, "opencode");
    assert_eq!(frozen.nodes[0].model, "fixture");
    let calls = server.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.path.starts_with("/search/"))
            .count(),
        3
    );
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.path.ends_with("/issues/80"))
            .count(),
        2
    );
}

#[tokio::test]
async fn release_rejects_ineligible_search_rows_before_fetching_detail() {
    for mode in ["closed", "pr", "author", "owner"] {
        let mut search = row("[RELEASE]");
        match mode {
            "closed" => search["state"] = json!("closed"),
            "pr" => search["pull_request"] = json!({}),
            "author" => search["user"]["login"] = json!("bob"),
            _ => search["repository_url"] = json!("https://api.github.com/repos/other/repo"),
        }
        let server = Server::start(move |r| {
            let search = search.clone();
            async move {
                if r.path == "/user" {
                    return (200, json!({"login":"alice"}));
                }
                if r.path.starts_with("/search/") {
                    return (
                        200,
                        json!({"incomplete_results":false,"total_count":1,"items":[search]}),
                    );
                }
                (500, json!({}))
            }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
        f.save_settings(release_settings()).unwrap();
        let result = f.poll_now().await;
        if mode == "owner" {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("outside token scope"));
        } else {
            result.unwrap();
        }
        assert!(f.snapshot().unwrap().runs.is_empty());
        assert!(!server
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.path.contains("/issues/80")));
    }
}

#[tokio::test]
async fn release_and_ai_share_one_scan_per_profile_without_crossing_credentials() {
    let server=Server::start(|r|async move{
        let actor=if r.authorization=="Bearer fixture-a" {"alice"}else if r.authorization=="Bearer fixture-b" {"bob"}else{panic!("unexpected credentials")};
        if r.path=="/user" {return (200,json!({"login":actor}));}
        if r.path.starts_with("/search/") {
            let rows:Vec<_>=[(81,"[AI] task"),(82,"[RELEASE] app")].into_iter().map(|(n,title)|json!({"number":n,"repository_url":format!("https://api.github.com/repos/{actor}/repo"),"title":title,"user":{"login":actor}})).collect();
            return (200,json!({"incomplete_results":false,"total_count":2,"items":rows}));
        }
        if r.path==format!("/repos/{actor}/repo/issues/81") || r.path==format!("/repos/{actor}/repo/issues/82") {
            let n=if r.path.ends_with("81") {81}else{82};let title=if n==81 {"[AI] task"}else{"[RELEASE] app"};
            return (200,json!({"state":"open","title":title,"user":{"login":actor},"html_url":format!("https://github.com/{actor}/repo/issues/{n}")}));
        }
        (500,json!({}))
    }).await;
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut s = release_settings();
    s.flows[0].enabled = true;
    s.source_connectors[0].owners = vec!["alice".into()];
    s.source_connectors.push(SourceConnector {
        reviewers: vec!["bob".into()],
        kind: factory_core::SourceKind::GithubIssues,
        id: "personal".into(),
        label: "Personal".into(),
        account: "bob".into(),
        owners: vec!["bob".into()],
        authors: vec!["bob".into()],
        token: Some("fixture-b".into()),
        ..Default::default()
    });
    s.flows
        .iter_mut()
        .find(|f| f.id == "release")
        .unwrap()
        .profile_activation
        .insert("personal".into(), false);
    f.save_settings(s).unwrap();
    f.poll_now().await.unwrap();
    let runs = f.snapshot().unwrap().runs;
    assert_eq!(runs.len(), 3);
    for r in runs {
        assert_eq!(
            r.origin_token_profile_id.as_deref(),
            Some(if r.actor == "alice" {
                "work"
            } else {
                "personal"
            })
        );
        if r.trigger_id == "release" {
            assert_eq!(r.actor, "alice");
        }
    }
    let calls = server.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.path.starts_with("/search/"))
            .count(),
        2
    );
    assert_eq!(
        calls.iter().filter(|r| r.path.contains("/issues/")).count(),
        3
    );
}

#[tokio::test]
async fn release_prefix_polling_uses_configured_literal_and_fresh_title() {
    for (prefix, searched, fresh, expected, fetched) in [
        ("ship:", "ship:candidate", "ship:fresh", 1, true),
        (
            " ship: ",
            " \tship:candidate",
            "\u{2003}ship:fresh\n",
            1,
            true,
        ),
        ("ship:", "ship:candidate", "Ship:fresh", 0, true),
        ("ship:", "ship:candidate", "note ship:fresh", 0, true),
        ("ship:", "ship:candidate", "[AI] task", 0, true),
        ("ship:", "[AI] candidate", "ship:fresh", 0, false),
        ("[RELEASE]", "[RELEASE]", "[RELEASE]", 1, true),
        ("[RELEASE]", "[Release]", "[Release]", 0, false),
        ("[.*]", "[.*]candidate", "[.*]fresh", 1, true),
        (
            "Team Release:",
            "Team  Release:2",
            "Team  Release:2",
            0,
            false,
        ),
    ] {
        let server = Server::start(move |r| async move {
            assert_eq!(r.method, "GET");
            assert_eq!(r.authorization, "Bearer fixture-a");
            if r.path == "/user" {
                return (200, json!({"login":"alice"}));
            }
            if r.path.starts_with("/search/") {
                return (
                    200,
                    json!({"incomplete_results":false,"total_count":1,"items":[row(searched)]}),
                );
            }
            if r.path == "/repos/acme/repo/issues/80" {
                return (200, detail(fresh));
            }
            panic!("unexpected {}", r.path);
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
        let mut s = release_settings();
        s.source_connectors[0].issue_title_prefix = "task:".into();
        s.source_connectors[0].release_issue_title_prefix = prefix.into();
        factory.save_settings(s).unwrap();
        factory.poll_now().await.unwrap();
        factory.poll_now().await.unwrap();
        let runs = factory.snapshot().unwrap().runs;
        assert_eq!(runs.len(), expected, "{prefix:?} {searched:?} {fresh:?}");
        assert_eq!(
            server
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.path.ends_with("/issues/80")),
            fetched
        );
        let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM dedup", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            expected as i64
        );
        if expected == 1 {
            let event = factory.run_detail(&runs[0].id).unwrap().event;
            assert_eq!(event.trigger_id, "release");
            assert_eq!(event.event_id, "release:acme/repo#80");
            assert_eq!(event.title, fresh);
            assert_eq!(event.payload["issue"]["title"], fresh);
            assert_eq!(event.origin_token_profile_id.as_deref(), Some("work"));
        }
    }
}

#[tokio::test]
async fn release_prefix_polling_reads_settings_after_detail_await() {
    use std::{sync::Arc, time::Duration};
    use tokio::sync::Notify;
    let reached = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let server = Server::start({
        let reached = reached.clone();
        let resume = resume.clone();
        move |r| {
            let reached = reached.clone();
            let resume = resume.clone();
            async move {
                if r.path == "/user" {
                    return (200, json!({"login":"alice"}));
                }
                if r.path.starts_with("/search/") {
                    return (
                        200,
                        json!({"incomplete_results":false,"total_count":1,"items":[row("ship:2")]}),
                    );
                }
                if r.path.ends_with("/issues/80") {
                    reached.notify_one();
                    resume.notified().await;
                    return (200, detail("ship:2"));
                }
                panic!("unexpected {}", r.path);
            }
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut original = release_settings();
    original.source_connectors[0].release_issue_title_prefix = "ship:".into();
    factory.save_settings(original.clone()).unwrap();
    let polling = {
        let factory = factory.clone();
        tokio::spawn(async move { factory.poll_now().await })
    };
    tokio::time::timeout(Duration::from_secs(5), reached.notified())
        .await
        .unwrap();
    let mut changed = factory.settings().unwrap();
    changed.source_connectors[0].release_issue_title_prefix = "deploy:".into();
    factory.save_settings(changed).unwrap();
    resume.notify_one();
    tokio::time::timeout(Duration::from_secs(5), polling)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(factory.snapshot().unwrap().runs.is_empty());
    let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM dedup", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    factory.save_settings(original).unwrap();
    resume.notify_one();
    factory.poll_now().await.unwrap();
    assert_eq!(factory.snapshot().unwrap().runs.len(), 1);
}
