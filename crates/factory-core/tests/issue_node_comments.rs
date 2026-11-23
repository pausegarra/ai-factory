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
    s.source_connectors.push(SourceConnector {
        owners: vec!["acme".into()],
        authors: vec!["alice".into()],
        reviewers: vec!["alice".into()],
        kind: factory_core::SourceKind::GithubIssues,
        id: "alice".into(),
        label: "Alice".into(),
        account: "alice".into(),
        token: Some("alice-token".into()),
        ..Default::default()
    });
    f.save_settings(s).unwrap();
}
fn enqueue(f: &Factory, number: u64) -> String {
    f.enqueue(common::from_source(
        TriggerEvent::issue("acme/repo", number, "[AI] notify", "alice", ""),
        "alice",
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
    configure(&f);
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

async fn post_automatic_summary_with_redaction(
    webhook_url: &str,
    summary: &str,
    redaction_checks: &[(&str, &str)],
) -> String {
    use factory_core::context::factory::infrastructure::repositories::store::Store;

    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    configure(&f);
    let mut settings = f.settings().unwrap();
    settings
        .notification_connectors
        .push(NotificationConnector {
            id: "redaction-fixture".into(),
            label: "Redaction fixture".into(),
            enabled: false,
            events: vec![],
            provider: ConnectorProvider::GoogleChat(GoogleChatConnectorConfig {
                webhook_url: Some(webhook_url.into()),
                ..Default::default()
            }),
        });
    f.save_settings(settings).unwrap();
    let id = enqueue(&f, 41);
    f.shutdown().await;
    drop(f);

    let store = Store::open_with_credentials(dir.path(), common::backend(dir.path())).unwrap();
    let mut node = store.nodes(&id).unwrap().remove(0);
    store.load_redaction_secrets().unwrap();
    assert_eq!(store.redact_cached(summary), summary);
    for &(input, expected) in redaction_checks {
        assert_eq!(store.redact_cached(input), expected);
    }
    node.status = "succeeded".into();
    node.result = Some(json!({"summary":summary}));
    store.save_node(&id, &node).unwrap();
    drop(store);

    let key: String = db(dir.path())
        .query_row("SELECT key FROM issue_node_comments", [], |row| row.get(0))
        .unwrap();
    let marker = format!("<!-- ai-factory:{key} -->");
    let path = "/repos/acme/repo/issues/41/comments";
    let mut server = Script::start(vec![
        user(),
        list(),
        Step::new("POST", path, Reply::Json(201, receipt(&marker))),
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
    server.shutdown().await;

    server.assert_done();
    assert_eq!(server.writes(), vec![("POST".into(), path.into())]);
    let requests = server.requests();
    let (_, _, posted) = requests
        .iter()
        .find(|(method, request_path, _)| method == "POST" && request_path == path)
        .unwrap();
    let body = posted["body"].as_str().unwrap();
    assert_eq!(body.matches("<!--").count(), 1);
    assert_eq!(body.matches(marker.as_str()).count(), 1);
    assert!(body.len() <= 6000);
    let suffix = format!("\n\n{marker}");
    let visible = body.strip_suffix(suffix.as_str()).unwrap();
    let published_summary = visible.splitn(3, "\n\n").nth(2).unwrap();
    for &(pattern, _) in redaction_checks {
        assert!(
            !body.contains(pattern),
            "synthetic pattern in POST: {pattern}"
        );
    }
    published_summary.to_owned()
}

#[tokio::test]
async fn issue_node_comments_redacts_overlapping_patterns_before_preserving_paragraphs() {
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=abc%20def&token=defghi",
        "abc\n\nde\u{200b}fghi",
        &[
            ("abc def", "[REDACTED]"),
            ("defghi", "[REDACTED]"),
            ("abc defghi", "[REDACTED]"),
        ],
    )
    .await;
    assert_eq!(published_summary, "[REDACTED]");
}

#[tokio::test]
async fn issue_node_comments_redacts_pattern_created_by_markdown_fence_escape() {
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=%5C~~~demo",
        "Decisión.\n\n~~~demo\n\nVerificación.",
        &[(r"\~~~demo", "[REDACTED]")],
    )
    .await;
    assert_eq!(
        published_summary,
        "Decisión.\n\n[REDACTED]\n\nVerificación."
    );
}

#[tokio::test]
async fn issue_node_comments_preserves_fence_like_diagnostics_in_lists() {
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=unrelated-fixture",
        "Verificación incompleta.\n\n- ~~~text\ntimeout\n\n2. + ```text\nfixture failure\n\n- ~~~unavailable\n\nConsulta [Check](https://example.invalid/check).",
        &[],
    )
    .await;
    assert_eq!(
        published_summary,
        "Verificación incompleta.\n\n- \\~~~text timeout\n\n2. + \\```text fixture failure\n\n- \\~~~unavailable\n\nConsulta [Check](https://example.invalid/check)."
    );
}

#[tokio::test]
async fn issue_node_comments_preserves_inline_code_at_start_of_summary_paragraphs() {
    let summary = "```cargo check``` pasó.\n\n- ````a ` b```` es un ejemplo.";
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=unrelated-fixture",
        summary,
        &[],
    )
    .await;
    assert_eq!(published_summary, summary);
}

#[tokio::test]
async fn issue_node_comments_redacts_pattern_created_by_list_fence_escape() {
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=%5C~~~text",
        "Verificación parcial.\n\n- ~~~text\ntimeout\n\nConsulta [Check](https://example.invalid/check).",
        &[(r"\~~~text", "[REDACTED]")],
    )
    .await;
    assert_eq!(
        published_summary,
        "Verificación parcial.\n\n- [REDACTED] timeout\n\nConsulta [Check](https://example.invalid/check)."
    );
}

