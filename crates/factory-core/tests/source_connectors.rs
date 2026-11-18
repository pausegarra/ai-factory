use factory_core::*;
use serde_json::json;

#[test]
fn old_profiles_and_global_scopes_do_not_create_sources() {
    let raw = json!({
        "owners": ["legacy-org"], "authors": ["alice"], "reviewers": ["alice"],
        "token_profiles": [{"id":"old", "label":"Old", "account":"alice",
            "token":"old-fixture-secret", "has_token":true}],
        "codex_bindings": {}, "notification_connectors": []
    });
    let settings: Settings = serde_json::from_value(raw).unwrap();
    assert!(settings.source_connectors.is_empty());
    let encoded = serde_json::to_value(settings).unwrap();
    assert_eq!(encoded["source_connectors"], json!([]));
    for key in ["token_profiles", "owners", "authors", "reviewers"] {
        assert!(encoded.get(key).is_none(), "retained old field: {key}");
    }
}

fn source(id: &str) -> SourceConnector {
    SourceConnector {
        id: id.into(),
        label: id.into(),
        account: "alice".into(),
        ..Default::default()
    }
}
#[test]
fn sources_allow_shared_accounts_and_deny_invalid_metadata() {
    let s = Settings {
        source_connectors: vec![source("a"), source("b")],
        ..Default::default()
    };
    s.validate().unwrap();
    for patch in ["duplicate", "unsafe", "label", "account", "scope"] {
        let mut invalid = s.clone();
        match patch {
            "duplicate" => invalid.source_connectors[1].id = "a".into(),
            "unsafe" => invalid.source_connectors[1].id = "../b".into(),
            "label" => invalid.source_connectors[1].label = " ".into(),
            "account" => invalid.source_connectors[1].account = "".into(),
            _ => invalid.source_connectors[1].owners = vec!["team/repo".into()],
        }
        assert!(invalid.validate().is_err(), "{patch}");
    }
}
#[test]
fn new_sources_do_not_inherit_legacy_scopes_and_require_a_supported_type() {
    let raw = json!({"owners":["legacy"],"authors":["alice"],"reviewers":["alice"],
        "source_connectors":[{"id":"a","label":"Work","type":"github_issues","account":"alice"}]});
    let s: Settings = serde_json::from_value(raw.clone()).unwrap();
    assert!(s.source_connectors[0].owners.is_empty());
    assert!(!s.source_connectors[0].allows_actor("new_issue"));
    for field in ["owners", "authors", "reviewers"] {
        let mut invalid = raw.clone();
        invalid["source_connectors"][0][field] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Settings>(invalid).is_err());
    }
    for kind in [None, Some("future")] {
        let mut invalid = raw.clone();
        if let Some(kind) = kind {
            invalid["source_connectors"][0]["type"] = json!(kind);
        } else {
            invalid["source_connectors"][0]
                .as_object_mut()
                .unwrap()
                .remove("type");
        }
        assert!(serde_json::from_value::<Settings>(invalid).is_err());
    }
}

#[test]
fn review_scope_json_defaults_false_and_round_trips_both_values() {
    let base = json!({
        "id": "work", "label": "Work", "type": "github_issues",
        "account": "alice", "owners": ["acme"], "reviewers": ["alice"]
    });
    for value in [None, Some(false), Some(true)] {
        let mut raw = base.clone();
        if let Some(value) = value {
            raw["review_all_open_pull_requests"] = json!(value);
        }
        let settings: Settings =
            serde_json::from_value(json!({"source_connectors": [raw]})).unwrap();
        settings.validate().unwrap();
        let encoded = serde_json::to_value(&settings).unwrap();
        assert_eq!(
            encoded["source_connectors"][0]["review_all_open_pull_requests"],
            json!(value.unwrap_or(false))
        );
        let decoded: Settings = serde_json::from_value(encoded).unwrap();
        assert_eq!(
            serde_json::to_value(decoded).unwrap()["source_connectors"][0]
                ["review_all_open_pull_requests"],
            json!(value.unwrap_or(false))
        );
    }
}

#[test]
fn review_scope_rejects_explicit_non_boolean_json() {
    for value in [json!(null), json!("true"), json!(1)] {
        let raw = json!({"source_connectors": [{
            "id": "work", "label": "Work", "type": "github_issues",
            "account": "alice", "review_all_open_pull_requests": value
        }]});
        assert!(serde_json::from_value::<Settings>(raw).is_err());
    }
}

#[test]
fn pull_request_authors_default_and_round_trip_independently() {
    let legacy = json!({"id":"a","label":"A","type":"github_issues",
        "account":"alice","authors":["alice"],"reviewers":["alice"]});
    let old: SourceConnector = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(old).unwrap()["pull_request_authors"],
        json!([])
    );
    let mut first = legacy;
    first["pull_request_authors"] = json!(["first", "second"]);
    let settings: Settings = serde_json::from_value(json!({
        "source_connectors":[first, serde_json::to_value(source("b")).unwrap()]
    }))
    .unwrap();
    settings.validate().unwrap();
    let encoded = serde_json::to_value(&settings).unwrap();
    assert_eq!(
        encoded["source_connectors"][0]["pull_request_authors"],
        json!(["first", "second"])
    );
    assert_eq!(
        encoded["source_connectors"][1]["pull_request_authors"],
        json!([])
    );
    assert_eq!(encoded["source_connectors"][0]["authors"], json!(["alice"]));
    let decoded: Settings = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
}

#[test]
fn pull_request_authors_reject_invalid_shapes_and_logins() {
    for value in [json!(null), json!("first"), json!(1), json!({}), json!([1])] {
        let mut raw = serde_json::to_value(source("a")).unwrap();
        raw["pull_request_authors"] = value;
        assert!(serde_json::from_value::<SourceConnector>(raw).is_err());
    }
    for value in ["team/repo", "", "with space"] {
        let mut raw = serde_json::to_value(source("a")).unwrap();
        raw["pull_request_authors"] = json!([value]);
        let invalid: SourceConnector = serde_json::from_value(raw).unwrap();
        assert!(invalid.validate().is_err());
    }
}

#[test]
fn pull_request_authors_predicate_respects_mode_empty_and_login() {
    let mut s = source("a");
    s.review_all_open_pull_requests = true;
    assert!(s.allows_pull_request_author(None));
    assert!(s.allows_pull_request_author(Some("outside")));
    s.pull_request_authors = vec!["first".into(), "SECOND".into(), "build[bot]".into()];
    for login in ["First", "second", "BUILD[bot]"] {
        assert!(s.allows_pull_request_author(Some(login)));
    }
    for login in [
        None,
        Some(""),
        Some("outside"),
        Some("with space"),
        Some("x/y"),
    ] {
        assert!(!s.allows_pull_request_author(login));
    }
    s.review_all_open_pull_requests = false;
    assert!(s.allows_pull_request_author(Some("outside")));
    assert!(s.allows_pull_request_author(None));
    assert_eq!(s.pull_request_authors, ["first", "SECOND", "build[bot]"]);
}
