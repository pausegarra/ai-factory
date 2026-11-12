use crate::context::factory::{
    application::interfaces::run_cleanup::RunCleanup, infrastructure::repositories::store::Store,
};
use crate::*;
use anyhow::ensure;
use std::sync::{Arc, Mutex};

impl Factory {
    pub fn run_view(&self, id: &str) -> Result<RunView> {
        self.store.run_view(id)
    }

    pub fn delete_run(&self, id: &str) -> Result<DeleteRunOutcome> {
        ensure!(valid_deletion_id(id), "Invalid run ID");
        let lock = {
            let mut locks = self.deletion_locks.lock().unwrap();
            locks.retain(|_, lock| lock.strong_count() > 0);
            match locks.get(id).and_then(std::sync::Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(Mutex::new(()));
                    locks.insert(id.into(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        let _delete = lock.lock().unwrap();
        {
            let _gate = self.lifecycle_gate.lock().unwrap();
            ensure!(
                !self.active.lock().unwrap().contains_key(id),
                "This run is still stopping. Try again when cleanup finishes."
            );
            if self.store.prepare_deletion(id)?.is_none() {
                return Ok(DeleteRunOutcome::Deleted { run_id: id.into() });
            }
        }
        self.emit("run_deletion_pending", Some(id), None);
        let result = resume_with_gate(
            &self.store,
            self.cleanup.as_ref(),
            id,
            Some(&self.lifecycle_gate),
        )?;
        match &result {
            DeleteRunOutcome::Deleted { .. } => self.emit("run_deleted", Some(id), None),
            DeleteRunOutcome::Pending { .. } => self.emit("run_deletion_failed", Some(id), None),
        }
        Ok(result)
    }
}

/// Used during startup before any worker exists, and by the serialized use case.
/// Intent and progress survive a crash at every filesystem/SQLite boundary.
pub(crate) fn resume(
    store: &Store,
    cleanup: &dyn RunCleanup,
    id: &str,
) -> Result<DeleteRunOutcome> {
    resume_with_gate(store, cleanup, id, None)
}

fn resume_with_gate(
    store: &Store,
    cleanup: &dyn RunCleanup,
    id: &str,
    gate: Option<&Mutex<()>>,
) -> Result<DeleteRunOutcome> {
    let Some(record) = store.deletion(id)? else {
        return Ok(DeleteRunOutcome::Deleted { run_id: id.into() });
    };
    store.resume_deletion(id)?;
    for target in record
        .targets
        .iter()
        .filter(|t| !record.completed.contains(t))
    {
        if let Err(error) = cleanup.remove(&record.public.repository, id, target) {
            // Never format arbitrary OS errors, paths or filenames into public messages.
            let target = serde_json::to_value(target)?.as_str().unwrap().to_owned();
            let class = match error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind)
            {
                Some(std::io::ErrorKind::PermissionDenied) => "permission denied",
                Some(std::io::ErrorKind::NotADirectory) => "invalid parent directory",
                Some(std::io::ErrorKind::DirectoryNotEmpty) => "directory changed during cleanup",
                _ => "I/O or safety check failed",
            };
            store.fail_deletion(id, &format!("Deletion pending for {id}: cannot remove {target} ({class}). Some files may already be deleted. Retry deletion to finish."))?;
            return Ok(DeleteRunOutcome::Pending {
                deletion: store.deletion(id)?.unwrap().public,
            });
        }
        store.mark_target_complete(id, target)?;
    }
    let _gate = gate.map(|gate| gate.lock().unwrap());
    store.finish_deletion(id)?;
    Ok(DeleteRunOutcome::Deleted { run_id: id.into() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    struct BlockCleanup {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        calls: AtomicUsize,
    }
    impl RunCleanup for BlockCleanup {
        fn remove(&self, _: &str, _: &str, _: &CleanupTarget) -> Result<()> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.entered.send(())?;
                self.release.lock().unwrap().recv()?;
            }
            Ok(())
        }
    }

    #[test]
    fn accepted_delete_blocks_retry_but_not_other_runs_and_serializes_waiters() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut f = Factory::open(dir.path().into())?;
        let id = f
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                1,
                "[AI] delete",
                "alice",
                "",
            ))?
            .unwrap();
        f.cancel(&id)?;
        let (entered, received) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let cleaner = Arc::new(BlockCleanup {
            entered,
            release: Mutex::new(released),
            calls: AtomicUsize::new(0),
        });
        Arc::get_mut(&mut f).unwrap().cleanup = cleaner.clone();
        std::thread::scope(|s| -> Result<()> {
            let first = s.spawn(|| f.delete_run(&id));
            received.recv()?;
            assert!(f.retry(&id).is_err());
            assert!(f.run_detail(&id).is_err());
            assert!(matches!(f.run_view(&id)?, RunView::Deleting { .. }));
            let other = f
                .enqueue(TriggerEvent::issue(
                    "acme/repo",
                    2,
                    "[AI] other",
                    "alice",
                    "",
                ))?
                .unwrap();
            assert_eq!(f.snapshot()?.runs[0].id, other);
            let second = s.spawn(|| f.delete_run(&id));
            release.send(())?;
            assert!(matches!(
                first.join().unwrap()?,
                DeleteRunOutcome::Deleted { .. }
            ));
            assert!(matches!(
                second.join().unwrap()?,
                DeleteRunOutcome::Deleted { .. }
            ));
            assert_eq!(cleaner.calls.load(Ordering::SeqCst), 3);
            Ok(())
        })?;
        Ok(())
    }