#[tokio::test]
async fn issue_node_comments_neutralizes_a_fence_created_by_truncating_inline_code() {
    let summary = format!("```long fixture {}``` passed.", "é".repeat(1800));
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=unrelated-fixture",
        &summary,
        &[],
    )
    .await;
    assert!(published_summary.starts_with(r"\```long fixture "));
    assert!(published_summary.ends_with('…'));
    assert!(published_summary.len() <= 3500);
}

#[tokio::test]
async fn issue_node_comments_preserves_reference_like_diagnostics() {
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=unrelated-fixture",
        "Verificación incompleta.\n\n[ERROR]: timeout\n\n[build\\]]: failed\n\n[Check](https://example.invalid/check)\n\n[INFO] See [check]: details",
        &[],
    )
    .await;
    assert_eq!(
        published_summary,
        "Verificación incompleta.\n\n\\[ERROR]: timeout\n\n\\[build\\]]: failed\n\n[Check](https://example.invalid/check)\n\n[INFO] See [check]: details"
    );
}

#[tokio::test]
async fn issue_node_comments_redacts_pattern_created_by_reference_escape() {
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=%5C%5BERROR%5D%3A%20timeout",
        "Verificación incompleta.\n\n[ERROR]: timeout",
        &[(r"\[ERROR]: timeout", "[REDACTED]")],
    )
    .await;
    assert_eq!(published_summary, "Verificación incompleta.\n\n[REDACTED]");
}

#[tokio::test]
async fn issue_node_comments_preserves_reference_like_diagnostics_in_lists() {
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=unrelated-fixture",
        "Verificación incompleta.\n\n- [ERROR]: timeout\n\n2) [RETRY]: unavailable\n\n+ * [BUILD]: failed\n\n- [Check](https://example.invalid/check)",
        &[],
    )
    .await;
    assert_eq!(
        published_summary,
        "Verificación incompleta.\n\n- \\[ERROR]: timeout\n\n2) \\[RETRY]: unavailable\n\n+ * \\[BUILD]: failed\n\n- [Check](https://example.invalid/check)"
    );
}

#[tokio::test]
async fn issue_node_comments_preserves_diagnostic_after_partial_redaction_of_reference_escape() {
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=%5C%5BERROR%5D",
        "Verificación incompleta.\n\n[ERROR]: timeout\n\nConsulta [Check](https://example.invalid/check).",
        &[(r"\[ERROR]", "[REDACTED]")],
    )
    .await;
    assert_eq!(
        published_summary,
        "Verificación incompleta.\n\n\\[REDACTED]: timeout\n\nConsulta [Check](https://example.invalid/check)."
    );
}

#[tokio::test]
async fn issue_node_comments_rechecks_redaction_after_escaping_replacement() {
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=%5C%5BERROR%5D&token=%5C%5BREDACTED%5D",
        "[ERROR]: timeout",
        &[(r"\[ERROR]", "[REDACTED]"), (r"\[REDACTED]", "[REDACTED]")],
    )
    .await;
    assert_eq!(published_summary, "Step completed; no summary available.");
}

