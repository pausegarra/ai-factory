use super::{fixture, terminal, FixtureDir};
use factory_core::*;
use serde_json::json;
use std::time::Duration;

pub(super) fn committed_comment_snapshot(db: &rusqlite::Connection, body: &str) -> bool {
    let Some(key) = body
        .lines()
        .last()
        .and_then(|line| line.strip_prefix("<!-- ai-factory:"))
        .and_then(|line| line.strip_suffix(" -->"))
        .filter(|key| key.starts_with("node-result:v1:"))
    else {
        return false;
    };
    // Another connection can see this row only after the terminal transition commits.
    // Reviews may already have requeued the current node by the time its comment arrives.
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM issue_node_comments
         WHERE key=?1
         AND json_extract(snapshot,'$.run_id')=run_id
         AND json_extract(snapshot,'$.node_id')=node_id
         AND json_extract(snapshot,'$.revision')=revision
         AND json_extract(snapshot,'$.status') IN ('succeeded','blocked','failed','skipped'))",
        [key],
        |r| r.get(0),
    )
    .unwrap()
}

fn snapshot_fixture() -> (tempfile::TempDir, rusqlite::Connection, String) {
    let dir = tempfile::tempdir().unwrap();
    let db = rusqlite::Connection::open(dir.path().join("snapshot.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE nodes(run_id TEXT,node_id TEXT,json TEXT);
        CREATE TABLE issue_node_comments(key TEXT,run_id TEXT,node_id TEXT,revision INTEGER,snapshot TEXT);
        INSERT INTO nodes VALUES('run','builder','{\"status\":\"pending\"}');
        INSERT INTO issue_node_comments VALUES('node-result:v1:known','run','builder',1,
            '{\"run_id\":\"run\",\"node_id\":\"builder\",\"revision\":1,\"status\":\"succeeded\"}');").unwrap();
    (
        dir,
        db,
        "Result\n\n<!-- ai-factory:node-result:v1:known -->".into(),
    )
}

#[test]
fn committed_comment_snapshot_survives_current_node_requeue() {
    let (_dir, db, body) = snapshot_fixture();
    assert!(committed_comment_snapshot(&db, &body));
}

#[test]
fn committed_comment_snapshot_requires_matching_marker_and_revision() {
    let (_dir, db, body) = snapshot_fixture();
    db.execute("UPDATE nodes SET json='{\"status\":\"succeeded\"}'", [])
        .unwrap();
    assert!(!committed_comment_snapshot(
        &db,
        &body.replace("known", "unknown")
    ));
    assert!(!committed_comment_snapshot(&db, "No marker"));
    db.execute(
        "UPDATE issue_node_comments SET snapshot=json_set(snapshot,'$.revision',2)",
        [],
    )
    .unwrap();
    assert!(!committed_comment_snapshot(&db, &body));
}

#[test]
fn committed_comment_snapshot_rejects_nonterminal_or_uncommitted_snapshot() {
    let (dir, db, body) = snapshot_fixture();
    db.execute("UPDATE nodes SET json='{\"status\":\"succeeded\"}'", [])
        .unwrap();
    db.execute(
        "UPDATE issue_node_comments SET snapshot=json_set(snapshot,'$.status','running')",
        [],
    )
    .unwrap();
    assert!(!committed_comment_snapshot(&db, &body));
    db.execute("DELETE FROM issue_node_comments", []).unwrap();
    let writer = rusqlite::Connection::open(dir.path().join("snapshot.sqlite3")).unwrap();
    writer.execute_batch("BEGIN;
        INSERT INTO issue_node_comments VALUES('node-result:v1:known','run','builder',1,
            '{\"run_id\":\"run\",\"node_id\":\"builder\",\"revision\":1,\"status\":\"succeeded\"}');").unwrap();
    assert!(!committed_comment_snapshot(&db, &body));
    writer.execute_batch("COMMIT").unwrap();
    assert!(committed_comment_snapshot(&db, &body));
}

#[tokio::test]
async fn issue_node_comments_deliver_previous_round_while_correction_is_running() {
    let script = r#"set -eu
cat >/dev/null
memory="$AI_FACTORY_MEMORY_DIR"
status=succeeded
data='{}'
case "$AI_FACTORY_NODE_ID" in
  builder)
    if [ -f "$memory/builder-first" ]; then
      : > "$memory/correction-started"
      while [ ! -f "$memory/release-correction" ]; do sleep 0.02; done
    else
      : > "$memory/builder-first"
    fi
    ;;
  review_tests)
    data='{"approved":true}'
    if [ ! -f "$memory/review-first" ]; then
      : > "$memory/review-first"
      status=blocked
      data='{"approved":false}'
    fi
    ;;
esac
printf '{"status":"%s","summary":"Round complete","artifacts":[],"data":%s}' "$status" "$data" > "$AI_FACTORY_RESULT_PATH"
"#;
    let (dir, factory) = fixture(script, 4);
    let mut settings = factory.settings().unwrap();
    let template = settings.flows[0].nodes[0].clone();
    settings.flows[0].nodes = ["builder", "review_tests", "finish"]
        .into_iter()
        .enumerate()
        .map(|(index, id)| {
            let mut node = template.clone();
            node.id = id.into();
            node.name = id.into();
            node.depends_on = match index {
                0 => vec![],
                1 => vec!["builder".into()],
                _ => vec!["review_tests".into()],
            };
            if id == "review_tests" {
                node.access = "read".into();
                node.result_schema = json!({"type":"object","required":["approved"],"properties":{"approved":{"type":"boolean"}}});
            }
            node
        }).collect();
    factory.save_settings(settings).unwrap();
    dir.comment_status
        .store(503, std::sync::atomic::Ordering::SeqCst);
    let id = enqueue(&factory);
    let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
    factory.start();
    let delivery = tokio::time::timeout(Duration::from_secs(10), async {
        while !memory.join("correction-started").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let before = factory.run_detail(&id).unwrap();
        dir.comment_status
            .store(200, std::sync::atomic::Ordering::SeqCst);
        connection(&dir)
            .execute(
                "UPDATE issue_node_comments SET next_at='2000-01-01T00:00:00Z'",
                [],
            )
            .unwrap();
        loop {
            let count: u32 = connection(&dir)
                .query_row(
                    "SELECT count(*) FROM issue_node_comments WHERE state='delivered'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if count == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        (before, factory.run_detail(&id).unwrap(), bodies(&dir))
    })
    .await;
    // Release the fake process even if the delivery deadline failed.
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(memory.join("release-correction"), "").unwrap();
    let final_detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    let (before, after, messages) =
        delivery.expect("previous round comments delivered during correction");
    for detail in [before, after] {
        assert!(detail
            .nodes
            .iter()
            .all(|node| matches!(node.status.as_str(), "pending" | "running")));
        assert_eq!(
            detail
                .nodes
                .iter()
                .find(|node| node.id == "builder")
                .unwrap()
                .status,
            "running"
        );
    }
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|body| body.contains("result 1")));
    assert_eq!(final_detail.run.status, "succeeded");
}

fn enqueue(f: &Factory) -> String {
    f.enqueue(TriggerEvent::issue(
        "acme/repo",
        41,
        "[AI] results",
        "alice",
        "",
    ))
    .unwrap()
    .unwrap()
}
fn connection(dir: &FixtureDir) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.path().join("data/factory.sqlite3")).unwrap()
}
pub(super) async fn delivered(dir: &FixtureDir, count: u32) {
    tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            let actual: u32 = connection(dir)
                .query_row(
                    "SELECT count(*) FROM issue_node_comments WHERE state='delivered'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if actual == count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("all comments delivered");
}
fn bodies(dir: &FixtureDir) -> Vec<String> {
    dir.requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(m, p, _)| m == "POST" && p == "/repos/acme/repo/issues/41/comments")
        .map(|(_, _, b)| b["body"].as_str().unwrap().into())
        .collect()
}
const SUCCESS: &str = r#"cat >/dev/null
printf '%s' '{"status":"succeeded","summary":"Pruebas verificadas","artifacts":[],"data":{"repository":"evil/repo","number":999,"private":"DO_NOT_PUBLISH"}}' > "$AI_FACTORY_RESULT_PATH""#;
#[tokio::test]
async fn issue_node_comments_success_and_blocked_ignore_agent_destination() {
    for (status, access) in [
        ("succeeded", "write"),
        ("succeeded", "read"),
        ("blocked", "write"),
    ] {
        let (dir, f) = fixture(&SUCCESS.replace("succeeded", status), 4);
        let mut s = f.settings().unwrap();
        s.flows[0].nodes[0].access = access.into();
        s.flows[0].nodes[0].result_schema = json!({"type":"object"});
        f.save_settings(s).unwrap();
        let id = enqueue(&f);
        f.start();
        let d = terminal(&f, &id).await;
        delivered(&dir, 1).await;
        f.shutdown().await;
        assert_eq!(d.nodes[0].status, status, "{d:?}");
        let messages = bodies(&dir);
        assert_eq!(messages.len(), 1);
        let body = &messages[0];
        assert!(body.contains("Pruebas verificadas"));
        assert!(body.contains(status));
        for private in ["evil/repo", "DO_NOT_PUBLISH", "\"number\":999"] {
            assert!(!body.contains(private));
        }
        assert!(body.contains(&d.nodes[0].name));
    }
}
#[tokio::test]
async fn issue_node_comments_preparation_failure_skips_root_without_dependency_claim() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, f) = fixture(
        r#"cat >/dev/null
: > "$(dirname "$0")/agent-started"
exit 17"#,
        4,
    );
    let mut s = f.settings().unwrap();
    assert_eq!(s.flows[0].nodes.len(), 1);
    assert!(s.flows[0].nodes[0].depends_on.is_empty());
    let git = dir.path().join("fail-prepare");
    std::fs::write(
        &git,
        r#"#!/bin/sh
if [ "$1" = clone ]; then echo 'fixture preparation failure' >&2; exit 19; fi
exit 0
"#,
    )
    .unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o700)).unwrap();
    s.commands.git = git.to_string_lossy().into();
    f.save_settings(s).unwrap();
    let id = enqueue(&f);
    f.start();
    terminal(&f, &id).await;
    delivered(&dir, 1).await;
    let d = f.run_detail(&id).unwrap();
    f.shutdown().await;
    assert_eq!(d.run.status, "failed");
    assert!(d.run.error.unwrap().contains("fixture preparation failure"));
    assert_eq!(d.nodes.len(), 1);
    let n = &d.nodes[0];
    assert_eq!(n.status, "skipped");
    assert!(n.error.is_none());
    assert!(n.started_at.is_none());
    assert!(n.result.is_none());
    assert!(!dir.path().join("agent-started").exists());
    let messages = bodies(&dir);
    assert_eq!(messages.len(), 1);
    let b = &messages[0];
    assert!(b.contains("skipped"));
    assert!(b.contains("Step was not executed."));
    assert!(!b.to_lowercase().contains("dependency"));
    assert!(!b.to_lowercase().contains("dependencia"));
}
#[tokio::test]
async fn issue_node_comments_dependency_skip_preserves_recorded_reason() {
    let (dir, f) = fixture("cat >/dev/null\nexit 17", 4);
    let mut s = f.settings().unwrap();
    let mut child = s.flows[0].nodes[0].clone();
    child.id = "child".into();
    child.name = "Child".into();
    child.depends_on = vec![s.flows[0].nodes[0].id.clone()];
    s.flows[0].nodes.push(child);
    f.save_settings(s).unwrap();
    let id = enqueue(&f);
    f.start();
    terminal(&f, &id).await;
    delivered(&dir, 2).await;
    let d = f.run_detail(&id).unwrap();
    f.shutdown().await;
    assert_eq!(d.nodes[0].status, "failed");
    let child = d.nodes.iter().find(|n| n.id == "child").unwrap();
    assert_eq!(child.status, "skipped");
    assert_eq!(child.error.as_deref(), Some("Dependency did not succeed"));
    let messages = bodies(&dir);
    assert_eq!(messages.len(), 2);
    let child = messages.iter().find(|b| b.contains("(child)")).unwrap();
    assert!(child.contains("skipped"));
    assert!(child.contains("Step was not executed."));
    assert!(child.contains("Dependency did not succeed"));
}
#[tokio::test]
async fn issue_node_comments_invalid_json_and_explicit_action_coexistence() {
    let explicit = r#"cat >/dev/null
printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{"comment":"Comentario explícito"}}' > "$AI_FACTORY_RESULT_PATH""#;
    for (script, access, status, count) in [
        (
            r#"cat >/dev/null
printf '%s' 'invalid' > "$AI_FACTORY_RESULT_PATH""#,
            "write",
            "failed",
            1,
        ),
        (explicit, "write", "succeeded", 2),
        (explicit, "read", "failed", 1),
    ] {
        let (dir, f) = fixture(script, 4);
        let mut s = f.settings().unwrap();
        s.flows[0].nodes[0].result_schema = json!({"type":"object"});
        s.flows[0].nodes[0].access = access.into();
        f.save_settings(s).unwrap();
        let id = enqueue(&f);
        f.start();
        let d = terminal(&f, &id).await;
        delivered(&dir, 1).await;
        f.shutdown().await;
        assert_eq!(d.nodes[0].status, status, "{:?}", d.nodes);
        let messages = bodies(&dir);
        assert_eq!(messages.len(), count);
        assert_eq!(
            messages
                .iter()
                .filter(|b| b.contains("node-result:v1:"))
                .count(),
            1
        );
        if count == 2 {
            assert!(messages
                .iter()
                .any(|b| b.contains("Comentario explícito") && !b.contains("node-result:v1:")));
        }
    }
}

#[tokio::test]
async fn issue_node_comments_disabled_preserves_explicit_action() {
    let script = r#"cat >/dev/null
printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{"comment":"Comentario explícito"}}' > "$AI_FACTORY_RESULT_PATH""#;
    let (dir, f) = fixture(script, 4);
    let mut s = f.settings().unwrap();
    s.issue_node_comments_enabled = false;
    s.flows[0].nodes[0].result_schema = json!({"type":"object"});
    s.flows[0].nodes[0].access = "write".into();
    f.save_settings(s).unwrap();
    let id = enqueue(&f);
    f.start();
    let detail = terminal(&f, &id).await;
    f.shutdown().await;
    assert_eq!(detail.nodes[0].status, "succeeded");
    let messages = bodies(&dir);
    assert_eq!(messages.len(), 1);
    assert!(messages[0].contains("Comentario explícito"));
    assert!(!messages[0].contains("node-result:v1:"));
    assert_eq!(
        connection(&dir)
            .query_row("SELECT count(*) FROM issue_node_comments", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
}