    #[test]
    fn terminal_status_is_not_proof_worker_has_drained() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let f = Factory::open(dir.path().into())?;
        let id = f
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                1,
                "[AI] delete",
                "alice",
                "",
            ))?
            .unwrap();
        assert!(f.register_queued_worker(&id, 4)?.is_some());
        f.cancel(&id)?;
        assert!(f
            .delete_run(&id)
            .unwrap_err()
            .to_string()
            .contains("still stopping"));
        assert!(f.run_detail(&id).is_ok());
        f.active.lock().unwrap().remove(&id);
        f.delete_run(&id)?;
        Ok(())
    }

    #[test]
    fn stale_scheduler_candidate_cannot_register_after_cancel_or_deletion() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let f = Factory::open(dir.path().into())?;
        let id = f
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                1,
                "[AI] delete",
                "alice",
                "",
            ))?
            .unwrap();
        let stale = f.snapshot()?.runs.remove(0);
        f.cancel(&id)?;
        assert!(f.register_queued_worker(&stale.id, 4)?.is_none());
        f.delete_run(&id)?;
        assert!(f.register_queued_worker(&stale.id, 4)?.is_none());
        assert!(f.active.lock().unwrap().is_empty());
        let id = f
            .enqueue(TriggerEvent::issue(
                "acme/repo",
                2,
                "[AI] other",
                "alice",
                "",
            ))?
            .unwrap();
        let gate = f.lifecycle_gate.lock().unwrap();
        assert!(
            f.register_queued_worker(&id, 4)?.is_none(),
            "scheduler must defer rather than block on an import"
        );
        drop(gate);
        assert!(f.register_queued_worker(&id, 4)?.is_some());
        assert!(f.register_queued_worker(&id, 4)?.is_none());
        f.active.lock().unwrap().remove(&id);
        Ok(())
    }

    #[test]
    fn every_cleanup_target_failure_keeps_durable_progress_and_retries_only_remaining() -> Result<()>
    {
        struct FailAt {
            fail: usize,
            calls: Mutex<Vec<CleanupTarget>>,
        }
        impl RunCleanup for FailAt {
            fn remove(&self, _: &str, _: &str, target: &CleanupTarget) -> Result<()> {
                let mut calls = self.calls.lock().unwrap();
                calls.push(target.clone());
                anyhow::ensure!(
                    calls.len() != self.fail,
                    "fixture secret that must not escape"
                );
                Ok(())
            }
        }
        for fail in 1..=3 {
            let dir = tempfile::tempdir()?;
            let f = Factory::open(dir.path().into())?;
            let id = f
                .enqueue(TriggerEvent::issue(
                    "acme/repo",
                    1,
                    "[AI] delete",
                    "alice",
                    "",
                ))?
                .unwrap();
            f.cancel(&id)?;
            f.store.prepare_deletion(&id)?;
            let cleaner = FailAt {
                fail,
                calls: Mutex::new(vec![]),
            };
            let DeleteRunOutcome::Pending { deletion } = resume(&f.store, &cleaner, &id)? else {
                panic!("expected pending")
            };
            assert!(!deletion.last_error.unwrap().contains("secret"));
            assert_eq!(f.store.deletion(&id)?.unwrap().completed.len(), fail - 1);
            let cleaner = FailAt {
                fail: usize::MAX,
                calls: Mutex::new(vec![]),
            };
            assert!(matches!(
                resume(&f.store, &cleaner, &id)?,
                DeleteRunOutcome::Deleted { .. }
            ));
            assert_eq!(cleaner.calls.lock().unwrap().len(), 4 - fail);
        }
        Ok(())
    }
}
