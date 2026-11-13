use super::*;

fn output_fixture(
    agent: &str,
    responses: &[serde_json::Value],
) -> (super::FixtureDir, std::sync::Arc<Factory>) {
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
    print('agent process started', flush=True)
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
    if response.get('verification_log'):
        log = artifact.with_name('verification.log')
        log.write_text('original verification log')
        value['artifacts'].append({'path': str(log), 'kind': 'verification_log',
            'sha256': hashlib.sha256(log.read_bytes()).hexdigest()})
    if response.get('legacy_type'):
        for item in value['artifacts']:
            item['type'] = item.pop('kind')
    if response.get('missing_kind'):
        for item in value['artifacts']:
            item.pop('kind', None)
    if response.get('blank_kind'):
        for item in value['artifacts']:
            item['kind'] = ''
    if response.get('drop_artifact'):
        value['artifacts'].pop()
    if response.get('change_hash'):
        value['artifacts'][0]['sha256'] = '0' * 64
    if response.get('change_path'):
        value['artifacts'][0]['path'] += '.different'
    if response.get('artifact_map'):
        names = ['report', 'patch', 'verification_log']
        value['artifacts'] = {names[i]: item for i, item in enumerate(value['artifacts'])}
    if response.get('artifact_single'):
        value['artifacts'] = value['artifacts'][0]
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

async fn run_output_fixture(factory: &std::sync::Arc<Factory>) -> (String, RunDetail) {
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
async fn invalid_first_response_fails_without_a_second_agent_invocation() {
    for agent in ["codex", "opencode"] {
        for value in [
            json!("Implementation complete. No JSON envelope."),
            json!("{invalid"),
            json!({"status":"complete","summary":"done","artifacts":[],"data":{}}),
            json!({"status":"succeeded","data":{}}),
            json!({"status":"succeeded","summary":"rejected","artifacts":[],"data":{"approved":false}}),
        ] {
            let (dir, factory) = output_fixture(
                agent,
                &[
                    json!({"value":value,"stdout":true,"noise":true}),
                    json!({"value":valid_result()}),
                ],
            );
            let mut settings = factory.settings().unwrap();
            let mut consumer = settings.flows[0].nodes[0].clone();
            consumer.id = "consumer".into();
            consumer.depends_on = vec![settings.flows[0].nodes[0].id.clone()];
            settings.flows[0].nodes.push(consumer);
            factory.save_settings(settings).unwrap();
            let (id, detail) = run_output_fixture(&factory).await;
            assert_eq!(
                detail.run.status, "failed",
                "{agent}/{value}: {:?}",
                detail.nodes
            );
            assert_eq!(attempts(&dir, &id), 1, "{agent}/{value}");
            assert_eq!(
                detail
                    .nodes
                    .iter()
                    .find(|node| node.id == "consumer")
                    .unwrap()
                    .status,
                "skipped"
            );
            let session = dir.path().join("data/sessions").join(&id);
            let rejected =
                std::fs::read_to_string(session.join("spec.invalid-response.txt")).unwrap();
            assert!(!rejected.contains("irrelevant execution trace"));
            let error = std::fs::read_to_string(session.join("spec.validation-error.txt")).unwrap();
            assert!(!error.is_empty());
            assert!(detail
                .nodes
                .iter()
                .find(|node| node.id == "spec")
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains(&error));
            assert!(!session.join("spec.launch-2.json").exists());
            assert!(!session.join("spec.repair-1.prompt.txt").exists());
        }
    }
}

#[tokio::test]
async fn first_response_contract_is_injected_into_system_prompt_and_native_schema() {
    for agent in ["codex", "opencode"] {
        let value = if agent == "codex" {
            json!({"status":"succeeded","summary":"work complete","artifacts":[],"data_json":"{\"branch\":\"feature\"}"})
        } else {
            valid_result()
        };
        let (dir, factory) = output_fixture(
            agent,
            &[json!({"value":value,"stdout":agent == "opencode"})],
        );
        let (id, detail) = run_output_fixture(&factory).await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        assert_eq!(attempts(&dir, &id), 1);
        let session = dir.path().join("data/sessions").join(&id);
        let prompt = std::fs::read_to_string(session.join("spec.stdin-1.txt")).unwrap();
        for instruction in [
            "Single-attempt output contract",
            "No response repair or automatic retry",
            "Before ending this execution",
            "Return exactly one JSON object",
        ] {
            assert!(
                prompt.contains(instruction),
                "{agent}: missing {instruction}"
            );
        }
        let launch: serde_json::Value =
            serde_json::from_slice(&std::fs::read(session.join("spec.launch-1.json")).unwrap())
                .unwrap();
        if agent == "codex" {
            assert!(launch["args"]
                .as_array()
                .unwrap()
                .contains(&json!("--output-schema")));
            let schema: serde_json::Value =
                serde_json::from_slice(&std::fs::read(session.join("spec.schema.json")).unwrap())
                    .unwrap();
            assert_eq!(schema["properties"]["data_json"]["type"], "string");
        } else {
            let config: serde_json::Value =
                serde_json::from_str(launch["config"].as_str().unwrap()).unwrap();
            let system = config["agent"]["ai-factory"]["prompt"].as_str().unwrap();
            assert!(system.contains("Single-attempt output contract"));
            assert!(system.contains("AI Factory unattended execution contract"));
            assert!(system.contains("Output JSON Schema"));
        }
    }
}

#[tokio::test]
async fn missing_or_blank_artifact_kind_uses_neutral_category_without_repeating_work() {
    for agent in ["codex", "opencode"] {
        for field in ["missing_kind", "blank_kind"] {
            let mut response = json!({"value":valid_result(),"artifact":true,"patch":true,"stdout":agent == "opencode"});
            response[field] = json!(true);
            let (dir, factory) = output_fixture(agent, &[response]);
            let (id, detail) = run_output_fixture(&factory).await;
            assert_eq!(
                detail.run.status, "succeeded",
                "{agent}/{field}: {:?}",
                detail.nodes
            );
            assert_eq!(attempts(&dir, &id), 1);
            let result = detail.nodes[0].result.as_ref().unwrap();
            for (artifact, expected) in result["artifacts"]
                .as_array()
                .unwrap()
                .iter()
                .zip(["original evidence", "original patch"])
            {
                assert_eq!(artifact["kind"], "artifact");
                let bytes = std::fs::read(artifact["path"].as_str().unwrap()).unwrap();
                assert_eq!(bytes, expected.as_bytes());
                use sha2::{Digest, Sha256};
                assert_eq!(artifact["sha256"], format!("{:x}", Sha256::digest(bytes)));
            }
        }
    }
}

#[tokio::test]
async fn neutral_artifact_category_preserves_failure_status_and_physical_file_checks() {
    for agent in ["codex", "opencode"] {
        for status in ["blocked", "failed"] {
            let mut value = valid_result();
            value["status"] = json!(status);
            let (dir, factory) = output_fixture(
                agent,
                &[json!({"value":value,"artifact":true,"missing_kind":true})],
            );
            let (id, detail) = run_output_fixture(&factory).await;
            assert_eq!(detail.run.status, status, "{agent}: {:?}", detail.nodes);
            assert_eq!(attempts(&dir, &id), 1);
            assert_eq!(
                detail.nodes[0].result.as_ref().unwrap()["artifacts"][0]["kind"],
                "artifact"
            );
        }
        let (dir, factory) = output_fixture(
            agent,
            &[
                json!({"value":{"status":"succeeded","summary":"done","artifacts":[{"path":"missing","sha256":"bad"}],"data":{}}}),
            ],
        );
        let (id, detail) = run_output_fixture(&factory).await;
        assert_eq!(detail.run.status, "failed");
        assert_eq!(attempts(&dir, &id), 1);
        assert!(detail.nodes[0]
            .error
            .as_deref()
            .unwrap()
            .contains("Artifact does not exist"));
    }
}

#[tokio::test]
async fn artifact_collection_shape_does_not_require_repeating_completed_work() {
    for agent in ["codex", "opencode"] {
        for shape in ["artifact_map", "artifact_single"] {
            let mut response = json!({"value":valid_result(),"artifact":true,"missing_kind":true,"stdout":agent == "opencode"});
            response[shape] = json!(true);
            if shape == "artifact_map" {
                response["patch"] = json!(true);
                response["verification_log"] = json!(true);
            }
            let (dir, factory) = output_fixture(agent, &[response]);
            let (id, detail) = run_output_fixture(&factory).await;
            assert_eq!(
                detail.run.status, "succeeded",
                "{agent}/{shape}: {:?}",
                detail.nodes
            );
            assert_eq!(attempts(&dir, &id), 1);
            let artifacts = detail.nodes[0].result.as_ref().unwrap()["artifacts"]
                .as_array()
                .unwrap();
            assert_eq!(artifacts.len(), if shape == "artifact_map" { 3 } else { 1 });
            for artifact in artifacts {
                use sha2::{Digest, Sha256};
                let bytes = std::fs::read(artifact["path"].as_str().unwrap()).unwrap();
                assert_eq!(artifact["sha256"], format!("{:x}", Sha256::digest(bytes)));
                assert_eq!(artifact["kind"], "artifact");
            }
        }
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
            let (dir, factory) = output_fixture(agent, &[response]);
            let (id, detail) = run_output_fixture(&factory).await;
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
async fn node_schema_and_codex_encoded_data_errors_fail_on_first_response() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) = output_fixture(
            agent,
            &[
                json!({"value":valid_result()}),
                json!({"value":{"status":"succeeded","summary":"done","artifacts":[],"data":{"deliverable":"invented on retry"}}}),
            ],
        );
        let mut settings = factory.settings().unwrap();
        settings.flows[0].nodes[0].result_schema = json!({"type":"object","required":["deliverable"],"properties":{"deliverable":{"type":"string"}},"additionalProperties":false});
        factory.save_settings(settings).unwrap();
        let (id, detail) = run_output_fixture(&factory).await;
        assert_eq!(detail.run.status, "failed");
        assert_eq!(attempts(&dir, &id), 1);
        let error = detail.nodes[0].error.as_deref().unwrap();
        assert!(error.contains("/data"));
        assert!(error.contains("deliverable"));
    }
    let (dir, factory) = output_fixture(
        "codex",
        &[
            json!({"value":{"status":"succeeded","summary":"done","artifacts":[],"data_json":"{"}}),
            json!({"value":valid_result()}),
        ],
    );
    let (id, detail) = run_output_fixture(&factory).await;
    assert_eq!(detail.run.status, "failed");
    assert_eq!(attempts(&dir, &id), 1);
    assert!(detail.nodes[0]
        .error
        .as_deref()
        .unwrap()
        .contains("data_json contains invalid JSON"));
}

#[tokio::test]
async fn legacy_artifact_category_is_normalized_on_first_response() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) = output_fixture(
            agent,
            &[
                json!({"value":valid_result(),"artifact":true,"patch":true,"legacy_type":true,"stdout":agent == "opencode"}),
            ],
        );
        let (id, detail) = run_output_fixture(&factory).await;
        assert_eq!(detail.run.status, "succeeded", "{:?}", detail.nodes);
        assert_eq!(attempts(&dir, &id), 1);
        let artifacts = detail.nodes[0].result.as_ref().unwrap()["artifacts"]
            .as_array()
            .unwrap();
        assert_eq!(artifacts[0]["kind"], "report");
        assert_eq!(artifacts[1]["kind"], "implementation_patch");
        for artifact in artifacts {
            use sha2::{Digest, Sha256};
            let bytes = std::fs::read(artifact["path"].as_str().unwrap()).unwrap();
            assert_eq!(artifact["sha256"], format!("{:x}", Sha256::digest(bytes)));
            assert!(artifact.get("type").is_none());
        }
    }
}

#[tokio::test]
async fn cancellation_stops_initial_process_without_another_invocation() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) = output_fixture(agent, &[json!({"sleep":30})]);
        let id = factory
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                191,
                "[AI] cancel initial",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        factory.start();
        let session = dir.path().join("data/sessions").join(&id);
        for _ in 0..200 {
            if session.join("spec.launch-1.json").exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(session.join("spec.launch-1.json").exists());
        factory.cancel(&id).unwrap();
        let detail = terminal(&factory, &id).await;
        factory.shutdown().await;
        assert_eq!(detail.run.status, "cancelled");
        assert_eq!(attempts(&dir, &id), 1);
        assert!(!session.join("spec.launch-2.json").exists());
    }
}
