#![cfg(unix)]
mod common;

use factory_core::*;
use serde_json::{json, Value};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn git(path: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

struct Fixture {
    dir: tempfile::TempDir,
    factory: Arc<Factory>,
    event: TriggerEvent,
    base_sha: String,
    remote: PathBuf,
    writes: Arc<Mutex<Vec<String>>>,
    reads: Arc<Mutex<Vec<String>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        git(&source, &["init", "-b", "main"]);
        git(&source, &["config", "user.name", "Alice"]);
        git(&source, &["config", "user.email", "alice@example.com"]);
        std::fs::write(source.join("shared.txt"), "initial\n").unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-m", "initial"]);
        let stale_base = git(&source, &["rev-parse", "HEAD"]);
        git(&source, &["checkout", "-b", "feature"]);
        std::fs::write(source.join("shared.txt"), "feature\n").unwrap();
        git(&source, &["commit", "-am", "feature"]);
        let head_sha = git(&source, &["rev-parse", "HEAD"]);
        git(&source, &["checkout", "main"]);
        std::fs::write(source.join("shared.txt"), "main\n").unwrap();
        git(&source, &["commit", "-am", "base advanced"]);
        let base_sha = git(&source, &["rev-parse", "HEAD"]);
        let remote = dir.path().join("remote.git");
        git(
            dir.path(),
            &[
                "clone",
                "--bare",
                source.to_str().unwrap(),
                remote.to_str().unwrap(),
            ],
        );
        git(&remote, &["update-ref", "refs/pull/5/head", &head_sha]);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_base = format!("http://{}", listener.local_addr().unwrap());
        let writes = Arc::new(Mutex::new(Vec::new()));
        let requests = writes.clone();
        let reads = Arc::new(Mutex::new(Vec::new()));
        let read_requests = reads.clone();
        let repo = remote.clone();
        let flags = dir.path().to_path_buf();
        let approved_head = head_sha.clone();
        let server = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let repo = repo.clone();
                let requests = requests.clone();
                let read_requests = read_requests.clone();
                let stale_base = stale_base.clone();
                let flags = flags.clone();
                let approved_head = approved_head.clone();
                tokio::spawn(async move {
                    let mut raw = vec![0; 65536];
                    let n = socket.read(&mut raw).await.unwrap();
                    let req = String::from_utf8_lossy(&raw[..n]);
                    assert!(req
                        .to_ascii_lowercase()
                        .contains("authorization: bearer fixture-token"));
                    let mut line = req.lines().next().unwrap().split_whitespace();
                    let method = line.next().unwrap();
                    let path = line.next().unwrap();
                    let (status, data) = if method != "GET" {
                        requests.lock().unwrap().push(format!("{method} {path}"));
                        if method == "PUT"
                            && path == "/repos/acme/repo/pulls/5/merge"
                            && flags.join("allow-merge").exists()
                        {
                            let advance = flags.join("advance-base-on-merge");
                            if advance.exists() {
                                let merged_sha = std::fs::read_to_string(advance).unwrap();
                                git(&repo, &["update-ref", "refs/heads/main", merged_sha.trim()]);
                            }
                            std::fs::write(flags.join("merged"), "").unwrap();
                            (200, json!({"merged":true}))
                        } else if method == "DELETE"
                            && path == "/repos/acme/repo/git/refs/heads/feature"
                            && flags.join("allow-delete").exists()
                        {
                            git(&repo, &["update-ref", "-d", "refs/heads/feature"]);
                            (204, Value::Null)
                        } else {
                            (500, json!({"message":"Unexpected mutation"}))
                        }
                    } else if path == "/user" {
                        (200, json!({"login":"alice","id":1}))
                    } else if path.starts_with("/search/") {
                        (
                            200,
                            json!({"total_count":0,"incomplete_results":false,"items":[]}),
                        )
                    } else if path == "/repos/acme/repo" {
                        let default = if flags.join("source-is-default").exists() {
                            "feature"
                        } else {
                            "main"
                        };
                        (
                            200,
                            json!({"full_name":"acme/repo","default_branch":default}),
                        )
                    } else if path == "/repos/acme/repo/pulls/5"
                        || path == "/repos/acme/repo/pulls/6"
                    {
                        read_requests.lock().unwrap().push(path.into());
                        let second = path.ends_with("/6");
                        let number = if second { 6 } else { 5 };
                        let branch = if second { "feature-two" } else { "feature" };
                        let reference = format!("refs/heads/{branch}");
                        let output = std::process::Command::new("git")
                            .current_dir(&repo)
                            .args(["rev-parse", "--verify", &reference])
                            .output()
                            .unwrap();
                        let sha = if output.status.success() {
                            String::from_utf8(output.stdout).unwrap().trim().to_owned()
                        } else {
                            assert!(
                                !second && flags.join("merged").exists(),
                                "Unmerged fixture PR lost its head reference"
                            );
                            approved_head.clone()
                        };
                        let head_repo = if !second && flags.join("fork").exists() {
                            "other/repo"
                        } else {
                            "acme/repo"
                        };
                        let base_ref = if !second && flags.join("source-is-target").exists() {
                            "feature"
                        } else {
                            "main"
                        };
                        let merged = !second && flags.join("merged").exists();
                        (
                            200,
                            json!({"number":number,"state":if merged {"closed"}else{"open"},"merged":merged,"title":"PR","html_url":format!("https://github.com/acme/repo/pull/{number}"),"user":{"login":"alice"},"base":{"ref":base_ref,"sha":stale_base,"repo":{"full_name":"acme/repo"}},"head":{"ref":branch,"sha":sha,"repo":{"full_name":head_repo}}}),
                        )
                    } else if let Some(branch) =
                        path.strip_prefix("/repos/acme/repo/git/ref/heads/")
                    {
                        read_requests.lock().unwrap().push(path.into());
                        let reference = format!("refs/heads/{branch}");
                        let output = std::process::Command::new("git")
                            .current_dir(&repo)
                            .args(["rev-parse", "--verify", &reference])
                            .output()
                            .unwrap();
                        if output.status.success() {
                            let sha = String::from_utf8(output.stdout).unwrap().trim().to_owned();
                            (
                                200,
                                json!({"ref":reference,"object":{"type":"commit","sha":sha}}),
                            )
                        } else {
                            (404, json!({"message":"Reference not found"}))
                        }
                    } else {
                        (404, json!({"message":"Unknown fixture route"}))
                    };
                    let body = data.to_string();
                    let response = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });

        let wrapper = dir.path().join("git-wrapper");
        executable(
            &wrapper,
            &format!(
                r#"#!/usr/bin/env python3
import os,sys,subprocess,json,pathlib
args=sys.argv[1:]
root=pathlib.Path({root:?})
if args[0] in ('clone','fetch','push'):
    assert os.environ['GIT_CONFIG_VALUE_0'].startswith('AUTHORIZATION: basic ')
    with (root/'remote-commands.jsonl').open('a') as f: f.write(json.dumps(args)+'\n')
if args[0] in ('clone','fetch','push'):
    args=[{remote:?} if a=='https://github.com/acme/repo.git' or a=='origin' else a for a in args]
if args[0]=='push' and (root/'race-head').exists():
    subprocess.check_call(['git','--git-dir',{remote:?},'update-ref','refs/heads/feature',(root/'race-head').read_text().strip()])
if args[0]=='fetch' and (root/'race-base-fetch').exists():
    subprocess.check_call(['git','--git-dir',{remote:?},'update-ref','refs/heads/main',(root/'race-base-fetch').read_text().strip()])
code=subprocess.call(['git']+args)
if args[0]=='push' and (root/'lose-response').exists() and code==0: sys.exit(1)
sys.exit(code)
"#,
                root = dir.path().to_str().unwrap(),
                remote = remote.to_str().unwrap()
            ),
        );
        let factory = common::open_with_api(dir.path().join("app"), &api_base).unwrap();
        let mut s = factory.settings().unwrap();
        s.token_profiles = vec![TokenProfile {
            id: "alice".into(),
            account: "alice".into(),
            label: "Alice".into(),
            token: Some("fixture-token".into()),
            has_token: false,
            owners: vec!["acme".into()],
            authors: vec!["alice".into()],
            reviewers: vec!["alice".into()],
        }];
        s.commands.git = wrapper.to_string_lossy().into();
        s.commands.opencode = dir.path().join("agent").to_string_lossy().into();
        s.poll_interval_seconds = 3600;
        let flow = &mut s.flows[1];
        let mut repair = flow.nodes[0].clone();
        repair.id = "repair".into();
        repair.name = "Resolve conflicts".into();
        repair.agent = "opencode".into();
        repair.prompt = "Resolve conflicts and update PR".into();
        let mut merge = repair.clone();
        merge.id = "merge".into();
        merge.depends_on = vec!["repair".into()];
        flow.nodes = vec![repair, merge];
        factory.save_settings(s).unwrap();
        let mut event = TriggerEvent::issue("acme/repo", 5, "PR", "alice", "");
        event.trigger_id = "pr_approved".into();
        event.head_sha = head_sha;
        event.event_id = format!("approval:{}", event.head_sha);
        event.url = "https://github.com/acme/repo/pull/5".into();
        event.payload =
            json!({"pull_request":{"base":{"ref":"main","sha":stale_base_for_event(&source)}}});
        Self {
            dir,
            factory,
            event,
            base_sha,
            remote,
            writes,
            reads,
            server,
        }
    }

    fn agent(&self, before: &str, data: Value) {
        executable(
            &self.dir.path().join("agent"),
            &format!(
                r#"#!/bin/sh
set -eu
cat > "$AI_FACTORY_MEMORY_DIR/$AI_FACTORY_NODE_ID.prompt"
if [ "$AI_FACTORY_NODE_ID" = merge ]; then
  printf '%s' '{{"status":"succeeded","summary":"merge","artifacts":[],"data":{{"merge":{{"method":"squash"}}}}}}' > "$AI_FACTORY_RESULT_PATH"
  exit 0
fi
[ -z "${{GH_TOKEN:-}}" ] && [ -z "${{GITHUB_TOKEN:-}}" ] && [ -z "${{GIT_ASKPASS:-}}" ]
[ "$(git config credential.helper)" = '' ]
{before}
printf '%s' '{{"status":"succeeded","summary":"local repair done","artifacts":[],"data":{data}}}' > "$AI_FACTORY_RESULT_PATH"
"#
            ),
        );
    }

    fn add_second_approval(&self) -> String {
        let source = self.dir.path().join("source");
        git(&source, &["checkout", "main"]);
        git(
            &source,
            &[
                "merge",
                "--no-ff",
                "-s",
                "ours",
                "feature",
                "-m",
                "fixture merge",
            ],
        );
        let merged = git(&source, &["rev-parse", "HEAD"]);
        git(
            &self.remote,
            &[
                "fetch",
                source.to_str().unwrap(),
                "main:refs/fixture/merged",
            ],
        );
        git(
            &self.remote,
            &["update-ref", "refs/heads/feature-two", &self.event.head_sha],
        );
        git(
            &self.remote,
            &["update-ref", "refs/pull/6/head", &self.event.head_sha],
        );
        std::fs::write(self.dir.path().join("advance-base-on-merge"), &merged).unwrap();
        merged
    }

    fn second_event(&self) -> TriggerEvent {
        let mut event = self.event.clone();
        event.number = 6;
        event.event_id = format!("approval:6:{}", event.head_sha);
        event.url = "https://github.com/acme/repo/pull/6".into();
        event
    }

    async fn run(&self) -> RunDetail {
        let id = self.factory.enqueue(self.event.clone()).unwrap().unwrap();
        self.factory.start();
        let detail = self.wait(&id).await;
        self.factory.shutdown().await;
        detail
    }

    async fn wait(&self, id: &str) -> RunDetail {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let d = self.factory.run_detail(id).unwrap();
                if !["queued", "running"].contains(&d.run.status.as_str()) {
                    break d;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    }
}

fn run_memory(fixture: &Fixture, id: &str) -> PathBuf {
    fixture
        .dir
        .path()
        .join("app/memory/acme/repo/runs")
        .join(id)
}

async fn wait_for_started(fixture: &Fixture, id: &str) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !run_memory(fixture, id).join("started").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

fn blocked_approval_agent() -> &'static str {
    r#"
printf '%s' "$AI_FACTORY_PR_BASE_SHA" > "$AI_FACTORY_MEMORY_DIR/base"
printf '%s' "$AI_FACTORY_PR_HEAD_SHA" > "$AI_FACTORY_MEMORY_DIR/head"
git rev-parse refs/ai-factory/base > "$AI_FACTORY_MEMORY_DIR/base-ref"
git rev-parse refs/ai-factory/head > "$AI_FACTORY_MEMORY_DIR/head-ref"
touch "$AI_FACTORY_MEMORY_DIR/started"
if [ "$AI_FACTORY_PR_HEAD_REF" = feature ]; then
  while [ ! -f "$AI_FACTORY_MEMORY_DIR/release" ]; do sleep 0.02; done
fi
"#
}

