use crate::{context::factory::infrastructure::repositories::store::Store, Result};
use anyhow::{ensure, Context};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) fn archive_artifact(
    memory: &Path,
    node: &str,
    name: &std::ffi::OsStr,
    bytes: &[u8],
) -> Result<PathBuf> {
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
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(target)
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

fn remove(root: &Path, run: &str) -> Result<()> {
    let workspace = path(root, run)?;
    match std::fs::symlink_metadata(&workspace) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(metadata) if metadata.file_type().is_symlink() => {
            // Remove the link itself; never traverse its target.
            #[cfg(windows)]
            std::fs::remove_file(&workspace).or_else(|_| std::fs::remove_dir(&workspace))?;
            #[cfg(not(windows))]
            std::fs::remove_file(&workspace)?;
            Ok(())
        }
        Ok(metadata) if metadata.is_dir() => {
            ensure!(
                workspace.canonicalize()? == workspace,
                "Workspace directory resolved outside its expected path"
            );
            std::fs::remove_dir_all(&workspace).context("Cannot remove temporary workspace")
        }
        Ok(_) => {
            std::fs::remove_file(&workspace)?;
            Ok(())
        }
    }
}

pub(crate) async fn cleanup(root: PathBuf, run: String) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let mut last = None;
        for attempt in 0..5 {
            match remove(&root, &run) {
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

pub(crate) fn recover(root: &Path, store: &Store) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    for run in store.runs()? {
        if ["queued", "running"].contains(&run.status.as_str())
            || uuid::Uuid::parse_str(&run.id).is_err()
        {
            continue;
        }
        match remove(root, &run.id) {
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
