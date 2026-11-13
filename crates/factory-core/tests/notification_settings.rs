use factory_core::Settings;
use serde_json::json;
#[test]
fn notification_settings_round_trip_retains_provider_and_preferences() {
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    wire["notification_connectors"] = json!([{"id":"team","label":"Team","enabled":false,"events":["run_failed"],"type":"telegram","config":{"chat_id":"-1001","has_token":false}}]);
    let settings: Settings = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(settings).unwrap()["notification_connectors"],
        wire["notification_connectors"]
    );
}
#[test]
fn notification_legacy_settings_and_unknown_types_round_trip_without_activation() {
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    wire.as_object_mut()
        .unwrap()
        .remove("notification_connectors");
    let legacy: Settings = serde_json::from_value(wire).unwrap();
    assert!(legacy.notification_connectors.is_empty());
    let mut value = serde_json::to_value(legacy).unwrap();
    value["notification_connectors"] = json!([
      {"id":"telegram","label":"Team","enabled":false,"events":[],"type":"telegram","config":{"chat_id":"","has_token":false}},
      {"id":"future","label":"Future","enabled":true,"events":["run_started"],"type":"future","config":{"nested":{"option":7}}}
    ]);
    let settings: Settings = serde_json::from_value(value.clone()).unwrap();
    settings.validate().unwrap();
    assert_eq!(
        serde_json::to_value(settings).unwrap()["notification_connectors"],
        value["notification_connectors"]
    );
}
#[test]
fn notification_invalid_structure_cannot_fall_back_to_unknown_provider() {
    let mut value = serde_json::to_value(Settings::default()).unwrap();
    value["notification_connectors"] = json!([{"id":"team","label":"Team","enabled":false,"events":[],"type":"telegram","config":{"chat_id":17,"bot_token":"PRIVATE_SENTINEL"}}]);
    let err = serde_json::from_value::<Settings>(value).unwrap_err();
    assert!(!err.to_string().contains("PRIVATE_SENTINEL"));
}
#[test]
fn notification_validation_rejects_duplicate_ids_events_and_active_incomplete_connectors() {
    let base = json!({"id":"team","label":"Team","enabled":false,"events":["run_failed"],"type":"telegram","config":{"chat_id":"","has_token":false}});
    for case in ["duplicate", "id", "label", "events", "chat", "no_events"] {
        let mut connector = base.clone();
        match case {
            "id" => connector["id"] = json!("bad/ID"),
            "label" => connector["label"] = json!(" "),
            "events" => connector["events"] = json!(["run_failed", "run_failed"]),
            "chat" => connector["enabled"] = json!(true),
            "no_events" => {
                connector["enabled"] = json!(true);
                connector["config"]["chat_id"] = json!("@channel");
                connector["events"] = json!([]);
            }
            _ => {}
        }
        let mut wire = serde_json::to_value(Settings::default()).unwrap();
        wire["notification_connectors"] = if case == "duplicate" {
            json!([connector.clone(), connector])
        } else {
            json!([connector])
        };
        let settings: Settings = serde_json::from_value(wire).unwrap();
        assert!(settings.validate().is_err(), "{case}");
    }
}
#[test]
fn notification_input_secret_is_not_serialized_or_debugged() {
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    wire["notification_connectors"] = json!([{"id":"team","label":"Team","enabled":false,"events":[],"type":"telegram","config":{"chat_id":"@channel","bot_token":"PRIVATE_SENTINEL"}}]);
    let settings: Settings = serde_json::from_value(wire).unwrap();
    assert!(!serde_json::to_string(&settings)
        .unwrap()
        .contains("PRIVATE_SENTINEL"));
    assert!(!format!("{settings:?}").contains("PRIVATE_SENTINEL"));
    if let factory_core::ConnectorProvider::Telegram(config) =
        &settings.redacted().notification_connectors[0].provider
    {
        assert!(config.bot_token.is_none());
    } else {
        panic!("provider changed");
    }
}