fn one_node_approval_flow(fixture: &Fixture) {
    let mut settings = fixture.factory.settings().unwrap();
    settings.flows[1].nodes.truncate(1);
    fixture.factory.save_settings(settings).unwrap();
}

fn stale_base_for_event(source: &Path) -> String {
    git(source, &["rev-parse", "feature^"])
}

const REPAIR: &str = r#"
[ "$(git rev-parse refs/ai-factory/base)" = "$AI_FACTORY_PR_BASE_SHA" ]
[ "$(git rev-parse refs/ai-factory/head)" = "$AI_FACTORY_PR_HEAD_SHA" ]
set +e
git merge --no-edit refs/ai-factory/base
status=$?
set -e
[ "$status" = 1 ]
printf 'main and feature resolved\n' > shared.txt
git add shared.txt
git commit -m 'fix: resolve PR conflicts'
"#;

#[tokio::test]
async fn release_authorization_does_not_survive_a_published_conflict_correction() {
    let mut f = Fixture::new().await;
    f.event.payload["approval"] = json!({"command":"release","actor":f.event.actor,"head_sha":f.event.head_sha,"comment_id":7});
    f.agent(REPAIR, json!({"update_pr":{}}));
    let d = f.run().await;
    assert_eq!(d.run.status, "blocked", "{d:?}");
    assert_eq!(
        d.nodes
            .iter()
            .find(|node| node.id == "factory_release")
            .unwrap()
            .status,
        "skipped"
    );
    assert!(f.writes.lock().unwrap().is_empty());
    assert_ne!(git(&f.remote, &["rev-parse", "feature"]), f.event.head_sha);
}

