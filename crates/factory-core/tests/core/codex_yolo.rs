use super::*;

#[test]
fn codex_yolo_defaults_off_for_existing_settings_and_round_trips_explicit_values() {
    let mut legacy = serde_json::to_value(Settings::default()).unwrap();
    legacy.as_object_mut().unwrap().remove("codex_yolo");
    let settings: Settings = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(settings).unwrap()["codex_yolo"], false);
    for enabled in [true, false] {
        legacy["codex_yolo"] = json!(enabled);
        let settings: Settings = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(settings).unwrap()["codex_yolo"],
            enabled
        );
    }
}

#[tokio::test]
#[cfg(unix)]
async fn codex_yolo_launches_without_sandbox_only_when_enabled() {
    for enabled in [false, true] {
        for access in ["read", "write"] {
            let (dir, factory) = fixture(
                r#"cat >/dev/null
printf '%s\n' "$@" > "${AI_FACTORY_RESULT_PATH%.result.json}.args.txt"
printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH""#,
                4,
            );
            let mut settings = serde_json::to_value(factory.settings().unwrap()).unwrap();
            settings["codex_yolo"] = json!(enabled);
            settings["flows"][0]["nodes"][0]["access"] = json!(access);
            factory
                .save_settings(serde_json::from_value(settings).unwrap())
                .unwrap();
            let id = factory
                .enqueue(TriggerEvent::issue(
                    "acme/repo",
                    410,
                    "[AI] execution permissions",
                    "alice",
                    "",
                ))
                .unwrap()
                .unwrap();
            factory.start();
            let detail = terminal(&factory, &id).await;
            factory.shutdown().await;
            assert_eq!(detail.run.status, "succeeded", "{:?}", detail.nodes);
            let args = std::fs::read_to_string(
                dir.path()
                    .join("data/sessions")
                    .join(&id)
                    .join("spec.args.txt"),
            )
            .unwrap();
            let args: Vec<_> = args.lines().collect();
            assert_eq!(
                args.contains(&"--dangerously-bypass-approvals-and-sandbox"),
                enabled
            );
            assert_eq!(args.contains(&"--sandbox"), !enabled);
            assert_eq!(args.contains(&"approval_policy=\"never\""), !enabled);
            if !enabled {
                assert!(args.windows(2).any(|pair| pair
                    == [
                        "--sandbox",
                        if access == "read" {
                            "read-only"
                        } else {
                            "workspace-write"
                        }
                    ]));
            }
            assert_eq!(
                serde_json::to_value(factory.settings().unwrap()).unwrap()["codex_yolo"],
                enabled
            );
            for required in [
                "--output-schema",
                "--output-last-message",
                "--ephemeral",
                "cli_auth_credentials_store=\"file\"",
            ] {
                assert!(args.contains(&required), "missing {required}: {args:?}");
            }
        }
    }
}

#[tokio::test]
#[cfg(unix)]
async fn codex_yolo_does_not_change_opencode_permissions() {
    let (dir, factory) = fixture(
        r#"cat >/dev/null
printf '%s\n' "$@" > "${AI_FACTORY_RESULT_PATH%.result.json}.args.txt"
printf '%s' "$OPENCODE_CONFIG_CONTENT" > "${AI_FACTORY_RESULT_PATH%.result.json}.config.json"
printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH""#,
        4,
    );
    let mut settings = serde_json::to_value(factory.settings().unwrap()).unwrap();
    settings["codex_yolo"] = json!(true);
    settings["flows"][0]["nodes"][0]["agent"] = json!("opencode");
    settings["flows"][0]["nodes"][0]["access"] = json!("read");
    factory
        .save_settings(serde_json::from_value(settings).unwrap())
        .unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            411,
            "[AI] OpenCode unchanged",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "succeeded", "{:?}", detail.nodes);
    let session = dir.path().join("data/sessions").join(id);
    let args = std::fs::read_to_string(session.join("spec.args.txt")).unwrap();
    assert!(args.lines().any(|arg| arg == "--auto"));
    assert!(!args.contains("--dangerously-bypass-approvals-and-sandbox"));
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(session.join("spec.config.json")).unwrap()).unwrap();
    for permissions in [
        &config["permission"],
        &config["agent"]["ai-factory"]["permission"],
    ] {
        assert_eq!(permissions["*"], "deny");
        assert_eq!(permissions["question"], "deny");
        assert_eq!(permissions["external_directory"]["*"], "deny");
    }
}
