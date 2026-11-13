use crate::context::factory::infrastructure::processes::workspaces;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

struct ArtifactRoots {
    workspace: PathBuf,
    memory: PathBuf,
}

impl ArtifactRoots {
    // Check every component before canonicalizing: canonicalization alone hides
    // links pointing back into an authorized directory.
    fn checked(&self, path: &Path) -> Result<Option<(PathBuf, std::fs::Metadata)>> {
        let root = [&self.workspace, &self.memory]
            .into_iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .context("Artifact escaped authorized workspace/memory")?;
        let mut current = root.clone();
        let mut metadata = std::fs::symlink_metadata(&current)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "Artifact symlink forbidden: {}",
            current.display()
        );
        for component in path.strip_prefix(root)?.components() {
            current.push(component);
            metadata = match std::fs::symlink_metadata(&current) {
                Ok(metadata) => metadata,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) =>
                {
                    return Ok(None)
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Cannot inspect artifact: {}", current.display()))
                }
            };
            ensure!(
                !metadata.file_type().is_symlink(),
                "Artifact symlink forbidden: {}",
                current.display()
            );
        }
        let resolved = current.canonicalize()?;
        ensure!(
            resolved.starts_with(root),
            "Artifact escaped authorized workspace/memory"
        );
        Ok(Some((resolved, metadata)))
    }

    fn resolve(&self, raw: &str) -> Result<(PathBuf, std::fs::Metadata)> {
        let path = Path::new(raw);
        ensure!(
            !path.components().any(|part| part == Component::ParentDir),
            "Artifact parent traversal forbidden: {raw}"
        );
        ensure!(
            !raw.trim().is_empty()
                && path
                    .components()
                    .any(|part| matches!(part, Component::Normal(_))),
            "Artifact path must name a file or directory"
        );
        let path: PathBuf = path.components().collect();
        let candidates = if path.is_absolute() {
            vec![path]
        } else {
            vec![self.workspace.join(&path), self.memory.join(&path)]
        };
        let mut matches = Vec::new();
        for candidate in candidates {
            if let Some(found) = self.checked(&candidate)? {
                if !matches.iter().any(|(path, _)| path == &found.0) {
                    matches.push(found);
                }
            }
        }
        ensure!(
            matches.len() <= 1,
            "Ambiguous artifact path: {raw}; exists in both workspace and memory; use an absolute path"
        );
        matches.pop().with_context(|| {
            format!("Artifact does not exist: {raw}; checked workspace and memory")
        })
    }

    fn collect_files(&self, path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        let (path, metadata) = self
            .checked(path)?
            .with_context(|| format!("Artifact does not exist: {}", path.display()))?;
        if metadata.is_dir() {
            let mut children = std::fs::read_dir(&path)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            children.sort();
            for child in children {
                self.collect_files(&child, files)?;
            }
        } else {
            ensure!(
                metadata.is_file(),
                "Artifact must be a regular file: {}",
                path.display()
            );
            files.push(path);
        }
        Ok(())
    }
}

pub(in super::super) fn archive(
    value: &mut Value,
    workspace: &Path,
    memory: &Path,
    node: &str,
) -> Result<()> {
    let roots = ArtifactRoots {
        workspace: workspace.canonicalize()?,
        memory: memory.canonicalize()?,
    };
    let mut planned = Vec::new();
    for artifact in value["artifacts"]
        .as_array()
        .context("Artifact array missing")?
    {
        let raw = artifact["path"].as_str().context("Artifact path missing")?;
        let expected = artifact["sha256"]
            .as_str()
            .context("Artifact hash missing")?;
        let (path, metadata) = roots.resolve(raw)?;
        let mut files = Vec::new();
        if metadata.is_dir() {
            ensure!(
                expected.is_empty(),
                "Directory artifact cannot declare a file hash: {raw}; declare individual files or an empty hash"
            );
            roots.collect_files(&path, &mut files)?;
            ensure!(
                !files.is_empty(),
                "Artifact directory contains no files: {raw}"
            );
        } else {
            ensure!(metadata.is_file(), "Artifact must be a regular file: {raw}");
            files.push(path);
        }
        for file in files {
            planned.push((file, expected.to_owned(), artifact["kind"].clone()));
        }
    }
    // Resolve the whole manifest before writing into the archive.
    // Otherwise expanding a memory directory can consume its own new copies.
    // File contents are streamed, never retained for the whole directory.
    let mut archived = Vec::new();
    for (file, expected, kind) in planned {
        roots
            .checked(&file)?
            .context("Artifact disappeared before archival")?;
        let mut source = std::fs::File::open(&file)
            .with_context(|| format!("Cannot read artifact: {}", file.display()))?;
        let (path, hash) = workspaces::archive_artifact(
            memory,
            node,
            file.file_name().context("Artifact file name missing")?,
            &mut source,
            &expected,
        )
        .with_context(|| format!("Cannot archive artifact: {}", file.display()))?;
        archived.push(json!({"path":path,"sha256":hash,"kind":kind}));
    }
    value["artifacts"] = json!(archived);
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn peak_memory_bytes() -> u64 {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        assert_eq!(
            unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
            0
        );
        let peak = unsafe { usage.assume_init() }.ru_maxrss as u64;
        if cfg!(target_os = "macos") {
            peak
        } else {
            peak * 1024
        }
    }

    #[test]
    fn directory_archival_memory_does_not_grow_with_total_file_bytes() {
        const CHILD: &str = "AI_FACTORY_ARTIFACT_MEMORY_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    std::thread::current().name().unwrap(),
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let memory = temp.path().join("memory");
        std::fs::create_dir_all(workspace.join("logs")).unwrap();
        std::fs::create_dir(&memory).unwrap();
        for name in ["one.log", "two.log", "three.log"] {
            std::fs::File::create(workspace.join("logs").join(name))
                .unwrap()
                .set_len(32 * 1024 * 1024)
                .unwrap();
        }
        let mut expected = Sha256::new();
        for _ in 0..512 {
            expected.update([0; 64 * 1024]);
        }
        let expected = format!("{:x}", expected.finalize());
        let mut value = json!({"artifacts":[{"path":"logs","sha256":"","kind":"log"}]});
        let before = peak_memory_bytes();
        archive(
            &mut value,
            &workspace,
            &memory.canonicalize().unwrap(),
            "builder",
        )
        .unwrap();
        let growth = peak_memory_bytes().saturating_sub(before);
        assert!(
            growth < 32 * 1024 * 1024,
            "Archiving 96 MiB raised peak memory by {growth} bytes"
        );
        assert_eq!(value["artifacts"].as_array().unwrap().len(), 3);
        for artifact in value["artifacts"].as_array().unwrap() {
            assert_eq!(artifact["sha256"], expected);
            assert_eq!(
                std::fs::metadata(artifact["path"].as_str().unwrap())
                    .unwrap()
                    .len(),
                32 * 1024 * 1024
            );
        }
    }
}
