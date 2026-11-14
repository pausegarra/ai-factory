mod codex;
mod opencode;

fn reset_at(value: Option<&serde_json::Value>, millis: bool) -> crate::Result<Option<String>> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let number = value
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("Invalid quota reset"))?;
    let date = if millis {
        chrono::DateTime::from_timestamp_millis(number)
    } else {
        chrono::DateTime::from_timestamp(number, 0)
    };
    Ok(Some(
        date.ok_or_else(|| anyhow::anyhow!("Invalid quota reset"))?
            .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
    ))
}

use crate::context::factory::application::interfaces::agent_usage_source::{
    AgentUsageSource, UsageQuery,
};
use crate::*;
use std::{future::Future, pin::Pin, time::Duration};
use tokio_util::sync::CancellationToken;
pub struct CliUsageSource;
impl AgentUsageSource for CliUsageSource {
    fn read<'a>(
        &'a self,
        query: UsageQuery<'a>,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = UsageObservation> + Send + 'a>> {
        Box::pin(read_with_timeout(query, cancel, Duration::from_secs(25)))
    }
}
async fn read_with_timeout(
    query: UsageQuery<'_>,
    cancel: CancellationToken,
    duration: Duration,
) -> UsageObservation {
    let deadline = tokio::time::Instant::now() + duration;
    let result = async {
        if query.agent == "codex" {
            if query.codex_home.is_none() {
                return Err(UsageCode::HomeUnavailable);
            }
            let output = run(
                &query,
                &[
                    "app-server",
                    "--stdio",
                    "--config",
                    "cli_auth_credentials_store=\"file\"",
                ],
                true,
                deadline,
                &cancel,
            )
            .await?;
            Ok(codex::parse(&output.0))
        } else if query.agent == "opencode" {
            let (_, help, success) =
                run(&query, &["api", "--help"], false, deadline, &cancel).await?;
            if !success || !String::from_utf8_lossy(&help).contains("--standalone") {
                return Err(UsageCode::Unsupported);
            }
            let (_, bytes, success) = run(
                &query,
                &[
                    "api",
                    "experimental.usage.limits",
                    "--standalone",
                    "--log-level",
                    "none",
                ],
                false,
                deadline,
                &cancel,
            )
            .await?;
            let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
                if success {
                    UsageCode::Protocol
                } else {
                    UsageCode::QueryFailed
                }
            })?;
            if !success || value.get("error").is_some() {
                return Err(UsageCode::QueryFailed);
            }
            Ok(opencode::parse(&value))
        } else {
            Err(UsageCode::Unsupported)
        }
    }
    .await;
    match result {
        Ok(observation) => observation,
        Err(code) => {
            let status = if code == UsageCode::Unsupported {
                UsageStatus::Unavailable
            } else {
                UsageStatus::Error
            };
            UsageObservation::state(status, code)
        }
    }
}

use super::agents::{configure_codex_home, resolve_command, ProcessTree};
use serde_json::Value;
use std::process::Stdio;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
const MAX_BYTES: u64 = 1024 * 1024;

/// Every exit drains the child before releasing its account lease. The tree and
/// kill_on_drop also cover an abandoned caller future, including descendants.
async fn run(
    query: &UsageQuery<'_>,
    args: &[&str],
    rpc: bool,
    deadline: tokio::time::Instant,
    cancel: &CancellationToken,
) -> std::result::Result<(Value, Vec<u8>, bool), UsageCode> {
    if cancel.is_cancelled() {
        return Err(UsageCode::Stopping);
    }
    if tokio::time::Instant::now() >= deadline {
        return Err(UsageCode::TimedOut);
    }
    let mut command = Command::new(resolve_command(query.executable));
    command
        .args(args)
        .current_dir(query.directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if query.agent == "codex" {
        if let Some(home) = query.codex_home {
            configure_codex_home(&mut command, home);
        }
    }
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            UsageCode::CliMissing
        } else {
            UsageCode::QueryFailed
        }
    })?;
    let mut tree = match ProcessTree::attach(&mut child) {
        Ok(tree) => tree,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(UsageCode::QueryFailed);
        }
    };
    let operation = async {
        let output = child.stdout.take().ok_or(UsageCode::Protocol)?;
        if rpc {
            let mut input = child.stdin.take().ok_or(UsageCode::Protocol)?;
            let mut lines = BufReader::new(output.take(MAX_BYTES + 1));
            let mut bytes = 0;
            let mut messages = 0;
            input.write_all(b"{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"ai_factory\",\"version\":\"0.2.0\"}}}\n").await.map_err(|_| UsageCode::Protocol)?;
            rpc_response(&mut lines, 1, &mut bytes, &mut messages).await?;
            input.write_all(b"{\"method\":\"initialized\"}\n{\"id\":2,\"method\":\"account/rateLimits/read\"}\n").await.map_err(|_| UsageCode::Protocol)?;
            let value = rpc_response(&mut lines, 2, &mut bytes, &mut messages).await?;
            Ok((value, vec![], true))
        } else {
            drop(child.stdin.take());
            let mut bytes = Vec::new();
            output
                .take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| UsageCode::Protocol)?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(UsageCode::Protocol);
            }
            let status = child.wait().await.map_err(|_| UsageCode::QueryFailed)?;
            Ok((Value::Null, bytes, status.success()))
        }
    };
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(UsageCode::Stopping),
        _ = tokio::time::sleep_until(deadline) => Err(UsageCode::TimedOut),
        result = operation => result,
    };
    tree.terminate();
    let _ = child.start_kill();
    let _ = child.wait().await;
    result
}
async fn rpc_response<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    id: u64,
    total: &mut u64,
    messages: &mut usize,
) -> std::result::Result<Value, UsageCode> {
    loop {
        if *messages >= 512 {
            return Err(UsageCode::Protocol);
        }
        let mut line = Vec::new();
        let count = reader
            .read_until(b'\n', &mut line)
            .await
            .map_err(|_| UsageCode::Protocol)?;
        *total += count as u64;
        *messages += 1;
        if count == 0 || *total > MAX_BYTES {
            return Err(UsageCode::Protocol);
        }
        let message: Value = serde_json::from_slice(&line).map_err(|_| UsageCode::Protocol)?;
        if message["id"] != id {
            continue;
        }
        if let Some(error) = message.get("error") {
            return Err(if error["code"] == -32601 {
                UsageCode::Unsupported
            } else {
                UsageCode::QueryFailed
            });
        }
        return message.get("result").cloned().ok_or(UsageCode::Protocol);
    }
}
#[cfg(all(test, unix))]
mod tests;
