#![cfg(unix)]
mod common;

use factory_core::*;
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn git(path: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

async fn fixture() -> (
    tempfile::TempDir,
    Arc<Factory>,
    String,
    tokio::task::JoinHandle<()>,
) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir(&source).unwrap();
    git(&source, &["init", "-b", "main"]);
    git(
        &source,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    let sha = git(&source, &["rev-parse", "HEAD"]);
    let server_sha = sha.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let sha = server_sha.clone();
            tokio::spawn(async move {
                let mut raw = vec![0; 16384];
                let count = socket.read(&mut raw).await.unwrap();
                let request = String::from_utf8_lossy(&raw[..count]);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let body = if path == "/user" {
                    json!({"login":"alice"})
                } else if path.starts_with("/search/issues?") {
                    json!({"total_count":0,"incomplete_results":false,"items":[]})
                } else if path == "/repos/acme/repo/git/ref/heads/main" {
                    json!({"ref":"refs/heads/main","object":{"type":"commit","sha":sha}})
                } else if path.starts_with("/repos/acme/repo/pulls/") {
                    let number = path.rsplit('/').next().unwrap().parse::<u64>().unwrap();
                    json!({"number":number,"state":"open","merged":false,"html_url":format!("https://github.com/acme/repo/pull/{number}"),"base":{"ref":"main","repo":{"full_name":"acme/repo"}},"head":{"ref":"feature","sha":sha,"repo":{"full_name":"acme/repo"}}})
                } else {
                    let number = path
                        .rsplit('/')
                        .next()
                        .unwrap_or("1")
                        .parse::<u64>()
                        .unwrap_or(1);
                    json!({"state":"open","title":"[AI] Runtime test","user":{"login":"alice"},"number":number,"html_url":format!("https://github.com/acme/repo/issues/{number}")})
                };
                let body = serde_json::to_vec(&body).unwrap();
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    let agent = dir.path().join("agent");
    std::fs::write(
        &agent,
        "#!/bin/sh\ncat >/dev/null\nsleep 30 &\necho CHILD:$!\nwait\n",
    )
    .unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Avoid GitHub network access while retaining real clone/checkout behavior.
    let git_wrapper = common::remote_git(dir.path(), &source);
    let factory = common::open_with_api(dir.path().join("data"), &address).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.token_profiles = vec![TokenProfile {
        id: "alice".into(),
        label: "Test".into(),
        account: "alice".into(),
        token: Some("fixture-token".into()),
        has_token: false,
        owners: vec!["acme".into()],
        authors: vec!["alice".into()],
        reviewers: vec!["alice".into()],
    }];
    settings.projects = vec![Project {
        id: "repo".into(),
        repository: "acme/repo".into(),
        source_path: source.to_string_lossy().into(),
    }];
    settings.commands.codex = agent.to_string_lossy().into();
    settings.commands.opencode = settings.commands.codex.clone();
    settings.commands.git = git_wrapper.to_string_lossy().into();
    for flow in &mut settings.flows {
        flow.nodes.truncate(1);
        flow.nodes[0].depends_on.clear();
    }
    factory.save_settings(settings).unwrap();
    (dir, factory, sha, server)
}

fn install_signalled_agent(dir: &Path) {
    let agent = dir.join("agent");
    std::fs::write(
        &agent,
        r#"#!/bin/sh
set -eu
cat >/dev/null
touch "$AI_FACTORY_MEMORY_DIR/started"
while [ ! -f "$AI_FACTORY_MEMORY_DIR/release" ]; do sleep 0.02; done
printf '%s' '{"status":"succeeded","summary":"finished","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn install_outcome_agent(dir: &Path) {
    let agent = dir.join("agent");
    std::fs::write(
        &agent,
        r#"#!/bin/sh
set -eu
cat >/dev/null
touch "$AI_FACTORY_MEMORY_DIR/started"
while [ ! -f "$AI_FACTORY_MEMORY_DIR/release" ]; do sleep 0.02; done
if [ -f "$AI_FACTORY_MEMORY_DIR/outcome" ]; then
  cat "$AI_FACTORY_MEMORY_DIR/outcome" > "$AI_FACTORY_RESULT_PATH"
else
  printf '%s' '{"status":"succeeded","summary":"finished","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH"
fi
"#,
    )
    .unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn run_memory(dir: &Path, id: &str) -> std::path::PathBuf {
    dir.join("data/memory/acme/repo/runs").join(id)
}

async fn started(dir: &Path, id: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !run_memory(dir, id).join("started").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn status(factory: &Factory, id: &str, expected: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while factory.run_detail(id).unwrap().run.status != expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

fn runtime_event(trigger: &str, number: u64, sha: &str) -> TriggerEvent {
    let mut event = TriggerEvent::issue("acme/repo", number, "[AI] Runtime test", "alice", "");
    event.trigger_id = trigger.into();
    if trigger != "new_issue" {
        event.head_sha = sha.into();
        event.event_id = format!("{trigger}:{number}");
        event.url = format!("https://github.com/acme/repo/pull/{number}");
    }
    event
}

async fn child(factory: &Factory, id: &str) -> Option<i32> {
    let detail = factory.run_detail(id).unwrap();
    let node = detail.nodes.first().unwrap();
    let logs = factory.logs(id, &node.id, 0).unwrap();
    logs.text
        .lines()
        .find_map(|line| line.strip_prefix("CHILD:").and_then(|pid| pid.parse().ok()))
}

async fn assert_gone(pid: i32) {
    for _ in 0..100 {
        if unsafe { libc::kill(pid, 0) } == -1 {
            return;
        }
        #[cfg(target_os = "linux")]
        if std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
            s.rsplit_once(')')
                .is_some_and(|(_, tail)| tail.trim_start().starts_with('Z'))
        }) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("descendant process {pid} survived cancellation");
}

#[tokio::test]
async fn disabled_workflow_does_not_stop_queued_or_running_runs() {
    let (dir, factory, _sha, server) = fixture().await;
    std::fs::write(dir.path().join("agent"),r#"#!/bin/sh
cat >/dev/null
touch "$AI_FACTORY_MEMORY_DIR/started"
while [ ! -f "$AI_FACTORY_MEMORY_DIR/release" ]; do sleep 0.02; done
printf '%s' '{"status":"succeeded","summary":"continued","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH"
"#).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.max_concurrent = 1;
    factory.save_settings(settings).unwrap();
    let mut ids = Vec::new();
    for n in [1, 2] {
        ids.push(
            factory
                .enqueue(TriggerEvent::issue(
                    "acme/repo",
                    n,
                    "[AI] Runtime test",
                    "alice",
                    "",
                ))
                .unwrap()
                .unwrap(),
        );
    }
    let snapshots: Vec<_> = ids
        .iter()
        .map(|id| serde_json::to_value(factory.run_detail(id).unwrap().flow).unwrap())
        .collect();
    factory.start();
    for (index, id) in ids.iter().enumerate() {
        let memory = dir.path().join("data/memory/acme/repo/runs").join(id);
        tokio::time::timeout(Duration::from_secs(10), async {
            while !memory.join("started").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        if index == 0 {
            assert_eq!(factory.snapshot().unwrap().metrics.running, 1);
            assert_eq!(factory.snapshot().unwrap().metrics.queued, 1);
            let mut settings = factory.settings().unwrap();
            settings.flows[0].enabled = false;
            settings.flows[0]
                .profile_activation
                .insert("alice".into(), false);
            factory.save_settings(settings).unwrap();
            assert_eq!(factory.snapshot().unwrap().metrics.running, 1);
            assert_eq!(factory.snapshot().unwrap().metrics.queued, 1);
        }
        assert_eq!(
            serde_json::to_value(factory.run_detail(id).unwrap().flow).unwrap(),
            snapshots[index]
        );
        std::fs::write(memory.join("release"), "").unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while factory.run_detail(id).unwrap().run.status != "succeeded" {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    factory.shutdown().await;
    server.abort();
    assert_eq!(factory.snapshot().unwrap().metrics.succeeded, 2);
}

#[tokio::test]
async fn four_real_processes_from_three_triggers_share_default_capacity() {
    let (_dir, factory, sha, server) = fixture().await;
    let mut ids = Vec::new();
    for (number, trigger) in [
        (1, "new_issue"),
        (2, "new_issue"),
        (3, "pr_approved"),
        (4, "pr_review"),
        (5, "new_issue"),
    ] {
        let mut event = TriggerEvent::issue("acme/repo", number, "[AI] Runtime test", "alice", "");
        event.trigger_id = trigger.into();
        if trigger != "new_issue" {
            event.head_sha = sha.clone();
            event.event_id = format!("{trigger}:{number}");
            event.url = format!("https://github.com/acme/repo/pull/{number}");
        }
        ids.push(factory.enqueue(event).unwrap().unwrap());
    }
    factory.start();
    let mut children = Vec::new();
    for _ in 0..300 {
        children.clear();
        for id in &ids {
            if let Some(pid) = child(&factory, id).await {
                children.push(pid);
            }
        }
        if children.len() == 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        children.len(),
        4,
        "four CLI child processes must actually overlap"
    );
    let snapshot = factory.snapshot().unwrap();
    assert_eq!(snapshot.metrics.running, 4);
    assert_eq!(snapshot.metrics.queued, 1);
    for run in snapshot.runs {
        if ["queued", "running"].contains(&run.status.as_str()) {
            factory.cancel(&run.id).unwrap();
        }
    }
    factory.shutdown().await;
    assert!(std::fs::read_dir(_dir.path().join("data/workspaces"))
        .unwrap()
        .next()
        .is_none());
    for pid in children {
        assert_gone(pid).await;
    }
    server.abort();
}

#[tokio::test]
async fn graceful_shutdown_stops_descendants_and_leaves_no_running_history() {
    let (_dir, factory, _, server) = fixture().await;
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            1,
            "[AI] Runtime test",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    let mut pid = None;
    for _ in 0..200 {
        pid = child(&factory, &id).await;
        if pid.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let pid = pid.expect("agent child started");
    factory.shutdown().await;
    assert_gone(pid).await;
    let detail = factory.run_detail(&id).unwrap();
    assert!(detail.run.workspace.is_none());
    assert!(!_dir.path().join("data/workspaces").join(&id).exists());
    assert!(
        !["queued", "running"].contains(&detail.run.status.as_str()),
        "graceful shutdown left status {}",
        detail.run.status
    );
    assert!(detail.nodes.iter().all(|node| node.status != "running"));
    server.abort();
}

#[tokio::test]
async fn approvals_wait_without_blocking_other_triggers_and_resume_in_stable_order() {
    let (dir, factory, sha, server) = fixture().await;
    install_signalled_agent(dir.path());
    let mut settings = factory.settings().unwrap();
    settings.poll_interval_seconds = 3600;
    factory.save_settings(settings).unwrap();
    let mut events = factory.subscribe();

    let first = factory
        .enqueue(runtime_event("pr_approved", 1, &sha))
        .unwrap()
        .unwrap();
    factory.start();
    started(dir.path(), &first).await;

    let second = factory
        .enqueue(runtime_event("pr_approved", 2, &sha))
        .unwrap()
        .unwrap();
    let third = factory
        .enqueue(runtime_event("pr_approved", 3, &sha))
        .unwrap()
        .unwrap();
    let mut other_runs = Vec::new();
    for (trigger, number) in [("new_issue", 4), ("pr_review", 5), ("new_issue", 6)] {
        let id = factory
            .enqueue(runtime_event(trigger, number, &sha))
            .unwrap()
            .unwrap();
        started(dir.path(), &id).await;
        other_runs.push(id);
    }

    assert_eq!(factory.snapshot().unwrap().metrics.running, 4);
    assert_eq!(factory.snapshot().unwrap().metrics.queued, 2);
    for id in [&second, &third] {
        let detail = factory.run_detail(id).unwrap();
        assert_eq!(detail.run.status, "queued");
        assert!(detail.run.started_at.is_none() && detail.run.workspace.is_none());
        assert!(detail.nodes.iter().all(|node| node.status == "pending"));
        assert!(!run_memory(dir.path(), id).join("started").exists());
        assert!(!dir.path().join("data/workspaces").join(id).exists());
    }
    let saved_flows: Vec<_> = [&second, &third]
        .iter()
        .map(|id| serde_json::to_value(factory.run_detail(id).unwrap().flow).unwrap())
        .collect();
    let saved_events: Vec<_> = [&second, &third]
        .iter()
        .map(|id| {
            let event = factory.run_detail(id).unwrap().event;
            (event.event_id, event.actor, event.head_sha)
        })
        .collect();
    let clone_args = std::fs::read_to_string(dir.path().join("clone-args.log")).unwrap();
    assert_eq!(
        clone_args.lines().filter(|line| *line == "clone").count(),
        4
    );

    let mut settings = factory.settings().unwrap();
    let approval = settings
        .flows
        .iter_mut()
        .find(|flow| flow.id == "pr_approved")
        .unwrap();
    approval.enabled = false;
    approval.profile_activation.insert("alice".into(), false);
    factory.save_settings(settings).unwrap();

    for id in &other_runs {
        std::fs::write(run_memory(dir.path(), id).join("release"), "").unwrap();
        status(&factory, id, "succeeded").await;
    }
    status(&factory, &first, "running").await;

    let mut pending = [second.clone(), third.clone()];
    pending.sort_by_key(|id| {
        let run = factory.run_detail(id).unwrap().run;
        (run.created_at, run.id)
    });
    std::fs::write(run_memory(dir.path(), &first).join("release"), "").unwrap();
    status(&factory, &first, "succeeded").await;
    for (index, id) in pending.iter().enumerate() {
        started(dir.path(), id).await;
        if index == 0 {
            assert_eq!(
                factory.run_detail(&pending[1]).unwrap().run.status,
                "queued"
            );
        }
        std::fs::write(run_memory(dir.path(), id).join("release"), "").unwrap();
        status(&factory, id, "succeeded").await;
    }

    for ((id, flow), event) in [&second, &third].iter().zip(saved_flows).zip(saved_events) {
        assert_eq!(
            serde_json::to_value(factory.run_detail(id).unwrap().flow).unwrap(),
            flow
        );
        let detail = factory.run_detail(id).unwrap();
        assert_eq!(
            (
                detail.event.event_id,
                detail.event.actor,
                detail.event.head_sha
            ),
            event
        );
    }
    let clone_args = std::fs::read_to_string(dir.path().join("clone-args.log")).unwrap();
    assert_eq!(
        clone_args.lines().filter(|line| *line == "clone").count(),
        6
    );

    let all_runs = [
        first.clone(),
        second.clone(),
        third.clone(),
        other_runs[0].clone(),
        other_runs[1].clone(),
        other_runs[2].clone(),
    ];
    let mut observed = std::collections::HashMap::<String, Vec<String>>::new();
    while let Ok(event) = events.try_recv() {
        if let Some(id) = event.run_id {
            observed.entry(id).or_default().push(event.kind);
        }
    }
    for id in all_runs {
        let kinds = observed.get(&id).expect("subscribed run events");
        assert_eq!(
            kinds.iter().filter(|kind| *kind == "run_created").count(),
            1
        );
        assert!(kinds.iter().filter(|kind| *kind == "run_changed").count() >= 2);
    }

    factory.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn blocked_and_failed_approvals_release_the_next_queued_worker() {
    for (outcome, terminal) in [("blocked", "blocked"), ("failed", "failed")] {
        let (dir, factory, sha, server) = fixture().await;
        install_outcome_agent(dir.path());
        let first = factory
            .enqueue(runtime_event("pr_approved", 1, &sha))
            .unwrap()
            .unwrap();
        factory.start();
        started(dir.path(), &first).await;
        let next = factory
            .enqueue(runtime_event("pr_approved", 2, &sha))
            .unwrap()
            .unwrap();
        assert_eq!(factory.run_detail(&next).unwrap().run.status, "queued");
        std::fs::write(
            run_memory(dir.path(), &first).join("outcome"),
            format!(
                "{{\"status\":\"{outcome}\",\"summary\":\"{outcome} fixture\",\"artifacts\":[],\"data\":{{}}}}"
            ),
        )
        .unwrap();
        std::fs::write(run_memory(dir.path(), &first).join("release"), "").unwrap();
        status(&factory, &first, terminal).await;
        started(dir.path(), &next).await;
        std::fs::write(run_memory(dir.path(), &next).join("release"), "").unwrap();
        status(&factory, &next, "succeeded").await;
        factory.shutdown().await;
        server.abort();
    }
}

#[tokio::test]
async fn cancelling_queued_approval_does_not_stop_active_descendant_and_next_runs_after_drain() {
    let (_dir, factory, sha, server) = fixture().await;
    let first = factory
        .enqueue(runtime_event("pr_approved", 1, &sha))
        .unwrap()
        .unwrap();
    factory.start();
    let pid = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(pid) = child(&factory, &first).await {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let cancelled = factory
        .enqueue(runtime_event("pr_approved", 2, &sha))
        .unwrap()
        .unwrap();
    factory.cancel(&cancelled).unwrap();
    assert_eq!(
        factory.run_detail(&cancelled).unwrap().run.status,
        "cancelled"
    );
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0);

    let next = factory
        .enqueue(runtime_event("pr_approved", 3, &sha))
        .unwrap()
        .unwrap();
    factory.cancel(&first).unwrap();
    assert_gone(pid).await;
    status(&factory, &first, "cancelled").await;
    let next_pid = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(pid) = child(&factory, &next).await {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(factory.run_detail(&next).unwrap().run.status, "running");
    factory.cancel(&next).unwrap();
    factory.shutdown().await;
    assert_gone(next_pid).await;
    server.abort();
}

#[tokio::test]
async fn global_capacity_one_still_serializes_other_triggers_with_approval() {
    let (dir, factory, sha, server) = fixture().await;
    install_signalled_agent(dir.path());
    let mut settings = factory.settings().unwrap();
    settings.max_concurrent = 1;
    factory.save_settings(settings).unwrap();
    let approval = factory
        .enqueue(runtime_event("pr_approved", 1, &sha))
        .unwrap()
        .unwrap();
    factory.start();
    started(dir.path(), &approval).await;
    let issue = factory
        .enqueue(runtime_event("new_issue", 2, &sha))
        .unwrap()
        .unwrap();
    assert_eq!(factory.run_detail(&issue).unwrap().run.status, "queued");
    assert!(!run_memory(dir.path(), &issue).join("started").exists());
    assert_eq!(factory.snapshot().unwrap().metrics.running, 1);

    std::fs::write(run_memory(dir.path(), &approval).join("release"), "").unwrap();
    status(&factory, &approval, "succeeded").await;
    started(dir.path(), &issue).await;
    std::fs::write(run_memory(dir.path(), &issue).join("release"), "").unwrap();
    status(&factory, &issue, "succeeded").await;
    factory.shutdown().await;
    server.abort();
}
