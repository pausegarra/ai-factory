mod common;
use factory_core::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn profile(id: &str, owner: &str) -> Value {
    json!({"id":id,"label":id,"account":id,"token":format!("{id}-token"),
        "owners":[owner],"authors":[id],"reviewers":[id]})
}

#[test]
fn global_settings_migrate_to_profiles_without_resurrecting_empty_scopes() {
    let mut raw = serde_json::to_value(Settings::default()).unwrap();
    raw["owners"] = json!(["personal", "business"]);
    raw["authors"] = json!(["ALICE", "bob", "other"]);
    raw["reviewers"] = json!(["alice", "BOB"]);
    raw["token_profiles"] = json!([
        {"id":"alice","label":"Personal","account":"alice","has_token":false},
        {"id":"bob","label":"Work","account":"bob","owners":[],"authors":[],"reviewers":["bob"],"has_token":false}
    ]);
    let settings: Settings = serde_json::from_value(raw).unwrap();
    let migrated = serde_json::to_value(&settings).unwrap();
    assert_eq!(
        migrated["token_profiles"][0]["owners"],
        json!(["personal", "business"])
    );
    assert_eq!(migrated["token_profiles"][0]["authors"], json!(["ALICE"]));
    assert_eq!(migrated["token_profiles"][0]["reviewers"], json!(["alice"]));
    assert_eq!(migrated["token_profiles"][1]["owners"], json!([]));
    assert_eq!(migrated["token_profiles"][1]["authors"], json!([]));
    assert_eq!(migrated["token_profiles"][1]["reviewers"], json!(["bob"]));
    for field in ["owners", "authors", "reviewers"] {
        assert!(
            migrated.get(field).is_none(),
            "global {field} must disappear"
        );
    }
    let again: Settings = serde_json::from_value(migrated.clone()).unwrap();
    assert_eq!(serde_json::to_value(again).unwrap(), migrated);
}

#[test]
fn new_profiles_have_empty_scopes_without_global_fallback() {
    let mut raw = serde_json::to_value(Settings::default()).unwrap();
    for field in ["owners", "authors", "reviewers"] {
        raw.as_object_mut().unwrap().remove(field);
    }
    raw["token_profiles"] = json!([
        profile("alice", "personal"),
        {"id":"bob","label":"Work","account":"bob","has_token":false}
    ]);
    let settings: Settings = serde_json::from_value(raw).unwrap();
    let value = serde_json::to_value(settings).unwrap();
    for field in ["owners", "authors", "reviewers"] {
        assert_eq!(value["token_profiles"][1][field], json!([]), "{field}");
    }
}

#[test]
fn null_scope_is_invalid_instead_of_inheriting_legacy_permissions() {
    for field in ["owners", "authors", "reviewers"] {
        let mut raw = serde_json::to_value(Settings::default()).unwrap();
        raw[field] = json!(["alice"]);
        raw["token_profiles"] = json!([profile("alice", "personal")]);
        raw["token_profiles"][0][field] = Value::Null;
        assert!(serde_json::from_value::<Settings>(raw).is_err(), "{field}");
    }
}