#[tokio::test]
async fn release_step_never_starts_after_failed_merge() {
    let mut f = Fixture::new().await;
    f.event.payload["approval"] = json!({"command":"release","actor":f.event.actor,"head_sha":f.event.head_sha,"comment_id":7});
    one_node_approval_flow(&f);
    // Fixture denies merge; release preparation must never reach GitHub.
    f.agent("", json!({"merge":{"method":"squash"}}));
    let d = f.run().await;
    assert_eq!(d.run.status, "failed", "{d:?}");
    assert_eq!(
        d.nodes
            .iter()
            .find(|node| node.id == "factory_release")
            .unwrap()
            .status,
        "skipped"
    );
    assert_eq!(
        *f.writes.lock().unwrap(),
        vec!["PUT /repos/acme/repo/pulls/5/merge"]
    );
}

#[tokio::test]
async fn approved_trigger_cannot_succeed_without_a_factory_merge_receipt() {
    for data in [
        json!({"verification":{"approval_current":false,"merge_requested":false}}),
        json!({"pr_merge":{"merged":true,"cleanup_completed":true}}),
    ] {
        let f = Fixture::new().await;
        one_node_approval_flow(&f);
        f.agent("", data);
        let d = f.run().await;
        assert_eq!(d.run.status, "blocked", "{d:?}");
        assert!(d
            .run
            .error
            .as_deref()
            .unwrap()
            .contains("merge and source branch cleanup were not confirmed"));
        assert!(f.writes.lock().unwrap().is_empty());
        assert_eq!(git(&f.remote, &["rev-parse", "feature"]), f.event.head_sha);
    }
}

