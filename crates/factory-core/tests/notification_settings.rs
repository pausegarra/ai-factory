use factory_core::Settings;
use serde_json::json;
#[test]
fn notification_settings_round_trip_retains_provider_and_preferences() {
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    wire["notification_connectors"] = json!([{"id":"team","label":"Team","enabled":false,"events":["run_failed"],"type":"telegram","config":{"chat_id":"-1001","has_token":false,"triggers":["new_issue","pr_approved","pr_review"],"token_profile_ids":[]}}]);
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
      {"id":"telegram","label":"Team","enabled":false,"events":[],"type":"telegram","config":{"chat_id":"","has_token":false,"triggers":["new_issue","pr_approved","pr_review"],"token_profile_ids":[]}},
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
    valid["source_connectors"] = json!([{
        "id":"work", "label":"Work", "type":"github_issues","account":"alice", "has_token":false,
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
            value["source_connectors"] = json!([]);
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
        if ["missing_profile", "no_profiles"].contains(&case) {
            s.validate().unwrap();
            continue;
        }
        let error = s.validate().expect_err(case);
        assert_eq!(error.to_string(), expected_error, "{case}");
    }
    let mut value = valid;
    value["source_connectors"] = json!([]);
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
    assert!(s.validate().is_ok());
    assert_eq!(s.concurrency_for("new_issue").unwrap(), s.max_concurrent);
}

#[test]
fn notification_telegram_legacy_filters_are_canonical_and_do_not_expand_after_save() {
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    wire["source_connectors"] =
        json!([{"id":"work","label":"Work","type":"github_issues","account":"alice"}]);
    wire["notification_connectors"] = json!([{
        "id":"tg","label":"Telegram","enabled":true,"events":["run_failed"],
        "type":"telegram","config":{"chat_id":"fixture","has_token":true}
    }]);
    let loaded: Settings = serde_json::from_value(wire).unwrap();
    let mut canonical = serde_json::to_value(&loaded).unwrap();
    let config = &canonical["notification_connectors"][0]["config"];
    assert_eq!(
        config["triggers"],
        json!(["new_issue", "pr_approved", "pr_review", "release"])
    );
    assert_eq!(config["token_profile_ids"], json!([]));
    assert_eq!(canonical["notification_connectors"][0]["enabled"], true);
    canonical["source_connectors"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"other","label":"Other","type":"github_issues","account":"bob","has_token":false,
            "owners":[],"authors":[],"reviewers":[],"review_all_open_pull_requests":false
        }));
    let reloaded: Settings = serde_json::from_value(canonical.clone()).unwrap();
    assert_eq!(serde_json::to_value(reloaded).unwrap(), canonical);
    assert_eq!(
        canonical["notification_connectors"][0]["config"]["token_profile_ids"],
        json!([])
    );
}

#[test]
fn notification_telegram_rejects_unknown_trigger_without_echoing_input() {
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    wire["source_connectors"] =
        json!([{"id":"work","label":"Work","type":"github_issues","account":"alice"}]);
    wire["notification_connectors"] = json!([{
        "id":"tg","label":"Telegram","enabled":true,"events":["run_started"],
        "type":"telegram","config":{"chat_id":"fixture","has_token":true,
        "triggers":["PRIVATE_SENTINEL"],"token_profile_ids":["work"]}
    }]);
    let settings: Settings = serde_json::from_value(wire).unwrap();
    let error = settings
        .validate()
        .expect_err("unknown Telegram trigger must fail");
    assert!(error.to_string().contains("triggers"));
    assert!(!error.to_string().contains("PRIVATE_SENTINEL"));
}

fn filter_settings(kind: &str) -> serde_json::Value {
    let mut wire = serde_json::to_value(Settings::default()).unwrap();
    wire["source_connectors"] =
        json!([{"id":"work","label":"Work","type":"github_issues","account":"alice"}]);
    wire["notification_connectors"] = json!([{
        "id":"team","label":"Team","enabled":true,"events":["run_started"],"type":kind,
        "config":{"chat_id":"fixture","has_token":true,"has_webhook_url":true,
        "triggers":["new_issue"],"token_profile_ids":["work"]}
    }]);
    wire
}