#[test]
fn profile_scope_persists_normalized_without_changing_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open(dir.path().into()).unwrap();
    let mut raw = serde_json::to_value(f.settings().unwrap()).unwrap();
    raw["token_profiles"] = json!([profile("alice", "Personal")]);
    raw["token_profiles"][0]["owners"] = json!(["Personal", "personal"]);
    raw["token_profiles"][0]["authors"] = json!(["Alice", "alice"]);
    let saved = serde_json::to_value(
        f.save_settings(serde_json::from_value(raw).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(saved["token_profiles"][0]["owners"], json!(["personal"]));
    assert_eq!(saved["token_profiles"][0]["authors"], json!(["alice"]));
    assert_eq!(saved["token_profiles"][0]["has_token"], true);
    assert!(saved["token_profiles"][0].get("token").is_none());
    drop(f);
    let f = common::open(dir.path().into()).unwrap();
    assert_eq!(serde_json::to_value(f.settings().unwrap()).unwrap(), saved);
    let mut invalid = saved;
    invalid["token_profiles"][0]["owners"] = json!(["bad/owner"]);
    assert!(f
        .save_settings(serde_json::from_value(invalid).unwrap())
        .is_err());
}

async fn server(fail_personal: bool, calls: Arc<Mutex<Vec<(String, String)>>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let calls = calls.clone();
            tokio::spawn(async move {
                let mut bytes = vec![0; 65536];
                let n = stream.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..n]);
                let actor = if request.to_lowercase().contains("bearer alice-token") {
                    "alice"
                } else {
                    "bob"
                };
                let path = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let url = reqwest::Url::parse(&format!("http://fixture{path}")).unwrap();
                let owner = if actor == "alice" {
                    "personal"
                } else {
                    "business"
                };
                let repo = format!("{owner}/repo");
                let (code, body) = if path == "/user" {
                    (200, json!({"login":actor}))
                } else if url.path() == "/search/issues" {
                    let q = url
                        .query_pairs()
                        .find(|(key, _)| key == "q")
                        .unwrap()
                        .1
                        .into_owned();
                    calls.lock().unwrap().push((actor.into(), q.clone()));
                    if !q.contains(&format!("user:{owner}")) || (fail_personal && actor == "alice")
                    {
                        (422, json!({"message":"Validation Failed"}))
                    } else {
                        let review = q.contains("is:pr");
                        let number = if actor == "alice" {
                            if review {
                                10
                            } else {
                                1
                            }
                        } else if review {
                            20
                        } else {
                            2
                        };
                        let mut item = json!({"number":number,"repository_url":format!("https://api.github.com/repos/{repo}")});
                        if review {
                            item["pull_request"] = json!({});
                        }
                        (
                            200,
                            json!({"incomplete_results":false,"total_count":1,"items":[item]}),
                        )
                    }
                } else if path.contains("/issues/") {
                    let number = if actor == "alice" { 1 } else { 2 };
                    (
                        200,
                        json!({"number":number,"state":"open","title":"[AI] work","body":"work","user":{"login":actor},"html_url":format!("https://github.com/{repo}/issues/{number}")}),
                    )
                } else {
                    let number = if actor == "alice" { 10 } else { 20 };
                    (
                        200,
                        json!({"number":number,"state":"open","title":"Review","body":"","user":{"login":"contributor"},"html_url":format!("https://github.com/{repo}/pull/{number}"),"base":{"repo":{"full_name":repo}},"head":{"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"requested_reviewers":[{"login":actor}]}),
                    )
                };
                let body = body.to_string();
                let response = format!("HTTP/1.1 {code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
            });
        }
    });
    format!("http://{address}")
}

#[tokio::test]
async fn polling_uses_only_each_tokens_owners_and_continues_after_another_tokens_422() {
    for fail_personal in [false, true] {
        let calls = Arc::new(Mutex::new(vec![]));
        let base = server(fail_personal, calls.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let f = common::open_with_api(dir.path().into(), &base).unwrap();
        let mut raw = serde_json::to_value(f.settings().unwrap()).unwrap();
        raw["token_profiles"] = json!([profile("alice", "personal"), profile("bob", "business")]);
        f.save_settings(serde_json::from_value(raw).unwrap())
            .unwrap();
        let result = f.poll_now().await;
        assert_eq!(result.is_err(), fail_personal);
        let snapshot = f.snapshot().unwrap();
        assert_eq!(snapshot.runs.len(), if fail_personal { 2 } else { 4 });
        assert_eq!(
            snapshot
                .runs
                .iter()
                .filter(|run| run.actor == "bob")
                .count(),
            2
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 4);
        for (actor, query) in calls.iter() {
            let owner = if actor == "alice" {
                "personal"
            } else {
                "business"
            };
            assert!(query.contains(&format!("user:{owner}")), "{actor}: {query}");
            assert!(
                query.contains(&format!("author:{actor}"))
                    || query.contains(&format!("review-requested:{actor}"))
            );
        }
    }
}

#[tokio::test]
async fn execution_cannot_borrow_owner_or_actor_permissions_from_another_token() {
    for (remove_owner, expected) in [
        (true, "Repository owner no longer authorized"),
        (false, "Trigger actor no longer authorized"),
    ] {
        let calls = Arc::new(Mutex::new(vec![]));
        let base = server(false, calls.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let f = common::open_with_api(dir.path().into(), &base).unwrap();
        let mut raw = serde_json::to_value(f.settings().unwrap()).unwrap();
        let mut actor = profile("alice", "personal");
        actor["reviewers"] = json!([]);
        if remove_owner {
            actor["owners"] = json!([]);
        } else {
            actor["authors"] = json!([]);
        }
        let mut unrelated = profile("bob", "personal");
        unrelated["authors"] = json!(["alice"]);
        unrelated["reviewers"] = json!([]);
        raw["token_profiles"] = json!([actor, unrelated]);
        f.save_settings(serde_json::from_value(raw).unwrap())
            .unwrap();
        let id = f
            .enqueue(TriggerEvent::issue(
                "personal/repo",
                1,
                "[AI] scope",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        f.start();
        let mut detail = f.run_detail(&id).unwrap();
        for _ in 0..200 {
            if !["queued", "running"].contains(&detail.run.status.as_str()) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            detail = f.run_detail(&id).unwrap();
        }
        f.shutdown().await;
        assert_eq!(detail.run.status, "failed");
        assert!(
            detail.run.error.as_deref().unwrap().contains(expected),
            "{:?}",
            detail.run.error
        );
        assert!(calls.lock().unwrap().is_empty());
    }
}