#[tokio::test]
async fn issue_node_comments_checks_reconstructed_pattern_before_truncation() {
    let incomplete_pattern = r"\[REDACTED]: ti";
    // The 3500-byte limit reserves three bytes for the ellipsis.
    let padding = "x".repeat(3500 - '…'.len_utf8() - "\n\n".len() - incomplete_pattern.len());
    let summary = format!("{padding}\n\n[ERROR]: timeout");
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=%5C%5BERROR%5D&token=%5C%5BREDACTED%5D%3A%20timeout",
        &summary,
        &[
            (r"\[ERROR]", "[REDACTED]"),
            (r"\[REDACTED]: timeout", "[REDACTED]"),
        ],
    )
    .await;
    assert!(
        !published_summary.contains(incomplete_pattern),
        "the POST contains the prefix of a reconstructed redaction pattern"
    );
    assert_eq!(published_summary, "Step completed; no summary available.");
}

#[tokio::test]
async fn issue_node_comments_checks_fence_escape_before_reapplying_limit() {
    let incomplete_pattern = r"\```";
    let padding = "x".repeat(3500 - '…'.len_utf8() - "\n\n".len() - "```a".len());
    let summary = format!("{padding}\n\n```ab``` passed.");
    let published_summary = post_automatic_summary_with_redaction(
        "https://example.invalid/messages?key=%5C%60%60%60a",
        &summary,
        &[(r"\```a", "[REDACTED]")],
    )
    .await;
    assert!(
        !published_summary.contains(incomplete_pattern),
        "the POST contains the prefix of a pattern reconstructed by the final fence escape"
    );
    assert_eq!(published_summary, "Step completed; no summary available.");
}

