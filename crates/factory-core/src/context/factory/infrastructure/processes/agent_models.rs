//! Read-only discovery through each installed agent's own model catalog.
use super::agents::{resolve_command, ProcessTree};
use crate::{AgentModel, Result};
use anyhow::{bail, ensure, Context};
use serde_json::{json, Value};
use std::{collections::HashSet, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

const MAX_BYTES: u64 = 16 * 1024 * 1024;

pub async fn discover(agent: &str, executable: &str, directory: &Path) -> Result<Vec<AgentModel>> {
    discover_with_home(agent, executable, directory, None).await
}

pub(crate) async fn discover_with_home(
    agent: &str,
    executable: &str,
    directory: &Path,
    home: Option<&Path>,
) -> Result<Vec<AgentModel>> {
    ensure!(["codex", "opencode"].contains(&agent), "Unsupported agent");
    let password = uuid::Uuid::new_v4().to_string();
    let mut command = Command::new(resolve_command(executable));
    if agent == "codex" {
        command.args(["app-server", "--stdio"]);
        if let Some(home) = home {
            super::agents::configure_codex_home(&mut command, home);
            command.args(["--config", "cli_auth_credentials_store=\"file\""]);
        }
    } else {
        command
            .args(["serve", "--hostname", "127.0.0.1", "--port", "0"])
            .env("OPENCODE_PASSWORD", &password)
            .env("OPENCODE_SERVER_PASSWORD", &password);
    }
    command
        .current_dir(directory)
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
        .with_context(|| format!("Cannot start {agent}; check the CLI path in Settings"))?;
    let mut tree = ProcessTree::attach(&mut child)?;
    let mut input = child.stdin.take().context("CLI input unavailable")?;
    let output = child.stdout.take().context("CLI output unavailable")?;
    let mut lines = BufReader::new(output.take(MAX_BYTES)).lines();
    let result = tokio::time::timeout(Duration::from_secs(25), async {
        if agent == "codex" {
            input.write_all(b"{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"ai_factory\",\"version\":\"0.2.0\"}}}\n").await?;
            rpc_response(&mut lines, 1).await?;
            input.write_all(b"{\"method\":\"initialized\"}\n").await?;
            let mut models = Vec::new();
            let mut cursor = Value::Null;
            let mut cursors = HashSet::new();
            for id in 2..=33 {
                let request = json!({"id":id,"method":"model/list","params":{"includeHidden":false,"limit":100,"cursor":cursor}});
                input.write_all(format!("{request}\n").as_bytes()).await?;
                let page = rpc_response(&mut lines, id).await?;
                models.extend(parse_codex(&page)?);
                cursor = page["nextCursor"].clone();
                if cursor.is_null() { return Ok(models); }
                let next = cursor.as_str().context("Invalid Codex model cursor")?;
                ensure!(cursors.insert(next.to_owned()), "Codex repeated a model page");
            }
            bail!("Codex model pagination limit reached")
        } else {
            for _ in 0..100 {
                let line = lines.next_line().await?.context("OpenCode stopped before model discovery; check CLI setup")?;
                let Some(address) = line.strip_prefix("server listening on ") else { continue };
                let mut url = reqwest::Url::parse(address.trim()).context("Invalid OpenCode server address")?;
                ensure!(url.scheme() == "http" && url.host_str() == Some("127.0.0.1") && url.port().is_some(), "OpenCode model server must use loopback");
                url.set_path("/api/model");
                let mut response = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20)).build()?
                    .get(url).basic_auth("opencode", Some(&password)).send().await?;
                ensure!(response.status().is_success(), "OpenCode model discovery returned HTTP {}", response.status().as_u16());
                ensure!(response.content_length().is_none_or(|size| size <= MAX_BYTES), "OpenCode catalog too large");
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await? {
                    ensure!((bytes.len() as u64).saturating_add(chunk.len() as u64) <= MAX_BYTES, "OpenCode catalog too large");
                    bytes.extend_from_slice(&chunk);
                }
                return parse_opencode(&serde_json::from_slice(&bytes).context("Invalid OpenCode model catalog")?);
            }
            bail!("OpenCode did not announce its model server")
        }
    }).await;
    tree.terminate();
    let _ = child.kill().await;
    let mut models =
        result.context("Model discovery timed out; check CLI setup and authentication")??;
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    Ok(models)
}

async fn rpc_response<R: tokio::io::AsyncBufRead + Unpin>(
    lines: &mut tokio::io::Lines<R>,
    id: u64,
) -> Result<Value> {
    for _ in 0..512 {
        let line = lines
            .next_line()
            .await?
            .context("Codex stopped before model discovery; check CLI setup")?;
        let message: Value =
            serde_json::from_str(&line).context("Invalid Codex catalog protocol")?;
        if message["id"] != id {
            continue;
        }
        if let Some(error) = message.get("error") {
            bail!(
                "Codex rejected model discovery (code {}); check CLI authentication",
                error["code"].as_i64().unwrap_or(0)
            );
        }
        return message
            .get("result")
            .cloned()
            .context("Codex model response missing result");
    }
    bail!("Codex model response limit reached")
}

fn parse_codex(page: &Value) -> Result<Vec<AgentModel>> {
    page["data"]
        .as_array()
        .context("Invalid Codex model list")?
        .iter()
        .filter(|m| m["hidden"] != true)
        .map(|model| {
            let id = model["model"].as_str().context("Codex model ID missing")?;
            Ok(AgentModel {
                id: id.into(),
                name: model["displayName"].as_str().unwrap_or(id).into(),
                efforts: model["supportedReasoningEfforts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e["reasoningEffort"].as_str().map(String::from))
                    .collect(),
            })
        })
        .collect()
}
fn parse_opencode(value: &Value) -> Result<Vec<AgentModel>> {
    value
        .get("data")
        .unwrap_or(value)
        .as_array()
        .context("Invalid OpenCode model list")?
        .iter()
        .filter(|m| m["enabled"] != false && m["type"].as_str().is_none_or(|kind| kind == "chat"))
        .map(|model| {
            let provider = model["providerID"]
                .as_str()
                .context("OpenCode provider ID missing")?;
            let model_id = model["id"].as_str().context("OpenCode model ID missing")?;
            Ok(AgentModel {
                id: format!("{provider}/{model_id}"),
                name: model["name"].as_str().unwrap_or(model_id).into(),
                efforts: model["variants"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v["id"].as_str().map(String::from))
                    .collect(),
            })
        })
        .collect()
}
