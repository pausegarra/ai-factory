#![cfg(unix)]
#[path = "common/actions_http.rs"]
mod actions_http;
mod common;
use actions_http::{Reply, Script, Step};
use factory_core::{context::factory::infrastructure::rest_clients::github::Github, *};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn server(
    handler: impl Fn(&str, &str, &Value) -> (u16, Value) + Send + Sync + 'static,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let h = handler.clone();
            tokio::spawn(async move {
                let mut all = Vec::new();
                let (head, length) = loop {
                    let mut bytes = [0; 4096];
                    let n = socket.read(&mut bytes).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    all.extend_from_slice(&bytes[..n]);
                    if let Some(end) = all.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&all[..end]);
                        let length = header
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while all.len() < head + length {
                    let mut bytes = [0; 4096];
                    let n = socket.read(&mut bytes).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    all.extend_from_slice(&bytes[..n]);
                }
                let header = String::from_utf8_lossy(&all[..head]);
                let mut line = header.lines().next().unwrap().split_whitespace();
                let method = line.next().unwrap();
                let path = line.next().unwrap();
                let body = serde_json::from_slice(&all[head..]).unwrap_or(Value::Null);
                let (status, data) = h(method, path, &body);
                let data = data.to_string();
                let response=format!("HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",data.len());
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    base
}
fn event(trigger: &str) -> TriggerEvent {
    let mut e = TriggerEvent::issue("acme/repo", 1, "[AI] work", "alice", "");
    e.trigger_id = trigger.into();
    if trigger != "new_issue" {
        e.event_id = format!("{trigger}:acme/repo#5@{}", "a".repeat(40));
        e.number = 5;
        e.head_sha = "a".repeat(40);
        e.url = "https://github.com/acme/repo/pull/5".into();
    }
    e
}
fn pr(e: &TriggerEvent, branch: &str, sha: &str) -> Value {
    json!({"number":5,"title":"PR","body":"Closes #1",
        "html_url":"https://github.com/acme/repo/pull/5","state":"open",
        "merged":false,"merged_at":null,
        "base":{"ref":"main","repo":{"full_name":"acme/repo"}},
        "head":{"sha":sha,"ref":branch,"repo":{"full_name":"acme/repo"}},
        "user":{"login":e.actor}})
}
fn factory(base: &str) -> (tempfile::TempDir, Arc<Factory>) {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().join("app"), base).unwrap();
    let mut s = f.settings().unwrap();
    s.token_profiles = vec![TokenProfile {
        id: "alice".into(),
        account: "alice".into(),
        label: "Alice".into(),
        token: Some("alice-token".into()),
        has_token: false,
        ..Default::default()
    }];
    f.save_settings(s).unwrap();
    (dir, f)
}
fn result(data: Value) -> Value {
    json!({"status":"succeeded","summary":"done","artifacts":[],"data":data})
}
// Action fixtures must own a live durable run; missing runs now fail closed.
fn seed_run(f: &Factory, e: &TriggerEvent, id: &str) {
    let root = f.snapshot().unwrap().service.app_data_dir;
    let db = rusqlite::Connection::open(Path::new(&root).join("factory.sqlite3")).unwrap();
    let flow = f
        .settings()
        .unwrap()
        .flows
        .into_iter()
        .find(|flow| flow.id == e.trigger_id)
        .unwrap();
    let run = json!({"id":id,"event_id":e.event_id,"trigger_id":e.trigger_id,"repository":e.repository,"number":e.number,"title":e.title,"actor":e.actor,"url":e.url,"status":"queued","current_step":null,"created_at":now(),"started_at":null,"finished_at":null,"error":null,"attempt":1,"workspace":null});
    db.execute(
        "INSERT OR IGNORE INTO runs VALUES(?1,?2,?3,?4,?5,?6)",
        rusqlite::params![
            id,
            e.event_id,
            run.to_string(),
            serde_json::to_string(e).unwrap(),
            serde_json::to_string(&flow).unwrap(),
            now()
        ],
    )
    .unwrap();
}

#[tokio::test]
async fn release_action_requires_explicit_comment_authorization_before_any_http() {
    let (_dir, f) = factory("http://127.0.0.1:9");
    let e = event("pr_approved");
    seed_run(&f, &e, "release-denied");
    let api = Github::with_base("alice-token", "http://127.0.0.1:9").unwrap();
    let error = api.publish_actions(&f, &e, "release-denied", "factory_release", Path::new("."),
        &result(json!({"release":{"tag":"1.2.3","title":"1.2.3","body":"Fix","bump":"patch","reason":"bug fix"}})))
        .await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Release requires an authorized release comment"),
        "{error:#}"
    );
}

#[tokio::test]
async fn release_action_is_reserved_for_its_own_node_and_cannot_combine_actions() {
    let (_dir, f) = factory("http://127.0.0.1:9");
    let mut e = event("pr_approved");
    e.payload = json!({"approval":{"command":"release","actor":e.actor,"head_sha":e.head_sha,"comment_id":9}});
    seed_run(&f, &e, "release-node");
    let api = Github::with_base("alice-token", "http://127.0.0.1:9").unwrap();
    for (node, data, message) in [
        (
            "publish",
            json!({"release":{}}),
            "Only the assigned release node can publish a release",
        ),
        (
            "factory_release",
            json!({"release":{},"merge":{}}),
            "release cannot be combined with other actions",
        ),
    ] {
        let error = api
            .publish_actions(&f, &e, "release-node", node, Path::new("."), &result(data))
            .await
            .unwrap_err();
        assert!(error.to_string().contains(message), "{error:#}");
    }
}

