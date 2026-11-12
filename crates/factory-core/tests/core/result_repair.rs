use super::*;

fn repair_fixture(
    agent: &str,
    responses: &[serde_json::Value],
) -> (tempfile::TempDir, std::sync::Arc<Factory>) {
    let (dir, factory) = fixture("exit 1", 4);
    let script = r#"#!/usr/bin/env python3
import hashlib, json, os, pathlib, sys, time
responses = json.loads(RESPONSES)
result = pathlib.Path(os.environ['AI_FACTORY_RESULT_PATH'])
node = os.environ['AI_FACTORY_NODE_ID']
count = result.parent / (node + '.test-count')
attempt = int(count.read_text()) + 1 if count.exists() else 1
count.write_text(str(attempt))
prompt = sys.stdin.read()
(result.parent / f'{node}.stdin-{attempt}.txt').write_text(prompt)
(result.parent / f'{node}.launch-{attempt}.json').write_text(json.dumps({
    'args': sys.argv[1:], 'config': os.environ.get('OPENCODE_CONFIG_CONTENT', '')
}))
response = responses[min(attempt - 1, len(responses) - 1)]
if response.get('noise'):
    print(json.dumps({'type': 'tool_use', 'content': 'irrelevant execution trace'}))
if response.get('sleep'):
    print('repair process started', flush=True)
    time.sleep(response['sleep'])
if response.get('exit'):
    sys.exit(response['exit'])
value = response.get('value')
if response.get('artifact'):
    artifact = pathlib.Path(os.environ['AI_FACTORY_MEMORY_DIR']) / 'evidence.txt'
    artifact.write_text('original evidence')
    value['artifacts'] = [{'path': str(artifact), 'kind': 'report',
        'sha256': hashlib.sha256(artifact.read_bytes()).hexdigest()}]
    if response.get('patch'):
        patch = artifact.with_name('implementation.patch')
        patch.write_text('original patch')
        value['artifacts'].append({'path': str(patch), 'kind': 'implementation_patch',
            'sha256': hashlib.sha256(patch.read_bytes()).hexdigest()})
    if response.get('legacy_type'):
        for item in value['artifacts']:
            item['type'] = item.pop('kind')
if response.get('direct'):
    print(json.dumps(value))
elif response.get('stdout'):
    print(json.dumps({'type': 'text', 'part': {'text': value if isinstance(value, str) else json.dumps(value)}}))
elif value is not None:
    result.write_text(value if isinstance(value, str) else json.dumps(value))
"#
    .replace(
        "RESPONSES",
        &serde_json::to_string(&serde_json::to_string(responses).unwrap()).unwrap(),
    );
    std::fs::write(dir.path().join("fake-agent"), script).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.flows[0].nodes[0].agent = agent.into();
    factory.save_settings(settings).unwrap();
    (dir, factory)
}

fn valid_result() -> serde_json::Value {
    json!({"status":"succeeded","summary":"work complete","artifacts":[],"data":{"branch":"feature"}})
}

#[tokio::test]
async fn repair_accepts_type_to_kind_rename_with_original_files_and_hashes() {
    for agent in ["codex", "opencode"] {
        let summary = "Implemented issue #8; native GUI persistence remains unverified.";
        let data = json!({"native_smoke":{"status":"not_verified"}});
        let (dir, factory) = repair_fixture(
            agent,
            &[
                json!({"value":{"status":"partial","summary":summary,"artifacts":[],"data":data},"artifact":true,"patch":true,"legacy_type":true,"stdout":agent == "opencode"}),
                json!({"value":{"status":"succeeded","summary":summary,"artifacts":[],"data":data},"artifact":true,"patch":true,"stdout":agent == "opencode"}),
            ],
        );
        let (id, detail) = run_repair_fixture(&factory).await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        assert_eq!(attempts(&dir, &id), 2);
        let result = detail.nodes[0].result.as_ref().unwrap();
        assert_eq!(result["summary"], summary);
        assert_eq!(result["data"], data);
        let artifacts = result["artifacts"].as_array().unwrap();
        assert_eq!(artifacts.len(), 2);
        for (artifact, expected) in artifacts
            .iter()
            .zip(["original evidence", "original patch"])
        {
            let path = std::path::Path::new(artifact["path"].as_str().unwrap());
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(bytes, expected.as_bytes());
            use sha2::{Digest, Sha256};
            assert_eq!(artifact["sha256"], format!("{:x}", Sha256::digest(&bytes)));
            assert!(artifact.get("type").is_none());
            assert!(artifact["kind"].is_string());
        }
    }
}

