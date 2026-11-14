#[path = "core/codex_yolo.rs"]
mod codex_yolo;
mod common;
use factory_core::*;
use serde_json::json;

#[tokio::test]
#[cfg(unix)]
async fn read_only_agent_cannot_request_merge_or_cleanup_even_in_yolo_mode() {
    let (_dir, f) = fixture(
        r#"cat >/dev/null
printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{"merge":{"method":"squash"}}}' > "$AI_FACTORY_RESULT_PATH""#,
        4,
    );
    let mut settings = f.settings().unwrap();
    settings.codex_yolo = true;
    settings.flows[0].nodes[0].access = "read".into();
    settings.flows[0].nodes[0].result_schema = json!({"type":"object"});
    f.save_settings(settings).unwrap();
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            91,
            "[AI] readonly",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.start();
    let detail = terminal(&f, &id).await;
    f.shutdown().await;
    assert_eq!(detail.run.status, "failed");
    let text = serde_json::to_string(&detail).unwrap();
    assert!(
        text.contains("Read-only node cannot publish, merge or comment"),
        "{text}"
    );
}

#[test]
fn defaults_cover_three_triggers_and_concurrency() {
    let s = Settings::default();
    assert_eq!(s.max_concurrent, 4);
    for trigger in ["new_issue", "pr_review", "pr_approved"] {
        assert_eq!(s.concurrency_for(trigger).unwrap(), 4);
    }
    assert_eq!(s.flows.len(), 3);
    s.validate().unwrap();
}

#[test]
fn default_review_prompts_match_static_and_test_execution_access() {
    let settings = Settings::default();
    for flow in &settings.flows {
        for node in &flow.nodes {
            if ["plan_review", "review_code", "review_architecture"].contains(&node.id.as_str()) {
                assert_eq!(node.access, "read");
                assert!(node.prompt.contains("read-only inspection"), "{}", node.id);
                assert!(node.prompt.contains("Do not modify"), "{}", node.id);
            }
            if node.id == "review_tests" || flow.id == "pr_review" {
                assert_eq!(node.access, "write", "{}/{}", flow.id, node.id);
                assert!(node.prompt.contains("caches and logs"));
                assert!(node.prompt.contains("Do not modify"));
            }
            if flow.id == "pr_review" {
                assert!(node.prompt.contains("APPROVE requires status=succeeded"));
                assert!(node
                    .prompt
                    .contains("REQUEST_CHANGES requires status=blocked"));
                assert!(node.prompt.contains("COMMENT with status=blocked"));
            }
        }
    }
    settings.validate().unwrap();
}

#[test]
fn approval_gate_schemas_allow_reports_but_reject_github_action_keys() {
    let settings = Settings::default();
    for node in &settings.flows[0].nodes {
        if node.result_schema["properties"].get("approved").is_none() {
            continue;
        }
        let validator = jsonschema::validator_for(&node.result_schema).unwrap();
        assert!(validator.is_valid(&json!({
            "approved": true,
            "review_details": {"architecture": "checked", "scope": "checked"},
            "findings": [],
            "verification": {"mode": "static_read_only"}
        })));
        for key in ["publish", "update_pr", "merge", "review", "comment"] {
            let mut data = json!({"approved": true});
            data[key] = json!({"architecture": "descriptive report"});
            assert!(
                !validator.is_valid(&data),
                "{}/{} accepted {key}",
                settings.flows[0].id,
                node.id
            );
        }
    }
}

#[tokio::test]
#[cfg(unix)]
async fn plan_review_report_cannot_be_dispatched_as_github_review() {
    for field in ["review_details", "review"] {
        let script = format!(
            "cat >/dev/null\nprintf '%s' '{}' > \"$AI_FACTORY_RESULT_PATH\"",
            json!({"status":"succeeded","summary":"approved plan","artifacts":[],"data":{"approved":true,field:{"architecture":"checked"}}})
        );
        let (_dir, factory) = fixture(&script, 4);
        let mut settings = factory.settings().unwrap();
        let mut gate = Settings::default().flows[0]
            .nodes
            .iter()
            .find(|n| n.id == "plan_review")
            .unwrap()
            .clone();
        gate.depends_on.clear();
        // Preserve the fixture's isolated Codex account binding.
        gate.codex_account_id = settings.flows[0].nodes[0].codex_account_id.clone();
        settings.flows[0].nodes = vec![gate];
        factory.save_settings(settings).unwrap();
        let id = factory
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                420,
                "[AI] plan report",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        factory.start();
        let detail = terminal(&factory, &id).await;
        factory.shutdown().await;
        if field == "review_details" {
            assert_eq!(detail.run.status, "succeeded", "{:?}", detail.nodes);
            assert_eq!(
                detail.nodes[0].result.as_ref().unwrap()["data"]["approved"],
                true
            );
        } else {
            assert_eq!(detail.run.status, "failed");
            let error = detail.nodes[0].error.as_deref().unwrap();
            assert!(error.contains("Invalid first node response"), "{error}");
            assert!(error.contains("/data/review"), "{error}");
            assert!(
                !error.contains("Only review flow submits reviews"),
                "{error}"
            );
        }
    }
}

#[test]
fn settings_ignore_and_drop_legacy_agent_timeout() {
    for timeout in [0, 1, 3600] {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value["node_timeout_seconds"] = json!(timeout);
        let settings: Settings = serde_json::from_value(value).unwrap();
        settings.validate().unwrap();
        assert!(serde_json::to_value(settings)
            .unwrap()
            .get("node_timeout_seconds")
            .is_none());
    }
}

