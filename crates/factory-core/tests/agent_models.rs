#![cfg(unix)]
mod common;
use factory_core::*;
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn fixture(script: &str) -> (tempfile::TempDir, Arc<Factory>) {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("agent");
    std::fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let factory = common::open(dir.path().join("data")).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.commands.codex = executable.to_string_lossy().into();
    settings.commands.opencode = settings.commands.codex.clone();
    factory.save_settings(settings).unwrap();
    (dir, factory)
}

#[tokio::test]
async fn codex_catalog_pages_keep_each_models_supported_efforts() {
    let script = r#"
while IFS= read -r request; do
 case "$request" in
 *'"method":"initialize"'*) echo '{"id":1,"result":{}}';;
 *'"method":"model/list"'*)
  case "$request" in
   *'"cursor":"page2"'*) echo '{"id":3,"result":{"data":[{"model":"codex-small","displayName":"Small","supportedReasoningEfforts":[{"reasoningEffort":"low"}]}],"nextCursor":null}}';;
   *) echo '{"id":2,"result":{"data":[{"model":"codex-large","displayName":"Large","supportedReasoningEfforts":[{"reasoningEffort":"high"},{"reasoningEffort":"ultra"}]}],"nextCursor":"page2"}}';;
  esac;;
 esac
done
"#;
    let (_dir, factory) = fixture(script);
    let models = factory.agent_models("codex").await.unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "codex-large");
    assert_eq!(models[0].efforts, ["high", "ultra"]);
    assert_eq!(models[1].efforts, ["low"]);
}

#[tokio::test]
async fn opencode_catalog_uses_provider_ids_and_configured_variants_and_stops_server() {
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", server.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = server.accept().await.unwrap();
        let mut bytes = [0; 8192];
        let count = socket.read(&mut bytes).await.unwrap();
        let request = String::from_utf8_lossy(&bytes[..count]).to_lowercase();
        assert!(request.starts_with("get /api/model "));
        assert!(request.contains("authorization: basic "));
        let body = json!({"location":{},"data":[
            {"id":"model","providerID":"work","name":"Work model","enabled":true,"variants":[{"id":"fast"},{"id":"deep"}]},
            {"id":"disabled","providerID":"work","enabled":false,"variants":[]},
            {"id":"image","providerID":"work","type":"image","enabled":true,"variants":[]}
        ]}).to_string();
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let (dir, factory) = fixture(&format!(
        "echo $$ > cli.pid\necho 'server listening on {url}'\nexec sleep 30"
    ));
    let models = factory.agent_models("opencode").await.unwrap();
    task.await.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "work/model");
    assert_eq!(models[0].efforts, ["fast", "deep"]);
    let pid: i32 = std::fs::read_to_string(dir.path().join("data/cli.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "catalog server must be reaped"
    );
}

#[tokio::test]
async fn catalog_errors_never_return_server_output_or_credentials() {
    let (_dir, factory) = fixture("echo 'secret-example'\nexit 3");
    assert!(factory
        .agent_models("unknown")
        .await
        .unwrap_err()
        .to_string()
        .contains("Unsupported agent"));
    let error = factory.agent_models("codex").await.unwrap_err().to_string();
    assert!(!error.contains("secret-example"));
}

#[tokio::test]
#[ignore = "Requires installed and authenticated Codex and OpenCode CLIs"]
async fn installed_agent_catalogs() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().join("data")).unwrap();
    for agent in ["codex", "opencode"] {
        let models = factory.agent_models(agent).await.unwrap();
        assert!(!models.is_empty());
        assert!(models
            .iter()
            .all(|m| !m.id.is_empty() && !m.name.is_empty()));
        println!(
            "{agent}: {} models, {} with effort options",
            models.len(),
            models.iter().filter(|m| !m.efforts.is_empty()).count()
        );
    }
}

#[tokio::test]
async fn chunked_catalog_is_rejected_at_limit_without_waiting_for_eof() {
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", server.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = server.accept().await.unwrap();
        let mut request = [0; 8192];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await
            .unwrap();
        let chunk = vec![b' '; 1024 * 1024];
        for _ in 0..17 {
            if socket.write_all(b"100000\r\n").await.is_err()
                || socket.write_all(&chunk).await.is_err()
                || socket.write_all(b"\r\n").await.is_err()
            {
                return;
            }
        }
        // No terminating chunk: a bounded reader must fail before EOF.
        std::future::pending::<()>().await;
    });
    let (_dir, factory) = fixture(&format!("echo 'server listening on {url}'\nexec sleep 30"));
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        factory.agent_models("opencode"),
    )
    .await;
    task.abort();
    let error = response
        .expect("catalog reader waited for EOF after exceeding its limit")
        .unwrap_err();
    assert!(error.to_string().contains("catalog too large"), "{error}");
}
