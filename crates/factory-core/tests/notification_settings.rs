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

#[test]
fn google_chat_input_url_is_never_serialized() {
    let mut value = serde_json::to_value(Settings::default()).unwrap();
    value["notification_connectors"] = json!([{
        "id":"gchat", "label":"Team", "enabled":false,
        "events":["run_started"], "type":"google_chat",
        "config":{"has_webhook_url":false,
            "webhook_url":"https://chat.googleapis.com/v1/spaces/fixture/messages?key=PRIVATE_KEY&token=PRIVATE_TOKEN",
            "triggers":["new_issue"], "token_profile_ids":[]}
    }]);
    let settings: Settings = serde_json::from_value(value).unwrap();
    let public = serde_json::to_value(&settings).unwrap();
    assert!(!public.to_string().contains("PRIVATE_KEY"));
    assert!(!public.to_string().contains("PRIVATE_TOKEN"));
    assert!(public["notification_connectors"][0]["config"]
        .get("webhook_url")
        .is_none());
    assert!(!format!("{settings:?}").contains("PRIVATE_TOKEN"));
}

#[test]
fn google_chat_filters_are_structural_and_referential() {
    let base = json!({"id":"gchat","label":"Team","enabled":true,"events":["run_started"],"type":"google_chat","config":{"has_webhook_url":true,"triggers":["new_issue"],"token_profile_ids":["work"]}});
    let mut valid = serde_json::to_value(Settings::default()).unwrap();
    valid["token_profiles"] = json!([{
        "id":"work", "label":"Work", "account":"alice", "has_token":false,
        "owners":[], "authors":[], "reviewers":[]
    }]);
    valid["notification_connectors"] = json!([base]);
    serde_json::from_value::<Settings>(valid.clone())
        .unwrap()
        .validate()
        .expect("enabled Google Chat filter baseline must be valid");

    for (case, expected_error) in [
        (
            "missing_profile",
            "Select an available GitHub profile for Google Chat",
        ),
        (
            "no_events",
            "Enabled Google Chat connector requires a run event",
        ),
        (
            "no_triggers",
            "Enabled Google Chat connector requires a trigger",
        ),
        (
            "no_profiles",
            "Enabled Google Chat connector requires a GitHub profile",
        ),
        (
            "duplicate_trigger",
            "Invalid or duplicate Google Chat triggers",
        ),
        (
            "unknown_trigger",
            "Invalid or duplicate Google Chat triggers",
        ),
        (
            "duplicate_profile",
            "Invalid or duplicate Google Chat profiles",
        ),
        (
            "unsafe_profile",
            "Invalid or duplicate Google Chat profiles",
        ),
    ] {
        let mut value = valid.clone();
        if case == "missing_profile" {
            value["token_profiles"] = json!([]);
        }
        let c = &mut value["notification_connectors"][0];
        match case {
            "no_events" => c["events"] = json!([]),
            "no_triggers" => c["config"]["triggers"] = json!([]),
            "no_profiles" => c["config"]["token_profile_ids"] = json!([]),
            "duplicate_trigger" => c["config"]["triggers"] = json!(["new_issue", "new_issue"]),
            "unknown_trigger" => c["config"]["triggers"] = json!(["future"]),
            "duplicate_profile" => c["config"]["token_profile_ids"] = json!(["work", "work"]),
            "unsafe_profile" => c["config"]["token_profile_ids"] = json!(["../work"]),
            _ => {}
        }
        let s: Settings = serde_json::from_value(value).unwrap();
        let error = s.validate().expect_err(case);
        assert_eq!(error.to_string(), expected_error, "{case}");
    }
    let mut value = valid;
    value["token_profiles"] = json!([]);
    value["notification_connectors"][0]["enabled"] = json!(false);
    serde_json::from_value::<Settings>(value.clone())
        .unwrap()
        .validate()
        .unwrap();
    value["notification_connectors"][0]["config"]["triggers"] = json!(17);
    assert!(serde_json::from_value::<Settings>(value).is_err());
}

#[test]
fn notification_validation_does_not_change_execution_capacity_queries() {
    let mut value = serde_json::to_value(Settings::default()).unwrap();
    value["notification_connectors"] = json!([{"id":"chat","label":"Chat","enabled":true,"events":["run_started"],"type":"google_chat","config":{"triggers":["new_issue"],"token_profile_ids":["deleted"]}}]);
    let s: Settings = serde_json::from_value(value).unwrap();
    assert!(s.validate().is_err());
    assert_eq!(s.concurrency_for("new_issue").unwrap(), s.max_concurrent);
}
