#[path = "common/activation_http.rs"]
mod activation_http;
mod common;

use activation_http::Server;
use factory_core::*;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;

fn settings(prefix: &str) -> Settings {
    let mut settings = common::configured_settings();
    settings.source_connectors[0].owners = vec!["acme".into()];
    settings.source_connectors[0].reviewers.clear();
    settings.source_connectors[0].issue_title_prefix = prefix.into();
    for flow in &mut settings.flows {
        flow.enabled = flow.id == "new_issue";
    }
    settings
}

fn search_row(title: &str) -> Value {
    json!({"number":97,"title":title,"state":"open",
           "repository_url":"https://api.github.com/repos/acme/repo",
           "user":{"login":"alice"}})
}

fn issue_detail(title: &str) -> Value {
    json!({"number":97,"title":title,"state":"open","body":"fixture",
           "html_url":"https://github.com/acme/repo/issues/97",
           "user":{"login":"alice"}})
}

#[tokio::test]
async fn same_account_sources_match_their_own_issue_and_release_prefixes() {
    const TITLES: [&str; 6] = [
        "Task A: issue",
        "Ship A: release",
        "Task B: issue",
        "Ship B: release",
        "[AI] unused",
        "[RELEASE] unused",
    ];
    let server = Server::start(|request| async move {
        if request.path == "/user" {
            return (200, json!({"login":"alice"}));
        }
        if request.path.starts_with("/search/issues?") {
            let items: Vec<_> = TITLES
                .iter()
                .enumerate()
                .map(|(idx, title)| {
                    let mut row = search_row(title);
                    row["number"] = json!(idx + 1);
                    row
                })
                .collect();
            return (
                200,
                json!({"total_count":items.len(),"incomplete_results":false,"items":items}),
            );
        }
        let number: usize = request
            .path
            .strip_prefix("/repos/acme/repo/issues/")
            .expect("issue detail request")
            .parse()
            .unwrap();
        assert_eq!(
            request.authorization,
            match number {
                1 | 2 => "Bearer fixture-source-only",
                3 | 4 => "Bearer fixture-second",
                _ => panic!("Selected a title that matches neither source"),
            }
        );
        let mut detail = issue_detail(TITLES[number - 1]);
        detail["number"] = json!(number);
        detail["html_url"] = json!(format!("https://github.com/acme/repo/issues/{number}"));
        (200, detail)
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut configured = settings("Task A:");
    configured.source_connectors[0].release_issue_title_prefix = "Ship A:".into();
    configured.source_connectors.push(SourceConnector {
        id: "second".into(),
        label: "Second source".into(),
        issue_title_prefix: "Task B:".into(),
        release_issue_title_prefix: "Ship B:".into(),
        token: Some("fixture-second".into()),
        ..configured.source_connectors[0].clone()
    });
    configured
        .flows
        .iter_mut()
        .find(|flow| flow.id == "release")
        .unwrap()
        .enabled = true;
    factory.save_settings(configured).unwrap();
    factory.poll_now().await.unwrap();
    let mut runs = factory.snapshot().unwrap().runs;
    runs.sort_by_key(|run| run.number);
    let actual: Vec<_> = runs
        .iter()
        .map(|run| {
            (
                run.number,
                run.trigger_id.as_str(),
                run.origin_token_profile_id.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        actual,
        vec![
            (1, "new_issue", Some("alice")),
            (2, "release", Some("alice")),
            (3, "new_issue", Some("second")),
            (4, "release", Some("second"))
        ]
    );
}

#[tokio::test]
async fn issue_prefix_polling_checks_custom_prefix_and_fresh_title() {
    for (search_title, fresh_title, expected) in [
        ("bot: candidate", "bot: fresh", 1),
        (" \tbot: candidate ", "\tbot:fresh\n", 1),
        ("bot: candidate", "[AI] stale", 0),
        ("bot: candidate", "Bot: wrong case", 0),
        ("bot: candidate", "note bot: interior", 0),
        ("normal", "normal", 0),
    ] {
        let server = Server::start(move |r| async move {
            assert_eq!(r.method, "GET");
            assert_eq!(r.authorization, "Bearer fixture-source-only");
            if r.path == "/user" {
                return (200, json!({"login":"alice"}));
            }
            if r.path.starts_with("/search/") {
                return (
                    200,
                    json!({"total_count":1,"incomplete_results":false,
                                   "items":[search_row(search_title)]}),
                );
            }
            if r.path == "/repos/acme/repo/issues/97" {
                return (200, issue_detail(fresh_title));
            }
            panic!("unexpected {}", r.path);
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
        factory.save_settings(settings("bot:")).unwrap();
        factory.poll_now().await.unwrap();
        factory.poll_now().await.unwrap();
        let runs = factory.snapshot().unwrap().runs;
        assert_eq!(runs.len(), expected);
        let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
        let consumed: i64 = db
            .query_row("SELECT count(*) FROM dedup", [], |r| r.get(0))
            .unwrap();
        assert_eq!(consumed, expected as i64);
        for call in server
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path.starts_with("/search/"))
        {
            let url = reqwest::Url::parse(&format!("{}{}", server.base, call.path)).unwrap();
            assert_eq!(
                url.query_pairs().find(|(key, _)| key == "q").unwrap().1,
                "is:issue is:open author:alice user:acme"
            );
        }
        if expected == 1 {
            let event = factory.run_detail(&runs[0].id).unwrap().event;
            assert_eq!(event.event_id, "issue:acme/repo#97");
            assert_eq!(event.title, fresh_title);
            assert_eq!(event.payload["issue"]["title"], json!(fresh_title));
            assert_eq!(event.origin_token_profile_id.as_deref(), Some("alice"));
        }
    }
}

#[tokio::test]
async fn issue_prefix_polling_matches_literals_and_keeps_fresh_filters() {
    for (prefix, search_title, field, value, expected, fetch, error) in [
        ("[AI]", Some("[AI]"), "title", json!("[AI]"), 1, true, false),
        (
            "[AI]",
            Some("[AI]tarea"),
            "title",
            json!("[AI]tarea"),
            1,
            true,
            false,
        ),
        (
            "[AI]",
            Some("  [AI] tarea  "),
            "title",
            json!("\u{2003}[AI] tarea\u{2003}"),
            1,
            true,
            false,
        ),
        (
            "[AI]",
            Some("[ai] tarea"),
            "title",
            json!("[ai] tarea"),
            0,
            false,
            false,
        ),
        (
            "[AI]",
            Some("Nota [AI] tarea"),
            "title",
            json!("Nota [AI] tarea"),
            0,
            false,
            false,
        ),
        (
            "[.*]",
            Some("[.*]literal"),
            "title",
            json!("[.*]literal"),
            1,
            true,
            false,
        ),
        (
            "Team Bot:",
            Some("Team  Bot: tarea"),
            "title",
            json!("Team  Bot: tarea"),
            0,
            false,
            false,
        ),
        (
            "BOT:",
            Some("bot: tarea"),
            "title",
            json!("bot: tarea"),
            0,
            false,
            false,
        ),
        (
            "BOT:",
            Some("BOT:tarea"),
            "title",
            json!("BOT:tarea"),
            1,
            true,
            false,
        ),
        ("bot:", None, "title", json!("bot: tarea"), 1, true, false),
        ("bot:", None, "title", json!("normal"), 0, true, false),
        (
            "bot:",
            Some("bot: old"),
            "title",
            json!("normal"),
            0,
            true,
            false,
        ),
        (
            "bot:",
            Some("bot: old"),
            "state",
            json!("closed"),
            0,
            true,
            false,
        ),
        (
            "bot:",
            Some("bot: old"),
            "user",
            json!({"login":"mallory"}),
            0,
            true,
            false,
        ),
        (
            "bot:",
            Some("bot: old"),
            "pull_request",
            json!({}),
            0,
            true,
            false,
        ),
        (
            "bot:",
            Some("bot: old"),
            "html_url",
            json!("https://github.com/acme/other/issues/97"),
            0,
            true,
            true,
        ),
    ] {
        let mut row = search_row(search_title.unwrap_or(""));
        if search_title.is_none() {
            row.as_object_mut().unwrap().remove("title");
        }
        let mut fresh = issue_detail("bot: old");
        fresh[field] = value;
        let server = Server::start(move |r| {
            let row = row.clone();
            let fresh = fresh.clone();
            async move {
                assert_eq!(r.method, "GET");
                assert_eq!(r.authorization, "Bearer fixture-source-only");
                if r.path == "/user" {
                    return (200, json!({"login":"alice"}));
                }
                if r.path.starts_with("/search/") {
                    return (
                        200,
                        json!({"total_count":1,"incomplete_results":false,"items":[row]}),
                    );
                }
                if r.path == "/repos/acme/repo/issues/97" {
                    return (200, fresh);
                }
                panic!("unexpected {}", r.path);
            }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
        factory.save_settings(settings(prefix)).unwrap();
        assert_eq!(
            factory.poll_now().await.is_err(),
            error,
            "{prefix} {search_title:?} {field}"
        );
        assert_eq!(
            factory.snapshot().unwrap().runs.len(),
            expected,
            "{prefix} {search_title:?} {field}"
        );
        assert_eq!(
            server
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.path.ends_with("/issues/97")),
            fetch
        );
        let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM dedup", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            expected as i64
        );
    }
}

#[tokio::test]
async fn issue_prefix_polling_preserves_search_and_workflow_barriers() {
    for mode in ["owner", "author", "closed", "pr", "global", "profile"] {
        let mut row = search_row("bot: task");
        match mode {
            "owner" => row["repository_url"] = json!("https://api.github.com/repos/outside/repo"),
            "author" => row["user"]["login"] = json!("mallory"),
            "closed" => row["state"] = json!("closed"),
            "pr" => row["pull_request"] = json!({}),
            _ => (),
        }
        let server = Server::start(move |r| {
            let row = row.clone();
            async move {
                if r.path == "/user" {
                    return (200, json!({"login":"alice"}));
                }
                if r.path.starts_with("/search/") {
                    return (
                        200,
                        json!({"total_count":1,"incomplete_results":false,"items":[row]}),
                    );
                }
                panic!("unexpected {}", r.path);
            }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
        let mut s = settings("bot:");
        let flow = s.flows.iter_mut().find(|f| f.id == "new_issue").unwrap();
        if mode == "global" {
            flow.enabled = false;
        }
        if mode == "profile" {
            flow.profile_activation.insert("alice".into(), false);
        }
        factory.save_settings(s).unwrap();
        let result = factory.poll_now().await;
        if mode == "owner" {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("outside token scope"));
        } else {
            result.unwrap();
        }
        assert!(factory.snapshot().unwrap().runs.is_empty(), "{mode}");
        assert!(!server
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.path.ends_with("/issues/97")));
        let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM dedup", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn issue_prefix_overlap_keeps_distinct_triggers_and_one_detail_fetch() {
    for (prefix, title, release, expected_ids) in [
        (
            "[",
            "[RELEASE]2",
            true,
            vec!["issue:acme/repo#97", "release:acme/repo#97"],
        ),
        (
            "[RELEASE]",
            "[RELEASE]2",
            true,
            vec!["issue:acme/repo#97", "release:acme/repo#97"],
        ),
        ("[", "[RELEASE]2", false, vec!["issue:acme/repo#97"]),
        ("bot:", "[RELEASE]2", true, vec!["release:acme/repo#97"]),
        ("bot:", " [RELEASE]2", true, vec!["release:acme/repo#97"]),
    ] {
        let server = Server::start(move |r| async move {
            if r.path == "/user" {
                return (200, json!({"login":"alice"}));
            }
            if r.path.starts_with("/search/") {
                return (
                    200,
                    json!({"total_count":1,"incomplete_results":false,"items":[search_row(title)]}),
                );
            }
            if r.path.ends_with("/issues/97") {
                return (200, issue_detail(title));
            }
            panic!("unexpected {}", r.path);
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
        let mut s = settings(prefix);
        s.flows
            .iter_mut()
            .find(|f| f.id == "release")
            .unwrap()
            .enabled = release;
        factory.save_settings(s).unwrap();
        factory.poll_now().await.unwrap();
        factory.poll_now().await.unwrap();
        let runs = factory.snapshot().unwrap().runs;
        assert_eq!(runs.len(), expected_ids.len());
        let mut ids: Vec<_> = runs
            .iter()
            .map(|r| factory.run_detail(&r.id).unwrap().event.event_id)
            .collect();
        ids.sort();
        assert_eq!(ids, expected_ids);
        assert_eq!(
            server
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.path.ends_with("/issues/97"))
                .count(),
            usize::from(!expected_ids.is_empty())
        );
    }
}

#[tokio::test]
async fn issue_prefix_does_not_cross_source_credentials() {
    let server = Server::start(|r| async move {
        assert_eq!(r.method, "GET");
        let actor = match r.authorization.as_str() { "Bearer fixture-source-only" => "alice", "Bearer fixture-b" => "bob", _ => panic!("unexpected credentials") };
        let title = if actor == "alice" { "bot: scoped" } else { "personal: scoped" };
        if r.path == "/user" { return (200, json!({"login":actor})); }
        if r.path.starts_with("/search/") {
            return (200, json!({"total_count":1,"incomplete_results":false,"items":[{"number":97,"title":title,"state":"open","repository_url":format!("https://api.github.com/repos/{actor}/repo"),"user":{"login":actor}}]}));
        }
        assert_eq!(r.path, format!("/repos/{actor}/repo/issues/97"));
        (200, json!({"number":97,"title":title,"state":"open","user":{"login":actor},"html_url":format!("https://github.com/{actor}/repo/issues/97")}))
    }).await;
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut s = settings("bot:");
    s.source_connectors[0].owners = vec!["alice".into()];
    s.source_connectors.push(SourceConnector {
        id: "bob".into(),
        label: "Personal".into(),
        account: "bob".into(),
        issue_title_prefix: "personal:".into(),
        owners: vec!["bob".into()],
        authors: vec!["bob".into()],
        token: Some("fixture-b".into()),
        ..Default::default()
    });
    factory.save_settings(s).unwrap();
    factory.poll_now().await.unwrap();
    let runs = factory.snapshot().unwrap().runs;
    assert_eq!(runs.len(), 2);
    for run in runs {
        assert_eq!(
            run.origin_token_profile_id.as_deref(),
            Some(run.actor.as_str())
        );
        assert_eq!(run.repository, format!("{}/repo", run.actor));
        assert_eq!(
            factory.run_detail(&run.id).unwrap().event.title,
            if run.actor == "alice" {
                "bot: scoped"
            } else {
                "personal: scoped"
            }
        );
    }
    let calls = server.calls.lock().unwrap();
    let searches: Vec<_> = calls
        .iter()
        .filter(|r| r.path.starts_with("/search/"))
        .collect();
    assert_eq!(searches.len(), 2);
    for call in searches {
        let actor = if call.authorization == "Bearer fixture-b" {
            "bob"
        } else {
            "alice"
        };
        let url = reqwest::Url::parse(&format!("{}{}", server.base, call.path)).unwrap();
        assert_eq!(
            url.query_pairs().find(|(key, _)| key == "q").unwrap().1,
            format!("is:issue is:open author:{actor} user:{actor}")
        );
    }
}

#[tokio::test]
async fn issue_prefix_saved_while_detail_waits_is_used_after_response() {
    for enabling in [false, true] {
        let reached = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let server = Server::start({
        let reached = reached.clone();
        let resume = resume.clone();
        move |r| {
            let reached = reached.clone();
            let resume = resume.clone();
            async move {
                if r.path == "/user" { return (200, json!({"login":"alice"})); }
                if r.path.starts_with("/search/") {
                    return (200, json!({"total_count":1,"incomplete_results":false,
                                       "items":[if enabling { let mut row = search_row("unused"); row.as_object_mut().unwrap().remove("title"); row } else { search_row("[AI] candidate") }]}));
                }
                if r.path == "/repos/acme/repo/issues/97" {
                    reached.notify_one();
                    resume.notified().await;
                    return (200, issue_detail(if enabling { "bot: candidate" } else { "[AI] candidate" }));
                }
                panic!("unexpected {}", r.path);
            }
        }
    }).await;
        let dir = tempfile::tempdir().unwrap();
        let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
        factory.save_settings(settings("[AI]")).unwrap();
        let polling = {
            let factory = factory.clone();
            tokio::spawn(async move { factory.poll_now().await })
        };
        tokio::time::timeout(Duration::from_secs(5), reached.notified())
            .await
            .unwrap();
        let mut changed = factory.settings().unwrap();
        changed.source_connectors[0].issue_title_prefix = "bot:".into();
        factory.save_settings(changed).unwrap();
        resume.notify_one();
        tokio::time::timeout(Duration::from_secs(5), polling)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            factory.snapshot().unwrap().runs.len(),
            usize::from(enabling)
        );
        let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
        let consumed: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM dedup WHERE event_id='issue:acme/repo#97'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(consumed, i64::from(enabling));
    }
}
