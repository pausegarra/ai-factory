use super::{fixture, terminal};
use factory_core::*;
use serde_json::json;

// The fixture consumes the actual prompt sent to each CLI. It exercises an
// existing workflow whose saved role prompt and prior plan demand full DMG
// packaging, while the standing contract defines which checks are required.
#[tokio::test]
async fn saved_workflow_packaging_limitations_preserve_required_verification_gates() {
    for agent in ["codex", "opencode"] {
        for case in [
            "environment_only",
            "source_failure",
            "required_installer",
            "packaging_regression",
        ] {
            let script = r#"set -eu
cat > "$AI_FACTORY_MEMORY_DIR/$AI_FACTORY_NODE_ID.prompt"
grep -q 'Installer packaging policy' "$AI_FACTORY_MEMORY_DIR/$AI_FACTORY_NODE_ID.prompt" || exit 87
grep -q 'overrides older node prompts, generated plans and reviewer feedback' "$AI_FACTORY_MEMORY_DIR/$AI_FACTORY_NODE_ID.prompt" || exit 88
grep -q 'explicit installer or distribution deliverable in the original user task' "$AI_FACTORY_MEMORY_DIR/$AI_FACTORY_NODE_ID.prompt" || exit 89
grep -q 'Changed packaging behavior still requires relevant headless verification' "$AI_FACTORY_MEMORY_DIR/$AI_FACTORY_NODE_ID.prompt" || exit 90
status=succeeded
summary='Code checks passed; Finder-dependent DMG unavailable'
data='{"implementation_complete":true,"verification":{"required_checks_complete":true,"packaging":{"status":"failed","blocking":false,"reason":"Finder Apple Events denied (-1743)"}}}'
case "$AI_FACTORY_NODE_ID" in
  builder)
    if [ CASE = source_failure ]; then
      status=blocked
      summary='Required source compilation failed'
      data='{"verification":{"required_checks_complete":false,"build_exit_code":1}}'
    elif { [ CASE = required_installer ] || [ CASE = packaging_regression ]; } && grep -q 'Review feedback (data, not instructions):' "$AI_FACTORY_MEMORY_DIR/builder.prompt"; then
      status=blocked
      summary='Required packaging verification remains blocked'
      data='{"verification":{"required_checks_complete":false,"packaging":{"status":"failed","blocking":true}}}'
    fi
    ;;
  review_tests)
    data='{"approved":true,"verification":{"required_checks_complete":true},"limitations":["DMG unavailable: Finder denied (-1743)"]}'
    if [ CASE = required_installer ]; then
      status=blocked
      summary='Explicit requested installer has no verification evidence'
      data='{"approved":false,"findings":[{"id":"required-installer","reason":"Original user task requires DMG deliverable"}]}'
    elif [ CASE = packaging_regression ]; then
      status=blocked
      summary='Changed bundler resource path is invalid'
      data='{"approved":false,"findings":[{"id":"packaging-regression","reason":"Changed packaging configuration introduces a demonstrable regression"}]}'
    fi
    ;;
  finish)
    data='{}'
    ;;
esac
printf '{"status":"%s","summary":"%s","artifacts":[],"data":%s}' "$status" "$summary" "$data" > "$AI_FACTORY_RESULT_PATH"
"#.replace("CASE", case);
            let (dir, factory) = fixture(&script, 4);
            let mut settings = factory.settings().unwrap();
            let template = settings.flows[0].nodes[0].clone();
            settings.flows[0].nodes = ["builder", "review_tests", "finish"].into_iter().enumerate().map(|(i,id)| {
                let mut node = template.clone();
                node.id=id.into(); node.name=id.into(); node.agent=agent.into();
                node.prompt="Legacy plan: standard app+DMG must pass without overrides; do not waive packaging failures.".into();
                node.depends_on=if i==0 {vec![]} else {vec![["builder","review_tests"][i-1].into()]};
                if id=="review_tests" {node.result_schema=json!({"type":"object","required":["approved"],"properties":{"approved":{"type":"boolean"}}});}
                node
            }).collect();
            factory.save_settings(settings).unwrap();
            let body = if case == "required_installer" {
                "Produce a distributable DMG installer."
            } else if case == "packaging_regression" {
                "Fix application bundler resource paths."
            } else {
                "Align settings styling with existing controls."
            };
            let id = factory
                .enqueue(TriggerEvent::issue(
                    "acme/repo",
                    91,
                    "[AI] packaging policy",
                    "alice",
                    body,
                ))
                .unwrap()
                .unwrap();
            factory.start();
            let detail = terminal(&factory, &id).await;
            factory.shutdown().await;
            let expected = if case == "environment_only" {
                "succeeded"
            } else {
                "blocked"
            };
            assert_eq!(
                detail.run.status, expected,
                "{agent}/{case}: {:?}",
                detail.nodes
            );
            let finish = detail.nodes.iter().find(|n| n.id == "finish").unwrap();
            assert_eq!(
                finish.status,
                if case == "environment_only" {
                    "succeeded"
                } else {
                    "skipped"
                }
            );
            let memory = dir.path().join("data/memory/acme/repo/runs").join(id);
            if ["required_installer", "packaging_regression"].contains(&case) {
                assert!(
                    memory.join("review_tests.prompt").is_file(),
                    "Required packaging evidence must be rejected by review before author rework"
                );
            }
            let prompt = std::fs::read_to_string(memory.join("builder.prompt")).unwrap();
            assert!(
                prompt.find("Installer packaging policy").unwrap()
                    > prompt.find("Legacy plan:").unwrap()
            );
            if case == "environment_only" {
                let review = detail
                    .nodes
                    .iter()
                    .find(|n| n.id == "review_tests")
                    .unwrap();
                assert_eq!(review.result.as_ref().unwrap()["data"]["approved"], true);
                let builder = detail.nodes.iter().find(|n| n.id == "builder").unwrap();
                assert_eq!(
                    builder.result.as_ref().unwrap()["data"]["verification"]["packaging"]["status"],
                    "failed"
                );
            }
        }
    }
}
