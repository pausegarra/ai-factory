use crate::context::factory::application::interfaces::run_cleanup::RunCleanup;
use crate::{valid_deletion_id, valid_repository, CleanupTarget, Result};
use anyhow::ensure;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use std::{ffi::OsStr, io, path::Path};

/// Root capability stays open for the lifetime of the owning Factory. Paths from
/// agents, run.workspace and artifacts are deliberately never consulted.
pub(crate) struct LocalRunCleanup {
    root: Dir,
}

impl LocalRunCleanup {
    pub(crate) fn open(root: &Path) -> Result<Self> {
        Ok(Self {
            root: Dir::open_ambient_dir(root, cap_std::ambient_authority())?,
        })
    }

    fn parent(&self, parts: &[&str]) -> io::Result<Option<Dir>> {
        let mut dir = self.root.try_clone()?;
        for part in parts {
            match dir.open_dir_nofollow(part) {
                Ok(next) => {
                    ordinary_directory(&next)?;
                    dir = next;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e),
            }
        }
        Ok(Some(dir))
    }
}

impl RunCleanup for LocalRunCleanup {
    fn remove(&self, repository: &str, run_id: &str, target: &CleanupTarget) -> Result<()> {
        ensure!(
            valid_deletion_id(run_id) && valid_repository(repository),
            "Invalid cleanup identity"
        );
        let (owner, repo) = repository.split_once('/').unwrap();
        let legacy = run_id.starts_with("legacy-");
        let parts = match target {
            CleanupTarget::Sessions => vec!["sessions"],
            CleanupTarget::Memory => vec!["memory", owner, repo, "runs"],
            CleanupTarget::Workspace => {
                ensure!(!legacy, "Legacy workspace is not managed");
                vec!["workspaces"]
            }
            CleanupTarget::Legacy => {
                ensure!(legacy, "Modern run has no legacy tree");
                vec!["legacy"]
            }
        };
        if let Some(parent) = self.parent(&parts)? {
            remove_tree(&parent, OsStr::new(run_id))?;
        }
        Ok(())
    }
}

fn remove_tree(parent: &Dir, leaf: &OsStr) -> io::Result<()> {
    // A concurrent disappearance is success; all other races fail safely and
    // remain recoverable. No recursive primitive that reopens absolute paths.
    let result = (|| {
        let metadata = parent.symlink_metadata(leaf)?;
        remove_checked_entry(parent, leaf, &metadata)
    })();
    match result {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn remove_checked_entry(
    parent: &Dir,
    leaf: &OsStr,
    metadata: &cap_std::fs::Metadata,
) -> io::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return parent.remove_file_or_symlink(leaf);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Unknown reparse point",
            ));
        }
    }
    let dir = parent.open_dir_nofollow(leaf)?;
    ordinary_directory(&dir)?;
    // Nofollow alone rejects links, but not a different ordinary directory
    // renamed over this leaf between inspection and open. Pin its identity.
    use cap_fs_ext::MetadataExt;
    let opened = dir.dir_metadata()?;
    if metadata.dev() != opened.dev() || metadata.ino() != opened.ino() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Directory changed during cleanup",
        ));
    }
    for entry in dir.entries()? {
        remove_tree(&dir, &entry?.file_name())?;
    }
    drop(dir);
    parent.remove_dir(leaf)
}