#[tokio::test]
async fn approved_trigger_merges_without_querying_github_reviews() {
    let f = Fixture::new().await;
    one_node_approval_flow(&f);
    // This saved custom prompt reproduces the misleading instruction from the run.
    let mut settings = f.factory.settings().unwrap();
    settings.flows[1].nodes[0].prompt = "Check current GitHub reviews before merging".into();
    f.factory.save_settings(settings).unwrap();
    std::fs::write(f.dir.path().join("allow-merge"), "").unwrap();
    std::fs::write(f.dir.path().join("allow-delete"), "").unwrap();
    f.agent("", json!({"merge":{"method":"squash"}}));
    let d = f.run().await;
    assert_eq!(d.run.status, "succeeded", "{d:?}");
    assert_eq!(
        *f.writes.lock().unwrap(),
        vec![
            "PUT /repos/acme/repo/pulls/5/merge",
            "DELETE /repos/acme/repo/git/refs/heads/feature"
        ]
    );
    let prompt = std::fs::read_to_string(run_memory(&f, &d.run.id).join("repair.prompt")).unwrap();
    assert!(prompt.contains("The pr_approved trigger is the approval authority for event.head_sha"));
    assert!(prompt.contains("Do not query GitHub reviews or comments to reconfirm approval"));
}

