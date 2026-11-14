#![cfg(unix)]
mod common;
use factory_core::*;
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, sync::Arc};

fn fixture(script: &str) -> (tempfile::TempDir, Arc<Factory>) {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("agent");
    std::fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let factory = common::open(dir.path().join("data")).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.commands.codex = executable.to_string_lossy().into_owned();
    settings.commands.opencode = executable.to_string_lossy().into_owned();
    factory.save_settings(settings).unwrap();
    (dir, factory)
}
const RPC: &str = r#"while IFS= read -r request; do
  case "$request" in
    *'"method":"initialize"'*) echo '{"id":1,"result":{}}';;
    *'"method":"account/rateLimits/read"'*) cat quota-response.json;;
  esac
done"#;
#[tokio::test]
async fn usage_codex_remaining_contract_matches_wire() {
    let fixture_data: Value = serde_json::from_str(include_str!(
        "fixtures/agent_usage/codex-remaining-contract.json"
    ))
    .unwrap();
    for case in fixture_data["cases"].as_array().unwrap() {
        let (dir, factory) = fixture(RPC);
        common::seed_codex_account(&factory, "9ab14479-4d99-41c7-b347-e5b3acffb316");
        std::fs::write(
            dir.path().join("data/quota-response.json"),
            format!("{}\n", case["rpc_response"]),
        )
        .unwrap();
        let actual = factory.agent_usage("codex").await.unwrap();
        let mut wire = serde_json::to_value(actual).unwrap();
        let expected: AgentUsage = serde_json::from_value(case["wire"].clone()).unwrap();
        let expected = serde_json::to_value(expected).unwrap();
        for date in [&wire["checked_at"], &wire["entries"][0]["checked_at"]] {
            assert!(chrono::DateTime::parse_from_rfc3339(date.as_str().unwrap()).is_ok());
        }
        wire["checked_at"] = expected["checked_at"].clone();
        wire["entries"][0]["checked_at"] = expected["entries"][0]["checked_at"].clone();
        assert_eq!(wire, expected, "{}", case["name"]);
        assert!(!wire.to_string().contains("fixture-secret"));
        factory.shutdown().await;
    }
}
#[tokio::test]
async fn usage_accounts_include_unbound_pending_failed_and_isolate_homes() {
    let (dir, factory) = fixture(
        r#"printf '%s\n' "$CODEX_HOME" >> queried-homes
while IFS= read -r request; do
  case "$request" in
    *'"method":"initialize"'*) echo '{"id":1,"result":{}}';;
    *'"method":"account/rateLimits/read"'*)
      case "$CODEX_HOME" in
        *316) echo '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":27}}}}';;
        *) echo '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":100}}}}';;
      esac;;
  esac
