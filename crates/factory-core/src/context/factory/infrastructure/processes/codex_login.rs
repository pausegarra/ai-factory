//! Bounded, private app-server transport for managed ChatGPT OAuth.
use super::agents::{clean_env, resolve_command, ProcessTree};
use crate::Result;
use anyhow::{ensure, Context};
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout, Command},
    sync::oneshot,
};
use tokio_util::sync::CancellationToken;

type Lines = tokio::io::Lines<BufReader<tokio::io::Take<ChildStdout>>>;
struct Rpc {
    input: ChildStdin,
    lines: Lines,
    notifications: Vec<Value>,
    messages: usize,
}
impl Rpc {
    async fn send(&mut self, value: Value) -> Result<()> {
        self.input
            .write_all(format!("{value}\n").as_bytes())
            .await
            .context("Cannot contact Codex login process")?;
        Ok(())
    }
    async fn next(&mut self) -> Result<Value> {
        self.messages += 1;
        ensure!(self.messages <= 1024, "Codex login protocol limit reached");
        let line = self
            .lines
            .next_line()
            .await
            .map_err(|_| anyhow::anyhow!("Invalid Codex login output"))?
            .context("Codex login process stopped")?;
        ensure!(line.len() <= 64 * 1024, "Codex login response too large");
        serde_json::from_str(&line).map_err(|_| anyhow::anyhow!("Invalid Codex login protocol"))
    }
    async fn response(&mut self, id: u64) -> Result<Value> {
        loop {
            let value = self.next().await?;
            if value["id"] == id {
                ensure!(
                    value.get("error").is_none(),
                    "Codex rejected the login request"
                );
                return value
                    .get("result")
                    .cloned()
                    .context("Codex login response missing result");
            }
            if value["method"] == "account/login/completed" {
                self.notifications.push(value);
            }
        }
    }
    async fn completed(&mut self, login_id: &str) -> Result<()> {
        loop {
            let value = if self.notifications.is_empty() {
                self.next().await?
            } else {
                self.notifications.remove(0)
            };
            if value["method"] == "account/login/completed"
                && value["params"]["loginId"] == login_id
            {
                ensure!(
                    value["params"]["success"] == true,
                    "Codex login was not completed"
                );
                return Ok(());
            }
        }
    }
}

pub(crate) fn validate_url(value: &str) -> Result<String> {
    ensure!(value.len() <= 16 * 1024, "Invalid Codex login URL");
    let url = reqwest::Url::parse(value).map_err(|_| anyhow::anyhow!("Invalid Codex login URL"))?;
    ensure!(
        url.scheme() == "https"
            && [Some("auth.openai.com"), Some("chatgpt.com")].contains(&url.host_str())
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443),
        "Invalid Codex login URL"
    );
    Ok(value.into())
}

pub(crate) async fn login(
    executable: &str,
    home: &Path,
    cancel: CancellationToken,
    ready: &mut Option<oneshot::Sender<Result<String>>>,
    on_ready: impl FnOnce(&str),
) -> Result<Option<String>> {
    let mut command = Command::new(resolve_command(executable));
    clean_env(&mut command);
    command
        .args([
            "app-server",
            "--stdio",
            "-c",
            "cli_auth_credentials_store=\"file\"",
        ])
        .env("CODEX_HOME", home)
        .env_remove("CODEX_API_KEY")
        .env_remove("CODEX_ACCESS_TOKEN")
        .env_remove("OPENAI_API_KEY")
        .current_dir(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    let mut child = command
        .spawn()
        .map_err(|_| anyhow::anyhow!("Cannot start Codex; check the CLI path in Settings"))?;
    let mut tree = ProcessTree::attach(&mut child)?;
    let mut rpc = Rpc {
        input: child
            .stdin
            .take()
            .context("Codex login input unavailable")?,
        lines: BufReader::new(
            child
                .stdout
                .take()
                .context("Codex login output unavailable")?
                .take(2 * 1024 * 1024),
        )
        .lines(),
        notifications: Vec::new(),
        messages: 0,
    };
    let result = tokio::select! {
        _ = cancel.cancelled() => Err(anyhow::anyhow!("Codex login cancelled")),
        result = tokio::time::timeout(Duration::from_secs(300), async {
            let (login_id, auth_url) = tokio::time::timeout(Duration::from_secs(20), async {
                rpc.send(json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"ai_factory","title":"AI Factory","version":env!("CARGO_PKG_VERSION")}}})).await?;
                rpc.response(1).await?;
                rpc.send(json!({"method":"initialized"})).await?;
                rpc.send(json!({"id":2,"method":"account/login/start","params":{"type":"chatgpt"}})).await?;
                let response = rpc.response(2).await?;
                ensure!(response["type"] == "chatgpt", "Codex did not start ChatGPT login");
                let login_id = response["loginId"].as_str().filter(|v| !v.is_empty() && v.len() <= 256).context("Codex login ID missing")?.to_owned();
                let auth_url = validate_url(response["authUrl"].as_str().context("Codex login URL missing")?)?;
                Ok::<_,anyhow::Error>((login_id,auth_url))
            }).await.context("Codex login startup timed out")??;
            on_ready(&auth_url);
            if let Some(sender) = ready.take() { let _ = sender.send(Ok(auth_url)); }
            rpc.completed(&login_id).await?;
            rpc.send(json!({"id":3,"method":"account/read","params":{"refreshToken":false}})).await?;
            let response = rpc.response(3).await?;
            ensure!(response["account"]["type"] == "chatgpt", "Codex login did not authenticate a ChatGPT account");
            let email = match &response["account"]["email"] {
                Value::Null => None,
                Value::String(email) => {
                    ensure!(!email.is_empty() && email.len() <= 320 && !email.chars().any(char::is_control), "Invalid Codex account email");
                    Some(email.clone())
                }
                _ => anyhow::bail!("Invalid Codex account email"),
            };
            Ok(email)
        }) => match result { Ok(value) => value, Err(_) => Err(anyhow::anyhow!("Codex login timed out; connect the account again")) },
    };
    tree.terminate();
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}
