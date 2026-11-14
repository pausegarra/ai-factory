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
fn stored_approval_workflow_is_never_rewritten_on_read() {
    for version in [None, Some(0), Some(1)] {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("approval_workflow_version");
        if let Some(version) = version {
            value["approval_workflow_version"] = json!(version);
        }
        value["flows"][1]["nodes"]
            .as_array_mut()
            .unwrap()
            .truncate(1);
        let node = &mut value["flows"][1]["nodes"][0];
        node["name"] = json!("publish");
        node["agent"] = json!("opencode");
        node["model"] = json!("custom-model");
        node["prompt"] = json!("Only perform this configured step.");
        value["flows"][1]["enabled"] = json!(false);
        let expected = value["flows"].clone();
        let settings: Settings = serde_json::from_value(value).unwrap();
        assert_eq!(serde_json::to_value(&settings.flows).unwrap(), expected);
        let roundtrip: Settings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(&roundtrip.flows).unwrap(), expected);
    }
}

#[test]
fn legacy_graph_matches_editor_on_admission_retry_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    let mut value = serde_json::to_value(Settings::default()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("approval_workflow_version");
    value["flows"][1]["nodes"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    value["flows"][1]["nodes"][0]["name"] = json!("publish");
    let expected = value["flows"][1].clone();
    // Seed the legacy boundary directly: saving already-deserialized settings would hide injection.
    let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
    db.execute(
        "INSERT OR REPLACE INTO settings VALUES(1,?1)",
        [value.to_string()],
    )
    .unwrap();
    drop(db);
    let mut event = TriggerEvent::issue("acme/repo", 5, "PR", "alice", "OK");
    event.trigger_id = "pr_approved".into();
    event.head_sha = "a".repeat(40);
    let editor = factory.settings().unwrap();
    assert_eq!(serde_json::to_value(&editor.flows[1]).unwrap(), expected);
    let id = factory.enqueue(event).unwrap().unwrap();
    let detail = factory.run_detail(&id).unwrap();
    assert_eq!(serde_json::to_value(&detail.flow).unwrap(), expected);
    assert_eq!(
        detail
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["publish"]
    );
    factory.cancel(&id).unwrap();
    let retry = factory.retry(&id).unwrap();
    assert_eq!(
        serde_json::to_value(factory.run_detail(&retry).unwrap().flow).unwrap(),
        expected
    );
    factory.cancel(&retry).unwrap();
    factory.save_settings(editor).unwrap();
    drop(factory);
    let reopened = common::open(dir.path().into()).unwrap();
    assert_eq!(
        serde_json::to_value(&reopened.settings().unwrap().flows[1]).unwrap(),
        expected
    );
}

#[test]
fn deleting_configured_version_node_stays_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.flows[1]
        .nodes
        .retain(|node| node.id != "version_bump");
    let expected = serde_json::to_value(&settings.flows[1]).unwrap();
    factory.save_settings(settings).unwrap();
    drop(factory);
    let reopened = common::open(dir.path().into()).unwrap();
    assert_eq!(
        serde_json::to_value(&reopened.settings().unwrap().flows[1]).unwrap(),
        expected
    );
    let mut event = TriggerEvent::issue("acme/repo", 5, "PR", "alice", "OK");
    event.trigger_id = "pr_approved".into();
    event.head_sha = "a".repeat(40);
    let id = reopened.enqueue(event).unwrap().unwrap();
    let detail = reopened.run_detail(&id).unwrap();
    assert_eq!(serde_json::to_value(&detail.flow).unwrap(), expected);
    assert_eq!(detail.nodes.len(), 1);
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
