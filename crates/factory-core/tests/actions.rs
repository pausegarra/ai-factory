#![cfg(unix)]
mod common;
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
    json!({"number":5,"title":"PR","body":"Closes #1","html_url":"https://github.com/acme/repo/pull/5","state":"open","merged":false,"merged_at":null,"base":{"repo":{"full_name":"acme/repo"}},"head":{"sha":sha,"ref":branch},"user":{"login":e.actor}})
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
    }];
    f.save_settings(s).unwrap();
    (dir, f)
}
fn result(data: Value) -> Value {
    json!({"status":"succeeded","summary":"done","artifacts":[],"data":data})
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

#[tokio::test]
async fn merge_reconciles_already_merged_pull_request_without_second_write() {
    let writes = Arc::new(AtomicUsize::new(0));
    let w = writes.clone();
    let e = event("pr_approved");
    let served = e.clone();
    let base = server(move |method, path, _| {
        if path == "/user" {
            return (200, json!({"login":"alice"}));
        }
        if method == "PUT" {
            w.fetch_add(1, Ordering::SeqCst);
            return (405, json!({"message":"already merged"}));
        }
        let mut p = pr(&served, "feature", &served.head_sha);
        p["state"] = json!("closed");
        p["merged"] = json!(true);
        p["merged_at"] = json!("2026-11-01T00:00:00Z");
        (200, p)
    })
    .await;
    let (dir, f) = factory(&base);
    let api = Github::with_base("alice-token", &base).unwrap();
    api.publish_actions(
        &f,
        &e,
        "attempt-2",
        "merge",
        dir.path(),
        &result(json!({"merge":{"method":"squash"}})),
    )
    .await
    .unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 0);
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
    assert!(api
        .publish_actions(&f, &e, "attempt-1", "report", dir.path(), &r)
        .await
        .is_err());
    api.publish_actions(&f, &e, "attempt-2", "report", dir.path(), &r)
        .await
        .unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 1);
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
    r["status"] = json!("blocked");
    api.publish_actions(&f, &e, "attempt-1", "review", dir.path(), &r)
        .await
        .unwrap();
    api.publish_actions(&f, &e, "attempt-2", "review", dir.path(), &r)
        .await
        .unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 1);
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
    api.publish_actions(
        &f,
        &e,
        "attempt-2",
        "publish",
        &ws,
        &result(json!({"publish":{"title":"PR","body":"Closes #1","base":"main"}})),
    )
    .await
    .unwrap();
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