async fn run_repair_fixture(factory: &std::sync::Arc<Factory>) -> (String, RunDetail) {
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            190,
            "[AI] format",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    let detail = terminal(factory, &id).await;
    factory.shutdown().await;
    (id, detail)
}

fn attempts(dir: &tempfile::TempDir, id: &str) -> usize {
    std::fs::read_to_string(
        dir.path()
            .join("data/sessions")
            .join(id)
            .join("spec.test-count"),
    )
    .unwrap()
    .parse()
    .unwrap()
}

#[tokio::test]
async fn invalid_envelope_is_repaired_without_repeating_work_for_both_agents() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) = repair_fixture(
            agent,
            &[
                json!({"value":{"status":"implemented_with_native_smoke_unverified","branch":"feature","artifacts":[]},"artifact":true,"stdout":agent == "opencode"}),
                json!({"value":valid_result(),"artifact":true,"stdout":agent == "opencode"}),
            ],
        );
        let (id, detail) = run_repair_fixture(&factory).await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        assert_eq!(attempts(&dir, &id), 2);
        let session = dir.path().join("data/sessions").join(&id);
        let initial = std::fs::read_to_string(session.join("spec.stdin-1.txt")).unwrap();
        assert!(initial.contains("Output JSON Schema"));
        assert!(initial.contains("branch"));
        assert!(initial.contains("summary"));
        assert!(initial.contains("Do not invent status values"));
        let repair = std::fs::read_to_string(session.join("spec.stdin-2.txt")).unwrap();
        assert!(repair.contains("Repair only the response format"));
        assert!(repair.contains("Do not execute the original task again"));
        assert!(repair.contains("implemented_with_native_smoke_unverified"));
        assert!(repair.contains("/status"));
        assert!(repair.contains("required property"));
        assert!(repair.contains("data_json") == (agent == "codex"));
        assert!(session.join("spec.attempt-1.response.txt").exists());
        let launch: serde_json::Value =
            serde_json::from_slice(&std::fs::read(session.join("spec.launch-2.json")).unwrap())
                .unwrap();
        if agent == "codex" {
            let args = launch["args"].as_array().unwrap();
            assert!(args.contains(&json!("read-only")));
            assert!(!args.contains(&json!("--add-dir")));
        } else {
            let config: serde_json::Value =
                serde_json::from_str(launch["config"].as_str().unwrap()).unwrap();
            assert_eq!(config["permission"]["*"], "deny");
            assert!(config["permission"]
                .as_object()
                .unwrap()
                .values()
                .all(|v| v == "deny"));
        }
        let result = detail.nodes[0].result.as_ref().unwrap();
        assert_eq!(result["data"]["branch"], "feature");
        let artifact = std::path::Path::new(result["artifacts"][0]["path"].as_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(artifact).unwrap(),
            "original evidence"
        );
    }
}

