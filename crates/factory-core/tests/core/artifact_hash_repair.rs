use super::*;
use crate::common;
use sha2::{Digest, Sha256};
use std::path::Path;

// The fake CLI records actual launches and commits in a disposable repository.
// A repeated work prompt increments work-executions, even on a later launch.
const SCRIPT: &str = r#"#!/usr/bin/env python3
import copy, hashlib, json, os, pathlib, subprocess, sys, time
scenario = SCENARIO
if len(sys.argv) > 1 and sys.argv[1] == "app-server":
    pathlib.Path(sys.argv[0]).resolve().with_name("unexpected-skill-discovery").touch()
    sys.exit(87)
prompt = sys.stdin.read()
result = pathlib.Path(os.environ['AI_FACTORY_RESULT_PATH'])
session = result.parent
memory = pathlib.Path(os.environ['AI_FACTORY_MEMORY_DIR'])
node = os.environ['AI_FACTORY_NODE_ID']
def git(*args):
    return subprocess.check_output(['git', *args], stderr=subprocess.STDOUT).decode().strip()
def emit(value):
    if '--output-schema' in sys.argv:
        value['data_json'] = json.dumps(value.pop('data'))
    text = json.dumps(value)
    if scenario == 'empty':
        print(json.dumps({'type':'text','part':{'text':text}}))
    else:
        result.write_text(text)
if node == 'consumer':
    deps = json.loads(prompt.split('Dependency results:\n',1)[1].split('\n\nWorkspace:',1)[0])
    assert len(deps[0]['result']['artifacts']) == 3
    for a in deps[0]['result']['artifacts']:
        assert hashlib.sha256(pathlib.Path(a['path']).read_bytes()).hexdigest() == a['sha256']
    marker = memory / 'consumer-count'
    marker.write_text(str(int(marker.read_text())+1 if marker.exists() else 1))
    emit({'status':'succeeded','summary':'consumed','artifacts':[],'data':{'branch':'feature','verified':True}})
    sys.exit(0)