#[test]
fn notification_filter_validation_is_symmetric_and_safe() {
    for (kind, name) in [("telegram", "Telegram"), ("google_chat", "Google Chat")] {
        let base = filter_settings(kind);
        serde_json::from_value::<Settings>(base.clone())
            .unwrap()
            .validate()
            .unwrap();
        for (field, value, suffix, structural) in [
            ("triggers", json!(["PRIVATE_SENTINEL"]), "triggers", true),
            (
                "triggers",
                json!(["new_issue", "new_issue"]),
                "triggers",
                true,
            ),
            (
                "token_profile_ids",
                json!(["work", "work"]),
                "profiles",
                true,
            ),
            ("token_profile_ids", json!(["../work"]), "profiles", true),
            ("token_profile_ids", json!(["."]), "profiles", true),
            ("token_profile_ids", json!([".."]), "profiles", true),
            (
                "token_profile_ids",
                json!(["x".repeat(129)]),
                "profiles",
                true,
            ),
            (
                "events",
                json!(["run_started", "run_started"]),
                "events",
                true,
            ),
            ("events", json!([]), "run event", false),
            ("triggers", json!([]), "trigger", false),
            ("token_profile_ids", json!([]), "GitHub profile", false),
            (
                "token_profile_ids",
                json!(["deleted"]),
                "available GitHub profile",
                false,
            ),
        ] {
            for enabled in [true, false] {
                let mut wire = base.clone();
                let c = &mut wire["notification_connectors"][0];
                c["enabled"] = json!(enabled);
                if field == "events" {
                    c[field] = value.clone();
                } else {
                    c["config"][field] = value.clone();
                }
                let s: Settings = serde_json::from_value(wire).unwrap();
                if field == "token_profile_ids" && !structural {
                    s.validate().unwrap();
                    continue;
                }
                if enabled || structural {
                    let error = s.validate().expect_err(field).to_string();
                    assert!(error.contains(suffix), "{kind} {field}: {error}");
                    if field != "events" {
                        assert!(error.contains(name));
                    }
                    assert!(!error.contains("PRIVATE_SENTINEL"));
                } else {
                    s.validate().unwrap();
                }
            }
        }
    }
}

#[test]
fn notification_telegram_migration_only_fills_missing_fields_and_is_idempotent() {
    for enabled in [true, false] {
        for (triggers, profiles) in [
            (None, None),
            (Some(json!([])), None),
            (None, Some(json!([]))),
            (Some(json!(["pr_review"])), Some(json!(["work"]))),
            (Some(json!([])), Some(json!([]))),
        ] {
            let mut wire = filter_settings("telegram");
            wire["source_connectors"].as_array_mut().unwrap().push(
                json!({"id":"personal","label":"Personal","type":"github_issues","account":"bob"}),
            );
            let c = &mut wire["notification_connectors"][0];
            c["enabled"] = json!(enabled);
            let cfg = c["config"].as_object_mut().unwrap();
            cfg.remove("has_webhook_url");
            cfg.remove("triggers");
            cfg.remove("token_profile_ids");
            if let Some(value) = &triggers {
                cfg.insert("triggers".into(), value.clone());
            }
            if let Some(value) = &profiles {
                cfg.insert("token_profile_ids".into(), value.clone());
            }
            let s: Settings = serde_json::from_value(wire).unwrap();
            let canonical = serde_json::to_value(s).unwrap();
            let c = &canonical["notification_connectors"][0];
            assert_eq!(c["enabled"], enabled);
            assert_eq!(c["config"]["chat_id"], "fixture");
            assert_eq!(c["events"], json!(["run_started"]));
            assert_eq!(
                c["config"]["triggers"],
                triggers.unwrap_or(json!(["new_issue", "pr_approved", "pr_review", "release"]))
            );
            assert_eq!(
                c["config"]["token_profile_ids"],
                profiles.unwrap_or(json!([]))
            );
            for _ in 0..2 {
                assert_eq!(
                    serde_json::to_value(
                        serde_json::from_value::<Settings>(canonical.clone()).unwrap()
                    )
                    .unwrap(),
                    canonical
                );
            }
        }
    }
    let mut wire = filter_settings("telegram");
    wire["source_connectors"] = json!([]);
    wire["notification_connectors"][0]["config"]
        .as_object_mut()
        .unwrap()
        .remove("token_profile_ids");
    let mut s: Settings = serde_json::from_value(wire).unwrap();
    assert!(s.notification_connectors[0].enabled);
    assert_eq!(
        serde_json::to_value(&s).unwrap()["notification_connectors"][0]["config"]
            ["token_profile_ids"],
        json!([])
    );
    s.validate().unwrap();
    s.notification_connectors[0].enabled = false;
    s.validate().unwrap();
}

