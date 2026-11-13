use super::*;

async fn output_fixture(agent: &str, output: &str) -> (tempfile::TempDir, RunDetail) {
    let script = format!(
        "cat >/dev/null\nexec python3 - <<'PY'\nimport json, os, pathlib, sys\n{output}\nPY"
    );
    let (dir, factory) = fixture(&script, 4);
    let mut settings = factory.settings().unwrap();
    let node = &mut settings.flows[0].nodes[0];
    node.agent = agent.into();
    let mut next = node.clone();
    next.id = "after-output".into();
    next.depends_on = vec![node.id.clone()];
    settings.flows[0].nodes.push(next);
    factory.save_settings(settings).unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            195,
            "[AI] bounded output",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    let detail = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let detail = factory.run_detail(&id).unwrap();
            if !["queued", "running"].contains(&detail.run.status.as_str()) {
                break detail;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await;
    factory.shutdown().await;
    (
        dir,
        detail.expect("output readers stopped draining CLI pipes"),
    )
}

const FINAL_RESPONSE: &str = r#"
value = {'status': 'succeeded', 'summary': 'real final response', 'artifacts': [], 'data': {}}
sys.stdout.write(json.dumps({'type': 'text', 'sessionID': 'session-after-large-output', 'part': {'text': json.dumps(value)}}))
sys.stdout.flush()
"#;

#[tokio::test]
async fn oversized_lines_warn_without_blocking_results_or_dependents() {
    for agent in ["codex", "opencode"] {
        for channel in ["stdout", "stderr"] {
            let output = format!(
                r#"
stream = sys.{channel}.buffer
stream.write(b'normal before\n')
stream.flush()
for _ in range(6):
    stream.write(b'x' * (1024 * 1024))
    stream.flush()
# This suffix belongs to the oversized line; it must never become a final response.
stream.write(b'{{"status":"failed","artifacts":[]}}\n')
stream.write(b'normal after dummy-token\n')
stream.flush()
{FINAL_RESPONSE}
if '{channel}' == 'stderr':
    stream.write(b'y' * (5 * 1024 * 1024))
    stream.flush()
"#
            );
            let (dir, detail) = output_fixture(agent, &output).await;
            assert_eq!(
                detail.run.status, "succeeded",
                "{agent}/{channel}: {:?}",
                detail.nodes
            );
            assert!(detail.nodes.iter().all(|node| node.status == "succeeded"));
            let session = dir.path().join("data/sessions").join(&detail.run.id);
            for node in &detail.nodes {
                assert_eq!(
                    node.result.as_ref().unwrap()["summary"],
                    "real final response"
                );
                assert_eq!(
                    node.session_id.as_deref(),
                    Some("session-after-large-output")
                );
                let log =
                    std::fs::read_to_string(session.join(format!("{}.log", node.id))).unwrap();
                assert!(log.contains("oversized line"), "{log}");
                assert!(log.contains("normal before"));
                assert!(log.contains("normal after"));
                assert!(!log.contains("dummy-token"));
                assert!(!log.contains("xxxxxxxx"));
                assert!(!log.contains("yyyyyyyy"));
                assert!(log.len() < 8192, "oversized bytes leaked into log");
            }
        }
    }
}

#[tokio::test]
async fn output_capture_eviction_keeps_final_result_without_repeating_work() {
    for agent in ["codex", "opencode"] {
        for final_first in [false, true] {
            let final_before = if final_first {
                format!("{FINAL_RESPONSE}\nsys.stdout.write('\\n')\nsys.stdout.flush()\n")
            } else {
                String::new()
            };
            let final_after = if final_first { "" } else { FINAL_RESPONSE };
            let output = format!(
                r#"
{final_before}
for _ in range(70):
    sys.stdout.buffer.write(b'x' * (1024 * 1024) + b'\n')
    sys.stdout.buffer.flush()
{final_after}
"#
            );
            let (dir, detail) = output_fixture(agent, &output).await;
            assert_eq!(
                detail.run.status, "succeeded",
                "{agent}: {:?}",
                detail.nodes
            );
            let session = dir.path().join("data/sessions").join(&detail.run.id);
            for node in &detail.nodes {
                assert_eq!(
                    node.result.as_ref().unwrap()["summary"],
                    "real final response"
                );
                assert!(!session
                    .join(format!("{}.repair-1.prompt.txt", node.id))
                    .exists());
                let log =
                    std::fs::read_to_string(session.join(format!("{}.log", node.id))).unwrap();
                assert!(log.contains("capture limit"));
            }
        }
    }
}

#[tokio::test]
async fn oversized_warning_does_not_hide_a_real_process_failure() {
    for agent in ["codex", "opencode"] {
        let output = format!(
            r#"
sys.stdout.buffer.write(b'x' * (5 * 1024 * 1024) + b'\n')
sys.stdout.buffer.flush()
{FINAL_RESPONSE}
raise SystemExit(1)
"#
        );
        let (_dir, detail) = output_fixture(agent, &output).await;
        assert_eq!(detail.run.status, "failed");
        assert!(detail.nodes[0]
            .error
            .as_deref()
            .unwrap()
            .contains("Agent exited with"));
        assert_eq!(detail.nodes[1].status, "skipped");
    }
}

#[tokio::test]
async fn newer_invalid_final_response_cannot_fall_back_to_evicted_success() {
    for agent in ["codex", "opencode"] {
        let output = format!(
            r#"
{FINAL_RESPONSE}
sys.stdout.write('\n' * 9000)
sys.stdout.write(json.dumps({{'type': 'text', 'part': {{'text': '{{invalid'}}}}))
sys.stdout.flush()
"#
        );
        let (_dir, detail) = output_fixture(agent, &output).await;
        assert_eq!(detail.run.status, "failed");
        assert!(detail.nodes[0]
            .error
            .as_deref()
            .unwrap()
            .contains("Invalid node result JSON"));
        assert_eq!(detail.nodes[1].status, "skipped");
    }
}

#[tokio::test]
async fn cancellation_still_stops_a_cli_discarding_an_unterminated_line() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) = fixture(
            r#"cat >/dev/null
exec python3 - <<'PY'
import os, pathlib, sys, time
sys.stdout.buffer.write(b'x' * (6 * 1024 * 1024))
sys.stdout.buffer.flush()
pathlib.Path(os.environ['AI_FACTORY_RESULT_PATH']).with_suffix('.ready').write_text('ready')
time.sleep(30)
PY"#,
            4,
        );
        let mut settings = factory.settings().unwrap();
        settings.flows[0].nodes[0].agent = agent.into();
        factory.save_settings(settings).unwrap();
        let id = factory
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                196,
                "[AI] cancel output",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        factory.start();
        let marker = dir
            .path()
            .join("data/sessions")
            .join(&id)
            .join("spec.result.ready");
        let ready = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !marker.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await;
        factory.cancel(&id).unwrap();
        let detail = terminal(&factory, &id).await;
        factory.shutdown().await;
        ready.expect("oversized-line reader stopped draining before cancellation");
        assert_eq!(detail.run.status, "cancelled");
    }
}
