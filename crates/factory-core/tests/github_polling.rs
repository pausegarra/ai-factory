mod common;
use factory_core::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
async fn server(handler: impl Fn(&str, &str) -> (u16, Value) + Send + Sync + 'static) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let h = Arc::new(handler);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let h = h.clone();
            tokio::spawn(async move {
                let mut b = vec![0; 65536];
                let n = stream.read(&mut b).await.unwrap();
                let request = String::from_utf8_lossy(&b[..n]);
                let path = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let (status, body) = h(path, &request);
                let body = body.to_string();
                let response=format!("HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
            });
        }
    });
    format!("http://{address}")
}
fn setup(root: std::path::PathBuf, base: &str) -> Arc<Factory> {
    let f = common::open_with_api(root, base).unwrap();
    let mut s = f.settings().unwrap();
    s.owners = vec!["acme".into()];
    s.authors = vec!["alice".into()];
    s.token_profiles = vec![TokenProfile {
        id: "alice".into(),
        account: "alice".into(),
        label: "Alice".into(),
        token: Some("alice-token".into()),
        has_token: false,
    }];
    f.save_settings(s).unwrap();
    f
}
fn pr() -> Value {
    json!({"number":5,"title":"PR title","body":"Closes #1","state":"open","html_url":"https://github.com/acme/repo/pull/5","base":{"repo":{"full_name":"acme/repo"}},"head":{"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","ref":"feature"},"user":{"login":"alice"},"requested_reviewers":[{"login":"alice"}]})
}
#[tokio::test]
async fn issue_prefix_and_refresh_checked_locally_with_actor_token() {
    let refreshed = Arc::new(Mutex::new(false));
    let state = refreshed.clone();
    let base=server(move|path,req|{assert!(req.to_lowercase().contains("authorization: bearer alice-token"));if path=="/user"{return(200,json!({"login":"alice"}))}
if path.starts_with("/search/issues"){return(200,json!({"incomplete_results":false,"total_count":2,"items":[{"number":1,"repository_url":"https://api.github.com/repos/acme/repo"},{"number":2,"repository_url":"https://api.github.com/repos/acme/repo","pull_request":{}}]}))}
if path=="/repos/acme/repo/issues/1"{*state.lock().unwrap()=true;return(200,json!({"state":"open","title":"[ai] wrong case","user":{"login":"alice"},"html_url":"https://github.com/acme/repo/issues/1"}))}panic!("unexpected {path}")}).await;
    let dir = tempfile::tempdir().unwrap();
    let f = setup(dir.path().into(), &base);
    f.poll_now().await.unwrap();
    assert!(*refreshed.lock().unwrap());
    assert_eq!(f.snapshot().unwrap().metrics.total, 0);
}
#[tokio::test]
async fn approval_requires_current_head_and_consumes_comment_across_heads() {
    let mode = Arc::new(Mutex::new((false, false)));
    let state = mode.clone();
    let base=server(move|path,_|{if path=="/user"{return(200,json!({"login":"alice"}))}
if path.starts_with("/search/"){return(200,json!({"incomplete_results":false,"total_count":0,"items":[]}))}
if path.ends_with("/reviews?per_page=100&page=1"){return(200,json!([{"id":1,"state":"APPROVED","commit_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]))}
if path.contains("/comments?"){return(200,json!([{"id":7,"body":" OK ","user":{"login":"alice"}}]))}
if path=="/repos/acme/repo/pulls/5"{let mut p=pr();if state.lock().unwrap().0 {p["head"]["sha"]=json!("cccccccccccccccccccccccccccccccccccccccc")}return(200,p)}panic!("unexpected {path}")}).await;
    let dir = tempfile::tempdir().unwrap();
    let f = setup(dir.path().into(), &base);
    let mut settings = f.settings().unwrap();
    settings.flows[1].nodes[0].token_overrides.insert(
        "alice".into(),
        AgentConfig {
            agent: "opencode".into(),
            model: "provider/approval".into(),
            effort: "deep".into(),
        },
    );
    f.save_settings(settings).unwrap();
    let source = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            1,
            "[AI] issue",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    let store = factory_core::context::factory::infrastructure::repositories::store::Store::open(
        dir.path(),
    )
    .unwrap();
    store.track_pr("acme/repo", 5, &source).unwrap();
    f.poll_now().await.unwrap();
    assert_eq!(f.snapshot().unwrap().metrics.total, 2);
    let approval = f
        .snapshot()
        .unwrap()
        .runs
        .into_iter()
        .find(|run| run.trigger_id == "pr_approved")
        .unwrap();
    let approval = f.run_detail(&approval.id).unwrap();
    assert!(approval.event.payload.get("source_run").is_none());
    assert_eq!(
        approval.event.payload["source_issue"]
            .as_object()
            .unwrap()
            .len(),
        6
    );
    assert_eq!(approval.nodes[0].agent, "opencode");
    assert_eq!(approval.flow.nodes[0].model, "provider/approval");
    f.poll_now().await.unwrap();
    assert_eq!(f.snapshot().unwrap().metrics.total, 2);
    mode.lock().unwrap().0 = true;
    f.poll_now().await.unwrap();
    assert_eq!(
        f.snapshot().unwrap().metrics.total,
        2,
        "Old OK must not approve new head"
    );
}
#[tokio::test]
async fn review_requests_use_durable_rising_edges_and_do_not_clear_on_search_error() {
    let mode = Arc::new(Mutex::new(0u8));
    let state = mode.clone();
    let base=server(move|path,req|{assert!(req.to_lowercase().contains("bearer alice-token"));if path=="/user"{return(200,json!({"login":"alice"}))}
if path=="/repos/acme/repo/pulls/5"{return(200,pr())}
if path.starts_with("/search/"){let m=*state.lock().unwrap();if m==2{return(503,json!({}))}return(200,json!({"incomplete_results":false,"total_count":if m==1{0}else{1},"items":if m==1{vec![]}else{vec![json!({"number":5,"repository_url":"https://api.github.com/repos/acme/repo"})]}}))}panic!("unexpected {path}")}).await;
    let dir = tempfile::tempdir().unwrap();
    let f = setup(dir.path().into(), &base);
    let mut s = f.settings().unwrap();
    s.authors.clear();
    s.reviewers = vec!["alice".into()];
    s.flows[2].nodes[0].token_overrides.insert(
        "alice".into(),
        AgentConfig {
            agent: "opencode".into(),
            model: "provider/review".into(),
            effort: "fast".into(),
        },
    );
    f.save_settings(s).unwrap();
    f.poll_now().await.unwrap();
    f.poll_now().await.unwrap();
    assert_eq!(f.snapshot().unwrap().metrics.total, 1);
    let review = f.run_detail(&f.snapshot().unwrap().runs[0].id).unwrap();
    assert_eq!(review.nodes[0].agent, "opencode");
    assert_eq!(review.flow.nodes[0].model, "provider/review");
    *mode.lock().unwrap() = 2;
    assert!(f.poll_now().await.is_err());
    *mode.lock().unwrap() = 0;
    f.poll_now().await.unwrap();
    assert_eq!(f.snapshot().unwrap().metrics.total, 1);
    *mode.lock().unwrap() = 1;
    f.poll_now().await.unwrap();
    *mode.lock().unwrap() = 0;
    f.poll_now().await.unwrap();
    assert_eq!(f.snapshot().unwrap().metrics.total, 2);
}

#[tokio::test]
async fn deleted_source_origin_polls_without_history_and_revalidates_authorization() {
    for mode in [
        "valid",
        "actor_revoked",
        "owner_revoked",
        "wrong_token",
        "changed_author",
        "old_head",
    ] {
        let base=server(move |path,_| {
            if path=="/user" {return (200,json!({"login":if mode=="wrong_token" {"mallory"} else {"alice"}}));}
            if path=="/repos/acme/repo/pulls/5" {let mut p=pr();if mode=="changed_author" {p["user"]["login"]=json!("mallory");}return (200,p);}
            if path.contains("/reviews?") {return (200,json!([{"state":"APPROVED","commit_id":if mode=="old_head" {"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"} else {"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}]));}
            if path.contains("/comments?") {return (200,json!([]));}
            panic!("unexpected {mode}: {path}");
        }).await;
        let dir = tempfile::tempdir().unwrap();
        let f = setup(dir.path().into(), &base);
        let source = f
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                1,
                "[AI] origin",
                "alice",
                "private-body",
            ))
            .unwrap()
            .unwrap();
        let store =
            factory_core::context::factory::infrastructure::repositories::store::Store::open(
                dir.path(),
            )
            .unwrap();
        store.track_pr("acme/repo", 5, &source).unwrap();
        // Model finalized local deletion: operational origin must not read any source row.
        let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
        db.execute("DELETE FROM nodes WHERE run_id=?1", [&source])
            .unwrap();
        db.execute("DELETE FROM runs WHERE id=?1", [&source])
            .unwrap();
        let mut s = f.settings().unwrap();
        s.flows[0].enabled = false;
        if mode == "actor_revoked" {
            s.authors.clear();
        }
        if mode == "owner_revoked" {
            s.owners = vec!["other-owner".into()];
        }
        f.save_settings(s).unwrap();
        let result = f.poll_now().await;
        assert_eq!(
            result.is_ok(),
            matches!(mode, "valid" | "old_head"),
            "{mode}: {result:?}"
        );
        assert_eq!(
            f.snapshot().unwrap().runs.len(),
            usize::from(mode == "valid"),
            "{mode}"
        );
        if mode == "valid" {
            let run = &f.snapshot().unwrap().runs[0];
            let detail = f.run_detail(&run.id).unwrap();
            assert!(detail.event.payload.get("source_run").is_none());
            let text = detail.event.payload["source_issue"].to_string();
            assert!(!text.contains("private-body"));
            assert!(!text.contains(&source));
        }
    }
}