#[test]
fn notification_known_filters_reject_malformed_fields_without_reflecting_values() {
    for kind in ["telegram", "google_chat"] {
        for field in ["triggers", "token_profile_ids"] {
            for value in [
                json!(null),
                json!(17),
                json!({"PRIVATE_SENTINEL":1}),
                json!([17]),
                json!("PRIVATE_SENTINEL"),
            ] {
                let mut wire = filter_settings(kind);
                wire["notification_connectors"][0]["config"][field] = value;
                let error = serde_json::from_value::<Settings>(wire)
                    .unwrap_err()
                    .to_string();
                assert!(!error.contains("PRIVATE_SENTINEL"));
            }
        }
    }
}

#[test]
fn notification_migration_does_not_invent_filters_for_other_providers() {
    let mut wire = filter_settings("google_chat");
    wire["notification_connectors"][0]["enabled"] = json!(false);
    let cfg = wire["notification_connectors"][0]["config"]
        .as_object_mut()
        .unwrap();
    cfg.remove("triggers");
    cfg.remove("token_profile_ids");
    let s: Settings = serde_json::from_value(wire).unwrap();
    let config = &serde_json::to_value(s).unwrap()["notification_connectors"][0]["config"];
    assert_eq!(config["triggers"], json!([]));
    assert_eq!(config["token_profile_ids"], json!([]));
    let mut wire = filter_settings("future");
    wire["notification_connectors"][0]["config"] =
        json!({"nested":{"option":7},"triggers":null,"token_profile_ids":17});
    let s: Settings = serde_json::from_value(wire.clone()).unwrap();
    s.validate().unwrap();
    assert_eq!(
        serde_json::to_value(s).unwrap()["notification_connectors"],
        wire["notification_connectors"]
    );
}

#[test]
fn release_filters_distinguish_legacy_telegram_from_explicit_selection() {
    for explicit in [false, true] {
        let mut wire = serde_json::to_value(Settings::default()).unwrap();
        wire["notification_connectors"] = json!([{
            "id":"team","label":"Team","enabled":false,"events":["run_failed"],
            "type":"telegram","config":{"chat_id":"-1001","has_token":false}
        }]);
        if explicit {
            wire["notification_connectors"][0]["config"]["triggers"] = json!(["new_issue"]);
        }
        let s: Settings = serde_json::from_value(wire).unwrap();
        s.validate().unwrap();
        let stored = serde_json::to_value(s).unwrap();
        let triggers = stored["notification_connectors"][0]["config"]["triggers"]
            .as_array()
            .unwrap();
        assert_eq!(triggers.contains(&json!("release")), !explicit);
        if explicit {
            assert_eq!(triggers, &vec![json!("new_issue")]);
        }
    }
}

#[test]
fn release_notification_filters_validate_both_providers_without_expanding_explicit_lists() {
    for kind in ["telegram", "google_chat"] {
        for (triggers, valid) in [
            (json!(["release"]), true),
            (json!(["release", "release"]), false),
            (json!(["unknown"]), false),
            (json!([]), true),
        ] {
            let mut wire = filter_settings(kind);
            wire["notification_connectors"][0]["enabled"] = json!(false);
            wire["notification_connectors"][0]["config"]["triggers"] = triggers.clone();
            let s: Settings = serde_json::from_value(wire).unwrap();
            assert_eq!(s.validate().is_ok(), valid, "{kind}: {triggers}");
            assert_eq!(
                serde_json::to_value(s).unwrap()["notification_connectors"][0]["config"]
                    ["triggers"],
                triggers
            );
        }
    }
}