#[tokio::test]
async fn custom_schema_still_fails_after_two_repairs_for_both_agents() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) = repair_fixture(agent, &[json!({"value":valid_result()})]);
        let mut settings = factory.settings().unwrap();
        settings.flows[0].nodes[0].result_schema = json!({"type":"object","required":["approved"],"properties":{"approved":{"type":"boolean"}},"additionalProperties":false});
        factory.save_settings(settings).unwrap();
        let (id, detail) = run_repair_fixture(&factory).await;
        assert_eq!(detail.run.status, "failed");
        assert_eq!(attempts(&dir, &id), 3, "{agent}");
        let error = detail.nodes[0].error.as_deref().unwrap();
        assert!(error.contains("after 3 attempts"), "{error}");
        assert!(error.contains("/data"), "{error}");
        assert!(error.contains("approved"), "{error}");
        assert!(detail.nodes[0].result.is_none());
        assert!(factory
            .logs(&id, "spec", 0)
            .unwrap()
            .text
            .contains("repair 1/2"));
    }
}

#[tokio::test]
async fn malformed_json_and_codex_data_json_are_repaired() {
    for agent in ["codex", "opencode"] {
        let final_value = if agent == "codex" {
            json!({"status":"succeeded","summary":"work complete","artifacts":[],"data_json":"{\"branch\":\"feature\"}"})
        } else {
            valid_result()
        };
        let second = if agent == "codex" {
            json!({"status":"succeeded","summary":"work complete","artifacts":[],"data_json":"{"})
        } else {
            json!({"status":"succeeded","summary":"","artifacts":[],"data":{}})
        };
        let (dir, factory) = repair_fixture(
            agent,
            &[
                json!({"value":"{invalid"}),
                json!({"value":second}),
                json!({"value":final_value}),
            ],
        );
        let (id, detail) = run_repair_fixture(&factory).await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        assert_eq!(attempts(&dir, &id), 3);
        assert_eq!(
            detail.nodes[0].result.as_ref().unwrap()["data"]["branch"],
            "feature"
        );
    }
}

#[tokio::test]
async fn repairs_never_consume_stale_result_files() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) = repair_fixture(
            agent,
            &[
                json!({"value":{"status":"wrong","branch":"feature"}}),
                json!({}),
            ],
        );
        let (id, detail) = run_repair_fixture(&factory).await;
        assert_eq!(detail.run.status, "failed");
        assert_eq!(attempts(&dir, &id), 3, "{agent}");
        let error = detail.nodes[0].error.as_deref().unwrap();
        assert!(
            error.contains("no valid structured final result"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn malformed_stdout_repairs_receive_final_response_without_execution_trace() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) = repair_fixture(
            agent,
            &[
                json!({"value":"{invalid","stdout":true,"noise":true}),
                json!({"value":valid_result(),"stdout":true}),
            ],
        );
        let (id, detail) = run_repair_fixture(&factory).await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        let repair = std::fs::read_to_string(
            dir.path()
                .join("data/sessions")
                .join(id)
                .join("spec.stdin-2.txt"),
        )
        .unwrap();
        assert!(repair.contains("{invalid"));
        assert!(!repair.contains("irrelevant execution trace"));
    }
}

#[tokio::test]
async fn direct_stdout_with_missing_envelope_fields_is_preserved_for_repair() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) = repair_fixture(
            agent,
            &[
                json!({"value":{"status":"implemented","branch":"original-branch"},"direct":true}),
                json!({"value":valid_result()}),
            ],
        );
        let (id, detail) = run_repair_fixture(&factory).await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        let repair = std::fs::read_to_string(
            dir.path()
                .join("data/sessions")
                .join(id)
                .join("spec.stdin-2.txt"),
        )
        .unwrap();
        assert!(repair.contains("original-branch"));
    }
}

