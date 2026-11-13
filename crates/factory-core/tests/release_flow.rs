mod common;
use factory_core::*;
use serde_json::json;

fn approval(command: &str) -> TriggerEvent {
    let mut event = TriggerEvent::issue(
        "acme/repo",
        5,
        "PR",
        "alice",
        "release in body is not authorization",
    );
    event.trigger_id = "pr_approved".into();
    event.head_sha = "a".repeat(40);
    event.event_id = format!("approval:{command}:{}", event.head_sha);
    event.payload = json!({"approval":{"command":command,"actor":"alice","head_sha":event.head_sha,"comment_id":7}});
    event
}

#[test]
fn release_comment_adds_publication_after_every_configured_merge_step() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    let mut settings = factory.settings().unwrap();
    let flow = &mut settings.flows[1];
    let mut check = flow.nodes[0].clone();
    check.id = "conflicts".into();
    flow.nodes[0].depends_on = vec![check.id.clone()];
    flow.nodes.push(check);
    factory.save_settings(settings).unwrap();
    let id = factory.enqueue(approval("release")).unwrap().unwrap();
    let detail = factory.run_detail(&id).unwrap();
    let release = detail
        .flow
        .nodes
        .iter()
        .find(|node| node.id == "factory_release")
        .expect("release command must add a publication step");
    assert_eq!(release.name, "Publish release");
    assert_eq!(release.access, "write");
    assert_eq!(release.depends_on, vec!["publish", "conflicts"]);
    assert!(release.prompt.contains("major"));
    assert!(release.prompt.contains("minor"));
    assert!(release.prompt.contains("patch"));
    assert!(release.prompt.contains("diff"));
    assert!(detail.nodes.iter().any(|node| node.id == release.id));
    factory.cancel(&id).unwrap();
    let retry = factory.retry(&id).unwrap();
    assert_eq!(
        factory
            .run_detail(&retry)
            .unwrap()
            .flow
            .nodes
            .iter()
            .filter(|node| node.id == "factory_release")
            .count(),
        1
    );
    assert_eq!(factory.settings().unwrap().flows[1].nodes.len(), 2);
}

#[test]
fn ordinary_approval_and_forged_release_metadata_never_add_publication() {
    let mut wrong_actor = approval("release");
    wrong_actor.payload["approval"]["actor"] = json!("mallory");
    let mut wrong_sha = approval("release");
    wrong_sha.payload["approval"]["head_sha"] = json!("b".repeat(40));
    let mut missing_comment = approval("release");
    missing_comment.payload["approval"]
        .as_object_mut()
        .unwrap()
        .remove("comment_id");
    let mut wrong_trigger = approval("release");
    wrong_trigger.trigger_id = "pr_review".into();
    for event in [
        approval("ok"),
        approval("review"),
        wrong_actor,
        wrong_sha,
        missing_comment,
        wrong_trigger,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let factory = common::open(dir.path().into()).unwrap();
        let id = factory.enqueue(event).unwrap().unwrap();
        assert!(factory
            .run_detail(&id)
            .unwrap()
            .flow
            .nodes
            .iter()
            .all(|node| node.id != "factory_release"));
    }
}