#[tokio::test]
#[cfg(unix)]
async fn agents_complete_after_legacy_timeout_for_both_clis() {
    for agent in ["codex", "opencode"] {
        let (_dir, f) = fixture(
            r#"cat >/dev/null
echo READY
sleep 2
printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH""#,
            4,
        );
        let mut value = serde_json::to_value(f.settings().unwrap()).unwrap();
        value["node_timeout_seconds"] = json!(1);
        value["flows"][0]["nodes"][0]["agent"] = json!(agent);
        f.save_settings(serde_json::from_value(value).unwrap())
            .unwrap();
        let id = f
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                172,
                "[AI] long task",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        f.start();
        // Workspace preparation is separate from the agent's execution budget.
        wait_for_cli_marker(&f, &id, "READY").await;
        let detail = terminal(&f, &id).await;
        f.shutdown().await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
    }
}
#[test]
fn graph_rejects_cycles_unknown_dependencies_and_bad_schemas() {
    let mut s = Settings::default();
    s.flows[0].nodes[0].depends_on = vec!["missing".into()];
    assert!(s.validate().is_err());
    s.flows[0].nodes[0].depends_on = vec![s.flows[0].nodes[0].id.clone()];
    assert!(s.validate().is_err());
    s.flows[0].nodes[0].depends_on.clear();
    s.flows[0].nodes[0].result_schema = json!({"type":"nonsense"});
    assert!(s.validate().is_err());
}
#[tokio::test]
async fn durable_queue_dedup_redaction_and_immediate_events() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open(dir.path().into()).unwrap();
    let mut s = f.settings().unwrap();
    s.token_profiles.push(TokenProfile {
        id: "one".into(),
        label: "Primary".into(),
        account: "alice".into(),
        token: Some("secret-example".into()),
        has_token: false,
        ..Default::default()
    });
    f.save_settings(s).unwrap();
    assert!(!serde_json::to_string(&f.settings().unwrap())
        .unwrap()
        .contains("secret-example"));
    let mut rx = f.subscribe();
    let event = TriggerEvent::issue("acme/repo", 1, "[AI] test", "alice", "");
    let id = f.enqueue(event.clone()).unwrap().unwrap();
    assert_eq!(rx.recv().await.unwrap().kind, "run_created");
    assert!(f.enqueue(event).unwrap().is_none());
    assert_eq!(f.snapshot().unwrap().runs[0].status, "queued");
    drop(f);
    let f = common::open(dir.path().into()).unwrap();
    assert_eq!(f.run_detail(&id).unwrap().run.status, "queued");
    assert!(f.settings().unwrap().token_profiles[0].has_token);
}
#[test]
fn restart_interrupts_running_but_retains_history() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open(dir.path().into()).unwrap();
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            2,
            "[AI] test",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.set_run_status(&id, "running", None).unwrap();
    drop(f);
    let f = common::open(dir.path().into()).unwrap();
    assert_eq!(f.run_detail(&id).unwrap().run.status, "interrupted");
    assert_ne!(f.retry(&id).unwrap(), id);
}

