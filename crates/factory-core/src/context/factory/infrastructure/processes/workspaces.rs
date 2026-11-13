use crate::context::factory::application::interfaces::run_cleanup::RunCleanup;
use crate::CleanupTarget;
use crate::{context::factory::infrastructure::repositories::store::Store, Result};
use anyhow::{ensure, Context};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) fn archive_artifact(
    memory: &Path,
    node: &str,
    name: &std::ffi::OsStr,
    source: &mut impl Read,
    expected_hash: &str,
) -> Result<(PathBuf, String)> {
    ensure!(crate::safe_id(node), "Invalid artifact node ID");
    ensure!(
        memory.canonicalize()? == memory,
        "Artifact memory directory must not be a link"
    );
    let mut parent = memory.to_path_buf();
    for part in ["artifacts", node] {
        parent.push(part);
        match std::fs::create_dir(&parent) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let metadata = std::fs::symlink_metadata(&parent)?;
        ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && parent.canonicalize()? == parent,
            "Artifact archive directory must not be a link"
        );
    }
    let target = parent.join(format!(
        "{}-{}",
        uuid::Uuid::new_v4(),
        name.to_string_lossy()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)?;
    let copied = (|| -> Result<String> {
        let mut hash = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
            file.write_all(&buffer[..count])?;
        }
        let hash = format!("{:x}", hash.finalize());
        ensure!(
            expected_hash.is_empty() || expected_hash == hash,
            "Artifact hash mismatch"
        );
        file.sync_all()?;
        Ok(hash)
    })();
    match copied {
        Ok(hash) => Ok((target, hash)),
        Err(error) => {
            drop(file);
            std::fs::remove_file(&target).context("Cannot remove invalid artifact copy")?;
            Err(error)
        }
    }
}

pub(crate) fn path(root: &Path, run: &str) -> Result<PathBuf> {
    ensure!(
        uuid::Uuid::parse_str(run).is_ok(),
        "Invalid workspace run ID"
    );
    let parent = root.join("workspaces");
    let metadata = std::fs::symlink_metadata(&parent)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink() && parent.canonicalize()? == parent,
        "Workspace parent must be an app-owned directory, not a link"
    );
    Ok(parent.join(run))
}

pub(crate) async fn cleanup(
    cleaner: Arc<dyn RunCleanup>,
    repository: String,
    run: String,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let mut last = None;
        for attempt in 0..5 {
            match cleaner.remove(&repository, &run, &CleanupTarget::Workspace) {
                Ok(()) => return Ok(()),
                Err(error) => last = Some(error),
            }
            std::thread::sleep(std::time::Duration::from_millis(40 * (attempt + 1)));
        }
        Err(last.unwrap())
    })
    .await
    .context("Workspace cleanup worker stopped")?
}

pub(crate) fn recover(root: &Path, store: &Store, cleaner: &dyn RunCleanup) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    for run in store.runs()? {
        if ["queued", "running"].contains(&run.status.as_str())
            || uuid::Uuid::parse_str(&run.id).is_err()
        {
            continue;
        }
        match cleaner.remove(&run.repository, &run.id, &CleanupTarget::Workspace) {
            Ok(()) => {
                if run.workspace.as_deref() == path(root, &run.id)?.to_str() {
                    store.set_workspace(&run.id, None)?;
                }
            }
            Err(error) => errors.push(format!("Workspace cleanup pending for {}: {error}", run.id)),
        }
    }
    Ok(errors)
}
