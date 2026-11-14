use super::output::{extract_result, spawn_output};
use super::ProcessTree;
use crate::*;
use anyhow::{bail, Context};
use std::{path::Path, sync::Arc};
use tokio::{io::AsyncWriteExt, process::Command};
use tokio_util::sync::CancellationToken;

pub(super) struct Invocation<'a> {
    pub factory: &'a Arc<Factory>,
    pub run: &'a str,
    pub node_id: &'a str,
    pub agent: &'a str,
    pub track_session: bool,
    pub command: Command,
    pub prompt: &'a str,
    pub result_path: &'a Path,
    pub log_path: &'a Path,
    pub cancel: &'a CancellationToken,
}

pub(super) async fn run(invocation: Invocation<'_>) -> Result<Option<String>> {
    let Invocation {
        factory,
        run,
        node_id,
        agent,
        track_session,
        command: mut cmd,
        prompt,
        result_path,
        log_path,
        cancel,
    } = invocation;
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    let mut child = cmd.spawn().with_context(|| {
        format!(
            "Cannot start {} CLI; configure installed executable path",
            agent
        )
    })?;
    let mut process_tree = ProcessTree::attach(&mut child)?;
    let mut stdin = child.stdin.take().unwrap();
    let input_prompt = prompt.to_owned();
    let input = tokio::spawn(async move {
        stdin.write_all(input_prompt.as_bytes()).await?;
        stdin.shutdown().await
    });
    let log = Arc::new(std::sync::Mutex::new(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?,
    ));
    let out = spawn_output(
        factory,
        run,
        node_id,
        child.stdout.take().unwrap(),
        log.clone(),
        true,
        track_session,
    );
    let err = spawn_output(
        factory,
        run,
        node_id,
        child.stderr.take().unwrap(),
        log.clone(),
        false,
        false,
    );
    let status = tokio::select! {
        r = child.wait() => r.context("CLI wait failed"),
        _ = cancel.cancelled() => {
            process_tree.terminate();
            let _ = child.kill().await;
            Err(anyhow::anyhow!("Run cancelled"))
        }
    };
    // A CLI may exit while descendants still own its pipes. End the entire
    // session before draining output, including on normal leader exit.
    process_tree.terminate();
    let _ = input.await;
    let output = out.await;
    let _ = err.await;
    let output = output.context("CLI output reader failed")??;
    let status = status?;
    if !status.success() {
        bail!("Agent exited with {status}");
    }
    let raw = if result_path.exists() {
        let raw = std::fs::read_to_string(result_path)?;
        std::fs::write(result_path, factory.redact(&raw))?;
        Some(raw)
    } else {
        extract_result(&output)
    };
    Ok(raw)
}