#[tokio::test]
async fn repairs_cannot_change_approval_actions_artifacts_or_blocked_status() {
    for agent in ["codex", "opencode"] {
        for (original, replacement, artifact, diagnostic) in [
            (
                json!({"status":"succeeded","artifacts":[],"data":{"approved":false}}),
                json!({"status":"succeeded","summary":"approved","artifacts":[],"data":{"approved":true}}),
                false,
                "approval decision",
            ),
            (
                json!({"status":"succeeded","artifacts":[],"data":{}}),
                json!({"status":"succeeded","summary":"done","artifacts":[],"data":{"publish":{"title":"unexpected","body":"unexpected"}}}),
                false,
                "GitHub action",
            ),
            (
                json!({"status":"succeeded","artifacts":[],"data":{}}),
                valid_result(),
                true,
                "artifacts",
            ),
            (
                json!({"status":"blocked","artifacts":[],"data":{}}),
                valid_result(),
                false,
                "blocked or failed status",
            ),
        ] {
            let (dir, factory) = repair_fixture(
                agent,
                &[
                    json!({"value":original,"artifact":artifact}),
                    json!({"value":replacement}),
                ],
            );
            let (id, detail) = run_repair_fixture(&factory).await;
            assert_eq!(detail.run.status, "failed", "{agent}: {:?}", detail.nodes);
            assert_eq!(attempts(&dir, &id), 3);
            let error = detail.nodes[0].error.as_deref().unwrap();
            assert!(error.contains(diagnostic), "{agent}: {error}");
        }
    }
}

#[tokio::test]
async fn repair_can_correct_status_while_preserving_rejected_approval() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) = repair_fixture(
            agent,
            &[
                json!({"value":{"status":"succeeded","summary":"changes required","artifacts":[],"data":{"approved":false}}}),
                json!({"value":{"status":"blocked","summary":"changes required","artifacts":[],"data":{"approved":false}}}),
            ],
        );
        let (id, detail) = run_repair_fixture(&factory).await;
        assert_eq!(detail.run.status, "blocked", "{agent}: {:?}", detail.nodes);
        assert_eq!(attempts(&dir, &id), 2);
    }
}

#[tokio::test]
async fn failed_repair_process_retains_original_validation_error() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) =
            repair_fixture(agent, &[json!({"value":"{invalid"}), json!({"exit":7})]);
        let (id, detail) = run_repair_fixture(&factory).await;
        assert_eq!(detail.run.status, "failed");
        assert_eq!(attempts(&dir, &id), 2);
        let error = detail.nodes[0].error.as_deref().unwrap();
        assert!(
            error.contains("Invalid node result JSON"),
            "{agent}: {error}"
        );
        assert!(error.contains("exit status: 7"), "{agent}: {error}");
    }
}

#[tokio::test]
async fn process_failures_artifact_errors_and_valid_blockers_are_not_retried() {
    for agent in ["codex", "opencode"] {
        for (response, expected_status) in [
            (json!({"exit":7}), "failed"),
            (
                json!({"value":{"status":"succeeded","summary":"done","artifacts":[{"path":"missing","sha256":"bad","kind":"report"}],"data":{}}}),
                "failed",
            ),
            (
                json!({"value":{"status":"blocked","summary":"missing input","artifacts":[],"data":{}}}),
                "blocked",
            ),
        ] {
            let (dir, factory) = repair_fixture(agent, &[response]);
            let (id, detail) = run_repair_fixture(&factory).await;
            assert_eq!(
                detail.run.status, expected_status,
                "{agent}: {:?}",
                detail.nodes
            );
            assert_eq!(attempts(&dir, &id), 1);
        }
    }
}

#[tokio::test]
async fn cancellation_stops_repair_process_without_another_attempt() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) =
            repair_fixture(agent, &[json!({"value":"{invalid"}), json!({"sleep":30})]);
        let id = factory
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                191,
                "[AI] cancel repair",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        factory.start();
        let path = dir
            .path()
            .join("data/sessions")
            .join(&id)
            .join("spec.launch-2.json");
        for _ in 0..200 {
            if path.exists() {
                break;
            }
            if factory.run_detail(&id).unwrap().run.status == "failed" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(path.exists(), "{agent}: repair did not start");
        factory.cancel(&id).unwrap();
        let detail = terminal(&factory, &id).await;
        factory.shutdown().await;
        assert_eq!(detail.run.status, "cancelled");
        assert_eq!(attempts(&dir, &id), 2);
    }
}