fn ordinary_directory(dir: &Dir) -> io::Result<()> {
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        if dir.dir_metadata()?.file_attributes() & 0x400 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Unknown reparse point",
            ));
        }
    }
    #[cfg(not(windows))]
    let _ = dir;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::factory::application::interfaces::run_cleanup::RunCleanup;

    #[test]
    fn replaced_ordinary_leaf_cannot_authorize_a_neighbor_tree() -> Result<()> {
        let root = tempfile::tempdir()?;
        let leaf = OsStr::new("owned");
        std::fs::create_dir(root.path().join(leaf))?;
        std::fs::create_dir(root.path().join("neighbor"))?;
        std::fs::write(root.path().join("neighbor/keep"), b"keep")?;
        let parent = Dir::open_ambient_dir(root.path(), cap_std::ambient_authority())?;
        let metadata = parent.symlink_metadata(leaf)?;
        // Deterministic substitution exactly between metadata validation and
        // nofollow open; no timing sleeps or arbitrary absolute recursion.
        std::fs::rename(root.path().join(leaf), root.path().join("old-owned"))?;
        std::fs::rename(root.path().join("neighbor"), root.path().join(leaf))?;
        assert!(remove_checked_entry(&parent, leaf, &metadata).is_err());
        assert_eq!(std::fs::read(root.path().join("owned/keep"))?, b"keep");
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn junction_parent_is_rejected_and_locked_file_remains_pending() -> Result<()> {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let id = "b4b8c673-b24a-4a73-b276-c66bc75fd127";
        std::fs::write(outside.path().join("keep"), b"keep")?;
        let junction = root.path().join("sessions");
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(outside.path())
            .output()?;
        assert!(
            output.status.success(),
            "junction fixture could not be created"
        );
        let cleaner = LocalRunCleanup::open(root.path())?;
        assert!(cleaner
            .remove("acme/repo", id, &CleanupTarget::Sessions)
            .is_err());
        std::fs::remove_dir(junction)?;
        let session = root.path().join("sessions").join(id);
        std::fs::create_dir_all(&session)?;
        std::fs::write(session.join("locked"), b"private")?;
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(session.join("locked"))?;
        assert!(cleaner
            .remove("acme/repo", id, &CleanupTarget::Sessions)
            .is_err());
        drop(locked);
        cleaner.remove("acme/repo", id, &CleanupTarget::Sessions)?;
        assert_eq!(std::fs::read(outside.path().join("keep"))?, b"keep");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn nested_link_is_unlinked_without_touching_external_tree() -> crate::Result<()> {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let id = "b4b8c673-b24a-4a73-b276-c66bc75fd127";
        let session = root.path().join("sessions").join(id);
        std::fs::create_dir_all(session.join("nested"))?;
        std::fs::write(outside.path().join("keep.txt"), b"untouched")?;
        symlink(outside.path(), session.join("nested/link"))?;
        let cleaner = LocalRunCleanup::open(root.path())?;
        cleaner.remove("acme/repo", id, &crate::CleanupTarget::Sessions)?;
        assert!(!session.exists());
        assert_eq!(
            std::fs::read(outside.path().join("keep.txt"))?,
            b"untouched"
        );
        Ok(())
    }

    #[test]
    fn all_owned_targets_and_absence_are_idempotent() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cleaner = LocalRunCleanup::open(root.path())?;
        for id in [
            "b4b8c673-b24a-4a73-b276-c66bc75fd127",
            "legacy-0123456789abcdef0123456789abcdef",
        ] {
            let mut targets = vec![
                (CleanupTarget::Sessions, format!("sessions/{id}")),
                (CleanupTarget::Memory, format!("memory/acme/repo/runs/{id}")),
            ];
            if id.starts_with("legacy-") {
                targets.push((CleanupTarget::Legacy, format!("legacy/{id}")));
            } else {
                targets.push((CleanupTarget::Workspace, format!("workspaces/{id}")));
            }
            for (target, path) in targets {
                let path = root.path().join(path);
                std::fs::create_dir_all(path.join("nested"))?;
                std::fs::write(path.join("nested/artifact"), b"private")?;
                cleaner.remove("acme/repo", id, &target)?;
                cleaner.remove("acme/repo", id, &target)?;
                assert!(!path.exists());
            }
        }
        Ok(())
    }

    #[test]
    fn hostile_identity_and_file_parent_fail_closed() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cleaner = LocalRunCleanup::open(root.path())?;
        let id = "b4b8c673-b24a-4a73-b276-c66bc75fd127";
        for repo in ["../repo", "/acme/repo", "acme/../repo", "acme\\repo"] {
            assert!(cleaner.remove(repo, id, &CleanupTarget::Sessions).is_err());
        }
        for id in [
            "..",
            "../x",
            "B4B8C673-B24A-4A73-B276-C66BC75FD127",
            "legacy-abcd",
        ] {
            assert!(cleaner
                .remove("acme/repo", id, &CleanupTarget::Sessions)
                .is_err());
        }
        std::fs::write(root.path().join("sessions"), b"keep")?;
        assert!(cleaner
            .remove("acme/repo", id, &CleanupTarget::Sessions)
            .is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn redirected_parent_is_rejected_and_broken_leaf_unlinked() -> Result<()> {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let id = "b4b8c673-b24a-4a73-b276-c66bc75fd127";
        std::fs::write(outside.path().join("keep"), b"keep")?;
        symlink(outside.path(), root.path().join("sessions"))?;
        let cleaner = LocalRunCleanup::open(root.path())?;
        assert!(cleaner
            .remove("acme/repo", id, &CleanupTarget::Sessions)
            .is_err());
        std::fs::remove_file(root.path().join("sessions"))?;
        std::fs::create_dir(root.path().join("sessions"))?;
        symlink(
            outside.path().join("missing"),
            root.path().join("sessions").join(id),
        )?;
        cleaner.remove("acme/repo", id, &CleanupTarget::Sessions)?;
        assert!(std::fs::symlink_metadata(root.path().join("sessions").join(id)).is_err());
        assert_eq!(std::fs::read(outside.path().join("keep"))?, b"keep");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn broken_parent_is_not_misreported_as_absent() -> Result<()> {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir()?;
        symlink(root.path().join("missing"), root.path().join("sessions"))?;
        let cleaner = LocalRunCleanup::open(root.path())?;
        assert!(cleaner
            .remove(
                "acme/repo",
                "b4b8c673-b24a-4a73-b276-c66bc75fd127",
                &CleanupTarget::Sessions
            )
            .is_err());
        assert!(std::fs::symlink_metadata(root.path().join("sessions")).is_ok());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn opened_parent_survives_synchronized_path_substitution() -> Result<()> {
        use std::{
            os::unix::fs::symlink,
            sync::{Arc, Barrier},
        };
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let id = "b4b8c673-b24a-4a73-b276-c66bc75fd127";
        std::fs::create_dir_all(root.path().join("sessions").join(id))?;
        std::fs::create_dir_all(outside.path().join(id))?;
        std::fs::write(outside.path().join(id).join("keep"), b"keep")?;
        let cleaner = LocalRunCleanup::open(root.path())?;
        let parent = cleaner.parent(&["sessions"])?.unwrap();
        let barrier = Arc::new(Barrier::new(2));
        std::thread::scope(|scope| -> Result<()> {
            let b = barrier.clone();
            let worker = scope.spawn(|| {
                barrier.wait();
                remove_tree(&parent, OsStr::new(id))
            });
            std::fs::rename(
                root.path().join("sessions"),
                root.path().join("old-sessions"),
            )?;
            symlink(outside.path(), root.path().join("sessions"))?;
            b.wait();
            worker.join().unwrap()?;
            Ok(())
        })?;
        assert_eq!(
            std::fs::read(outside.path().join(id).join("keep"))?,
            b"keep"
        );
        assert!(!root.path().join("old-sessions").join(id).exists());
        Ok(())
    }
}