#[tokio::test]
async fn issue_node_comments_cold_cache_redacts_all_profiles_before_unicode_limit() {
    use factory_core::context::factory::infrastructure::repositories::store::Store;
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    configure(&f);
    let mut settings = f.settings().unwrap();
    settings.source_connectors.push(SourceConnector {
        owners: vec!["acme".into()],
        authors: vec!["bob".into()],
        reviewers: vec!["bob".into()],
        kind: factory_core::SourceKind::GithubIssues,
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
    let summary = format!(
        "Decision. Built locally.\r\n \r\nVerification. Unit checks passed.\nWrapped fixture output.\n\u{200b}\nLimitations. Fixture <tag> @alice \u{0000}alice-token bob-token bob-\u{202e}token.\n\n{}",
        "é🦀".repeat(1800)
    );
    n.result = Some(json!({"summary":summary,"data":{"private":"DO_NOT_PUBLISH"}}));
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
    let posted = Arc::new(tokio::sync::Notify::new());
    let accepted = posted.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let comments_path = "/repos/acme/repo/issues/41/comments";
    let mut server = Script::start(vec![
        user(),
        list(),
        Step::new(
            "POST",
            comments_path,
            Reply::Wait(release.clone(), receipt(&marker)),
        )
        .then(move || accepted.notify_one()),
    ])
    .await;
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    f.cancel(&id).unwrap();
    assert_eq!(f.run_detail(&id).unwrap().nodes[0].status, "failed");
    f.start_notification_delivery();
    tokio::time::timeout(Duration::from_secs(3), posted.notified())
        .await
        .unwrap();
    f.shutdown().await;
    assert_eq!(
        db(dir.path())
            .query_row(
                "SELECT count(*) FROM issue_node_comments WHERE state='pending' AND failures=0",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        1
    );
    drop(f);

    let requests = server.requests();
    let body = requests
        .iter()
        .find(|(method, path, _)| method == "POST" && path == comments_path)
        .unwrap()
        .2["body"]
        .as_str()
        .unwrap();
    server.assert_done();
    assert_eq!(server.writes(), vec![("POST".into(), comments_path.into())]);
    release.notify_one();
    server.shutdown().await;

    let mut reopened = Script::start(vec![
        user(),
        Step::new(
            "GET",
            &format!("{comments_path}?per_page=100&page=1"),
            Reply::Json(200, json!([receipt(body)])),
        ),
    ])
    .await;
    let f = common::open_with_api(dir.path().into(), &reopened.base).unwrap();
    f.start_notification_delivery();
    wait(
        dir.path(),
        "SELECT count(*) FROM issue_node_comments WHERE state='delivered'",
        1,
    )
    .await;
    f.shutdown().await;
    reopened.assert_done();
    assert!(reopened.writes().is_empty());
    reopened.shutdown().await;

    let (delivered_key, raw_receipt): (String, String) = db(dir.path())
        .query_row(
            "SELECT key, receipt FROM issue_node_comments WHERE state='delivered'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(delivered_key, key);
    assert_eq!(
        serde_json::from_str::<Value>(&raw_receipt).unwrap()["comment_id"],
        json!(9001)
    );
    assert!(body.starts_with("AI Factory · Builder [REDACTED] [REDACTED] ("));
    assert!(body.contains(" · failed\n\nRun "));
    assert_eq!(body.matches("<!--").count(), 1);
    assert!(body.len() <= 6000);
    for forbidden in [
        "alice-token",
        "bob-token",
        "PRIVATE_ERROR",
        "DO_NOT_PUBLISH",
        "<tag>",
        "@alice",
        "\u{0000}",
        "\u{202e}",
    ] {
        assert!(!body.contains(forbidden));
    }
    let log = f.logs(&id, &n.id, 0).unwrap().text;
    assert!(!log.contains("alice-token"));
    assert!(!log.contains("bob-token"));

    let suffix = format!("\n\n{marker}");
    let visible = body.strip_suffix(suffix.as_str()).unwrap();
    let summary = visible.splitn(3, "\n\n").nth(2).unwrap();
    assert!(summary.starts_with(concat!(
        "Decision. Built locally.\n\n",
        "Verification. Unit checks passed. Wrapped fixture output.\n\n",
        "Limitations. Fixture ‹tag› ＠alice [REDACTED] [REDACTED] [REDACTED].\n\n",
    )));
    assert!((3494..=3500).contains(&summary.len()));
    let (_, tail) = summary.rsplit_once("\n\n").unwrap();
    let payload = tail.strip_suffix('…').unwrap();
    assert!(payload.starts_with("é🦀"));
    assert!(payload.chars().all(|ch| ch == 'é' || ch == '🦀'));
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

fn comments_enabled(f: &Factory, enabled: bool) {
    let mut s = f.settings().unwrap();
    s.issue_node_comments_enabled = enabled;
    f.save_settings(s).unwrap();
}
fn pending_row(root: &Path) -> (String, String, u32, String, Option<String>, Option<String>) {
    db(root)
        .query_row(
            "SELECT key,state,failures,next_at,last_error,receipt FROM issue_node_comments",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .unwrap()
}

#[tokio::test]
async fn issue_node_comments_disable_during_list_suppresses_post_and_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    configure(&f);
    let id = enqueue(&f, 41);
    f.set_run_status(&id, "failed", None).unwrap();
    let text = body(dir.path());
    let before = pending_row(dir.path());
    f.shutdown().await;
    drop(f);
    let holder: Arc<std::sync::Mutex<Option<Arc<Factory>>>> = Arc::default();
    let h = holder.clone();
    let paused = Arc::new(tokio::sync::Notify::new());
    let signal = paused.clone();
    let mut server = Script::start(vec![
        user(),
        list().then(move || {
            comments_enabled(h.lock().unwrap().as_ref().unwrap(), false);
            signal.notify_one();
        }),
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
    *holder.lock().unwrap() = Some(f.clone());
    f.start_notification_delivery();
    tokio::time::timeout(Duration::from_secs(5), paused.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(pending_row(dir.path()), before);
    assert!(server.writes().is_empty());
    assert_eq!(server.requests().len(), 2);
    let node = &f.run_detail(&id).unwrap().nodes[0].id;
    assert!(!f
        .logs(&id, node, 0)
        .unwrap()
        .text
        .contains("issue_node_comment_delivery_failed"));
    comments_enabled(&f, true);
    wait(
        dir.path(),
        "SELECT count(*) FROM issue_node_comments WHERE state='delivered'",
        1,
    )
    .await;
    f.shutdown().await;
    holder.lock().unwrap().take();
    server.assert_done();
    assert_eq!(server.writes().len(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn issue_node_comments_disabled_transition_has_no_job_or_http() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = Script::start(vec![]).await;
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    configure(&f);
    comments_enabled(&f, false);
    let id = enqueue(&f, 41);
    f.set_run_status(&id, "failed", None).unwrap();
    f.start_notification_delivery();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(f.run_detail(&id).unwrap().run.status, "failed");
    assert_eq!(f.run_detail(&id).unwrap().nodes[0].status, "skipped");
    assert_eq!(
        db(dir.path())
            .query_row("SELECT count(*) FROM issue_node_comments", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert!(server.requests().is_empty());
    comments_enabled(&f, true);
    f.shutdown().await;
    drop(f);
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    f.start_notification_delivery();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        db(dir.path())
            .query_row("SELECT count(*) FROM issue_node_comments", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
    f.shutdown().await;
    assert!(server.requests().is_empty());
    assert!(server.writes().is_empty());
    server.assert_done();
    server.shutdown().await;
}

#[tokio::test]
async fn issue_node_comments_paused_pending_resumes_once_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    configure(&f);
    let id = enqueue(&f, 41);
    f.set_run_status(&id, "failed", None).unwrap();
    let text = body(dir.path());
    let before = pending_row(dir.path());
    comments_enabled(&f, false);
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
    assert!(!f.settings().unwrap().issue_node_comments_enabled);
    f.start_notification_delivery();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(server.requests().is_empty());
    assert_eq!(pending_row(dir.path()), before);
    comments_enabled(&f, true);
    wait(
        dir.path(),
        "SELECT count(*) FROM issue_node_comments WHERE state='delivered'",
        1,
    )
    .await;
    let after = pending_row(dir.path());
    assert_eq!(after.0, before.0);
    assert_eq!(after.1, "delivered");
    assert_eq!(after.2, before.2);
    assert_eq!(
        db(dir.path())
            .query_row("SELECT count(*) FROM issue_node_comments", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
    f.shutdown().await;
    drop(f);
    server.assert_done();
    assert_eq!(server.writes().len(), 1);
    server.shutdown().await;
    let mut empty = Script::start(vec![]).await;
    let f = common::open_with_api(dir.path().into(), &empty.base).unwrap();
    f.start_notification_delivery();
    tokio::time::sleep(Duration::from_millis(700)).await;
    f.shutdown().await;
    empty.assert_done();
    assert!(empty.requests().is_empty());
    empty.shutdown().await;
}

#[tokio::test]
async fn issue_node_comments_disable_after_post_keeps_receipt() {
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
    let received = Arc::new(tokio::sync::Notify::new());
    let signal = received.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let mut server = Script::start(vec![
        user(),
        list(),
        Step::new(
            "POST",
            "/repos/acme/repo/issues/41/comments",
            Reply::Wait(release.clone(), receipt(&text)),
        )
        .body(json!({"body":text}))
        .then(move || {
            comments_enabled(h.lock().unwrap().as_ref().unwrap(), false);
            signal.notify_one();
        }),
    ])
    .await;
    let f = common::open_with_api(dir.path().into(), &server.base).unwrap();
    *holder.lock().unwrap() = Some(f.clone());
    f.start_notification_delivery();
    tokio::time::timeout(Duration::from_secs(5), received.notified())
        .await
        .unwrap();
    assert!(!f.settings().unwrap().issue_node_comments_enabled);
    assert_eq!(pending_row(dir.path()).1, "pending");
    release.notify_one();
    wait(
        dir.path(),
        "SELECT count(*) FROM issue_node_comments WHERE state='delivered'",
        1,
    )
    .await;
    let row = pending_row(dir.path());
    assert_eq!(row.2, 0);
    let receipt: Value = serde_json::from_str(row.5.as_deref().unwrap()).unwrap();
    assert_eq!(receipt["comment_id"], json!(9001));
    comments_enabled(&f, true);
    f.shutdown().await;
    holder.lock().unwrap().take();
    drop(f);
    server.assert_done();
    assert_eq!(server.writes().len(), 1);
    server.shutdown().await;
    let mut empty = Script::start(vec![]).await;
    let f = common::open_with_api(dir.path().into(), &empty.base).unwrap();
    f.start_notification_delivery();
    tokio::time::sleep(Duration::from_millis(700)).await;
    f.shutdown().await;
    empty.assert_done();
    assert!(empty.requests().is_empty());
    empty.shutdown().await;
}

#[tokio::test]
async fn issue_node_comments_paused_reconciliation_avoids_duplicate_post() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    configure(&f);
    let id = enqueue(&f, 41);
    f.set_run_status(&id, "failed", None).unwrap();
    let text = body(dir.path());
    let before = pending_row(dir.path());
    comments_enabled(&f, false);
    f.shutdown().await;
    drop(f);
    let mut server = Script::start(vec![
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
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(server.requests().is_empty());
    assert_eq!(pending_row(dir.path()), before);
    comments_enabled(&f, true);
    wait(
        dir.path(),
        "SELECT count(*) FROM issue_node_comments WHERE state='delivered'",
        1,
    )
    .await;
    assert_eq!(pending_row(dir.path()).0, before.0);
    assert_eq!(
        db(dir.path())
            .query_row("SELECT count(*) FROM issue_node_comments", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
    let raw = pending_row(dir.path()).5.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&raw).unwrap()["comment_id"],
        json!(9001)
    );
    f.shutdown().await;
    server.assert_done();
    assert!(server.writes().is_empty());
    server.shutdown().await;
}
