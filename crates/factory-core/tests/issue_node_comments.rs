#[path = "common/actions_http.rs"]
mod actions_http;
mod common;
use actions_http::{Reply, Script, Step};
use factory_core::*;
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};
fn db(root: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(root.join("factory.sqlite3")).unwrap()
}
fn configure(f: &Factory) {
    let mut s = f.settings().unwrap();
    s.flows[0].nodes.truncate(1);
    s.token_profiles.push(TokenProfile {
        id: "alice".into(),
        label: "Alice".into(),
        account: "alice".into(),
        token: Some("alice-token".into()),
        ..Default::default()
    });
    f.save_settings(s).unwrap();
}
fn enqueue(f: &Factory, number: u64) -> String {
    f.enqueue(TriggerEvent::issue(
        "acme/repo",
        number,
        "[AI] notify",
        "alice",
        "",
    ))
    .unwrap()
    .unwrap()
}
async fn wait(root: &Path, sql: &str, want: i64) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let n: i64 = db(root).query_row(sql, [], |r| r.get(0)).unwrap();
            if n == want {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("notification condition");
}
fn user() -> Step {
    Step::new("GET", "/user", Reply::Json(200, json!({"login":"alice"})))
        .header("authorization", "Bearer alice-token")
}
fn list() -> Step {
    Step::new(
        "GET",
        "/repos/acme/repo/issues/41/comments?per_page=100&page=1",
        Reply::Json(200, json!([])),
    )
}
fn body(root: &Path) -> String {
    let raw: String = db(root)
        .query_row("SELECT snapshot FROM issue_node_comments", [], |r| r.get(0))
        .unwrap();
    let s: Value = serde_json::from_str(&raw).unwrap();
    let marker = db(root)
        .query_row("SELECT key FROM issue_node_comments", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    format!("AI Factory · {} ({}) · skipped\n\nRun {} · attempt 1 · result 1\n\nStep was not executed.\n\n<!-- ai-factory:{} -->",s["node_name"].as_str().unwrap(),s["node_id"].as_str().unwrap(),s["run_id"].as_str().unwrap(),marker)
}
fn receipt(body: &str) -> Value {
    json!({"id":9001,"body":body,"user":{"login":"alice"}})
}
#[tokio::test]
async fn issue_node_comments_restart_delivers_terminal_once_and_cancelled_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    configure(&f);
    let id = enqueue(&f, 41);
    f.set_run_status(&id, "failed", Some("global PRIVATE".into()))
        .unwrap();
    let cancelled = enqueue(&f, 42);
    f.cancel(&cancelled).unwrap();
    let text = body(dir.path());
    f.shutdown().await;
    drop(f);
    let mut server = Script::start(vec![
        user(),
        list(),
        Step::new(
            "POST",
            "/repos/acme/repo/issues/41/comments",
            Reply::Json(201, receipt(&text)),
        )
        .body(json!({"body":text})),
    ])
    .await;
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    f.start_notification_delivery();
    f.start_notification_delivery();
    wait(
        dir.path(),
        "SELECT count(*) FROM issue_node_comments WHERE state='delivered'",
        1,
    )
    .await;
    assert_eq!(f.run_detail(&id).unwrap().run.status, "failed");
    assert_eq!(
        f.run_detail(&cancelled).unwrap().nodes[0].status,
        "cancelled"
    );
    f.shutdown().await;
    server.assert_done();
    assert_eq!(server.writes().len(), 1);
    server.shutdown().await;
}
#[tokio::test]
async fn issue_node_comments_failures_are_durable_visible_and_do_not_change_nodes() {
    let mut server = Script::start(vec![
        user(),
        list(),
        Step::new(
            "POST",
            "/repos/acme/repo/issues/41/comments",
            Reply::Json(403, json!({"secret":"DO_NOT_LOG"})),
        ),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    configure(&f);
    let id = enqueue(&f, 41);
    f.set_run_status(&id, "failed", None).unwrap();
    let before = serde_json::to_value(f.run_detail(&id).unwrap().nodes).unwrap();
    let mut events = f.subscribe();
    f.start_notification_delivery();
    wait(dir.path(),"SELECT count(*) FROM issue_node_comments WHERE failures=1 AND receipt IS NULL AND last_error='http_rejected'",1).await;
    let node = f.run_detail(&id).unwrap().nodes[0].id.clone();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if f.logs(&id, &node, 0)
                .unwrap()
                .text
                .contains("issue_node_comment_delivery_failed")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(f.run_detail(&id).unwrap().nodes).unwrap(),
        before
    );
    let logs = f.logs(&id, &node, 0).unwrap().text;
    assert!(logs.contains("reason=http_rejected retry=pending"));
    assert!(!logs.contains("DO_NOT_LOG"));
    assert!(!logs.contains("alice-token"));
    let mut seen = false;
    while let Ok(e) = events.try_recv() {
        seen |= e.kind == "log" && e.run_id.as_deref() == Some(&id);
    }
    assert!(seen);
    f.shutdown().await;
    drop(f);
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    assert_eq!(
        db(dir.path())
            .query_row("SELECT failures FROM issue_node_comments", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
    f.shutdown().await;
    server.assert_done();
    server.shutdown().await;
}
#[tokio::test]
async fn issue_node_comments_deletion_during_list_suppresses_post() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    configure(&f);
    let id = enqueue(&f, 41);
    f.set_run_status(&id, "failed", None).unwrap();
    f.shutdown().await;
    drop(f);
    let holder: Arc<std::sync::Mutex<Option<Arc<Factory>>>> = Arc::default();
    let h = holder.clone();
    let deleted = id.clone();
    let mut server = Script::start(vec![
        user(),
        list().then(move || {
            h.lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .delete_run(&deleted)
                .unwrap();
        }),
    ])
    .await;
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    *holder.lock().unwrap() = Some(f.clone());
    f.start_notification_delivery();
    wait(dir.path(), "SELECT count(*) FROM runs", 0).await;
    f.shutdown().await;
    holder.lock().unwrap().take();
    assert!(!dir.path().join("sessions").join(&id).exists());
    assert_eq!(
        db(dir.path())
            .query_row("SELECT count(*) FROM issue_node_comments", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
    server.assert_done();
    assert!(server.writes().is_empty());
    server.shutdown().await;
}

#[tokio::test]
async fn issue_node_comments_shutdown_after_accepted_post_reconciles_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    configure(&f);
    let id = enqueue(&f, 41);
    f.set_run_status(&id, "failed", None).unwrap();
    let text = body(dir.path());
    f.shutdown().await;
    drop(f);
    let posted = Arc::new(tokio::sync::Notify::new());
    let accepted = posted.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let mut server = Script::start(vec![
        user(),
        list(),
        Step::new(
            "POST",
            "/repos/acme/repo/issues/41/comments",
            Reply::Wait(release.clone(), receipt(&text)),
        )
        .then(move || accepted.notify_one()),
        user(),
        Step::new(
            "GET",
            "/repos/acme/repo/issues/41/comments?per_page=100&page=1",
            Reply::Json(200, json!([receipt(&text)])),
        ),
    ])
    .await;
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    f.start_notification_delivery();
    tokio::time::timeout(Duration::from_secs(3), posted.notified())
        .await
        .unwrap();
    f.shutdown().await;
    assert_eq!(
        db(dir.path())
            .query_row(
                "SELECT failures FROM issue_node_comments WHERE state='pending'",
                [],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
        0
    );
    drop(f);
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    f.start_notification_delivery();
    wait(
        dir.path(),
        "SELECT count(*) FROM issue_node_comments WHERE state='delivered'",
        1,
    )
    .await;
    f.shutdown().await;
    assert_eq!(server.writes().len(), 1);
    server.assert_done();
    release.notify_one();
    server.shutdown().await;
}
#[tokio::test]
async fn issue_node_comments_post_response_after_deletion_cannot_recreate_files() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    configure(&f);
    let id = enqueue(&f, 41);
    f.set_run_status(&id, "failed", None).unwrap();
    let text = body(dir.path());
    f.shutdown().await;
    drop(f);
    let holder: Arc<std::sync::Mutex<Option<Arc<Factory>>>> = Arc::default();
    let h = holder.clone();
    let deleted = id.clone();
    let mut server = Script::start(vec![
        user(),
        list(),
        Step::new(
            "POST",
            "/repos/acme/repo/issues/41/comments",
            Reply::Json(201, receipt(&text)),
        )
        .then(move || {
            h.lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .delete_run(&deleted)
                .unwrap();
        }),
    ])
    .await;
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    *holder.lock().unwrap() = Some(f.clone());
    f.start_notification_delivery();
    wait(dir.path(), "SELECT count(*) FROM runs", 0).await;
    f.shutdown().await;
    holder.lock().unwrap().take();
    assert!(!dir.path().join("sessions").join(&id).exists());
    assert_eq!(
        db(dir.path())
            .query_row("SELECT count(*) FROM issue_node_comments", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
    server.assert_done();
    assert_eq!(server.writes().len(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn issue_node_comments_exhaustion_persists_without_changing_terminal_result() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    let mut settings = f.settings().unwrap();
    settings.flows[0].nodes.truncate(1);
    f.save_settings(settings).unwrap();
    let id = enqueue(&f, 41);
    f.set_run_status(&id, "failed", None).unwrap();
    f.start_notification_delivery();
    for failures in 1..=5 {
        wait(
            dir.path(),
            &format!("SELECT count(*) FROM issue_node_comments WHERE failures={failures}"),
            1,
        )
        .await;
        if failures < 5 {
            db(dir.path())
                .execute(
                    "UPDATE issue_node_comments SET next_at='2000-01-01T00:00:00Z'",
                    [],
                )
                .unwrap();
        }
    }
    let node = f.run_detail(&id).unwrap().nodes[0].id.clone();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if f.logs(&id, &node, 0)
                .unwrap()
                .text
                .contains("retry=exhausted")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    f.shutdown().await;
    drop(f);
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    let state: (String, u32, Option<String>) = db(dir.path())
        .query_row(
            "SELECT state,failures,receipt FROM issue_node_comments",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, ("exhausted".into(), 5, None));
    assert_eq!(f.run_detail(&id).unwrap().run.status, "failed");
    f.start_notification_delivery();
    f.shutdown().await;
}
#[tokio::test]
async fn issue_node_comments_one_retry_does_not_block_another_ready_run() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    configure(&f);
    let a = enqueue(&f, 41);
    f.set_run_status(&a, "failed", None).unwrap();
    let b = enqueue(&f, 42);
    f.set_run_status(&b, "failed", None).unwrap();
    let snapshot: String = db(dir.path())
        .query_row(
            "SELECT snapshot FROM issue_node_comments WHERE run_id=?1",
            [&b],
            |r| r.get(0),
        )
        .unwrap();
    let s: Value = serde_json::from_str(&snapshot).unwrap();
    let key: String = db(dir.path())
        .query_row(
            "SELECT key FROM issue_node_comments WHERE run_id=?1",
            [&b],
            |r| r.get(0),
        )
        .unwrap();
    let text=format!("AI Factory · {} ({}) · skipped\n\nRun {} · attempt 1 · result 1\n\nStep was not executed.\n\n<!-- ai-factory:{} -->",s["node_name"].as_str().unwrap(),s["node_id"].as_str().unwrap(),b,key);
    f.shutdown().await;
    drop(f);
    let mut server = Script::start(vec![
        user(),
        list(),
        Step::new(
            "POST",
            "/repos/acme/repo/issues/41/comments",
            Reply::Json(403, json!({})),
        ),
        user(),
        Step::new(
            "GET",
            "/repos/acme/repo/issues/42/comments?per_page=100&page=1",
            Reply::Json(200, json!([])),
        ),
        Step::new(
            "POST",
            "/repos/acme/repo/issues/42/comments",
            Reply::Json(201, receipt(&text)),
        ),
    ])
    .await;
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    f.start_notification_delivery();
    wait(
        dir.path(),
        "SELECT count(*) FROM issue_node_comments WHERE state='delivered'",
        1,
    )
    .await;
    f.shutdown().await;
    assert_eq!(
        db(dir.path())
            .query_row(
                "SELECT failures FROM issue_node_comments WHERE run_id=?1",
                [&a],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
        1
    );
    server.assert_done();
    server.shutdown().await;
}

#[tokio::test]
async fn issue_node_comments_cold_cache_redacts_all_profiles_before_unicode_limit() {
    use factory_core::context::factory::infrastructure::repositories::store::Store;
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    configure(&f);
    let mut settings = f.settings().unwrap();
    settings.token_profiles.push(TokenProfile {
        id: "bob".into(),
        label: "Bob".into(),
        account: "bob".into(),
        token: Some("bob-token".into()),
        ..Default::default()
    });
    settings.flows[0].nodes[0].name = "Builder alice-token bob-token".into();
    f.save_settings(settings).unwrap();
    let id = enqueue(&f, 41);
    f.shutdown().await;
    drop(f);
    // Save through the real terminal hook with a newly opened, cold secret cache.
    let store = Store::open_with_credentials(dir.path(), common::backend(dir.path())).unwrap();
    let mut n = store.nodes(&id).unwrap().remove(0);
    n.status = "failed".into();
    n.error = Some("alice-token bob-token PRIVATE_ERROR".into());
    n.result = Some(
        json!({"summary":format!("{} alice-token bob-token {}","x".repeat(3400),"é🦀".repeat(1800)),"data":{"private":"DO_NOT_PUBLISH"}}),
    );
    store.save_node(&id, &n).unwrap();
    drop(store);
    let snapshot: String = db(dir.path())
        .query_row("SELECT snapshot FROM issue_node_comments", [], |r| r.get(0))
        .unwrap();
    assert!(snapshot.contains("alice-token"));
    assert!(snapshot.contains("bob-token"));
    let key: String = db(dir.path())
        .query_row("SELECT key FROM issue_node_comments", [], |r| r.get(0))
        .unwrap();
    let marker = format!("<!-- ai-factory:{key} -->");
    let mut server = Script::start(vec![
        user(),
        list(),
        Step::new(
            "POST",
            "/repos/acme/repo/issues/41/comments",
            Reply::Json(201, receipt(&marker)),
        ),
    ])
    .await;
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    f.cancel(&id).unwrap();
    assert_eq!(f.run_detail(&id).unwrap().nodes[0].status, "failed");
    f.start_notification_delivery();
    wait(
        dir.path(),
        "SELECT count(*) FROM issue_node_comments WHERE state='delivered'",
        1,
    )
    .await;
    f.shutdown().await;
    let requests = server.requests();
    let body = requests.iter().find(|(m, _, _)| m == "POST").unwrap().2["body"]
        .as_str()
        .unwrap();
    assert!(body.contains("[REDACTED]"));
    assert!(body.contains("failed"));
    assert!(body.ends_with(&marker));
    assert!(body.len() <= 6000);
    for text in [
        "alice-token",
        "bob-token",
        "PRIVATE_ERROR",
        "DO_NOT_PUBLISH",
    ] {
        assert!(!body.contains(text));
    }
    let log = f.logs(&id, &n.id, 0).unwrap().text;
    assert!(!log.contains("alice-token"));
    assert!(!log.contains("bob-token"));
    server.assert_done();
    server.shutdown().await;
}
#[tokio::test]
async fn issue_node_comments_redaction_failure_fails_closed_without_http() {
    let mut server = Script::start(vec![]).await;
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    configure(&f);
    let mut settings = f.settings().unwrap();
    settings.flows[0].nodes[0].id = "alice-token".into();
    f.save_settings(settings).unwrap();
    let id = enqueue(&f, 41);
    f.set_run_status(&id, "failed", None).unwrap();
    f.shutdown().await;
    drop(f);
    let credentials = common::backend(dir.path());
    credentials
        .fail_get
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    f.start_notification_delivery();
    wait(dir.path(),"SELECT count(*) FROM issue_node_comments WHERE failures=1 AND last_error='redaction_unavailable' AND receipt IS NULL",1).await;
    f.shutdown().await;
    let raw = std::fs::read_to_string(
        dir.path()
            .join("sessions")
            .join(&id)
            .join("alice-token.log"),
    )
    .unwrap();
    assert!(
        !raw.contains("alice-token"),
        "diagnostics must fail closed when secrets cannot load"
    );
    assert!(server.requests().is_empty());
    server.assert_done();
    server.shutdown().await;
}

#[tokio::test]
async fn issue_node_comments_retry_recovers_failed_diagnostic_write() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    configure(&f);
    let id = enqueue(&f, 41);
    f.set_run_status(&id, "failed", None).unwrap();
    let text = body(dir.path());
    let node = f.run_detail(&id).unwrap().nodes[0].id.clone();
    std::fs::write(
        dir.path().join("sessions").join(&id),
        "fixture denies directory",
    )
    .unwrap();
    f.shutdown().await;
    drop(f);
    let mut server = Script::start(vec![
        user(),
        list(),
        Step::new(
            "POST",
            "/repos/acme/repo/issues/41/comments",
            Reply::Json(403, json!({})),
        ),
        user(),
        list(),
        Step::new(
            "POST",
            "/repos/acme/repo/issues/41/comments",
            Reply::Json(201, receipt(&text)),
        ),
    ])
    .await;
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    f.start_notification_delivery();
    wait(
        dir.path(),
        "SELECT count(*) FROM issue_node_comments WHERE failures=1",
        1,
    )
    .await;
    std::fs::remove_file(dir.path().join("sessions").join(&id)).unwrap();
    db(dir.path())
        .execute(
            "UPDATE issue_node_comments SET next_at='2000-01-01T00:00:00Z'",
            [],
        )
        .unwrap();
    wait(
        dir.path(),
        "SELECT count(*) FROM issue_node_comments WHERE state='delivered'",
        1,
    )
    .await;
    f.shutdown().await;
    let logs = f.logs(&id, &node, 0).unwrap().text;
    assert!(
        logs.contains("reason=http_rejected"),
        "prior failure must become visible after I/O recovery"
    );
    server.assert_done();
    server.shutdown().await;
}