#[tokio::test]
async fn resolves_against_live_base_publishes_original_branch_and_requires_new_approval() {
    let f = Fixture::new().await;
    f.agent(REPAIR, json!({"update_pr":{}}));
    let d = f.run().await;
    assert_eq!(d.run.status, "blocked", "{d:?}");
    let repair = d.nodes.iter().find(|n| n.id == "repair").unwrap();
    assert_eq!(repair.status, "blocked", "{repair:?}");
    let result = repair.result.as_ref().unwrap();
    assert_eq!(result["data"]["pr_update"]["base_sha"], f.base_sha);
    assert_eq!(result["data"]["pr_update"]["head_ref"], "feature");
    assert!(result["summary"].as_str().unwrap().contains("approval"));
    assert_eq!(
        d.nodes.iter().find(|n| n.id == "merge").unwrap().status,
        "skipped"
    );
    let head = git(&f.remote, &["rev-parse", "feature"]);
    assert_ne!(head, f.event.head_sha);
    assert_eq!(result["data"]["pr_update"]["head_sha"], head);
    git(
        &f.remote,
        &["merge-base", "--is-ancestor", &f.event.head_sha, &head],
    );
    git(
        &f.remote,
        &["merge-base", "--is-ancestor", &f.base_sha, &head],
    );
    assert_eq!(git(&f.remote, &["rev-parse", "main"]), f.base_sha);
    assert!(f.writes.lock().unwrap().is_empty());
    let prompt = std::fs::read_to_string(
        f.dir
            .path()
            .join("app/memory/acme/repo/runs")
            .join(&d.run.id)
            .join("repair.prompt"),
    )
    .unwrap();
    assert!(prompt.contains(&f.base_sha));
    let commands = std::fs::read_to_string(f.dir.path().join("remote-commands.jsonl")).unwrap();
    assert!(!commands.contains("fixture-token"));
    assert_eq!(
        commands
            .lines()
            .filter(|l| l.starts_with("[\"push\""))
            .count(),
        1
    );
}

