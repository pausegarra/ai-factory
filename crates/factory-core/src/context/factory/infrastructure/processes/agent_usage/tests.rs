use super::*;
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, path::Path};

fn fixture(script: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent");
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}
async fn query(
    dir: &Path,
    agent: &str,
    cancel: CancellationToken,
    duration: Duration,
) -> UsageObservation {
    let canonical = dir.canonicalize().unwrap();
    let dir = canonical.as_path();
    let executable = dir.join("agent");
    read_with_timeout(
        UsageQuery {
            agent,
            executable: executable.to_str().unwrap(),
            directory: dir,
            codex_home: Some(dir),
        },
        cancel,
        duration,
    )
    .await
}
fn rpc_script(response: &str) -> String {
    format!(
        r#"[ "$*" = 'app-server --stdio --config cli_auth_credentials_store="file"' ] || exit 81
[ "$CODEX_HOME" = "$PWD" ] || exit 82
[ -z "${{OPENAI_API_KEY+x}}${{CODEX_API_KEY+x}}${{CODEX_ACCESS_TOKEN+x}}" ] || exit 83
IFS= read -r first
[ "$first" = '{{"id":1,"method":"initialize","params":{{"clientInfo":{{"name":"ai_factory","version":"0.2.0"}}}}}}' ] || exit 84
printf '%s\n' '{{"id":1,"result":{{}}}}'
IFS= read -r initialized
[ "$initialized" = '{{"method":"initialized"}}' ] || exit 85
IFS= read -r request
[ "$request" = '{{"id":2,"method":"account/rateLimits/read"}}' ] || exit 86
{response}
while IFS= read -r ignored; do :; done"#
    )
}
#[tokio::test]
async fn usage_codex_transport_correlates_rpc_and_isolates_home_environment() {
    let dir=fixture(&rpc_script("echo '{\"method\":\"notification\",\"params\":{\"secret\":\"fixture-secret\"}}'\necho '{\"id\":99,\"result\":{}}'\necho '{\"id\":2,\"result\":{\"rateLimits\":{\"primary\":{\"usedPercent\":27}}}}'"));
    let result = query(
        dir.path(),
        "codex",
        CancellationToken::new(),
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(result.status, UsageStatus::Available);
    assert_eq!(result.metrics[0].value, 73.0);
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("fixture-secret"));
}
#[tokio::test]
async fn usage_codex_transport_rejects_malformed_eof_oversize_and_message_flood() {
    for response in [
        "echo malformed",
        "exit 0",
        "dd if=/dev/zero bs=1048577 count=1 2>/dev/null",
        "i=0; while [ $i -lt 513 ]; do echo '{\"id\":99,\"result\":{}}'; i=$((i+1)); done",
    ] {
        let dir = fixture(&rpc_script(response));
        let result = query(
            dir.path(),
            "codex",
            CancellationToken::new(),
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(result.code, Some(UsageCode::Protocol));
        assert!(result.metrics.is_empty());
    }
}
#[tokio::test]
async fn usage_codex_transport_rpc_errors_are_closed_codes() {
    for (code, expected, status) in [
        (-32601, UsageCode::Unsupported, UsageStatus::Unavailable),
        (-32000, UsageCode::QueryFailed, UsageStatus::Error),
    ] {
        let dir = fixture(&rpc_script(&format!(
            "echo '{}'",
            json!({"id":2,"error":{"code":code,"message":"fixture-secret","data":"fixture-secret"}})
        )));
        let result = query(
            dir.path(),
            "codex",
            CancellationToken::new(),
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(result.code, Some(expected));
        assert_eq!(result.status, status);
        assert!(result.metrics.is_empty());
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("fixture-secret"));
    }
}
#[tokio::test]
async fn usage_codex_transport_timeout_cancel_and_missing_cli() {
    let dir = fixture("echo $$ > parent.pid\nsleep 30 &\necho $! > descendant.pid\nwait");
    let result = query(
        dir.path(),
        "codex",
        CancellationToken::new(),
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(result.code, Some(UsageCode::TimedOut));
    assert_dead(dir.path().join("parent.pid")).await;
    assert_dead(dir.path().join("descendant.pid")).await;
    let cancel = CancellationToken::new();
    let future = query(dir.path(), "codex", cancel.clone(), Duration::from_secs(2));
    let (result, ()) = tokio::join!(future, async {
        tokio::time::sleep(Duration::from_millis(120)).await;
        cancel.cancel();
    });
    assert_eq!(result.code, Some(UsageCode::Stopping));
    assert_dead(dir.path().join("parent.pid")).await;
    assert_dead(dir.path().join("descendant.pid")).await;
    std::fs::remove_file(dir.path().join("agent")).unwrap();
    assert_eq!(
        query(
            dir.path(),
            "codex",
            CancellationToken::new(),
            Duration::from_secs(2)
        )
        .await
        .code,
        Some(UsageCode::CliMissing)
    );
}
async fn assert_dead(path: std::path::PathBuf) {
    let pid: i32 = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .trim()
        .parse()
        .unwrap();
    for _ in 0..100 {
        if unsafe { libc::kill(pid, 0) } != 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("fixture child {pid} survived cleanup");
}
fn oc_script(response: &str) -> String {
    format!(
        r#"printf '%s\n' "$*" >> args.log
if [ "$*" = 'api --help' ]; then echo 'api --standalone'; exit 0; fi
[ "$*" = 'api experimental.usage.limits --standalone --log-level none' ] || exit 87
{response}"#
    )
}
#[tokio::test]
async fn usage_opencode_transport_uses_private_api_and_no_stats() {
    for (response, status) in [
        (
            "echo '{\"location\":{},\"data\":null}'",
            UsageStatus::Unauthenticated,
        ),
        (
            "echo '{\"data\":{\"go\":{\"rolling\":{\"percent\":37}}}}'",
            UsageStatus::Available,
        ),
        ("echo '{\"tokens\":500,\"cost\":8}'", UsageStatus::Error),
    ] {
        let dir = fixture(&oc_script(response));
        let result = query(
            dir.path(),
            "opencode",
            CancellationToken::new(),
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(result.status, status);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("args.log")).unwrap(),
            "api --help\napi experimental.usage.limits --standalone --log-level none\n"
        );
    }
}
#[tokio::test]
async fn usage_opencode_transport_unsupported_and_failure_do_not_claim_no_account() {
    let dir = fixture("echo 'old CLI'; exit 0");
    assert_eq!(
        query(
            dir.path(),
            "opencode",
            CancellationToken::new(),
            Duration::from_secs(2)
        )
        .await
        .code,
        Some(UsageCode::Unsupported)
    );
    for response in ["echo '{\"error\":{\"name\":\"LocationNotFoundError\",\"message\":\"fixture-secret\"}}'; exit 1", "echo fixture-secret >&2; exit 1", "echo '{\"error\":{\"name\":\"UnauthorizedError\"}}'; exit 1"] {
        let dir=fixture(&oc_script(response));
        let result=query(dir.path(),"opencode",CancellationToken::new(),Duration::from_secs(2)).await;
        assert_eq!(result.status,UsageStatus::Error); assert_eq!(result.code,Some(UsageCode::QueryFailed));
        assert!(!serde_json::to_string(&result).unwrap().contains("fixture-secret"));
    }
}
