mod common;
use factory_core::*;
use serde_json::json;

#[test]
fn approval_workflow_shows_merge_then_version_bump_before_any_event() {
    let settings = Settings::default();
    let flow = &settings.flows[1];
    assert_eq!(flow.nodes.len(), 2);
    assert_eq!(flow.nodes[0].name, "Merge");
    let bump = &flow.nodes[1];
    assert_eq!(bump.id, "version_bump");
    assert_eq!(bump.name, "Bump version");
    assert_eq!(bump.depends_on, vec!["publish"]);
    for term in ["major", "minor", "patch", "diff", "Never create tags"] {
        assert!(bump.prompt.contains(term), "missing {term}");
    }
}

#[test]
fn stored_single_merge_workflow_migrates_preserving_agent_and_activation() {
    let mut value = serde_json::to_value(Settings::default()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("approval_workflow_version");
    value["flows"][1]["nodes"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    value["flows"][1]["nodes"][0]["agent"] = json!("opencode");
    value["flows"][1]["nodes"][0]["model"] = json!("custom-model");
    value["flows"][1]["enabled"] = json!(false);
    let settings: Settings = serde_json::from_value(value).unwrap();
    let flow = &settings.flows[1];
    assert!(!flow.enabled);
    assert_eq!(flow.nodes.len(), 2);
    assert_eq!(flow.nodes[1].agent, "opencode");
    assert_eq!(flow.nodes[1].model, "custom-model");
    assert_eq!(flow.nodes[1].depends_on, vec!["publish"]);
    let roundtrip: Settings =
        serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
    assert_eq!(roundtrip.flows[1].nodes.len(), 2);
}

#[test]
fn approvals_use_configured_graph_without_dynamic_release_node() {
    for command in ["ok", "review", "release"] {
        let dir = tempfile::tempdir().unwrap();
        let factory = common::open(dir.path().into()).unwrap();
        let mut event = TriggerEvent::issue("acme/repo", 5, "PR", "alice", command);
        event.trigger_id = "pr_approved".into();
        event.head_sha = "a".repeat(40);
        event.payload = json!({"approval":{"command":command,"actor":"alice","head_sha":event.head_sha,"comment_id":7}});
        let id = factory.enqueue(event).unwrap().unwrap();
        let detail = factory.run_detail(&id).unwrap();
        assert_eq!(detail.flow.nodes.len(), 2);
        assert!(detail
            .flow
            .nodes
            .iter()
            .all(|node| node.id != "factory_release"));
    }
}

#[test]
fn simultaneous_retries_cannot_run_the_same_approval_twice() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    let mut event = TriggerEvent::issue("acme/repo", 5, "PR", "alice", "OK");
    event.trigger_id = "pr_approved".into();
    event.head_sha = "a".repeat(40);
    let id = factory.enqueue(event).unwrap().unwrap();
    factory.cancel(&id).unwrap();
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    let outcomes = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let gate = gate.clone();
                let factory = &factory;
                let id = &id;
                scope.spawn(move || {
                    gate.wait();
                    factory.retry(id)
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(outcomes
        .iter()
        .filter_map(|result| result.as_ref().err())
        .all(|error| error.to_string().contains("active attempt")));
    let retry = outcomes.into_iter().find_map(Result::ok).unwrap();
    factory.cancel(&retry).unwrap();
    assert!(factory.retry(&id).is_ok());
}