#[cfg(unix)]
struct FixtureDir {
    dir: tempfile::TempDir,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    server: Option<std::thread::JoinHandle<()>>,
    requests: std::sync::Arc<std::sync::Mutex<Vec<(String, String, serde_json::Value)>>>,
    comment_status: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
#[cfg(unix)]
impl std::ops::Deref for FixtureDir {
    type Target = tempfile::TempDir;
    fn deref(&self) -> &Self::Target {
        &self.dir
    }
}
#[cfg(unix)]
impl Drop for FixtureDir {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(task) = self.server.take() {
            task.join().expect("HTTP fixture shutdown");
        }
    }
}
#[cfg(unix)]
fn fixture(script: &str, limit: usize) -> (FixtureDir, std::sync::Arc<Factory>) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir(&source).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(&source)
            .output()
            .unwrap()
            .status
            .success());
    }
    let cli = dir.path().join("fake-agent");
    std::fs::write(&cli, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let comment_status = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(200));
    let (halt, calls, status, root) = (
        stop.clone(),
        requests.clone(),
        comment_status.clone(),
        dir.path().join("data"),
    );
    let server = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let mut comments = Vec::new();
        while !halt.load(std::sync::atomic::Ordering::SeqCst) {
            let mut stream = match listener.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                Err(_) => break,
            };
            // macOS accepts inherit the nonblocking listener flag. Read the
            // complete HTTP request with the bounded blocking timeouts below.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                .unwrap();
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(1)))
                .unwrap();
            let mut raw = Vec::new();
            let parsed = (|| -> Option<(String, String, serde_json::Value)> {
                let (start, length) = loop {
                    let mut bytes = [0; 4096];
                    let n = stream.read(&mut bytes).ok()?;
                    if n == 0 {
                        return None;
                    }
                    raw.extend_from_slice(&bytes[..n]);
                    if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&raw[..end]);
                        let length = headers
                            .lines()
                            .filter_map(|s| s.split_once(':'))
                            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, n)| n.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while raw.len() < start + length {
                    let mut bytes = [0; 4096];
                    let n = stream.read(&mut bytes).ok()?;
                    if n == 0 {
                        return None;
                    }
                    raw.extend_from_slice(&bytes[..n]);
                }
                let headers = String::from_utf8_lossy(&raw[..start]);
                let mut first = headers.lines().next()?.split_whitespace();
                Some((
                    first.next()?.into(),
                    first.next()?.into(),
                    if length > 0 {
                        serde_json::from_slice(&raw[start..start + length]).ok()?
                    } else {
                        serde_json::Value::Null
                    },
                ))
            })();
            let Some((method, path, input)) = parsed else {
                continue;
            };
            calls
                .lock()
                .unwrap()
                .push((method.clone(), path.clone(), input.clone()));
            let is_comments =
                path.starts_with("/repos/acme/repo/issues/") && path.contains("/comments");
            let mut code = 200;
            let body = if path == "/user" {
                json!({"login":"alice"})
            } else if is_comments {
                code = status.load(std::sync::atomic::Ordering::SeqCst);
                if code != 200 {
                    json!({})
                } else if method == "GET" {
                    json!(comments)
                } else {
                    if input["body"]
                        .as_str()
                        .is_some_and(|b| b.contains("<!-- ai-factory:node-result:v1:"))
                    {
                        let db = rusqlite::Connection::open(root.join("factory.sqlite3")).unwrap();
                        assert!(
                            issue_node_comments::committed_comment_snapshot(
                                &db,
                                input["body"].as_str().unwrap(),
                            ),
                            "POST without matching committed terminal snapshot"
                        );
                    }
                    let row = json!({"id":9001+comments.len(),"body":input["body"],"user":{"login":"alice"}});
                    comments.push(row.clone());
                    row
                }
            } else {
                json!({"state":"open","title":"[AI] work","user":{"login":"alice"}})
            };
            let body = body.to_string();
            let _=write!(stream,"HTTP/1.1 {code} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
        }
    });
    let f = common::open_with_api(dir.path().join("data"), &base).unwrap();
    let mut s = f.settings().unwrap();
    s.commands.codex = cli.to_string_lossy().into();
    s.commands.opencode = s.commands.codex.clone();
    s.commands.git = common::remote_git(dir.path(), &source)
        .to_string_lossy()
        .into();
    s.max_concurrent = limit;
    s.token_profiles.push(TokenProfile {
        id: "alice".into(),
        label: "Alice".into(),
        account: "alice".into(),
        token: Some("dummy-token".into()),
        has_token: false,
        owners: vec!["acme".into()],
        authors: vec!["alice".into()],
        ..Default::default()
    });
    s.flows[1].enabled = false;
    s.flows[2].enabled = false;
    s.projects.push(Project {
        id: "repo".into(),
        repository: "acme/repo".into(),
        source_path: source.to_string_lossy().into(),
    });
    s.flows[0].nodes.truncate(1);
    s.flows[0].nodes[0].depends_on.clear();
    common::bind_fixture_codex(&f, &mut s);
    f.save_settings(s).unwrap();
    (
        FixtureDir {
            dir,
            stop,
            server: Some(server),
            requests,
            comment_status,
        },
        f,
    )
}
#[cfg(unix)]
async fn terminal(f: &Factory, id: &str) -> RunDetail {
    for _ in 0..200 {
        let d = f.run_detail(id).unwrap();
        if !["queued", "running"].contains(&d.run.status.as_str()) {
            if d.run.status == "failed" && d.nodes.iter().all(|n| n.status == "skipped") {
                eprintln!("Fixture preparation failed: {:?}", d.run.error);
            }
            return d;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let detail = f.run_detail(id).unwrap();
    let logs = f.logs(id, "spec", 0).unwrap().text;
    f.shutdown().await;
    panic!("run did not finish: {detail:?}\nCLI log:\n{logs}")
}
#[cfg(unix)]
async fn wait_for_cli_marker(f: &Factory, id: &str, marker: &str) -> String {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let logs = f.logs(id, "spec", 0).unwrap().text;
        if logs.lines().any(|line| line.starts_with(marker)) {
            return logs;
        }
        let detail = f.run_detail(id).unwrap();
        if !["queued", "running"].contains(&detail.run.status.as_str())
            || tokio::time::Instant::now() >= deadline
        {
            f.shutdown().await;
            panic!("CLI did not reach {marker}: {detail:?}\nCLI log:\n{logs}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
#[tokio::test]
#[cfg(unix)]
async fn per_token_agent_override_drives_cli_without_changing_other_nodes() {
    let (_dir, factory) = fixture(
        "cat >/dev/null\nprintf '%s\\n' \"$@\" >&2\nprintf '%s' '{\"status\":\"succeeded\",\"summary\":\"done\",\"artifacts\":[],\"data\":{}}' > \"$AI_FACTORY_RESULT_PATH\"",
        4,
    );
    let mut value = serde_json::to_value(factory.settings().unwrap()).unwrap();
    let mut second = value["flows"][0]["nodes"][0].clone();
    second["id"] = json!("second");
    second["depends_on"] = json!(["spec"]);
    second["model"] = json!("default-model");
    second["effort"] = json!("low");
    value["flows"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(second);
    value["flows"][0]["nodes"][0]["token_overrides"] = json!({
        "alice": {"agent":"opencode", "model":"provider/override", "effort":"deep"}
    });
    factory
        .save_settings(serde_json::from_value(value).unwrap())
        .unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            190,
            "[AI] config",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "succeeded", "{:?}", detail.nodes);
    assert_eq!(detail.nodes[0].agent, "opencode");
    assert_eq!(detail.nodes[1].agent, "codex");
    let first = factory.logs(&id, "spec", 0).unwrap().text;
    let second = factory.logs(&id, "second", 0).unwrap().text;
    assert!(first.contains("run\n"), "{first}");
    assert!(first.contains("provider/override#deep"), "{first}");
    assert!(second.contains("exec\n"), "{second}");
    assert!(second.contains("default-model"), "{second}");
    assert!(
        second.contains("model_reasoning_effort=\"low\""),
        "{second}"
    );
    assert!(!second.contains("provider/override"));
}

#[tokio::test]
#[cfg(unix)]
async fn cli_result_logs_and_workspace_are_real_and_private() {
    let (_dir,f)=fixture("cat >/dev/null\necho '{\"type\":\"thread.started\",\"thread_id\":\"session-1\"}'\necho log-line >&2\nprintf '%s' '{\"status\":\"succeeded\",\"summary\":\"done\",\"artifacts\":[],\"data\":{}}' > \"$AI_FACTORY_RESULT_PATH\"",4);
    let mut rx = f.subscribe();
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            10,
            "[AI] work",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.start();
    let d = terminal(&f, &id).await;
    assert_eq!(d.run.status, "succeeded", "{:?}", d.run.error);
    assert_eq!(d.nodes[0].session_id.as_deref(), Some("session-1"));
    assert!(d.run.workspace.is_none());
    assert!(f.logs(&id, "spec", 0).unwrap().text.contains("log-line"));
    let mut seen = false;
    while let Ok(e) = rx.try_recv() {
        seen |= e.kind == "log";
    }
    assert!(seen);
    f.shutdown().await;
}

#[tokio::test]
#[cfg(unix)]
async fn remote_clones_are_temporary_and_each_retry_gets_its_own_directory() {
    let (dir, f) = fixture(
        r#"cat >/dev/null
test -d .git || exit 71
pwd > "$AI_FACTORY_MEMORY_DIR/workspace-path"
git remote get-url origin > "$AI_FACTORY_MEMORY_DIR/origin"
printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH""#,
        4,
    );
    let mut settings = f.settings().unwrap();
    settings.projects.clear();
    f.save_settings(settings).unwrap();
    let first = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            101,
            "[AI] work",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.start();
    assert_eq!(terminal(&f, &first).await.run.status, "succeeded");
    let second = f.retry(&first).unwrap();
    assert_eq!(terminal(&f, &second).await.run.status, "succeeded");
    f.shutdown().await;
    for id in [&first, &second] {
        let expected = dir
            .path()
            .join("data")
            .canonicalize()
            .unwrap()
            .join("workspaces")
            .join(id);
        let memory = dir.path().join("data/memory/acme/repo/runs").join(id);
        assert_eq!(
            std::fs::read_to_string(memory.join("workspace-path"))
                .unwrap()
                .trim(),
            expected.to_str().unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(memory.join("origin"))
                .unwrap()
                .trim(),
            "https://github.com/acme/repo.git"
        );
        assert!(!expected.exists());
        assert!(f.run_detail(id).unwrap().run.workspace.is_none());
    }
    let clones = std::fs::read_to_string(dir.path().join("clone-args.log")).unwrap();
    assert_eq!(
        clones.matches("https://github.com/acme/repo.git").count(),
        2
    );
    assert!(dir.path().join("source/.git").exists());
}
#[tokio::test]
#[cfg(unix)]
async fn every_agent_role_receives_unattended_instructions_and_noninteractive_launch() {
    for agent in ["codex", "opencode"] {
        let (dir, f) = fixture(
            r#"cat > "${AI_FACTORY_RESULT_PATH%.result.json}.stdin.txt"
printf '%s\n' "$@" > "${AI_FACTORY_RESULT_PATH%.result.json}.args.txt"
printf '%s' "$OPENCODE_CONFIG_CONTENT" > "${AI_FACTORY_RESULT_PATH%.result.json}.config.json"
printf '%s' '{"status":"succeeded","summary":"role complete","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH""#,
            4,
        );
        let mut settings = f.settings().unwrap();
        let template = settings.flows[0].nodes[0].clone();
        settings.flows[0].nodes = ["spec", "planner", "builder", "reviewer"]
            .into_iter()
            .enumerate()
            .map(|(index, role)| {
                let mut node = template.clone();
                node.id = role.into();
                node.agent = agent.into();
                node.access = if role == "reviewer" { "read" } else { "write" }.into();
                node.prompt = format!("Act as {role}. Ask the user to approve before continuing. No ejecutes comandos ni delegues.");
                node.depends_on = if index == 0 {
                    vec![]
                } else {
                    vec![["spec", "planner", "builder"][index - 1].into()]
                };
                node
            })
            .collect();
        f.save_settings(settings).unwrap();
        let id = f
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                40,
                "[AI] unattended",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        f.start();
        let detail = terminal(&f, &id).await;
        f.shutdown().await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        for role in ["spec", "planner", "builder", "reviewer"] {
            let session = dir.path().join("data/sessions").join(&id);
            let prompt =
                std::fs::read_to_string(session.join(format!("{role}.stdin.txt"))).unwrap();
            assert!(
                prompt.contains("Do not ask the user questions or request approval"),
                "{agent}/{role}: missing unattended contract"
            );
            assert!(prompt.contains("downstream workflow steps"));
            assert!(prompt.contains("skills"));
            assert!(prompt.contains("Do not launch or open GUI applications"));
            assert!(prompt.contains("Code-level verification is allowed"));
            assert!(prompt.contains("do not block only because a GUI check was not performed"));
            assert!(prompt.contains("Installer packaging policy"));
            assert!(prompt
                .contains("overrides older node prompts, generated plans and reviewer feedback"));
            assert!(prompt.contains(
                "explicit installer or distribution deliverable in the original user task"
            ));
            assert!(prompt.contains("status=blocked"));
            assert!(prompt.contains("Do not expand the assigned role"));
            assert!(prompt.contains("Read-only inspection is allowed for every assigned role"));
            assert!(prompt.contains("When an older node prompt forbids commands"));
            assert!(prompt
                .contains("Reading an artifact path is not the same as receiving its contents"));
            assert!(prompt.contains("For read access, put findings in summary/data"));
            assert!(prompt.contains("Analysis-only nodes without an approved field"));
            assert!(prompt.contains("Reserved action keys in node data"));
            assert!(prompt.contains("review_details"));
            assert!(!prompt.contains("If review rejects changes return status=blocked."));
            assert!(prompt.contains(
                "Never modify source repository or files outside authorized workspace/memory"
            ));
            assert!(
                prompt
                    .find("AI Factory unattended execution contract")
                    .unwrap()
                    > prompt.find("Ask the user to approve").unwrap()
            );
            let args = std::fs::read_to_string(session.join(format!("{role}.args.txt"))).unwrap();
            if agent == "codex" {
                assert!(args.contains("approval_policy=\"never\""));
                assert!(args.contains(if role == "reviewer" {
                    "read-only"
                } else {
                    "workspace-write"
                }));
            } else {
                assert!(args.contains("--auto"));
                let config: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(session.join(format!("{role}.config.json"))).unwrap(),
                )
                .unwrap();
                for permission in [
                    &config["permission"],
                    &config["agent"]["ai-factory"]["permission"],
                ] {
                    assert_eq!(permission["question"], "deny");
                    assert_eq!(permission["external_directory"]["*"], "deny");
                    if role == "reviewer" {
                        assert_eq!(permission["*"], "deny");
                    }
                }
            }
        }
    }
}
#[tokio::test]
#[cfg(unix)]
async fn concurrency_limit_and_cancellation_stop_processes() {
    let (_dir, f) = fixture("cat >/dev/null\necho launched\nsleep 10", 2);
    let mut ids = vec![];
    for n in 1..=3 {
        ids.push(
            f.enqueue(TriggerEvent::issue(
                "acme/repo",
                n,
                "[AI] work",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap(),
        );
    }
    f.start();
    for _ in 0..100 {
        if f.snapshot().unwrap().metrics.running == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let s = f.snapshot().unwrap();
    assert_eq!(s.metrics.running, 2);
    assert_eq!(s.metrics.queued, 1);
    f.cancel(&ids[0]).unwrap();
    assert_eq!(terminal(&f, &ids[0]).await.run.status, "cancelled");
    f.shutdown().await;
}
#[tokio::test]
#[cfg(unix)]
async fn blocked_result_skips_descendants() {
    let (_dir,f)=fixture("cat >/dev/null\necho '{\"status\":\"succeeded\",\"summary\":\"bad approval\",\"artifacts\":[],\"data\":{\"approved\":false}}' > \"$AI_FACTORY_RESULT_PATH\"",4);
    let mut s = f.settings().unwrap();
    let mut child = s.flows[0].nodes[0].clone();
    child.id = "child".into();
    child.depends_on = vec!["spec".into()];
    s.flows[0].nodes.push(child);
    f.save_settings(s).unwrap();
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            1,
            "[AI] work",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.start();
    let d = terminal(&f, &id).await;
    assert_eq!(d.run.status, "failed");
    assert_eq!(d.nodes[1].status, "skipped");
    f.shutdown().await;
}
#[test]
fn terminal_cancellation_cannot_be_overwritten_by_late_worker_status() {
    let root = tempfile::tempdir().unwrap();
    let f = common::open(root.path().into()).unwrap();
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            11,
            "[AI] work",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.cancel(&id).unwrap();
    f.set_run_status(&id, "running", None).unwrap();
    assert_eq!(f.run_detail(&id).unwrap().run.status, "cancelled");
    f.set_run_status(&id, "succeeded", None).unwrap();
    assert_eq!(f.run_detail(&id).unwrap().run.status, "cancelled");
}
#[tokio::test]
#[cfg(unix)]
async fn every_agent_uses_its_workspace_as_process_and_logical_directory() {
    for agent in ["codex", "opencode"] {
        for access in ["read", "write"] {
            let (dir, f) = fixture("exit 1", 4);
            // A shell normalizes PWD on startup and would hide the inherited-directory bug.
            std::fs::write(
                dir.path().join("fake-agent"),
                r#"#!/usr/bin/env python3
import json, os, sys
sys.stdin.read()
result_path = os.environ["AI_FACTORY_RESULT_PATH"]
with open(result_path + ".launch.json", "w") as output:
    json.dump({"cwd": os.getcwd(), "pwd": os.environ.get("PWD")}, output)
with open(result_path, "w") as output:
    json.dump({"status": "succeeded", "summary": "done", "artifacts": [], "data": {}}, output)
"#,
            )
            .unwrap();
            let mut settings = f.settings().unwrap();
            let node = &mut settings.flows[0].nodes[0];
            node.agent = agent.into();
            node.access = access.into();
            let node_id = node.id.clone();
            f.save_settings(settings).unwrap();
            let id = f
                .enqueue(TriggerEvent::issue(
                    "acme/repo",
                    41,
                    "[AI] workspace directory",
                    "alice",
                    "",
                ))
                .unwrap()
                .unwrap();
            f.start();
            let detail = terminal(&f, &id).await;
            f.shutdown().await;
            assert_eq!(detail.run.status, "succeeded", "{agent}/{access}");
            let launch: serde_json::Value = serde_json::from_slice(
                &std::fs::read(
                    dir.path()
                        .join("data/sessions")
                        .join(&id)
                        .join(format!("{node_id}.result.json.launch.json")),
                )
                .unwrap(),
            )
            .unwrap();
            let workspace = dir
                .path()
                .canonicalize()
                .unwrap()
                .join("data/workspaces")
                .join(&id);
            assert_eq!(
                launch["cwd"],
                workspace.to_string_lossy().as_ref(),
                "{agent}/{access}: process must run in its workspace"
            );
            assert_eq!(
                launch["pwd"],
                workspace.to_string_lossy().as_ref(),
                "{agent}/{access}: logical directory must match its workspace"
            );
        }
    }
}
#[tokio::test]
#[cfg(unix)]
async fn opencode_uses_private_server_and_path_scoped_memory_permissions() {
    let (_dir,f)=fixture("cat >/dev/null\ncase \"$*\" in *--standalone*) ;; *) exit 44;; esac\ncase \"$OPENCODE_CONFIG_CONTENT\" in *external_directory*allow*) ;; *) exit 45;; esac\necho '{\"type\":\"text\",\"part\":{\"text\":\"{\\\"status\\\":\\\"succeeded\\\",\\\"summary\\\":\\\"done\\\",\\\"artifacts\\\":[],\\\"data\\\":{}}\"}}'",4);
    let mut s = f.settings().unwrap();
    s.flows[0].nodes[0].agent = "opencode".into();
    f.save_settings(s).unwrap();
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            19,
            "[AI] OpenCode",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.start();
    let d = terminal(&f, &id).await;
    assert_eq!(d.run.status, "succeeded", "{:?} {:?}", d.run.error, d.nodes);
    f.shutdown().await;
}
#[tokio::test]
#[cfg(unix)]
async fn codex_strict_transport_decodes_custom_data_before_validation() {
    let (_dir,f)=fixture("cat >/dev/null\necho '{\"status\":\"succeeded\",\"summary\":\"done\",\"artifacts\":[],\"data_json\":\"{\\\"approved\\\":true}\"}' > \"$AI_FACTORY_RESULT_PATH\"",4);
    let mut s = f.settings().unwrap();
    s.flows[0].nodes[0].result_schema = serde_json::json!({"type":"object","required":["approved"],"properties":{"approved":{"type":"boolean"}}});
    f.save_settings(s).unwrap();
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            20,
            "[AI] schema",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.start();
    let d = terminal(&f, &id).await;
    assert_eq!(d.run.status, "succeeded", "{:?}", d.nodes);
    assert_eq!(
        d.nodes[0].result.as_ref().unwrap()["data"]["approved"],
        true
    );
    f.shutdown().await;
}
#[tokio::test]
#[cfg(unix)]
async fn jobs_isolate_mutable_memory_and_git_identity_matches_actor() {
    let (_dir,f)=fixture("cat >/dev/null\necho identity:$GIT_AUTHOR_NAME:$GIT_COMMITTER_NAME\necho memory:$AI_FACTORY_MEMORY_DIR\necho '{\"status\":\"succeeded\",\"summary\":\"done\",\"artifacts\":[],\"data\":{}}' > \"$AI_FACTORY_RESULT_PATH\"",4);
    let mut ids = vec![];
    for n in 30..32 {
        ids.push(
            f.enqueue(TriggerEvent::issue(
                "acme/repo",
                n,
                "[AI] memory",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap(),
        );
    }
    f.start();
    for id in &ids {
        assert_eq!(terminal(&f, id).await.run.status, "succeeded");
    }
    let logs: Vec<_> = ids
        .iter()
        .map(|id| f.logs(id, "spec", 0).unwrap().text)
        .collect();
    assert!(logs[0].contains("identity:alice:alice"), "{}", logs[0]);
    let paths: Vec<_> = logs
        .iter()
        .map(|l| {
            l.lines()
                .find(|l| l.starts_with("memory:"))
                .unwrap()
                .to_string()
        })
        .collect();
    assert_ne!(paths[0], paths[1]);
    f.shutdown().await;
}
#[test]
fn application_data_directory_has_one_scheduler_owner() {
    let dir = tempfile::tempdir().unwrap();
    let first = common::open(dir.path().into()).unwrap();
    assert!(common::open(dir.path().into()).is_err());
    drop(first);
    assert!(common::open(dir.path().into()).is_ok());
}
#[test]
fn settings_normalize_github_identity_case_and_preserve_blank_token_edits() {
    let root = tempfile::tempdir().unwrap();
    let f = common::open(root.path().into()).unwrap();
    let mut s = f.settings().unwrap();
    s.token_profiles.push(TokenProfile {
        id: "actor".into(),
        label: "Alice".into(),
        account: "Alice".into(),
        token: Some("token-test".into()),
        has_token: false,
        owners: vec!["Acme".into(), "acme".into()],
        authors: vec!["Alice".into()],
        reviewers: vec!["Alice".into()],
    });
    let s = f.save_settings(s).unwrap();
    assert_eq!(s.token_profiles[0].owners, vec!["acme"]);
    assert_eq!(s.token_profiles[0].authors, vec!["alice"]);
    assert_eq!(s.token_profiles[0].reviewers, vec!["alice"]);
    assert_eq!(s.token_profiles[0].account, "alice");
    let mut s = s;
    s.token_profiles[0].token = Some(String::new());
    assert!(f.save_settings(s).unwrap().token_profiles[0].has_token);
}

#[tokio::test]
#[cfg(unix)]
async fn exited_cli_cannot_leave_descendant_holding_log_pipes() {
    let (_dir, f) = fixture("cat >/dev/null\nsleep 30 &\necho CHILD:$!\nwhile [ ! -f .finish-cli ]; do sleep 0.01; done\nprintf '%s' '{\"status\":\"succeeded\",\"summary\":\"done\",\"artifacts\":[],\"data\":{}}' > \"$AI_FACTORY_RESULT_PATH\"", 4);
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            90,
            "[AI] work",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.start();
    // Start the existing two-second completion assertion only after the CLI has
    // spawned its descendant. The handshake keeps it alive until we can observe
    // the child, independently of clone/setup latency.
    let text = wait_for_cli_marker(&f, &id, "CHILD:").await;
    let pid: i32 = text
        .lines()
        .find_map(|l| l.strip_prefix("CHILD:"))
        .unwrap()
        .parse()
        .unwrap();
    let workspace = f.run_detail(&id).unwrap().run.workspace.unwrap();
    std::fs::write(std::path::Path::new(&workspace).join(".finish-cli"), "").unwrap();
    let done = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let d = f.run_detail(&id).unwrap();
            if d.run.status == "succeeded" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    // Always cancel/clean up the fixture, including on the regression path.
    let detail = f.run_detail(&id).unwrap();
    let text = f.logs(&id, "spec", 0).unwrap().text;
    let mut alive = true;
    for _ in 0..100 {
        alive = unsafe { libc::kill(pid, 0) } == 0;
        #[cfg(target_os = "linux")]
        if std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
            s.rsplit_once(')')
                .is_some_and(|(_, tail)| tail.trim_start().starts_with('Z'))
        }) {
            alive = false; // Terminated orphan awaiting the container init's reap.
        }
        if !alive {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    if done.is_err() {
        let _ = f.cancel(&id);
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    f.shutdown().await;
    assert!(
        done.is_ok(),
        "CLI exited but inherited output pipes blocked completion: {detail:?}\nCLI log:\n{text}"
    );
    assert!(!alive, "descendant survived its CLI session");
}

#[test]
fn queued_snapshot_is_immutable_and_retry_uses_current_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open(dir.path().into()).unwrap();
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            101,
            "[AI] snapshot",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    let original = f.run_detail(&id).unwrap().flow.nodes[0].prompt.clone();
    let mut settings = f.settings().unwrap();
    settings.flows[0].nodes[0].prompt = "Updated instructions".into();
    f.save_settings(settings).unwrap();
    assert_eq!(f.run_detail(&id).unwrap().flow.nodes[0].prompt, original);
    f.set_run_status(&id, "failed", None).unwrap();
    let retry = f.retry(&id).unwrap();
    assert_eq!(
        f.run_detail(&retry).unwrap().flow.nodes[0].prompt,
        "Updated instructions"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn independent_writer_nodes_never_overlap_in_one_workspace() {
    let (_dir, f) = fixture("cat >/dev/null\nmkdir .writer-active || exit 17\nsleep 0.1\nrmdir .writer-active\nprintf '%s' '{\"status\":\"succeeded\",\"summary\":\"done\",\"artifacts\":[],\"data\":{}}' > \"$AI_FACTORY_RESULT_PATH\"", 4);
    let mut settings = f.settings().unwrap();
    let mut second = settings.flows[0].nodes[0].clone();
    second.id = "second".into();
    settings.flows[0].nodes.push(second);
    f.save_settings(settings).unwrap();
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            102,
            "[AI] writers",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.start();
    let d = terminal(&f, &id).await;
    f.shutdown().await;
    assert_eq!(d.run.status, "succeeded", "{:?}", d.nodes);
    assert_eq!(d.nodes.len(), 2);
    assert!(d.nodes.iter().all(|n| n.status == "succeeded"));
}

#[tokio::test]
#[cfg(unix)]
async fn real_cli_artifact_contract_checks_contents_and_authorized_paths() {
    use sha2::{Digest, Sha256};
    let good_hash = format!("{:x}", Sha256::digest(b"evidence"));
    for (setup, hash, expected) in [
        (
            "printf evidence > report.txt",
            good_hash.as_str(),
            "succeeded",
        ),
        ("printf evidence > report.txt", "wrong", "failed"),
        ("ln -s /etc/hosts report.txt", good_hash.as_str(), "failed"),
    ] {
        let result = serde_json::json!({"status":"succeeded","summary":"done","artifacts":[{"path":"report.txt","sha256":hash,"kind":"report"}],"data":{}});
        let script = format!(
            "cat >/dev/null\n{setup}\nprintf '%s' '{result}' > \"$AI_FACTORY_RESULT_PATH\""
        );
        let (_dir, f) = fixture(&script, 4);
        let id = f
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                103,
                "[AI] artifacts",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        f.start();
        let d = terminal(&f, &id).await;
        f.shutdown().await;
        assert_eq!(d.run.status, expected, "{:?}", d.nodes);
        assert!(f.run_detail(&id).unwrap().run.workspace.is_none());
        assert!(!_dir.path().join("data/workspaces").join(&id).exists());
        if expected == "succeeded" {
            let path = d.nodes[0].result.as_ref().unwrap()["artifacts"][0]["path"]
                .as_str()
                .unwrap();
            assert_eq!(std::fs::read(path).unwrap(), b"evidence");
        }
    }
}

#[tokio::test]
#[cfg(unix)]
async fn failed_and_blocked_runs_remove_the_clone() {
    for status in ["failed", "blocked"] {
        let script = format!("cat >/dev/null\nprintf '%s' '{{\"status\":\"{status}\",\"summary\":\"fixture\",\"artifacts\":[],\"data\":{{}}}}' > \"$AI_FACTORY_RESULT_PATH\"");
        let (dir, f) = fixture(&script, 4);
        let id = f
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                170,
                "[AI] cleanup",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        f.start();
        let detail = terminal(&f, &id).await;
        f.shutdown().await;
        assert_eq!(detail.run.status, status, "{:?}", detail.nodes);
        assert!(detail.run.workspace.is_none());
        assert!(!dir.path().join("data/workspaces").join(id).exists());
    }
}

#[tokio::test]
#[cfg(unix)]
async fn partially_failed_clone_is_removed() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, f) = fixture("cat >/dev/null", 4);
    let git = dir.path().join("failed-git");
    std::fs::write(&git, "#!/bin/sh\nfor arg do last=\"$arg\"; done\nmkdir -p \"$last\"\nprintf partial > \"$last/partial\"\nexit 42\n").unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = f.settings().unwrap();
    settings.commands.git = git.to_string_lossy().into();
    f.save_settings(settings).unwrap();
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            171,
            "[AI] cleanup",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.start();
    let detail = terminal(&f, &id).await;
    f.shutdown().await;
    assert_eq!(detail.run.status, "failed");
    assert!(detail.run.workspace.is_none());
    assert!(!dir.path().join("data/workspaces").join(id).exists());
}

#[tokio::test]
#[cfg(unix)]
async fn archived_artifacts_remain_in_failed_node_results_after_action_rejection() {
    use sha2::{Digest, Sha256};
    let hash = format!("{:x}", Sha256::digest(b"evidence"));
    let value = serde_json::json!({"status":"succeeded","summary":"report ready","artifacts":[{"path":"report.txt","sha256":hash,"kind":"report"}],"data":{"publish":{"title":"fixture","body":"missing issue reference","base":"main"}}});
    let script = format!("cat >/dev/null\nprintf evidence > report.txt\nprintf '%s' '{value}' > \"$AI_FACTORY_RESULT_PATH\"");
    let (dir, f) = fixture(&script, 4);
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            173,
            "[AI] evidence",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.start();
    let detail = terminal(&f, &id).await;
    f.shutdown().await;
    assert_eq!(detail.run.status, "failed");
    let result = detail.nodes[0]
        .result
        .as_ref()
        .expect("validated artifacts must survive action failure");
    assert_eq!(result["status"], "failed");
    let path = result["artifacts"][0]["path"].as_str().unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"evidence");
    assert!(!dir.path().join("data/workspaces").join(&id).exists());
    let recorded: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            dir.path()
                .join("data/sessions")
                .join(&id)
                .join("spec.result.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(recorded["artifacts"][0]["path"], path);
}

#[tokio::test]
#[cfg(unix)]
async fn cancellation_during_clone_kills_git_children_before_cleanup() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, f) = fixture("cat >/dev/null", 4);
    let git = dir.path().join("slow-git");
    let marker = dir.path().join("git-child");
    std::fs::write(&git, format!("#!/bin/sh\nfor arg do last=\"$arg\"; done\nmkdir -p \"$last\"\nsleep 30 &\nprintf '%s' \"$!\" > '{}'\nwait\n", marker.display())).unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = f.settings().unwrap();
    settings.commands.git = git.to_string_lossy().into();
    f.save_settings(settings).unwrap();
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            172,
            "[AI] cleanup",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.start();
    for _ in 0..200 {
        if marker.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let pid: i32 = std::fs::read_to_string(&marker).unwrap().parse().unwrap();
    f.cancel(&id).unwrap();
    for _ in 0..200 {
        if f.run_detail(&id).unwrap().run.workspace.is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    f.shutdown().await;
    assert!(f.run_detail(&id).unwrap().run.workspace.is_none());
    assert!(!dir.path().join("data/workspaces").join(id).exists());
    for _ in 0..100 {
        if unsafe { libc::kill(pid, 0) } == -1 {
            return;
        }
        #[cfg(target_os = "linux")]
        if std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
            s.rsplit_once(')')
                .is_some_and(|(_, state)| state.trim_start().starts_with('Z'))
        }) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("Git descendant survived cancellation");
}

#[test]
fn restart_removes_only_known_finished_workspaces_and_preserves_history() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open(dir.path().into()).unwrap();
    let interrupted = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            180,
            "[AI] fixture",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    let queued = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            181,
            "[AI] fixture",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.set_run_status(&interrupted, "running", None).unwrap();
    for name in [&interrupted, &queued, "manual-folder"] {
        std::fs::create_dir(dir.path().join("workspaces").join(name)).unwrap();
    }
    let session = dir.path().join("sessions").join(&interrupted);
    std::fs::create_dir(&session).unwrap();
    std::fs::write(session.join("spec.log"), "history").unwrap();
    drop(f);
    let f = common::open(dir.path().into()).unwrap();
    assert_eq!(
        f.run_detail(&interrupted).unwrap().run.status,
        "interrupted"
    );
    assert!(!dir.path().join("workspaces").join(interrupted).exists());
    assert!(dir.path().join("workspaces").join(queued).exists());
    assert!(dir.path().join("workspaces/manual-folder").exists());
    assert_eq!(
        std::fs::read_to_string(session.join("spec.log")).unwrap(),
        "history"
    );
}

#[test]
#[cfg(unix)]
fn workspace_recovery_removes_links_without_touching_external_content() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("keep"), "untouched").unwrap();
    let f = common::open(dir.path().into()).unwrap();
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            182,
            "[AI] fixture",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.set_run_status(&id, "failed", None).unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("workspaces").join(&id)).unwrap();
    drop(f);
    let _f = common::open(dir.path().into()).unwrap();
    assert!(!dir.path().join("workspaces").join(id).exists());
    assert_eq!(
        std::fs::read_to_string(outside.path().join("keep")).unwrap(),
        "untouched"
    );
}
#[cfg(unix)]
#[path = "core/node_output.rs"]
mod node_output;
#[cfg(unix)]
#[path = "core/output_limits.rs"]
mod output_limits;

#[cfg(unix)]
#[path = "core/artifact_delivery.rs"]
mod artifact_delivery;
#[cfg(unix)]
#[path = "core/review_rework.rs"]
mod review_rework;
#[cfg(unix)]
#[path = "core/skills.rs"]
mod skills;

#[cfg(unix)]
#[path = "core/codex_accounts_execution.rs"]
mod codex_accounts_execution;

#[cfg(unix)]
#[path = "core/issue_node_comments.rs"]
mod issue_node_comments;

#[cfg(unix)]
#[path = "core/packaging_policy.rs"]
mod packaging_policy;