count = session / 'launch-count'
attempt = int(count.read_text())+1 if count.exists() else 1
count.write_text(str(attempt))
(session / f'launch-{attempt}.json').write_text(json.dumps({'args':sys.argv[1:],'home':os.environ.get('CODEX_HOME'),'memory':str(memory),'config':os.environ.get('OPENCODE_CONFIG_CONTENT','')}))
(session / f'prompt-{attempt}.txt').write_text(prompt)
repair = 'AI Factory artifact hash correction' in prompt
if not repair:
    work = session / 'work-executions'
    work.write_text(str(int(work.read_text())+1 if work.exists() else 1))
    (session / 'initial-head').write_text(git('rev-parse','HEAD'))
    artifacts = []
    for name, text in [('evidence.md','report'),('patch.diff','patch'),('verification.log','tests passed')]:
        p = pathlib.Path(name)
        p.write_text(text)
        artifacts.append({'path':name,'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'kind':'report'})
    git('add','evidence.md','patch.diff','verification.log')
    git('commit','-m','work exactly once')
    (session / 'work-head').write_text(git('rev-parse','HEAD'))
    artifacts[2]['sha256'] = '47452e8575b83bef2aa9e37c1a0a90b??'
    if scenario == 'fixed_then_changed': artifacts[1]['sha256'] = 'bad??'
    if scenario == 'change_empty_hash': artifacts[1]['sha256'] = ''
    if scenario == 'mismatch': artifacts[2]['sha256'] = '0'*64
    if scenario == 'multi':
        artifacts[1]['sha256'] = 'also??'
        artifacts = {'first':artifacts[0],'second':artifacts[1],'third':artifacts[2]}
    if scenario == 'missing': artifacts[2]['path'] = 'missing.log'
    if scenario == 'traversal': artifacts[2]['path'] = '../verification.log'
    if scenario == 'symlink':
        pathlib.Path('linked.log').symlink_to(pathlib.Path('verification.log').absolute())
        artifacts[2]['path'] = 'linked.log'
    if scenario == 'directory': artifacts[2]['path'] = '.'
    value = {'status':scenario if scenario in ['blocked','failed'] else 'succeeded','summary':'work complete','artifacts':artifacts,'data':{'branch':'feature','verified':True}}
    if scenario.startswith('publish_'):
        value['data']['publish'] = {'title':'Hash recovery','body':'Closes #8'}
        if scenario == 'publish_mismatch': artifacts[2]['sha256'] = '0'*64
    (session / 'original.json').write_text(json.dumps(value))
else:
    assert 'WORK_PROMPT_SENTINEL' not in prompt
    assert not (memory / 'artifacts' / node).exists()
    assert git('rev-parse','HEAD') == (session / 'work-head').read_text()
    assert git('rev-list','--count',(session / 'initial-head').read_text()+'..HEAD') == '1'
    assert pathlib.Path('evidence.md').read_text() == 'report'
    assert pathlib.Path('patch.diff').read_text() == 'patch'
    assert pathlib.Path('verification.log').read_text() == 'tests passed'
    value = json.loads((session / 'original.json').read_text())
    if isinstance(value['artifacts'],dict): value['artifacts'] = list(value['artifacts'].values())
    for a in value['artifacts']:
        if '?' in a['sha256']:
            a['sha256'] = '' if scenario == 'empty' else hashlib.sha256(pathlib.Path(a['path']).read_bytes()).hexdigest()
    if scenario in ['exhaust','publish_exhaust'] or (scenario == 'second' and attempt == 2): value['artifacts'][2]['sha256'] = 'still??'
    if scenario == 'fixed_then_changed':
        value['artifacts'][1]['sha256'] = 'bad??' if attempt == 2 else ''
        if attempt == 3: value['artifacts'][2]['sha256'] = ''
    if scenario in ['repair_mismatch','publish_repair_mismatch']: value['artifacts'][2]['sha256'] = '0'*64
    if scenario == 'cancel':
        child = subprocess.Popen(['sleep','60'])
        (session / 'child-pid').write_text(str(child.pid))
        print('REPAIR_READY',flush=True)
        time.sleep(60)
    if scenario == 'exit': sys.exit(7)
    if scenario == 'json': result.write_text('{broken'); sys.exit(0)
    if scenario == 'schema': value['data'].pop('verified')
    if scenario == 'status_schema': value['status'] = 'complete'
    if scenario == 'data_json':
        value.pop('data'); value['data_json']='{broken'; result.write_text(json.dumps(value)); sys.exit(0)
    if scenario.startswith('change_') or scenario == 'publish_change':
        field = scenario[7:]
        if field == 'status': value['status'] = 'blocked'
        elif field == 'summary': value['summary'] = 'changed'
        elif field == 'branch': value['data']['branch'] = 'changed'
        elif field == 'action' or scenario == 'publish_change': value['data']['publish'] = {'title':'changed','body':'changed'}
        elif field in ['path','kind']: value['artifacts'][2][field] = 'changed'
        elif field == 'order': value['artifacts'].reverse()
        elif field == 'remove': value['artifacts'].pop()
        elif field == 'add': value['artifacts'].append(copy.deepcopy(value['artifacts'][0]))
        elif field == 'valid_hash': value['artifacts'][0]['sha256'] = ''
        elif field == 'empty_hash': value['artifacts'][1]['sha256'] = 'a'*64
    (session / f'repair-head-{attempt}').write_text(git('rev-parse','HEAD'))
emit(value)
"#;

fn setup(scenario: &str, agent: &str) -> (FixtureDir, std::sync::Arc<Factory>) {
    let (dir, factory) = fixture("exit 1", 4);
    std::fs::write(
        dir.path().join("fake-agent"),
        SCRIPT.replace("SCENARIO", &format!("{scenario:?}")),
    )
    .unwrap();
    let mut settings = factory.settings().unwrap();
    let node = &mut settings.flows[0].nodes[0];
    node.agent = agent.into();
    node.prompt = "WORK_PROMPT_SENTINEL".into();
    node.model = "fixture-model".into();
    node.effort = "high".into();
    node.result_schema = json!({"type":"object","required":["branch","verified"],"properties":{"branch":{"type":"string"},"verified":{"type":"boolean"}}});
    let mut next = node.clone();
    next.id = "consumer".into();
    next.depends_on = vec![node.id.clone()];
    settings.flows[0].nodes.push(next);
    factory.save_settings(settings).unwrap();
    (dir, factory)
}

async fn run(factory: &std::sync::Arc<Factory>) -> (String, RunDetail) {
    let id = factory
        .enqueue(common::from_source(
            TriggerEvent::issue("acme/repo", 8, "[AI] hashes", "alice", ""),
            "alice",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    let detail = terminal(factory, &id).await;
    factory.shutdown().await;
    (id, detail)
}
fn session(dir: &FixtureDir, id: &str) -> std::path::PathBuf {
    dir.path().join("data/sessions").join(id)
}
fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap()
}
fn node(detail: &RunDetail) -> &NodeRun {
    detail.nodes.iter().find(|n| n.id == "spec").unwrap()
}

#[tokio::test]
async fn malformed_hash_recovers_without_reexecuting_work() {
    for agent in ["codex", "opencode"] {
        for scenario in ["correct", "empty"] {
            let (dir, f) = setup(scenario, agent);
            let (id, d) = run(&f).await;
            assert_eq!(
                d.run.status, "succeeded",
                "{agent}/{scenario}: {:?}",
                d.nodes
            );
            let s = session(&dir, &id);
            assert_eq!(read(s.join("launch-count")), "2");
            assert_eq!(read(s.join("work-executions")), "1");
            assert_eq!(read(s.join("work-head")), read(s.join("repair-head-2")));
            let result = node(&d).result.as_ref().unwrap();
            assert_eq!(result["summary"], "work complete");
            assert_eq!(result["data"], json!({"branch":"feature","verified":true}));
            for a in result["artifacts"].as_array().unwrap() {
                let bytes = std::fs::read(a["path"].as_str().unwrap()).unwrap();
                assert_eq!(a["sha256"], format!("{:x}", Sha256::digest(bytes)));
                assert_eq!(a["kind"], "report");
            }
            let memory = Path::new(result["artifacts"][0]["path"].as_str().unwrap())
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap();
            assert_eq!(read(memory.join("consumer-count")), "1");
            let prompt = read(s.join("prompt-2.txt"));
            for text in [
                "1/2",
                "verification.log",
                "47452e8575b83bef2aa9e37c1a0a90b??",
                "length=",
                "non_hex=true",
                "work complete",
            ] {
                assert!(prompt.contains(text), "missing {text}");
            }
            assert!(!prompt.contains("WORK_PROMPT_SENTINEL"));
        }
    }
}

#[tokio::test]
async fn second_correction_can_finish_but_no_third_is_requested() {
    for agent in ["codex", "opencode"] {
        let (dir, f) = setup("second", agent);
        let (id, d) = run(&f).await;
        assert_eq!(d.run.status, "succeeded", "{:?}", d.nodes);
        assert_eq!(read(session(&dir, &id).join("launch-count")), "3");
        assert_eq!(read(session(&dir, &id).join("work-executions")), "1");
        assert!(read(session(&dir, &id).join("prompt-3.txt")).contains("2/2"));
    }
}

async fn rejected(scenario: &str, agent: &str, calls: &str, diagnostic: &str) {
    let (dir, f) = setup(scenario, agent);
    let (id, d) = run(&f).await;
    assert_eq!(d.run.status, "failed", "{agent}/{scenario}: {:?}", d.nodes);
    let n = node(&d);
    assert!(
        n.error.as_deref().unwrap().contains(diagnostic),
        "{scenario}: {:?}",
        n.error
    );
    assert!(n.result.is_none());
    assert_eq!(read(session(&dir, &id).join("launch-count")), calls);
    assert_eq!(read(session(&dir, &id).join("work-executions")), "1");
    assert_eq!(
        d.nodes.iter().find(|n| n.id == "consumer").unwrap().status,
        "skipped"
    );
    assert!(!session(&dir, &id).join("prompt-4.txt").exists());
    let launch: serde_json::Value =
        serde_json::from_str(&read(session(&dir, &id).join("launch-1.json"))).unwrap();
    let memory = Path::new(launch["memory"].as_str().unwrap());
    assert!(!memory.join("consumer-count").exists());
    if !scenario.contains("mismatch") {
        assert!(!memory.join("artifacts/spec").exists());
    } else if memory.join("artifacts/spec").exists() {
        for entry in std::fs::read_dir(memory.join("artifacts/spec")).unwrap() {
            assert!(!entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with("verification.log"));
        }
    }
}

#[tokio::test]
async fn malformed_hash_exhausts_two_additional_attempts() {
    for agent in ["codex", "opencode"] {
        rejected(
            "exhaust",
            agent,
            "3",
            "correction attempts exhausted (2 additional attempts)",
        )
        .await;
    }
}
#[tokio::test]
async fn correction_cannot_change_envelope_manifest_or_actions() {
    for agent in ["codex", "opencode"] {
        for field in [
            "status",
            "summary",
            "branch",
            "action",
            "path",
            "kind",
            "order",
            "remove",
            "add",
            "valid_hash",
            "empty_hash",
        ] {
            rejected(
                &format!("change_{field}"),
                agent,
                "2",
                "only malformed artifact sha256 fields may change",
            )
            .await;
        }
        rejected(
            "fixed_then_changed",
            agent,
            "3",
            "only malformed artifact sha256 fields may change",
        )
        .await;
    }
}
#[tokio::test]
async fn invalid_corrective_json_schema_or_process_is_not_retried() {
    for agent in ["codex", "opencode"] {
        for scenario in ["json", "schema", "status_schema", "exit"] {
            rejected(
                scenario,
                agent,
                "2",
                "artifact hash correction response (attempt 1/2)",
            )
            .await;
        }
    }
    rejected("data_json", "codex", "2", "data_json contains invalid JSON").await;
}
#[tokio::test]
async fn valid_content_mismatch_never_requests_a_correction() {
    for agent in ["codex", "opencode"] {
        rejected("mismatch", agent, "1", "Artifact hash mismatch").await;
        rejected("repair_mismatch", agent, "2", "Artifact hash mismatch").await;
    }
}
#[tokio::test]
async fn all_malformed_hashes_are_diagnosed_in_one_request() {
    for agent in ["codex", "opencode"] {
        let (dir, f) = setup("multi", agent);
        let (id, d) = run(&f).await;
        assert_eq!(d.run.status, "succeeded", "{:?}", d.nodes);
        assert_eq!(read(session(&dir, &id).join("launch-count")), "2");
        let p = read(session(&dir, &id).join("prompt-2.txt"));
        for s in [
            "patch.diff",
            "verification.log",
            "also??",
            "47452e8575b83bef2aa9e37c1a0a90b??",
        ] {
            assert!(p.contains(s));
        }
    }
}
#[tokio::test]
async fn repair_launch_is_restricted_and_preserves_agent_binding() {
    for agent in ["codex", "opencode"] {
        for yolo in [false, true] {
            for access in ["read", "write"] {
                let (dir, f) = setup("correct", agent);
                let mut settings = f.settings().unwrap();
                settings.codex_yolo = yolo;
                settings.flows[0].nodes[0].access = access.into();
                f.save_settings(settings).unwrap();
                let db =
                    rusqlite::Connection::open(dir.path().join("data/factory.sqlite3")).unwrap();
                let raw: String = db
                    .query_row("SELECT json FROM settings WHERE id=1", [], |row| row.get(0))
                    .unwrap();
                let mut wire: serde_json::Value = serde_json::from_str(&raw).unwrap();
                wire["flows"][0]["nodes"][0]["skills"] = json!([]);
                assert_eq!(
                    db.execute("UPDATE settings SET json=?1 WHERE id=1", [wire.to_string()])
                        .unwrap(),
                    1
                );
                drop(db);
                let (id, d) = run(&f).await;
                assert!(!dir.path().join("unexpected-skill-discovery").exists());
                assert_eq!(d.run.status, "succeeded", "{:?}", d.nodes);
                let s = session(&dir, &id);
                let first: serde_json::Value =
                    serde_json::from_str(&read(s.join("launch-1.json"))).unwrap();
                let repair: serde_json::Value =
                    serde_json::from_str(&read(s.join("launch-2.json"))).unwrap();
                let args = repair["args"].as_array().unwrap();
                assert!(args.contains(&json!(if agent == "codex" {
                    "fixture-model"
                } else {
                    "fixture-model#high"
                })));
                if agent == "codex" {
                    assert_eq!(first["home"], repair["home"]);
                    assert!(!repair["home"].is_null());
                    assert!(
                        !args
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .any(|arg| arg == "--strict-config" || arg.starts_with("skills.config")),
                        "filtered hash repair: {args:?}"
                    );
                    for arg in [
                        "read-only",
                        "approval_policy=\"never\"",
                        "--ephemeral",
                        "skills.include_instructions=false",
                        "model_reasoning_effort=\"high\"",
                    ] {
                        assert!(args.contains(&json!(arg)), "{args:?}");
                    }
                    for arg in [
                        "--dangerously-bypass-approvals-and-sandbox",
                        "--add-dir",
                        "workspace-write",
                    ] {
                        assert!(!args.contains(&json!(arg)));
                    }
                    assert_eq!(
                        first["args"]
                            .as_array()
                            .unwrap()
                            .contains(&json!("--dangerously-bypass-approvals-and-sandbox")),
                        yolo
                    );
                } else {
                    let config: serde_json::Value =
                        serde_json::from_str(repair["config"].as_str().unwrap()).unwrap();
                    for permission in [
                        &config["permission"],
                        &config["agent"]["ai-factory-repair"]["permission"],
                    ] {
                        assert_eq!(
                            permission,
                            &json!({"*":"deny","question":"deny","external_directory":"deny"})
                        );
                    }
                    assert!(!config.to_string().contains("WORK_PROMPT_SENTINEL"));
                }
            }
        }
    }
}
#[tokio::test]
async fn corrected_blocked_or_failed_results_keep_original_status() {
    for agent in ["codex", "opencode"] {
        for status in ["blocked", "failed"] {
            let (dir, f) = setup(status, agent);
            let (id, d) = run(&f).await;
            assert_eq!(d.run.status, status, "{:?}", d.nodes);
            let r = node(&d).result.as_ref().unwrap();
            assert_eq!(r["status"], status);
            assert_eq!(r["artifacts"].as_array().unwrap().len(), 3);
            assert!(d
                .nodes
                .iter()
                .find(|n| n.id == "consumer")
                .unwrap()
                .result
                .is_none());
            assert_eq!(read(session(&dir, &id).join("launch-count")), "2");
        }
    }
}
#[tokio::test]
async fn cancellation_during_hash_correction_stops_process_tree() {
    for agent in ["codex", "opencode"] {
        let (dir, f) = setup("cancel", agent);
        let id = f
            .enqueue(common::from_source(
                TriggerEvent::issue("acme/repo", 8, "[AI] hashes", "alice", ""),
                "alice",
            ))
            .unwrap()
            .unwrap();
        f.start();
        let marker = session(&dir, &id).join("spec.repair-1.log");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while !std::fs::read_to_string(&marker).is_ok_and(|text| text.contains("REPAIR_READY")) {
            if tokio::time::Instant::now() >= deadline
                || !["queued", "running"].contains(&f.run_detail(&id).unwrap().run.status.as_str())
            {
                f.shutdown().await;
                panic!("correction did not reach marker: {:?}", f.run_detail(&id));
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let pid: i32 = read(session(&dir, &id).join("child-pid")).parse().unwrap();
        f.cancel(&id).unwrap();
        let d = terminal(&f, &id).await;
        f.shutdown().await;
        assert_eq!(d.run.status, "cancelled");
        assert_eq!(read(session(&dir, &id).join("launch-count")), "2");
        assert!(node(&d).result.is_none());
        let mut stopped = false;
        for _ in 0..100 {
            stopped = unsafe { libc::kill(pid, 0) } == -1;
            #[cfg(target_os = "linux")]
            {
                stopped |= std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
                    s.rsplit_once(')')
                        .is_some_and(|(_, s)| s.trim_start().starts_with('Z'))
                });
            }
            if stopped {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(stopped, "correction descendant survived");
    }
}

// Keep publication traffic local and exercise Factory.start -> execute_node ->
// publish_actions. The Git wrapper intercepts push and records its actual HEAD.
fn publication_fixture(scenario: &str, agent: &str) -> (FixtureDir, std::sync::Arc<Factory>) {
    use std::{
        io::{Read, Write},
        sync::atomic::Ordering,
    };
    let (mut dir, f) = setup(scenario, agent);
    drop(f);
    dir.stop.store(true, Ordering::SeqCst);
    dir.server.take().unwrap().join().unwrap();
    dir.stop.store(false, Ordering::SeqCst);
    dir.requests.lock().unwrap().clear();
    let wrapper = dir.path().join("git-wrapper");
    let script = read(&wrapper);
    let push = format!(
        r#"if [ "$1" = push ]; then
  git rev-parse HEAD >> '{root}/push-heads'
  git branch --show-current > '{root}/push-branch'
  exit 0
fi
"#,
        root = dir.path().display()
    );
    std::fs::write(
        &wrapper,
        script.replacen(
            "if [ \"$1\" = clone ]; then",
            &(push + "if [ \"$1\" = clone ]; then"),
            1,
        ),
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let halt = dir.stop.clone();
    let calls = dir.requests.clone();
    let root = dir.path().to_path_buf();
    dir.server = Some(std::thread::spawn(move || {
        let mut comments = Vec::new();
        while !halt.load(Ordering::SeqCst) {
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("fixture accept: {error}"),
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                .unwrap();
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(1)))
                .unwrap();
            let mut bytes = Vec::new();
            let (start, len) = loop {
                let mut buf = [0; 4096];
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers
                        .lines()
                        .filter_map(|s| s.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, s)| s.trim().parse::<usize>().unwrap())
                        .unwrap_or(0);
                    break (end + 4, length);
                }
            };
            while bytes.len() < start + len {
                let mut buf = [0; 4096];
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            let header = String::from_utf8_lossy(&bytes[..start]);
            let mut first = header.lines().next().unwrap().split_whitespace();
            let method = first.next().unwrap();
            let path = first.next().unwrap();
            let input: serde_json::Value = if len == 0 {
                serde_json::Value::Null
            } else {
                serde_json::from_slice(&bytes[start..start + len]).unwrap()
            };
            calls
                .lock()
                .unwrap()
                .push((method.into(), path.into(), input.clone()));
            let body = match (method, path) {
                ("GET", "/user") => json!({"login":"alice"}),
                ("GET", p) if p.starts_with("/search/issues?") => {
                    json!({"total_count":0,"incomplete_results":false,"items":[]})
                }
                ("GET", "/repos/acme/repo/issues/8") => {
                    json!({"state":"open","title":"[AI] hashes","user":{"login":"alice"}})
                }
                ("GET", p) if p.starts_with("/repos/acme/repo/issues/8/sub_issues?") => {
                    json!([])
                }
                ("GET", "/repos/acme/repo") => {
                    json!({"full_name":"acme/repo","default_branch":"trunk"})
                }
                ("GET", p) if p.starts_with("/repos/acme/repo/issues/8/comments?") => {
                    json!(comments)
                }
                ("POST", "/repos/acme/repo/issues/8/comments") => {
                    let row = json!({"id":9001+comments.len(),"body":input["body"],"user":{"login":"alice"}});
                    comments.push(row.clone());
                    row
                }
                ("GET", p) if p.starts_with("/repos/acme/repo/pulls?state=open&head=acme:") => {
                    json!([])
                }
                ("POST", "/repos/acme/repo/pulls") => {
                    assert_eq!(input["base"], "trunk");
                    assert_eq!(input["title"], "Hash recovery");
                    assert_eq!(input["body"], "Closes #8");
                    // The single push must follow successful archival of all files.
                    let sessions = std::fs::read_dir(root.join("data/sessions"))
                        .unwrap()
                        .next()
                        .unwrap()
                        .unwrap()
                        .path();
                    let launch: serde_json::Value =
                        serde_json::from_str(&read(sessions.join("launch-1.json"))).unwrap();
                    let archive =
                        Path::new(launch["memory"].as_str().unwrap()).join("artifacts/spec");
                    let entries = std::fs::read_dir(&archive)
                        .unwrap()
                        .map(|e| e.unwrap().path())
                        .collect::<Vec<_>>();
                    assert_eq!(entries.len(), 3);
                    for (name, text) in [
                        ("evidence.md", "report"),
                        ("patch.diff", "patch"),
                        ("verification.log", "tests passed"),
                    ] {
                        let p = entries
                            .iter()
                            .find(|p| p.file_name().unwrap().to_string_lossy().ends_with(name))
                            .unwrap();
                        assert_eq!(read(p), text);
                        assert_eq!(
                            Sha256::digest(std::fs::read(p).unwrap()),
                            Sha256::digest(text.as_bytes())
                        );
                    }
                    assert_eq!(read(root.join("push-heads")).lines().count(), 1);
                    assert_eq!(input["head"], read(root.join("push-branch")).trim());
                    json!({"number":9})
                }
                ("GET", "/repos/acme/repo/pulls/9") => {
                    json!({"number":9,"html_url":"https://github.com/acme/repo/pull/9","state":"open","body":"Closes #8","user":{"login":"alice"},"base":{"ref":"trunk","repo":{"full_name":"acme/repo"}},"head":{"sha":read(root.join("push-heads")).trim(),"ref":read(root.join("push-branch")).trim(),"repo":{"full_name":"acme/repo"}}})
                }
                _ => panic!("Unexpected fixture request {method} {path}: {input}"),
            };
            let body = body.to_string();
            let _=write!(stream,"HTTP/1.1 200 Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
        }
    }));
    let f = common::open_with_api(dir.path().join("data"), &base).unwrap();
    (dir, f)
}

#[tokio::test]
async fn repaired_result_publishes_once_after_archival() {
    for agent in ["codex", "opencode"] {
        let (dir, f) = publication_fixture("publish_correct", agent);
        let (id, d) = run(&f).await;
        assert_eq!(d.run.status, "succeeded", "{:?}", d.nodes);
        assert_eq!(read(dir.path().join("push-heads")).lines().count(), 1);
        assert_eq!(
            read(dir.path().join("push-heads")).trim(),
            read(session(&dir, &id).join("work-head"))
        );
        assert_eq!(read(session(&dir, &id).join("launch-count")), "2");
        assert_eq!(read(session(&dir, &id).join("work-executions")), "1");
        assert_eq!(
            dir.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(m, p, _)| m == "POST" && p == "/repos/acme/repo/pulls")
                .count(),
            1
        );
        assert_eq!(
            node(&d).result.as_ref().unwrap()["data"]["publish"],
            json!({"title":"Hash recovery","body":"Closes #8"})
        );
    }
}
#[tokio::test]
async fn unresolved_or_mismatched_hash_prevents_publication() {
    for agent in ["codex", "opencode"] {
        for scenario in [
            "publish_exhaust",
            "publish_mismatch",
            "publish_repair_mismatch",
            "publish_change",
        ] {
            let (dir, f) = publication_fixture(scenario, agent);
            let (id, d) = run(&f).await;
            assert_eq!(d.run.status, "failed", "{scenario}: {:?}", d.nodes);
            assert!(node(&d).result.is_none());
            assert!(!dir.path().join("push-heads").exists());
            assert_eq!(
                dir.requests
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(m, p, _)| m == "POST" && p == "/repos/acme/repo/pulls")
                    .count(),
                0
            );
            assert_eq!(read(session(&dir, &id).join("work-executions")), "1");
            assert_eq!(
                d.nodes.iter().find(|n| n.id == "consumer").unwrap().status,
                "skipped"
            );
        }
    }
}

#[tokio::test]
async fn unsafe_or_missing_artifact_with_malformed_hash_never_launches_correction() {
    for agent in ["codex", "opencode"] {
        for (scenario, diagnostic) in [
            ("missing", "Artifact does not exist"),
            ("traversal", "Artifact parent traversal forbidden"),
            ("symlink", "Artifact symlink forbidden"),
            ("directory", "Artifact path must name a file or directory"),
        ] {
            rejected(scenario, agent, "1", diagnostic).await;
        }
    }
}