done"#,
    );
    let ids = [
        "9ab14479-4d99-41c7-b347-e5b3acffb316",
        "9ab14479-4d99-41c7-b347-e5b3acffb317",
        "9ab14479-4d99-41c7-b347-e5b3acffb318",
        "9ab14479-4d99-41c7-b347-e5b3acffb319",
    ];
    for id in ids {
        common::seed_codex_account(&factory, id);
    }
    let db = rusqlite::Connection::open(dir.path().join("data/factory.sqlite3")).unwrap();
    for (id, status) in [(ids[2], "pending"), (ids[3], "failed")] {
        db.execute("UPDATE agent_accounts SET json=?2 WHERE id=?1",rusqlite::params![id,json!({"id":id,"agent":"codex","label":status,"email":null,"status":status,"error":"fixture-secret"}).to_string()]).unwrap();
    }
    let mut settings = factory.settings().unwrap();
    settings.token_profiles.push(TokenProfile {
        id: "fixture-profile".into(),
        label: "Fixture profile".into(),
        account: "fixture-user".into(),
        owners: vec![],
        authors: vec![],
        reviewers: vec![],
        token: None,
        has_token: false,
    });
    settings
        .codex_bindings
        .insert("fixture-profile".into(), ids[0].into());
    factory.save_settings(settings).unwrap();
    let actual = factory.agent_usage("codex").await.unwrap();
    assert_eq!(
        actual.entries.iter().map(|e| &e.id).collect::<Vec<_>>(),
        factory
            .agent_accounts()
            .unwrap()
            .iter()
            .map(|a| &a.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(actual.status, UsageStatus::Available);
    assert_eq!(actual.entries.len(), 4);
    for (id, expected) in [(ids[0], 73.0), (ids[1], 0.0)] {
        let row = actual.entries.iter().find(|row| row.id == id).unwrap();
        assert_eq!(row.observation.metrics[0].value, expected);
    }
    for (id, status, code) in [
        (ids[2], UsageStatus::Pending, UsageCode::SignInPending),
        (ids[3], UsageStatus::Failed, UsageCode::SignInFailed),
    ] {
        let row = actual.entries.iter().find(|row| row.id == id).unwrap();
        assert_eq!(row.observation.status, status);
        assert_eq!(row.observation.code, Some(code));
        assert!(row.observation.metrics.is_empty());
    }
    let homes = std::fs::read_to_string(dir.path().join("data/queried-homes")).unwrap();
    assert_eq!(homes.lines().count(), 2);
    assert!(homes.contains(ids[0]) && homes.contains(ids[1]));
    assert!(!serde_json::to_string(&actual)
        .unwrap()
        .contains("fixture-secret"));
    factory.shutdown().await;
}
#[tokio::test]
async fn usage_accounts_no_global_fallback_and_bad_home_is_partial() {
    let (dir, factory) = fixture(RPC);
    assert_eq!(
        factory.agent_usage("codex").await.unwrap().status,
        UsageStatus::NoAccounts
    );
    let id = "9ab14479-4d99-41c7-b347-e5b3acffb316";
    common::seed_codex_account(&factory, id);
    std::fs::remove_dir_all(dir.path().join("data/agent-accounts/codex").join(id)).unwrap();
    let second = "9ab14479-4d99-41c7-b347-e5b3acffb317";
    common::seed_codex_account(&factory, second);
    std::fs::write(
        dir.path().join("data/quota-response.json"),
        "{\"id\":2,\"result\":{\"rateLimits\":{\"primary\":{\"usedPercent\":27}}}}\n",
    )
    .unwrap();
    let actual = factory.agent_usage("codex").await.unwrap();
    assert_eq!(actual.entries.len(), 2);
    assert_eq!(
        actual
            .entries
            .iter()
            .find(|e| e.id == id)
            .unwrap()
            .observation
            .code,
        Some(UsageCode::HomeUnavailable)
    );
    assert_eq!(
        actual
            .entries
            .iter()
            .find(|e| e.id == second)
            .unwrap()
            .observation
            .metrics[0]
            .value,
        73.0
    );
    assert!(factory.agent_usage("unknown").await.is_err());
    factory.shutdown().await;
    assert_eq!(
        factory.agent_usage("codex").await.unwrap().code,
        Some(UsageCode::Stopping)
    );
}
#[tokio::test]
async fn usage_opencode_console_absence_and_supported_reported_quota() {
    let (dir, factory) = fixture(
        r#"if [ "$*" = 'api --help' ]; then echo 'api --standalone'; exit 0; fi
[ "$*" = 'api experimental.usage.limits --standalone --log-level none' ] || exit 81
cat quota-response.json"#,
    );
    for (response, status, value) in [
        (
            json!({"location":{},"data":null}),
            UsageStatus::Unauthenticated,
            None,
        ),
        (
            json!({"data":{"go":{"rolling":{"percent":37,"limited":false,"resetsAt":1800000000000i64}}}}),
            UsageStatus::Available,
            Some(37.0),
        ),
    ] {
        std::fs::write(
            dir.path().join("data/quota-response.json"),
            response.to_string(),
        )
        .unwrap();
        let actual = factory.agent_usage("opencode").await.unwrap();
        assert_eq!(actual.entries[0].label, "OpenCode Console");
        assert_eq!(actual.entries[0].observation.status, status);
        if let Some(value) = value {
            assert_eq!(actual.entries[0].observation.metrics[0].value, value);
            assert_eq!(
                actual.entries[0].observation.metrics[0].kind,
                UsageMetricKind::Reported
            );
        }
    }
    factory.shutdown().await;
}
#[tokio::test]
async fn usage_accounts_shutdown_drains_process_tree_before_releasing_lease() {
    use std::time::Duration;
    let (dir, factory) = fixture(
        r#"echo $$ > parent.pid
sleep 30 &
echo $! > descendant.pid
while IFS= read -r request; do
  case "$request" in *'"method":"initialize"'*) echo '{"id":1,"result":{}}';; esac
done"#,
    );
    let id = "9ab14479-4d99-41c7-b347-e5b3acffb316";
    common::seed_codex_account(&factory, id);
    let task = {
        let factory = factory.clone();
        tokio::spawn(async move { factory.agent_usage("codex").await.unwrap() })
    };
    let started = tokio::time::timeout(Duration::from_secs(5), async {
        while !dir.path().join("data/descendant.pid").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if started.is_ok() {
        assert!(factory.remove_agent_account(id).await.is_err());
    }
    factory.shutdown().await;
    let actual = task.await.unwrap();
    started.unwrap();
    assert_eq!(
        actual.entries[0].observation.code,
        Some(UsageCode::Stopping)
    );
    for name in ["parent.pid", "descendant.pid"] {
        let pid: i32 = std::fs::read_to_string(dir.path().join("data").join(name))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        for _ in 0..100 {
            if unsafe { libc::kill(pid, 0) } != 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_ne!(
            unsafe { libc::kill(pid, 0) },
            0,
            "fixture process survived shutdown"
        );
    }
    factory.remove_agent_account(id).await.unwrap();
}
