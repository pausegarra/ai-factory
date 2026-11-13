//! Private homes created only by AI Factory; never inspect or copy global CLI auth.
use crate::context::factory::domain::entities::agent_account::validate_account_id;
use crate::Result;
use anyhow::{ensure, Context};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("Managed Codex home is unavailable")?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Unsafe managed Codex home"
    );
    ensure!(path.canonicalize()? == path, "Unsafe managed Codex home");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(crate) fn home(root: &Path, id: &str, require_auth: bool) -> Result<PathBuf> {
    validate_account_id(id)?;
    let mut path = root.to_path_buf();
    for part in ["agent-accounts", "codex", id] {
        path.push(part);
        directory(&path)?;
    }
    let auth = path.join("auth.json");
    match fs::symlink_metadata(&auth) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "Unsafe managed Codex credential file"
            );
            ensure!(
                !require_auth || metadata.len() > 0,
                "Managed Codex credentials are missing"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                ensure!(
                    metadata.nlink() == 1,
                    "Unsafe managed Codex credential file"
                );
                fs::set_permissions(&auth, fs::Permissions::from_mode(0o600))?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !require_auth => (),
        Err(_) => anyhow::bail!("Managed Codex credentials are missing"),
    }
    Ok(path)
}

/// Missing owned directories are already clean. Existing links never authorize deletion.
pub(crate) fn remove(root: &Path, id: &str) -> Result<()> {
    validate_account_id(id)?;
    directory(root)?;
    let mut path = root.to_path_buf();
    for part in ["agent-accounts", "codex", id] {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("Cannot inspect managed Codex home"),
            Ok(_) => directory(&path)?,
        }
    }
    let path = home(root, id, false)?;
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("Cannot remove managed Codex home"),
    }
}

pub(crate) fn create(root: &Path, id: &str) -> Result<PathBuf> {
    validate_account_id(id)?;
    let mut path = root.to_path_buf();
    for part in ["agent-accounts", "codex"] {
        path.push(part);
        match fs::create_dir(&path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        directory(&path)?;
    }
    path.push(id);
    fs::create_dir(&path).context("Cannot create managed Codex home")?;
    directory(&path)?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path.join("config.toml"))?
        .write_all(b"cli_auth_credentials_store = \"file\"\n")?;
    Ok(path)
}