#[tokio::test]
async fn update_pr_reconciles_a_successful_push_with_lost_response() {
    let f = Fixture::new().await;
    std::fs::write(f.dir.path().join("lose-response"), "").unwrap();
    f.agent(REPAIR, json!({"update_pr":{}}));
    let d = f.run().await;
    assert_eq!(d.run.status, "blocked", "{d:?}");
    assert_ne!(git(&f.remote, &["rev-parse", "feature"]), f.event.head_sha);
    assert!(f.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn update_pr_rejects_combining_a_correction_with_merge() {
    let f = Fixture::new().await;
    f.agent("", json!({"update_pr":{},"merge":{"method":"squash"}}));
    let d = f.run().await;
    assert_eq!(d.run.status, "failed", "{d:?}");
    let error = d
        .nodes
        .iter()
        .find(|n| n.id == "repair")
        .unwrap()
        .error
        .as_deref()
        .unwrap();
    assert!(error.contains("update_pr cannot be combined"), "{error}");
    let node = d.nodes.iter().find(|n| n.id == "repair").unwrap();
    assert!(node.result.as_ref().unwrap()["summary"]
        .as_str()
        .unwrap()
        .contains(error));
    let recorded: Value = serde_json::from_slice(
        &std::fs::read(
            f.dir
                .path()
                .join("app/sessions")
                .join(&d.run.id)
                .join("repair.result.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        recorded["summary"],
        node.result.as_ref().unwrap()["summary"]
    );
    assert!(f.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn correction_stops_independent_merge_nodes_as_well_as_dependents() {
    let f = Fixture::new().await;
    let mut s = f.factory.settings().unwrap();
    s.flows[1].nodes[1].depends_on.clear();
    f.factory.save_settings(s).unwrap();
    f.agent(REPAIR, json!({"update_pr":{}}));
    let d = f.run().await;
    assert_eq!(d.run.status, "blocked", "{d:?}");
    assert_eq!(
        d.nodes.iter().find(|n| n.id == "merge").unwrap().status,
        "skipped"
    );
    assert!(f.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn update_pr_rejects_dirty_uncommitted_rewritten_or_unrelated_work() {
    for (before, expected) in [
        (
            format!("{REPAIR}\nprintf extra > extra.txt"),
            "workspace is dirty",
        ),
        (
            "git commit --allow-empty -m unrelated".into(),
            "must include both original head and checked base",
        ),
        (
            "git reset --hard refs/ai-factory/base\ngit commit --allow-empty -m rewritten".into(),
            "must include both original head and checked base",
        ),
        (
            format!("{REPAIR}\ngit checkout -b unauthorized"),
            "outside event branch",
        ),
        (String::new(), "No committed PR corrections"),
    ] {
        let f = Fixture::new().await;
        f.agent(&before, json!({"update_pr":{}}));
        let d = f.run().await;
        assert_eq!(d.run.status, "failed", "{d:?}");
        let error = d
            .nodes
            .iter()
            .find(|n| n.id == "repair")
            .unwrap()
            .error
            .as_deref()
            .unwrap();
        assert!(error.contains(expected), "{error}");
        assert_eq!(git(&f.remote, &["rev-parse", "feature"]), f.event.head_sha);
        assert!(f.writes.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn update_pr_rejects_forks_default_and_target_branches() {
    for (flag, expected) in [
        ("fork", "fork PR branches"),
        ("source-is-default", "target or default branch"),
        ("source-is-target", "target or default branch"),
    ] {
        let f = Fixture::new().await;
        std::fs::write(f.dir.path().join(flag), "").unwrap();
        f.agent(
            "git commit --allow-empty -m candidate",
            json!({"update_pr":{}}),
        );
        let d = f.run().await;
        assert_eq!(d.run.status, "failed", "{d:?}");
        let error = d
            .nodes
            .iter()
            .find(|n| n.id == "repair")
            .unwrap()
            .error
            .as_deref()
            .unwrap();
        assert!(error.contains(expected), "{error}");
        assert_eq!(git(&f.remote, &["rev-parse", "feature"]), f.event.head_sha);
        assert!(f.writes.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn update_pr_rejects_read_only_nodes_and_unapproved_triggers() {
    for trigger in ["pr_approved", "pr_review"] {
        let mut f = Fixture::new().await;
        let mut s = f.factory.settings().unwrap();
        if trigger == "pr_approved" {
            s.flows[1].nodes[0].access = "read".into();
        } else {
            f.event.trigger_id = trigger.into();
            s.flows[2].nodes = s.flows[1].nodes.clone();
        }
        f.factory.save_settings(s).unwrap();
        f.agent("", json!({"update_pr":{}}));
        let d = f.run().await;
        assert_eq!(d.run.status, "failed", "{d:?}");
        let error = d
            .nodes
            .iter()
            .find(|n| n.id == "repair")
            .unwrap()
            .error
            .as_deref()
            .unwrap();
        assert!(
            error.contains(if trigger == "pr_review" {
                "Only approved flow"
            } else {
                "Read-only node"
            }),
            "{error}"
        );
        assert_eq!(git(&f.remote, &["rev-parse", "feature"]), f.event.head_sha);
        assert!(f.writes.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn update_pr_rejects_head_or_base_changes_since_preparation() {
    for branch in ["main", "feature"] {
        let f = Fixture::new().await;
        let replacement = if branch == "main" {
            &f.event.head_sha
        } else {
            &f.base_sha
        };
        let before = format!(
            "{REPAIR}\ngit --git-dir='{}' update-ref refs/heads/{branch} {replacement}",
            f.remote.display()
        );
        f.agent(&before, json!({"update_pr":{}}));
        let d = f.run().await;
        assert_eq!(d.run.status, "failed", "{d:?}");
        let error = d
            .nodes
            .iter()
            .find(|n| n.id == "repair")
            .unwrap()
            .error
            .as_deref()
            .unwrap();
        assert!(
            error.contains(if branch == "main" {
                "PR base changed"
            } else {
                "PR head changed"
            }),
            "{error}"
        );
        assert_eq!(git(&f.remote, &["rev-parse", branch]), *replacement);
        assert!(f.writes.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn update_pr_lease_preserves_a_head_changed_between_preflight_and_push() {
    let f = Fixture::new().await;
    std::fs::write(f.dir.path().join("race-head"), &f.base_sha).unwrap();
    f.agent(REPAIR, json!({"update_pr":{}}));
    let d = f.run().await;
    assert_eq!(d.run.status, "failed", "{d:?}");
    let error = d
        .nodes
        .iter()
        .find(|n| n.id == "repair")
        .unwrap()
        .error
        .as_deref()
        .unwrap();
    assert!(error.contains("push failed"), "{error}");
    assert_eq!(git(&f.remote, &["rev-parse", "feature"]), f.base_sha);
    assert!(f.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn merge_does_not_discard_unpublished_local_corrections() {
    let f = Fixture::new().await;
    f.agent(REPAIR, json!({"merge":{"method":"squash"}}));
    let d = f.run().await;
    assert_eq!(d.run.status, "failed", "{d:?}");
    let error = d
        .nodes
        .iter()
        .find(|n| n.id == "repair")
        .unwrap()
        .error
        .as_deref()
        .unwrap();
    assert!(error.contains("data.update_pr before merge"), "{error}");
    assert_eq!(git(&f.remote, &["rev-parse", "feature"]), f.event.head_sha);
    assert!(f.writes.lock().unwrap().is_empty());
}

#[test]
fn default_approved_flow_explains_conflict_resolution_and_existing_pr_updates() {
    let s = Settings::default();
    let flow = s.flows.iter().find(|f| f.id == "pr_approved").unwrap();
    assert!(flow.nodes.iter().any(|n| n.access == "write"
        && n.prompt.contains("update_pr")
        && n.prompt.contains("conflict")));
}

#[tokio::test]
async fn runtime_retry_after_merge_finishes_pending_cleanup_without_merging_again() {
    let f = Fixture::new().await;
    std::fs::write(f.dir.path().join("allow-merge"), "").unwrap();
    let mut settings = f.factory.settings().unwrap();
    settings.flows[1].nodes.truncate(1);
    f.factory.save_settings(settings).unwrap();
    f.agent("", json!({"merge":{"method":"squash"}}));
    let id = f.factory.enqueue(f.event.clone()).unwrap().unwrap();
    f.factory.start();
    let first = f.wait(&id).await;
    assert_eq!(first.run.status, "failed", "{first:?}");
    assert!(first.nodes[0]
        .error
        .as_deref()
        .unwrap()
        .contains("cleanup pending"));
    std::fs::write(f.dir.path().join("allow-delete"), "").unwrap();
    let retry = f.factory.retry(&id).unwrap();
    let second = f.wait(&retry).await;
    f.factory.shutdown().await;
    assert_eq!(second.run.status, "succeeded", "{second:?}");
    assert_eq!(
        *f.writes.lock().unwrap(),
        vec![
            "PUT /repos/acme/repo/pulls/5/merge",
            "DELETE /repos/acme/repo/git/refs/heads/feature",
            "DELETE /repos/acme/repo/git/refs/heads/feature"
        ]
    );
}

#[tokio::test]
async fn update_pr_never_pushes_to_an_already_merged_pr() {
    for merged_before_preparation in [true, false] {
        let f = Fixture::new().await;
        if merged_before_preparation {
            std::fs::write(f.dir.path().join("merged"), "").unwrap();
            f.agent("", json!({"update_pr":{}}));
        } else {
            let before = format!(
                "{REPAIR}\ntouch '{}'",
                f.dir.path().join("merged").display()
            );
            f.agent(&before, json!({"update_pr":{}}));
        }
        let d = f.run().await;
        assert_eq!(d.run.status, "failed", "{d:?}");
        let error = d
            .nodes
            .iter()
            .find(|n| n.id == "repair")
            .unwrap()
            .error
            .as_deref()
            .unwrap();
        assert!(
            error.contains("Cannot update an already merged PR"),
            "{error}"
        );
        assert_eq!(git(&f.remote, &["rev-parse", "feature"]), f.event.head_sha);
        let commands = std::fs::read_to_string(f.dir.path().join("remote-commands.jsonl")).unwrap();
        assert!(!commands.lines().any(|l| l.starts_with("[\"push\"")));
        assert!(f.writes.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn preparation_rejects_base_ref_moving_between_api_read_and_fetch() {
    let f = Fixture::new().await;
    std::fs::write(f.dir.path().join("race-base-fetch"), &f.event.head_sha).unwrap();
    f.agent("", json!({}));
    let d = f.run().await;
    assert_eq!(d.run.status, "failed", "{d:?}");
    assert!(d
        .run
        .error
        .as_deref()
        .unwrap()
        .contains("refs changed during workspace preparation"));
    assert!(d.nodes.iter().all(|n| n.started_at.is_none()));
    assert!(f.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn queued_approval_prepares_live_base_only_after_previous_merge_and_cleanup() {
    let f = Fixture::new().await;
    let merged = f.add_second_approval();
    std::fs::write(f.dir.path().join("allow-merge"), "").unwrap();
    std::fs::write(f.dir.path().join("allow-delete"), "").unwrap();
    f.agent(blocked_approval_agent(), json!({}));

    let first = f.factory.enqueue(f.event.clone()).unwrap().unwrap();
    f.factory.start();
    wait_for_started(&f, &first).await;
    one_node_approval_flow(&f);
    let second = f.factory.enqueue(f.second_event()).unwrap().unwrap();
    let queued = f.factory.run_detail(&second).unwrap();
    assert_eq!(queued.run.status, "queued");
    assert!(queued.run.started_at.is_none() && queued.run.workspace.is_none());
    assert!(!run_memory(&f, &second).join("started").exists());
    assert!(!f
        .reads
        .lock()
        .unwrap()
        .iter()
        .any(|path| path == "/repos/acme/repo/pulls/6"));
    let commands = std::fs::read_to_string(f.dir.path().join("remote-commands.jsonl")).unwrap();
    assert!(!commands.contains("refs/pull/6/head"));

    std::fs::write(run_memory(&f, &first).join("release"), "").unwrap();
    let first_result = f.wait(&first).await;
    let second_result = f.wait(&second).await;
    f.factory.shutdown().await;

    assert_eq!(first_result.run.status, "succeeded", "{first_result:?}");
    assert_eq!(second_result.run.status, "blocked", "{second_result:?}");
    assert_eq!(
        std::fs::read_to_string(run_memory(&f, &first).join("base"))
            .unwrap()
            .trim(),
        f.base_sha
    );
    assert_eq!(
        std::fs::read_to_string(run_memory(&f, &second).join("base"))
            .unwrap()
            .trim(),
        merged
    );
    assert_eq!(
        std::fs::read_to_string(run_memory(&f, &second).join("base-ref"))
            .unwrap()
            .trim(),
        merged
    );
    for id in [&first, &second] {
        assert_eq!(
            std::fs::read_to_string(run_memory(&f, id).join("head"))
                .unwrap()
                .trim(),
            f.event.head_sha
        );
        assert_eq!(
            std::fs::read_to_string(run_memory(&f, id).join("head-ref"))
                .unwrap()
                .trim(),
            f.event.head_sha
        );
    }
    assert_eq!(git(&f.remote, &["rev-parse", "refs/heads/main"]), merged);
    assert_eq!(
        *f.writes.lock().unwrap(),
        vec![
            "PUT /repos/acme/repo/pulls/5/merge",
            "DELETE /repos/acme/repo/git/refs/heads/feature"
        ]
    );
    assert!(!f
        .writes
        .lock()
        .unwrap()
        .iter()
        .any(|write| write.contains("/pulls/6/merge") || write.contains("feature-two")));
    let commands = std::fs::read_to_string(f.dir.path().join("remote-commands.jsonl")).unwrap();
    assert!(commands.contains("refs/pull/6/head"));
}

#[tokio::test]
async fn queued_approval_rejects_moved_head_and_new_head_requires_new_event() {
    let f = Fixture::new().await;
    let moved_head = f.add_second_approval();
    one_node_approval_flow(&f);
    f.agent(blocked_approval_agent(), json!({}));
    let first = f.factory.enqueue(f.event.clone()).unwrap().unwrap();
    f.factory.start();
    wait_for_started(&f, &first).await;

    let stale_event = f.second_event();
    let stale_head = stale_event.head_sha.clone();
    let stale = f.factory.enqueue(stale_event).unwrap().unwrap();
    git(
        &f.remote,
        &["update-ref", "refs/heads/feature-two", &moved_head],
    );
    git(&f.remote, &["update-ref", "refs/pull/6/head", &moved_head]);
    std::fs::write(run_memory(&f, &first).join("release"), "").unwrap();

    let first_result = f.wait(&first).await;
    let stale_result = f.wait(&stale).await;
    assert_eq!(first_result.run.status, "blocked", "{first_result:?}");
    assert_eq!(stale_result.run.status, "failed", "{stale_result:?}");
    assert_eq!(stale_result.event.head_sha, stale_head);
    assert!(!run_memory(&f, &stale).join("started").exists());
    assert!(f.writes.lock().unwrap().is_empty());

    let mut new_event = f.second_event();
    new_event.event_id = format!("approval:6:{moved_head}");
    new_event.head_sha = moved_head.clone();
    let new_attempt = f.factory.enqueue(new_event).unwrap().unwrap();
    let new_result = f.wait(&new_attempt).await;
    f.factory.shutdown().await;
    assert_eq!(new_result.run.status, "blocked", "{new_result:?}");
    assert_eq!(
        std::fs::read_to_string(run_memory(&f, &new_attempt).join("head"))
            .unwrap()
            .trim(),
        moved_head
    );
}

#[tokio::test]
async fn failed_merge_action_releases_next_queued_approval_without_advancing_base() {
    let f = Fixture::new().await;
    f.add_second_approval();
    f.agent(blocked_approval_agent(), json!({}));
    let first = f.factory.enqueue(f.event.clone()).unwrap().unwrap();
    f.factory.start();
    wait_for_started(&f, &first).await;
    one_node_approval_flow(&f);
    let second = f.factory.enqueue(f.second_event()).unwrap().unwrap();
    assert_eq!(f.factory.run_detail(&second).unwrap().run.status, "queued");
    assert!(!f
        .reads
        .lock()
        .unwrap()
        .iter()
        .any(|path| path == "/repos/acme/repo/pulls/6"));

    std::fs::write(run_memory(&f, &first).join("release"), "").unwrap();
    let first_result = f.wait(&first).await;
    let second_result = f.wait(&second).await;
    f.factory.shutdown().await;
    assert_eq!(first_result.run.status, "failed", "{first_result:?}");
    assert_eq!(second_result.run.status, "blocked", "{second_result:?}");
    assert_eq!(
        std::fs::read_to_string(run_memory(&f, &second).join("base"))
            .unwrap()
            .trim(),
        f.base_sha
    );
    assert_eq!(
        git(&f.remote, &["rev-parse", "refs/heads/main"]),
        f.base_sha
    );
    assert_eq!(
        *f.writes.lock().unwrap(),
        vec!["PUT /repos/acme/repo/pulls/5/merge"]
    );
    assert!(!f
        .writes
        .lock()
        .unwrap()
        .iter()
        .any(|write| write.contains("/pulls/6/merge") || write.contains("feature-two")));
}