#[tokio::test]
async fn request_accepts_empty_204_and_keeps_json_contract() {
    let script = Script::start(vec![
        Step::new("DELETE", "/empty", Reply::Raw(204, vec![])),
        Step::new("GET", "/json", Reply::Json(200, json!({"ok":true}))),
        Step::new("POST", "/created", Reply::Json(201, json!({"id":1}))),
        Step::new("GET", "/bad-json", Reply::Raw(200, b"broken".to_vec())),
        Step::new("GET", "/empty-200", Reply::Raw(200, vec![])),
        Step::new(
            "GET",
            "/denied",
            Reply::Json(403, json!({"message":"alice-token"})),
        ),
    ])
    .await;
    let api = Github::with_base("alice-token", &script.base).unwrap();
    assert_eq!(
        api.request(reqwest::Method::DELETE, "/empty", None)
            .await
            .unwrap(),
        Value::Null
    );
    assert_eq!(
        api.request(reqwest::Method::GET, "/json", None)
            .await
            .unwrap(),
        json!({"ok":true})
    );
    assert_eq!(
        api.request(reqwest::Method::POST, "/created", None)
            .await
            .unwrap(),
        json!({"id":1})
    );
    assert!(api
        .request(reqwest::Method::GET, "/bad-json", None)
        .await
        .is_err());
    assert!(api
        .request(reqwest::Method::GET, "/empty-200", None)
        .await
        .is_err());
    let error = api
        .request(reqwest::Method::GET, "/denied", None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("403"));
    assert!(!format!("{error:#}").contains("alice-token"));
    assert!(api
        .request(reqwest::Method::GET, "//elsewhere", None)
        .await
        .is_err());
    script.assert_done();
}
fn git(path: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().into()
}
fn workspace(root: &Path, e: &TriggerEvent) -> (PathBuf, String, String) {
    use sha2::{Digest, Sha256};
    let branch = format!(
        "ai-factory/{}-{}",
        e.number,
        &format!("{:x}", Sha256::digest(e.event_id.as_bytes()))[..12]
    );
    let path = root.join("workspace");
    std::fs::create_dir(&path).unwrap();
    git(&path, &["init", "-b", &branch]);
    git(
        &path,
        &[
            "-c",
            "user.name=Alice",
            "-c",
            "user.email=alice@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    let sha = git(&path, &["rev-parse", "HEAD"]);
    (path, branch, sha)
}

const PR_PATH: &str = "/repos/acme/repo/pulls/5";
// Strict scripts reject every unexpected write, including target/default DELETEs.
const REPO_PATH: &str = "/repos/acme/repo";
const MERGE_PATH: &str = "/repos/acme/repo/pulls/5/merge";
const SOURCE_GET: &str = "/repos/acme/repo/git/ref/heads/feature";
const SOURCE_DELETE: &str = "/repos/acme/repo/git/refs/heads/feature";

fn repo_value(default: &str) -> Value {
    json!({"full_name":"acme/repo", "default_branch":default})
}
fn reference(branch: &str, sha: &str) -> Value {
    json!({"ref":format!("refs/heads/{branch}"),"object":{"type":"commit","sha":sha}})
}
fn merged_pr(mut pr: Value) -> Value {
    pr["state"] = json!("closed");
    pr["merged"] = json!(true);
    pr["merged_at"] = json!("2026-11-11T18:00:00Z");
    pr
}
fn preflight(pr: Value, default: &str) -> Vec<Step> {
    vec![
        Step::new("GET", "/user", Reply::Json(200, json!({"login":"alice"}))),
        Step::new("GET", PR_PATH, Reply::Json(200, pr)),
        Step::new("GET", REPO_PATH, Reply::Json(200, repo_value(default))),
    ]
}
fn confirmed(pr: Value, default: &str) -> Vec<Step> {
    vec![
        Step::new("GET", PR_PATH, Reply::Json(200, pr)),
        Step::new("GET", REPO_PATH, Reply::Json(200, repo_value(default))),
    ]
}
fn absence_witness() -> Vec<Step> {
    vec![
        Step::new("GET", REPO_PATH, Reply::Json(200, repo_value("main"))),
        Step::new(
            "GET",
            "/repos/acme/repo/git/ref/heads/main",
            Reply::Json(200, reference("main", &"b".repeat(40))),
        ),
    ]
}
async fn merge_attempt(
    api: &Github,
    f: &Factory,
    e: &TriggerEvent,
    root: &Path,
    run: &str,
) -> factory_core::Result<()> {
    seed_run(f, e, run);
    api.publish_actions(
        f,
        e,
        run,
        "merge",
        root,
        &result(json!({"merge":{"method":"squash"}})),
    )
    .await
}
async fn execute_merge(e: &TriggerEvent, steps: Vec<Step>) -> (factory_core::Result<()>, Script) {
    let script = Script::start(steps).await;
    let (dir, f) = factory(&script.base);
    let api = Github::with_base("alice-token", &script.base).unwrap();
    let outcome = merge_attempt(&api, &f, e, dir.path(), "test-attempt").await;
    (outcome, script)
}

fn admit_then_disable(f: &Factory, e: &TriggerEvent) -> String {
    let id = f.enqueue(e.clone()).unwrap().unwrap();
    let before = serde_json::to_value(f.run_detail(&id).unwrap()).unwrap();
    let mut s = f.settings().unwrap();
    for flow in &mut s.flows {
        flow.enabled = false;
        flow.profile_activation.insert("alice".into(), false);
    }
    f.save_settings(s).unwrap();
    assert_eq!(
        serde_json::to_value(f.run_detail(&id).unwrap()).unwrap(),
        before
    );
    id
}

#[tokio::test]
async fn merge_deletes_only_source_after_confirmation_for_each_target() {
    for target in ["main", "develop"] {
        let e = event("pr_approved");
        let mut open = pr(&e, "feature", &e.head_sha);
        open["base"]["ref"] = json!(target);
        let mut steps = preflight(open.clone(), "main");
        steps.push(
            Step::new("PUT", MERGE_PATH, Reply::Json(200, json!({"merged":true})))
                .body(json!({"merge_method":"squash","sha":e.head_sha})),
        );
        steps.extend(confirmed(merged_pr(open), "main"));
        steps.push(Step::new(
            "GET",
            SOURCE_GET,
            Reply::Json(200, reference("feature", &e.head_sha)),
        ));
        steps.push(Step::new("DELETE", SOURCE_DELETE, Reply::Raw(204, vec![])));
        let script = Script::start(steps).await;
        let (dir, f) = factory(&script.base);
        let id = admit_then_disable(&f, &e);
        let api = Github::with_base("alice-token", &script.base).unwrap();
        let outcome = merge_attempt(&api, &f, &e, dir.path(), &id).await;
        outcome.unwrap();
        script.assert_done();
        assert_eq!(
            script.writes(),
            vec![
                ("PUT".into(), MERGE_PATH.into()),
                ("DELETE".into(), SOURCE_DELETE.into())
            ]
        );
    }
}

#[tokio::test]
async fn merge_reconciles_already_merged_pull_request_without_second_merge() {
    let e = event("pr_approved");
    let closed = merged_pr(pr(&e, "feature", &e.head_sha));
    let mut steps = preflight(closed.clone(), "main");
    steps.extend(confirmed(closed, "main"));
    steps.push(Step::new(
        "GET",
        SOURCE_GET,
        Reply::Json(200, reference("feature", &e.head_sha)),
    ));
    steps.push(Step::new("DELETE", SOURCE_DELETE, Reply::Raw(204, vec![])));
    let (outcome, script) = execute_merge(&e, steps).await;
    outcome.unwrap();
    script.assert_done();
    assert_eq!(
        script.writes(),
        vec![("DELETE".into(), SOURCE_DELETE.into())]
    );
}

#[tokio::test]
async fn comment_recovers_accepted_write_after_lost_response_without_duplicate() {
    let writes = Arc::new(AtomicUsize::new(0));
    let saved = Arc::new(Mutex::new(None::<Value>));
    let w = writes.clone();
    let state = saved.clone();
    let base = server(move |method, path, body| {
        if path == "/user" {
            return (200, json!({"login":"alice"}));
        }
        if method == "POST" {
            w.fetch_add(1, Ordering::SeqCst);
            *state.lock().unwrap() =
                Some(json!({"id":12,"body":body["body"],"user":{"login":"alice"}}));
            return (503, json!({"message":"lost response"}));
        }
        if path.contains("/comments?") {
            return (
                200,
                state
                    .lock()
                    .unwrap()
                    .clone()
                    .map(|v| json!([v]))
                    .unwrap_or(json!([])),
            );
        }
        panic!("unexpected {method} {path}")
    })
    .await;
    let (dir, f) = factory(&base);
    let api = Github::with_base("alice-token", &base).unwrap();
    let e = event("new_issue");
    let r = result(json!({"comment":"Build finished"}));
    seed_run(&f, &e, "attempt-1");
    seed_run(&f, &e, "attempt-2");
    assert!(api
        .publish_actions(&f, &e, "attempt-1", "report", dir.path(), &r)
        .await
        .is_err());
    api.publish_actions(&f, &e, "attempt-2", "report", dir.path(), &r)
        .await
        .unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    assert_compact_receipts(&f, 12);
}

#[tokio::test]
async fn review_receipt_prevents_duplicate_and_reconciles_actor_and_head() {
    let writes = Arc::new(AtomicUsize::new(0));
    let reviews = Arc::new(Mutex::new(Vec::<Value>::new()));
    let w = writes.clone();
    let saved = reviews.clone();
    let e = event("pr_review");
    let served = e.clone();
    let base=server(move|method,path,body|{if path=="/user"{return(200,json!({"login":"alice"}))}
if method=="POST"{w.fetch_add(1,Ordering::SeqCst);let review=json!({"id":9,"body":body["body"],"commit_id":body["commit_id"],"state":"CHANGES_REQUESTED","user":{"login":"alice"}});saved.lock().unwrap().push(review.clone());return(200,review)}
if path.contains("/reviews?"){return(200,json!(*saved.lock().unwrap()))}(200,pr(&served,"feature",&served.head_sha))}).await;
    let (dir, f) = factory(&base);
    let api = Github::with_base("alice-token", &base).unwrap();
    let mut r = result(json!({"review":{"event":"REQUEST_CHANGES","body":"Fix bug"}}));
    let id = admit_then_disable(&f, &e);
    r["status"] = json!("blocked");
    seed_run(&f, &e, "attempt-2");
    api.publish_actions(&f, &e, &id, "review", dir.path(), &r)
        .await
        .unwrap();
    api.publish_actions(&f, &e, "attempt-2", "review", dir.path(), &r)
        .await
        .unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    assert_compact_receipts(&f, 9);
}

fn assert_compact_receipts(f: &Factory, remote_id: u64) {
    let root = f.snapshot().unwrap().service.app_data_dir;
    let db = rusqlite::Connection::open(Path::new(&root).join("factory.sqlite3")).unwrap();
    let raw: String = db
        .query_row(
            "SELECT json FROM observations WHERE substr(key,1,7)='action:'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&raw).unwrap(),
        json!({"completed":true,"remote_id":remote_id})
    );
}

#[tokio::test]
async fn deleting_attempt_preserves_completed_comment_and_review_receipts_for_surviving_retry() {
    for trigger in ["new_issue", "pr_review"] {
        let e = event(trigger);
        let review = trigger == "pr_review";
        let path = if review {
            "/repos/acme/repo/pulls/5/reviews"
        } else {
            "/repos/acme/repo/issues/1/comments"
        };
        let mut steps = vec![Step::new(
            "GET",
            "/user",
            Reply::Json(200, json!({"login":"alice"})),
        )];
        if review {
            steps.push(Step::new(
                "GET",
                PR_PATH,
                Reply::Json(200, pr(&e, "feature", &e.head_sha)),
            ));
        }
        steps.push(Step::new(
            "GET",
            &format!("{path}?per_page=100&page=1"),
            Reply::Json(200, json!([])),
        ));
        steps.push(Step::new(
            "POST",
            path,
            Reply::Json(201, json!({"id":71,"body":"private response body","user":{"login":"alice"},"extra":"not retained"})),
        ));
        // Receipt short-circuits all reconciliation and POSTs on surviving attempt.
        steps.push(Step::new(
            "GET",
            "/user",
            Reply::Json(200, json!({"login":"alice"})),
        ));
        let script = Script::start(steps).await;
        let (dir, f) = factory(&script.base);
        let api = Github::with_base("alice-token", &script.base).unwrap();
        let first = f.enqueue(e.clone()).unwrap().unwrap();
        let r = result(if review {
            json!({"review":{"event":"COMMENT","body":"private review body"}})
        } else {
            json!({"comment":"private comment body"})
        });
        api.publish_actions(&f, &e, &first, "report", dir.path(), &r)
            .await
            .unwrap();
        assert_compact_receipts(&f, 71);
        f.cancel(&first).unwrap();
        let second = f.retry(&first).unwrap();
        assert_ne!(first, second);
        assert_eq!(f.run_detail(&second).unwrap().run.attempt, 2);
        assert_eq!(f.run_detail(&second).unwrap().event.event_id, e.event_id);
        assert!(matches!(
            f.delete_run(&first).unwrap(),
            DeleteRunOutcome::Deleted { .. }
        ));
        assert!(f.run_detail(&first).is_err());
        assert_compact_receipts(&f, 71);
        api.publish_actions(&f, &e, &second, "report", dir.path(), &r)
            .await
            .unwrap();
        assert_compact_receipts(&f, 71);
        script.assert_done();
        assert_eq!(script.writes(), vec![("POST".into(), path.into())]);
    }
}

#[tokio::test]
async fn missing_run_cannot_start_remote_action() {
    let script = Script::start(vec![]).await;
    let (dir, f) = factory(&script.base);
    let api = Github::with_base("alice-token", &script.base).unwrap();
    assert!(api
        .publish_actions(
            &f,
            &event("new_issue"),
            "missing",
            "node",
            dir.path(),
            &result(json!({"comment":"must not publish"}))
        )
        .await
        .is_err());
    script.assert_done();
}

#[tokio::test]
async fn publish_adopts_matching_existing_stable_branch_before_any_push() {
    use std::os::unix::fs::PermissionsExt;
    let e = event("new_issue");
    let values = Arc::new(Mutex::new((String::new(), String::new())));
    let state = values.clone();
    let served = e.clone();
    let base = server(move |method, path, _| {
        assert_eq!(method, "GET");
        if path == "/user" {
            return (200, json!({"login":"alice"}));
        }
        let (branch, sha) = state.lock().unwrap().clone();
        let p = pr(&served, &branch, &sha);
        if path.contains("/pulls?") {
            return (200, json!([p]));
        }
        (200, p)
    })
    .await;
    let (dir, f) = factory(&base);
    let (ws, branch, sha) = workspace(dir.path(), &e);
    *values.lock().unwrap() = (branch, sha);
    let git_path = dir.path().join("git-wrapper");
    let actual = git(&ws, &["--exec-path"]);
    let _ = actual;
    let binary = std::process::Command::new("which")
        .arg("git")
        .output()
        .unwrap();
    let binary = String::from_utf8(binary.stdout).unwrap();
    std::fs::write(&git_path,format!("#!/bin/sh\nif [ \"$1\" = push ]; then echo unexpected push >&2; exit 9; fi\nexec '{}' \"$@\"\n",binary.trim())).unwrap();
    std::fs::set_permissions(&git_path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut s = f.settings().unwrap();
    s.commands.git = git_path.to_string_lossy().into();
    f.save_settings(s).unwrap();
    let api = Github::with_base("alice-token", &base).unwrap();
    let id = admit_then_disable(&f, &e);
    api.publish_actions(
        &f,
        &e,
        &id,
        "publish",
        &ws,
        &result(json!({"publish":{"title":"PR","body":"Closes #1","base":"main"}})),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn publish_uses_repository_default_instead_of_agent_base_or_main_fallback() {
    for (default, requested) in [
        ("master", Some("main")),
        ("master", None),
        ("main", None),
        ("trunk", Some("develop")),
        ("release/Stable", Some("main")),
    ] {
        let e = event("new_issue");
        let root = tempfile::tempdir().unwrap();
        let (ws, branch, sha) = workspace(root.path(), &e);
        let remote = root.path().join("remote.git");
        git(root.path(), &["init", "--bare", remote.to_str().unwrap()]);
        git(&ws, &["remote", "add", "origin", remote.to_str().unwrap()]);
        let mut published = pr(&e, &branch, &sha);
        published["base"]["ref"] = json!(default);
        let script = Script::start(vec![
            Step::new("GET", "/user", Reply::Json(200, json!({"login":"alice"}))),
            Step::new(
                "GET",
                &format!(
                    "/repos/acme/repo/pulls?state=open&head=acme:{branch}&per_page=100&page=1"
                ),
                Reply::Json(200, json!([])),
            ),
            Step::new("GET", REPO_PATH, Reply::Json(200, repo_value(default))),
            Step::new(
                "POST",
                "/repos/acme/repo/pulls",
                Reply::Json(201, published.clone()),
            )
            .body(
                json!({"title":"PR","body":"Closes #1","head":branch,"base":default,"draft":false}),
            ),
            Step::new("GET", PR_PATH, Reply::Json(200, published)),
        ])
        .await;
        let (_app, f) = factory(&script.base);
        let api = Github::with_base("alice-token", &script.base).unwrap();
        seed_run(&f, &e, "attempt");
        let mut publish = json!({"title":"PR","body":"Closes #1"});
        if let Some(requested) = requested {
            publish["base"] = json!(requested);
        }
        api.publish_actions(
            &f,
            &e,
            "attempt",
            "publish",
            &ws,
            &result(json!({"publish":publish})),
        )
        .await
        .unwrap_or_else(|error| panic!("default={default}, requested={requested:?}: {error:#}"));
        script.assert_done();
        assert_eq!(
            script.writes(),
            vec![("POST".into(), "/repos/acme/repo/pulls".into())]
        );
        assert_eq!(
            git(
                &ws,
                &["ls-remote", "origin", &format!("refs/heads/{branch}")]
            ),
            format!("{sha}\trefs/heads/{branch}")
        );
    }
}

#[tokio::test]
async fn publish_requires_verified_default_before_pushing_or_creating_pr() {
    for (status, metadata) in [
        (200, json!({"full_name":"acme/repo"})),
        (
            200,
            json!({"full_name":"other/repo","default_branch":"master"}),
        ),
        (200, repo_value("")),
        (200, repo_value("invalid..branch")),
        (403, json!({"message":"alice-token"})),
    ] {
        let e = event("new_issue");
        let root = tempfile::tempdir().unwrap();
        let (ws, branch, _) = workspace(root.path(), &e);
        let remote = root.path().join("remote.git");
        git(root.path(), &["init", "--bare", remote.to_str().unwrap()]);
        git(&ws, &["remote", "add", "origin", remote.to_str().unwrap()]);
        let script = Script::start(vec![
            Step::new("GET", "/user", Reply::Json(200, json!({"login":"alice"}))),
            Step::new(
                "GET",
                &format!(
                    "/repos/acme/repo/pulls?state=open&head=acme:{branch}&per_page=100&page=1"
                ),
                Reply::Json(200, json!([])),
            ),
            Step::new("GET", REPO_PATH, Reply::Json(status, metadata)),
        ])
        .await;
        let (_app, f) = factory(&script.base);
        let api = Github::with_base("alice-token", &script.base).unwrap();
        seed_run(&f, &e, "attempt");
        let error = api
            .publish_actions(
                &f,
                &e,
                "attempt",
                "publish",
                &ws,
                &result(json!({"publish":{"title":"PR","body":"Closes #1","base":"main"}})),
            )
            .await
            .unwrap_err();
        assert!(!format!("{error:#}").contains("alice-token"));
        script.assert_done();
        assert!(script.writes().is_empty());
        assert!(
            git(&ws, &["ls-remote", "origin"]).is_empty(),
            "metadata error must precede push"
        );
    }
}

#[tokio::test]
async fn all_actions_are_validated_before_any_git_or_github_mutation() {
    let writes = Arc::new(AtomicUsize::new(0));
    let w = writes.clone();
    let base = server(move |method, _, _| {
        if method != "GET" {
            w.fetch_add(1, Ordering::SeqCst);
        }
        (200, json!({"login":"alice"}))
    })
    .await;
    let (dir, f) = factory(&base);
    let api = Github::with_base("alice-token", &base).unwrap();
    let e = event("new_issue");
    let error = api
        .publish_actions(
            &f,
            &e,
            "run",
            "node",
            dir.path(),
            &result(
                json!({"publish":{"title":"PR","body":"Closes #1"},"merge":{"method":"squash"}}),
            ),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("Only approved flow can merge"),
        "{error}"
    );
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn blocked_results_cannot_publish_merge_or_approve() {
    let writes = Arc::new(AtomicUsize::new(0));
    let w = writes.clone();
    let base = server(move |method, _, _| {
        if method != "GET" {
            w.fetch_add(1, Ordering::SeqCst);
        }
        (200, json!({"login":"alice"}))
    })
    .await;
    let (dir, f) = factory(&base);
    let api = Github::with_base("alice-token", &base).unwrap();
    for (trigger, data) in [
        (
            "new_issue",
            json!({"publish":{"title":"PR","body":"Closes #1"}}),
        ),
        ("pr_approved", json!({"merge":{"method":"squash"}})),
        (
            "pr_review",
            json!({"review":{"event":"APPROVE","body":"ok"}}),
        ),
    ] {
        let mut r = result(data);
        r["status"] = json!("blocked");
        let error = api
            .publish_actions(&f, &event(trigger), "run", "node", dir.path(), &r)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains(if trigger == "pr_review" {
                "Blocked result cannot approve"
            } else {
                "Only succeeded results"
            }),
            "{error}"
        );
    }
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancelled_run_cannot_comment_after_identity_read() {
    let current = Arc::new(Mutex::new(None::<(Arc<Factory>, String)>));
    let state = current.clone();
    let writes = Arc::new(AtomicUsize::new(0));
    let w = writes.clone();
    let base = server(move |method, path, _| {
        if path == "/user" {
            if let Some((f, id)) = state.lock().unwrap().as_ref() {
                f.cancel(id).unwrap();
            }
            return (200, json!({"login":"alice"}));
        }
        if method != "GET" {
            w.fetch_add(1, Ordering::SeqCst);
        }
        (200, json!([]))
    })
    .await;
    let (dir, f) = factory(&base);
    let e = event("new_issue");
    let id = f.enqueue(e.clone()).unwrap().unwrap();
    *current.lock().unwrap() = Some((f.clone(), id.clone()));
    let api = Github::with_base("alice-token", &base).unwrap();
    assert!(api
        .publish_actions(
            &f,
            &e,
            &id,
            "node",
            dir.path(),
            &result(json!({"comment":"done"}))
        )
        .await
        .is_err());
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn merge_rejects_unsafe_metadata_before_any_mutation() {
    let e = event("pr_approved");
    let good = pr(&e, "feature", &e.head_sha);
    let mut cases: Vec<(&str, Value, Value)> = vec![];
    for pointer in ["/head/repo/full_name", "/head/ref", "/base/ref"] {
        let mut p = good.clone();
        *p.pointer_mut(pointer).unwrap() = Value::Null;
        cases.push((pointer, p, repo_value("main")));
    }
    for (label, pointer, value) in [
        ("same-source-target", "/base/ref", json!("feature")),
        ("source-default", "/head/ref", json!("main")),
        ("fork", "/head/repo/full_name", json!("other/repo")),
        ("different-head", "/head/sha", json!("b".repeat(40))),
    ] {
        let mut p = good.clone();
        *p.pointer_mut(pointer).unwrap() = value;
        if label == "source-default" {
            p["base"]["ref"] = json!("develop");
        }
        cases.push((label, p, repo_value("main")));
    }
    cases.push((
        "missing-default",
        good.clone(),
        json!({"full_name":"acme/repo"}),
    ));
    cases.push((
        "wrong-repo-response",
        good.clone(),
        json!({"full_name":"other/repo","default_branch":"main"}),
    ));
    cases.push(("invalid-default", good.clone(), repo_value("../main")));
    for (label, p, repo) in cases {
        let mut steps = preflight(p, "main");
        steps[2] = Step::new("GET", REPO_PATH, Reply::Json(200, repo));
        let (outcome, script) = execute_merge(&e, steps).await;
        assert!(outcome.is_err(), "{label}");
        script.assert_done();
        assert!(script.writes().is_empty(), "{label}");
    }
    for (pointer, value) in [
        ("/base/repo/full_name", Value::Null),
        ("/base/repo/full_name", json!("other/repo")),
        ("/number", json!(6)),
        ("/html_url", json!("https://github.com/other/repo/pull/5")),
        ("/head/sha", json!("invalid")),
    ] {
        let mut p = good.clone();
        *p.pointer_mut(pointer).unwrap() = value;
        let mut steps = preflight(p, "main");
        steps.truncate(2);
        let (outcome, script) = execute_merge(&e, steps).await;
        assert!(outcome.is_err(), "{pointer}");
        script.assert_done();
        assert!(script.writes().is_empty());
    }
}

#[tokio::test]
async fn merge_rejects_invalid_branch_names_without_normalizing() {
    for branch in [
        "",
        "@",
        "HEAD",
        "-feature",
        "feature.",
        "feature..x",
        "feature@{1}",
        "/feature",
        "feature/",
        "feature//x",
        ".feature",
        "feature/.x",
        "feature.lock",
        "a/x.lock",
        "a b",
        "a~b",
        "a^b",
        "a:b",
        "a?b",
        "a*b",
        "a[b",
        "a\\b",
        "a\nb",
        "a\u{007f}b",
        "../refs/tags/v1",
    ] {
        for side in ["head", "base"] {
            let e = event("pr_approved");
            let mut p = pr(&e, "feature", &e.head_sha);
            p[side]["ref"] = json!(branch);
            let (outcome, script) = execute_merge(&e, preflight(p, "main")).await;
            assert!(outcome.is_err(), "{side}: {branch:?}");
            script.assert_done();
            assert!(script.writes().is_empty());
        }
    }
}

#[tokio::test]
async fn merge_errors_or_uncertainty_never_delete() {
    for reply in [
        Reply::Json(409, json!({})),
        Reply::Json(422, json!({})),
        Reply::Json(200, json!({"merged":false})),
        Reply::Raw(200, b"broken".to_vec()),
        Reply::Disconnect,
    ] {
        let e = event("pr_approved");
        let mut steps = preflight(pr(&e, "feature", &e.head_sha), "main");
        steps.push(Step::new("PUT", MERGE_PATH, reply));
        let (outcome, script) = execute_merge(&e, steps).await;
        let error = outcome.unwrap_err();
        assert!(format!("{error:#}").contains("merge") || format!("{error:#}").contains("Merge"));
        script.assert_done();
        assert_eq!(script.writes(), vec![("PUT".into(), MERGE_PATH.into())]);
    }
    let e = event("pr_approved");
    let mut p = pr(&e, "feature", &e.head_sha);
    p["state"] = json!("closed");
    let (outcome, script) = execute_merge(&e, preflight(p, "main")).await;
    assert!(outcome.is_err());
    script.assert_done();
    assert!(script.writes().is_empty());
}

#[tokio::test]
async fn merge_preserves_source_on_changed_identity_or_unconfirmed_fresh_state() {
    let e = event("pr_approved");
    let open = pr(&e, "feature", &e.head_sha);
    for (pointer, value, repo_read) in [
        ("/head/ref", json!("other"), true),
        ("/base/ref", json!("develop"), true),
        ("/head/repo/full_name", json!("other/repo"), true),
        ("/head/sha", json!("b".repeat(40)), true),
        ("/merged", json!(false), false),
    ] {
        let mut after = merged_pr(open.clone());
        *after.pointer_mut(pointer).unwrap() = value;
        let mut steps = preflight(open.clone(), "main");
        steps.push(Step::new(
            "PUT",
            MERGE_PATH,
            Reply::Json(200, json!({"merged":true})),
        ));
        let mut after_steps = confirmed(after, "main");
        if !repo_read {
            after_steps.truncate(1);
        }
        steps.extend(after_steps);
        let (outcome, script) = execute_merge(&e, steps).await;
        assert!(outcome.unwrap_err().to_string().contains("cleanup pending"));
        script.assert_done();
        assert_eq!(script.writes(), vec![("PUT".into(), MERGE_PATH.into())]);
    }
}

#[tokio::test]
async fn merge_rejects_moved_or_mismatched_source_reference() {
    let e = event("pr_approved");
    let closed = merged_pr(pr(&e, "feature", &e.head_sha));
    for value in [
        reference("feature", &"b".repeat(40)),
        reference("feature-extra", &e.head_sha),
        json!([reference("feature", &e.head_sha)]),
        json!({"ref":"refs/tags/feature","object":{"type":"tag","sha":e.head_sha}}),
        json!({"ref":"refs/heads/feature","object":{"type":"commit","sha":"bad"}}),
    ] {
        let mut steps = preflight(closed.clone(), "main");
        steps.extend(confirmed(closed.clone(), "main"));
        steps.push(Step::new("GET", SOURCE_GET, Reply::Json(200, value)));
        let (outcome, script) = execute_merge(&e, steps).await;
        assert!(outcome.unwrap_err().to_string().contains("cleanup pending"));
        script.assert_done();
        assert!(script.writes().is_empty());
    }
}

#[tokio::test]
async fn merge_keeps_branch_case_and_encodes_source_without_changing_namespace() {
    for (branch, encoded) in [
        ("ai-factory/2-abc123", "ai-factory/2-abc123"),
        ("Feature", "Feature"),
        ("Main", "Main"),
        ("feature%2Fother", "feature%252Fother"),
        ("feature#1+é", "feature%231%2B%C3%A9"),
        ("tags/v1", "tags/v1"),
    ] {
        let e = event("pr_approved");
        let mut closed = merged_pr(pr(&e, branch, &e.head_sha));
        if branch == "Feature" {
            closed["base"]["ref"] = json!("feature");
        }
        closed["head"]["repo"]["full_name"] = json!("ACME/Repo");
        closed["base"]["repo"]["full_name"] = json!("Acme/REPO");
        let get = format!("/repos/acme/repo/git/ref/heads/{encoded}");
        let delete = format!("/repos/acme/repo/git/refs/heads/{encoded}");
        let mut steps = preflight(closed.clone(), "main");
        steps.extend(confirmed(closed, "main"));
        steps.push(Step::new(
            "GET",
            &get,
            Reply::Json(200, reference(branch, &e.head_sha)),
        ));
        steps.push(Step::new("DELETE", &delete, Reply::Raw(204, vec![])));
        let (outcome, script) = execute_merge(&e, steps).await;
        outcome.unwrap();
        script.assert_done();
        assert_eq!(script.writes(), vec![("DELETE".into(), delete)]);
    }
}

#[tokio::test]
async fn merge_requires_actor_and_default_repository_verification() {
    let e = event("pr_approved");
    let (outcome, script) = execute_merge(
        &e,
        vec![Step::new(
            "GET",
            "/user",
            Reply::Json(200, json!({"login":"mallory"})),
        )],
    )
    .await;
    assert!(outcome.is_err());
    script.assert_done();
    assert!(script.writes().is_empty());
    for status in [401, 403, 404, 409, 500] {
        let mut steps = preflight(pr(&e, "feature", &e.head_sha), "main");
        steps[2] = Step::new("GET", REPO_PATH, Reply::Json(status, json!({})));
        let (outcome, script) = execute_merge(&e, steps).await;
        assert!(outcome.is_err());
        script.assert_done();
        assert!(script.writes().is_empty());
    }
}

#[tokio::test]
async fn merge_preserves_source_when_default_changes_to_source_after_merge() {
    let e = event("pr_approved");
    let open = pr(&e, "feature", &e.head_sha);
    let mut steps = preflight(open.clone(), "main");
    steps.push(Step::new(
        "PUT",
        MERGE_PATH,
        Reply::Json(200, json!({"merged":true})),
    ));
    steps.extend(confirmed(merged_pr(open), "feature"));
    let (outcome, script) = execute_merge(&e, steps).await;
    assert!(outcome.unwrap_err().to_string().contains("default branch"));
    script.assert_done();
    assert_eq!(script.writes(), vec![("PUT".into(), MERGE_PATH.into())]);
}

#[tokio::test]
async fn merge_accepts_already_absent_source_only_with_reference_access_witness() {
    let e = event("pr_approved");
    let closed = merged_pr(pr(&e, "feature", &e.head_sha));
    let mut steps = preflight(closed.clone(), "main");
    steps.extend(confirmed(closed, "main"));
    steps.push(Step::new("GET", SOURCE_GET, Reply::Json(404, json!({}))));
    steps.extend(absence_witness());
    let (outcome, script) = execute_merge(&e, steps).await;
    outcome.unwrap();
    script.assert_done();
    assert!(script.writes().is_empty());
}

#[tokio::test]
async fn merge_does_not_treat_opaque_404_as_absent_branch() {
    for repo_access in [false, true] {
        let e = event("pr_approved");
        let closed = merged_pr(pr(&e, "feature", &e.head_sha));
        let mut steps = preflight(closed.clone(), "main");
        steps.extend(confirmed(closed, "main"));
        steps.push(Step::new("GET", SOURCE_GET, Reply::Json(404, json!({}))));
        steps.push(Step::new(
            "GET",
            REPO_PATH,
            if repo_access {
                Reply::Json(200, repo_value("main"))
            } else {
                Reply::Json(404, json!({}))
            },
        ));
        if repo_access {
            steps.push(Step::new(
                "GET",
                "/repos/acme/repo/git/ref/heads/main",
                Reply::Json(404, json!({})),
            ));
        }
        let (outcome, script) = execute_merge(&e, steps).await;
        assert!(outcome.unwrap_err().to_string().contains("cleanup pending"));
        script.assert_done();
        assert!(script.writes().is_empty());
    }
}

#[tokio::test]
async fn merge_delete_errors_fail_if_source_remains_and_never_leak_response_body() {
    for status in [200, 401, 403, 404, 409, 422, 500] {
        let e = event("pr_approved");
        let closed = merged_pr(pr(&e, "feature", &e.head_sha));
        let mut steps = preflight(closed.clone(), "main");
        steps.extend(confirmed(closed.clone(), "main"));
        steps.push(Step::new(
            "GET",
            SOURCE_GET,
            Reply::Json(200, reference("feature", &e.head_sha)),
        ));
        steps.push(Step::new(
            "DELETE",
            SOURCE_DELETE,
            Reply::Json(status, json!({"message":"alice-token"})),
        ));
        steps.extend(confirmed(closed, "main"));
        steps.push(Step::new(
            "GET",
            SOURCE_GET,
            Reply::Json(200, reference("feature", &e.head_sha)),
        ));
        let (outcome, script) = execute_merge(&e, steps).await;
        let error = format!("{:#}", outcome.unwrap_err());
        assert!(error.contains("merged; cleanup pending"));
        assert!(error.contains("feature") && error.contains("acme/repo#5"));
        assert!(error.contains(&status.to_string()) && !error.contains("alice-token"));
        script.assert_done();
        assert_eq!(
            script.writes(),
            vec![("DELETE".into(), SOURCE_DELETE.into())]
        );
    }
}

#[tokio::test]
async fn merge_delete_error_reconciles_only_after_confirmed_absence() {
    for reply in [
        Reply::Json(404, json!({})),
        Reply::Json(422, json!({})),
        Reply::Disconnect,
    ] {
        let e = event("pr_approved");
        let closed = merged_pr(pr(&e, "feature", &e.head_sha));
        let mut steps = preflight(closed.clone(), "main");
        steps.extend(confirmed(closed.clone(), "main"));
        steps.push(Step::new(
            "GET",
            SOURCE_GET,
            Reply::Json(200, reference("feature", &e.head_sha)),
        ));
        steps.push(Step::new("DELETE", SOURCE_DELETE, reply));
        steps.extend(confirmed(closed, "main"));
        steps.push(Step::new("GET", SOURCE_GET, Reply::Json(404, json!({}))));
        steps.extend(absence_witness());
        let (outcome, script) = execute_merge(&e, steps).await;
        outcome.unwrap();
        script.assert_done();
        assert_eq!(
            script.writes(),
            vec![("DELETE".into(), SOURCE_DELETE.into())]
        );
    }
}

#[tokio::test]
async fn merge_retry_completes_only_pending_work() {
    for lost_merge in [false, true] {
        let e = event("pr_approved");
        let open = pr(&e, "feature", &e.head_sha);
        let closed = merged_pr(open.clone());
        let mut steps = preflight(open, "main");
        steps.push(Step::new(
            "PUT",
            MERGE_PATH,
            if lost_merge {
                Reply::Disconnect
            } else {
                Reply::Json(200, json!({"merged":true}))
            },
        ));
        if !lost_merge {
            steps.extend(confirmed(closed.clone(), "main"));
            steps.push(Step::new(
                "GET",
                SOURCE_GET,
                Reply::Json(200, reference("feature", &e.head_sha)),
            ));
            steps.push(Step::new(
                "DELETE",
                SOURCE_DELETE,
                Reply::Json(403, json!({})),
            ));
            steps.extend(confirmed(closed.clone(), "main"));
            steps.push(Step::new(
                "GET",
                SOURCE_GET,
                Reply::Json(200, reference("feature", &e.head_sha)),
            ));
        }
        steps.extend(preflight(closed.clone(), "main"));
        steps.extend(confirmed(closed, "main"));
        steps.push(Step::new(
            "GET",
            SOURCE_GET,
            Reply::Json(200, reference("feature", &e.head_sha)),
        ));
        steps.push(Step::new("DELETE", SOURCE_DELETE, Reply::Raw(204, vec![])));
        let script = Script::start(steps).await;
        let (dir, f) = factory(&script.base);
        let api = Github::with_base("alice-token", &script.base).unwrap();
        assert!(merge_attempt(&api, &f, &e, dir.path(), "attempt-1")
            .await
            .is_err());
        merge_attempt(&api, &f, &e, dir.path(), "attempt-2")
            .await
            .unwrap();
        script.assert_done();
        assert_eq!(
            script
                .writes()
                .iter()
                .filter(|(method, _)| method == "PUT")
                .count(),
            1
        );
        assert_eq!(
            script
                .writes()
                .iter()
                .filter(|(method, _)| method == "DELETE")
                .count(),
            if lost_merge { 1 } else { 2 }
        );
    }
}

#[tokio::test]
async fn merge_retry_recovers_lost_delete_with_uncertain_reconciliation_without_second_delete() {
    let e = event("pr_approved");
    let closed = merged_pr(pr(&e, "feature", &e.head_sha));
    let mut steps = preflight(closed.clone(), "main");
    steps.extend(confirmed(closed.clone(), "main"));
    steps.push(Step::new(
        "GET",
        SOURCE_GET,
        Reply::Json(200, reference("feature", &e.head_sha)),
    ));
    steps.push(Step::new("DELETE", SOURCE_DELETE, Reply::Disconnect));
    // DELETE took effect remotely; first reconciliation cannot read current PR.
    steps.push(Step::new("GET", PR_PATH, Reply::Json(503, json!({}))));
    steps.extend(preflight(closed.clone(), "main"));
    steps.extend(confirmed(closed, "main"));
    steps.push(Step::new("GET", SOURCE_GET, Reply::Json(404, json!({}))));
    steps.extend(absence_witness());
    let script = Script::start(steps).await;
    let (dir, f) = factory(&script.base);
    let api = Github::with_base("alice-token", &script.base).unwrap();
    assert!(merge_attempt(&api, &f, &e, dir.path(), "attempt-1")
        .await
        .is_err());
    merge_attempt(&api, &f, &e, dir.path(), "attempt-2")
        .await
        .unwrap();
    script.assert_done();
    assert_eq!(
        script.writes(),
        vec![("DELETE".into(), SOURCE_DELETE.into())]
    );
}

#[tokio::test]
async fn merge_cancel_before_put_or_before_delete_prevents_new_writes_and_retry_recovers() {
    for after_merge in [false, true] {
        let e = event("pr_approved");
        let open = pr(&e, "feature", &e.head_sha);
        let closed = merged_pr(open.clone());
        let holder = Arc::new(Mutex::new(None::<(Arc<Factory>, String)>));
        let hook_state = holder.clone();
        let mut steps = preflight(open.clone(), "main");
        if after_merge {
            steps.push(Step::new(
                "PUT",
                MERGE_PATH,
                Reply::Json(200, json!({"merged":true})),
            ));
            steps.extend(confirmed(closed.clone(), "main"));
            steps.push(Step::new(
                "GET",
                SOURCE_GET,
                Reply::Json(200, reference("feature", &e.head_sha)),
            ));
        }
        // Cancel after last read, immediately before next mutation check.
        let last = steps.pop().unwrap().then(move || {
            let guard = hook_state.lock().unwrap();
            let (f, id) = guard.as_ref().unwrap();
            f.cancel(id).unwrap();
        });
        steps.push(last);
        if after_merge {
            steps.extend(preflight(closed.clone(), "main"));
            steps.extend(confirmed(closed, "main"));
        } else {
            steps.extend(preflight(open.clone(), "main"));
            steps.push(Step::new(
                "PUT",
                MERGE_PATH,
                Reply::Json(200, json!({"merged":true})),
            ));
            steps.extend(confirmed(merged_pr(open), "main"));
        }
        steps.push(Step::new(
            "GET",
            SOURCE_GET,
            Reply::Json(200, reference("feature", &e.head_sha)),
        ));
        steps.push(Step::new("DELETE", SOURCE_DELETE, Reply::Raw(204, vec![])));
        let script = Script::start(steps).await;
        let (dir, f) = factory(&script.base);
        let id = f.enqueue(e.clone()).unwrap().unwrap();
        *holder.lock().unwrap() = Some((f.clone(), id.clone()));
        let api = Github::with_base("alice-token", &script.base).unwrap();
        assert!(merge_attempt(&api, &f, &e, dir.path(), &id).await.is_err());
        assert_eq!(f.run_detail(&id).unwrap().run.status, "cancelled");
        assert_eq!(script.writes().len(), usize::from(after_merge));
        let retry = f.retry(&id).unwrap();
        merge_attempt(&api, &f, &e, dir.path(), &retry)
            .await
            .unwrap();
        script.assert_done();
        assert_eq!(
            script
                .writes()
                .iter()
                .filter(|(method, _)| method == "PUT")
                .count(),
            1
        );
        assert_eq!(
            script
                .writes()
                .iter()
                .filter(|(method, _)| method == "DELETE")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn merge_cancel_on_put_response_prevents_source_delete() {
    let e = event("pr_approved");
    let holder = Arc::new(Mutex::new(None::<(Arc<Factory>, String)>));
    let hook = holder.clone();
    let mut steps = preflight(pr(&e, "feature", &e.head_sha), "main");
    steps.push(
        Step::new("PUT", MERGE_PATH, Reply::Json(200, json!({"merged":true}))).then(move || {
            let guard = hook.lock().unwrap();
            let (f, id) = guard.as_ref().unwrap();
            f.cancel(id).unwrap();
        }),
    );
    let script = Script::start(steps).await;
    let (dir, f) = factory(&script.base);
    let id = f.enqueue(e.clone()).unwrap().unwrap();
    *holder.lock().unwrap() = Some((f.clone(), id.clone()));
    let api = Github::with_base("alice-token", &script.base).unwrap();
    assert!(merge_attempt(&api, &f, &e, dir.path(), &id).await.is_err());
    script.assert_done();
    assert_eq!(script.writes(), vec![("PUT".into(), MERGE_PATH.into())]);
}

#[tokio::test]
async fn merge_cleanup_failure_prevents_later_comment() {
    let e = event("pr_approved");
    let closed = merged_pr(pr(&e, "feature", &e.head_sha));
    let mut steps = preflight(closed.clone(), "main");
    steps.extend(confirmed(closed, "main"));
    steps.push(Step::new(
        "GET",
        SOURCE_GET,
        Reply::Json(200, reference("feature", &"b".repeat(40))),
    ));
    let script = Script::start(steps).await;
    let (dir, f) = factory(&script.base);
    let api = Github::with_base("alice-token", &script.base).unwrap();
    seed_run(&f, &e, "attempt");
    let error = api
        .publish_actions(
            &f,
            &e,
            "attempt",
            "merge",
            dir.path(),
            &result(json!({"merge":{"method":"squash"},"comment":"Done"})),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cleanup pending"));
    script.assert_done();
    assert!(script.writes().is_empty());
}

#[tokio::test]
async fn merge_invalid_results_or_combined_actions_never_reach_http() {
    for (trigger, status, data) in [
        (
            "pr_approved",
            "failed",
            json!({"merge":{"method":"squash"}}),
        ),
        (
            "pr_approved",
            "blocked",
            json!({"merge":{"method":"squash"}}),
        ),
        (
            "new_issue",
            "succeeded",
            json!({"merge":{"method":"squash"}}),
        ),
        (
            "pr_review",
            "succeeded",
            json!({"merge":{"method":"squash"}}),
        ),
        (
            "pr_approved",
            "succeeded",
            json!({"merge":{"method":"merge"}}),
        ),
        (
            "pr_approved",
            "succeeded",
            json!({"merge":{"method":"squash"},"comment":""}),
        ),
        (
            "pr_approved",
            "succeeded",
            json!({"merge":{"method":"squash"},"review":{"event":"APPROVE","body":"ok"}}),
        ),
    ] {
        let script = Script::start(vec![]).await;
        let (dir, f) = factory(&script.base);
        let api = Github::with_base("alice-token", &script.base).unwrap();
        let mut value = result(data);
        value["status"] = json!(status);
        assert!(api
            .publish_actions(&f, &event(trigger), "attempt", "merge", dir.path(), &value)
            .await
            .is_err());
        script.assert_done();
        assert!(script.writes().is_empty());
    }
}
