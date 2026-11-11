mod common;
use factory_core::*;
use serde_json::json;

fn settings_with_overrides() -> Settings {
    let mut value = serde_json::to_value(Settings::default()).unwrap();
    value["token_profiles"] = json!([
        {"id":"work", "label":"Work", "account":"alice", "token":"fixture-token"},
        {"id":"other", "label":"Other", "account":"bob", "token":"other-fixture-token"}
    ]);
    let nodes = value["flows"][0]["nodes"].as_array_mut().unwrap();
    for node in nodes.iter_mut() {
        node["agent"] = json!("codex");
        node["model"] = json!("default-model");
        node["effort"] = json!("high");
    }
    nodes[0]["token_overrides"] = json!({
        "work": {"agent":"opencode", "model":"provider/work", "effort":"deep"},
        "other": {"agent":"codex", "model":"other-model", "effort":"low"}
    });
    nodes[1]["token_overrides"] = json!({
        "work": {"agent":"codex", "model":"different-node-model", "effort":"medium"}
    });
    serde_json::from_value(value).unwrap()
}

#[test]
fn snapshots_use_overrides_per_node_and_token_and_keep_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    factory.save_settings(settings_with_overrides()).unwrap();
    for (number, actor, expected) in [
        (
            1,
            "ALICE",
            [
                ("opencode", "provider/work", "deep"),
                ("codex", "different-node-model", "medium"),
            ],
        ),
        (
            2,
            "bob",
            [
                ("codex", "other-model", "low"),
                ("codex", "default-model", "high"),
            ],
        ),
        (
            3,
            "carol",
            [
                ("codex", "default-model", "high"),
                ("codex", "default-model", "high"),
            ],
        ),
    ] {
        let id = factory
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                number,
                "[AI] test",
                actor,
                "",
            ))
            .unwrap()
            .unwrap();
        let detail = factory.run_detail(&id).unwrap();
        for (index, (agent, model, effort)) in expected.iter().enumerate() {
            let node = &detail.flow.nodes[index];
            assert_eq!(
                (&*node.agent, &*node.model, &*node.effort),
                (*agent, *model, *effort)
            );
            assert_eq!(detail.nodes[index].agent, *agent);
        }
        assert_eq!(detail.flow.nodes[2].model, "default-model");
    }
    let saved = serde_json::to_value(factory.settings().unwrap()).unwrap();
    assert_eq!(
        saved["flows"][0]["nodes"][0]["token_overrides"]["work"]["model"],
        "provider/work"
    );
    assert_eq!(saved["flows"][0]["nodes"][0]["agent"], "codex");
}

#[test]
fn empty_override_model_and_effort_use_cli_defaults_without_inheriting() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    let mut value = serde_json::to_value(settings_with_overrides()).unwrap();
    value["flows"][0]["nodes"][0]["token_overrides"]["work"] =
        json!({"agent":"opencode", "model":"", "effort":""});
    factory
        .save_settings(serde_json::from_value(value).unwrap())
        .unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            1,
            "[AI] test",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    let node = factory.run_detail(&id).unwrap().flow.nodes.remove(0);
    assert_eq!(
        (&*node.agent, &*node.model, &*node.effort),
        ("opencode", "", "")
    );
}

#[test]
fn retries_resolve_latest_overrides_without_changing_previous_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    factory.save_settings(settings_with_overrides()).unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            1,
            "[AI] test",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    factory.set_run_status(&id, "failed", None).unwrap();
    let mut value = serde_json::to_value(factory.settings().unwrap()).unwrap();
    value["flows"][0]["nodes"][0]["token_overrides"]["work"]["model"] = json!("provider/updated");
    factory
        .save_settings(serde_json::from_value(value).unwrap())
        .unwrap();
    let retried = factory.retry(&id).unwrap();
    assert_eq!(
        factory.run_detail(&retried).unwrap().flow.nodes[0].model,
        "provider/updated"
    );
    assert_eq!(
        factory.run_detail(&id).unwrap().flow.nodes[0].model,
        "provider/work"
    );
}

#[test]
fn validates_each_override_even_when_its_profile_is_unavailable() {
    for config in [
        json!({"agent":"unsupported", "model":"", "effort":""}),
        json!({"agent":"codex", "model":"", "effort":"high"}),
        json!({"agent":"codex", "model":"   ", "effort":"high"}),
    ] {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value["flows"][0]["nodes"][0]["token_overrides"] = json!({"removed-profile": config});
        let settings: Settings = serde_json::from_value(value).unwrap();
        assert!(settings.validate().is_err());
    }
}

#[test]
fn legacy_settings_and_removed_profiles_fall_back_to_node_defaults() {
    let legacy: Settings =
        serde_json::from_value(serde_json::to_value(Settings::default()).unwrap()).unwrap();
    legacy.validate().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    let mut settings = settings_with_overrides();
    settings.token_profiles.clear();
    factory.save_settings(settings).unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            1,
            "[AI] test",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    assert_eq!(
        factory.run_detail(&id).unwrap().flow.nodes[0].model,
        "default-model"
    );
}
