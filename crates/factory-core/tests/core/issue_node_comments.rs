use super::{fixture, terminal, FixtureDir};
use factory_core::*;
use serde_json::json;
use std::time::Duration;
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
