mod common;
use factory_core::*;
use rusqlite::Connection;
use std::fs;

fn stopped(f: &Factory) -> Result<String> {
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            4,
            "[AI] deletion",
            "alice",
            "body",
        ))?
        .unwrap();
    f.cancel(&id)?;
    Ok(id)
}

#[test]
fn failed_prepare_changes_neither_history_nor_files() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let f = common::open(dir.path().into())?;
    let id = stopped(&f)?;
    let path = dir.path().join("sessions").join(&id);
    fs::create_dir_all(&path)?;
    fs::write(path.join("keep"), b"keep")?;
    let db = Connection::open(dir.path().join("factory.sqlite3"))?;
    db.execute_batch("CREATE TRIGGER fail_prepare BEFORE INSERT ON run_deletions BEGIN SELECT RAISE(ABORT,'fixture prepare failure'); END;")?;
    assert!(f.delete_run(&id).is_err());
    assert!(f.run_detail(&id).is_ok());
    assert_eq!(fs::read(path.join("keep"))?, b"keep");
    assert!(f.snapshot()?.deletions.is_empty());
    Ok(())
}

#[test]
fn failed_final_commit_recovers_on_reopen_without_replaying_event() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let f = common::open(dir.path().into())?;
    let id = stopped(&f)?;
    let db = Connection::open(dir.path().join("factory.sqlite3"))?;
    db.execute_batch("CREATE TRIGGER fail_final BEFORE DELETE ON runs BEGIN SELECT RAISE(ABORT,'fixture final failure'); END;")?;
    assert!(f.delete_run(&id).is_err());
    assert!(f.run_detail(&id).is_err());
    assert_eq!(f.snapshot()?.deletions.len(), 1);
    db.execute_batch("DROP TRIGGER fail_final;")?;
    drop(db);
    drop(f);
    let f = common::open(dir.path().into())?;
    assert!(matches!(f.run_view(&id)?, RunView::Missing));
    assert!(f.snapshot()?.deletions.is_empty());
    assert!(f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            4,
            "[AI] deletion",
            "alice",
            "body"
        ))?
        .is_none());
    f.delete_run(&id)?;
    Ok(())
}

#[test]
fn crash_after_each_remove_before_progress_commit_is_recoverable() -> Result<()> {
    for target in ["sessions", "memory", "workspace"] {
        let dir = tempfile::tempdir()?;
        let f = common::open(dir.path().into())?;
        let id = stopped(&f)?;
        let paths = [
            dir.path().join("sessions").join(&id),
            dir.path().join("memory/acme/repo/runs").join(&id),
            dir.path().join("workspaces").join(&id),
        ];
        for path in &paths {
            fs::create_dir_all(path.join("nested"))?;
            fs::write(path.join("nested/private"), b"private")?;
        }
        let db = Connection::open(dir.path().join("factory.sqlite3"))?;
        db.execute_batch(&format!("CREATE TRIGGER fail_progress BEFORE UPDATE ON run_deletion_targets WHEN NEW.target='{target}' AND NEW.done=1 BEGIN SELECT RAISE(ABORT,'fixture progress failure'); END;"))?;
        assert!(f.delete_run(&id).is_err());
        assert!(f.run_detail(&id).is_err());
        assert_eq!(f.snapshot()?.deletions.len(), 1);
        let index = match target {
            "sessions" => 0,
            "memory" => 1,
            _ => 2,
        };
        assert!(!paths[index].exists(), "remove precedes durable mark");
        db.execute_batch("DROP TRIGGER fail_progress;")?;
        drop(db);
        drop(f);
        let f = common::open(dir.path().into())?;
        assert!(matches!(f.run_view(&id)?, RunView::Missing));
        for path in paths {
            assert!(!path.exists());
        }
    }
    Ok(())
}

#[test]
fn crash_after_intent_before_first_remove_keeps_owner_and_content() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let f = common::open(dir.path().into())?;
    let id = stopped(&f)?;
    let session = dir.path().join("sessions").join(&id);
    fs::create_dir_all(&session)?;
    fs::write(session.join("keep"), b"private")?;
    let db = Connection::open(dir.path().join("factory.sqlite3"))?;
    db.execute_batch("CREATE TRIGGER fail_resume BEFORE UPDATE ON run_deletions BEGIN SELECT RAISE(ABORT,'fixture resume failure'); END;")?;
    assert!(f.delete_run(&id).is_err());
    assert_eq!(fs::read(session.join("keep"))?, b"private");
    assert!(matches!(f.run_view(&id)?, RunView::Deleting { .. }));
    db.execute_batch("DROP TRIGGER fail_resume;")?;
    drop(db);
    drop(f);
    let f = common::open(dir.path().into())?;
    assert!(matches!(f.run_view(&id)?, RunView::Missing));
    assert!(!session.exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn partial_cleanup_is_visible_sanitized_and_recovers_only_known_targets() -> Result<()> {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let f = common::open(dir.path().into())?;
    let id = stopped(&f)?;
    fs::create_dir_all(dir.path().join("sessions").join(&id))?;
    fs::write(outside.path().join("secret-path-token"), b"keep")?;
    fs::create_dir_all(dir.path().join("memory/acme"))?;
    symlink(outside.path(), dir.path().join("memory/acme/repo"))?;
    let result = f.delete_run(&id)?;
    let DeleteRunOutcome::Pending { deletion } = result else {
        panic!("must remain pending")
    };
    assert_eq!(deletion.state, "failed");
    assert!(!deletion.last_error.unwrap().contains("secret-path-token"));
    assert!(f.retry(&id).is_err());
    assert!(f.cancel(&id).is_err());
    assert!(f.logs(&id, "spec", 0).is_err());
    assert!(!dir.path().join("sessions").join(&id).exists());
    assert_eq!(f.snapshot()?.metrics.total, 0);
    // The rest of the service remains usable; no global GitHub failure.
    assert!(f.snapshot()?.service.last_error.is_none());
    assert!(f
        .enqueue(TriggerEvent::issue(
            "other/repo",
            7,
            "[AI] independent",
            "alice",
            ""
        ))?
        .is_some());
    drop(f);
    let f = common::open(dir.path().into())?;
    assert_eq!(f.snapshot()?.deletions[0].state, "failed");
    fs::remove_file(dir.path().join("memory/acme/repo"))?;
    assert!(matches!(
        f.delete_run(&id)?,
        DeleteRunOutcome::Deleted { .. }
    ));
    assert_eq!(fs::read(outside.path().join("secret-path-token"))?, b"keep");
    Ok(())
}

#[test]
fn deletion_is_offline_even_when_all_credential_operations_fail() -> Result<()> {
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir()?;
    let credentials = common::backend(dir.path());
    let f = common::open(dir.path().into())?;
    let id = stopped(&f)?;
    credentials.fail_get.store(true, Ordering::SeqCst);
    credentials.fail_set.store(true, Ordering::SeqCst);
    credentials.fail_delete.store(true, Ordering::SeqCst);
    assert!(matches!(
        f.delete_run(&id)?,
        DeleteRunOutcome::Deleted { .. }
    ));
    Ok(())
}

#[test]
fn simultaneous_delete_requests_serialize_and_keep_independent_retry() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let f = common::open(dir.path().into())?;
    let id = stopped(&f)?;
    let retry = f.retry(&id)?;
    let barrier = std::sync::Barrier::new(3);
    std::thread::scope(|s| -> Result<()> {
        let first = s.spawn(|| {
            barrier.wait();
            f.delete_run(&id)
        });
        let second = s.spawn(|| {
            barrier.wait();
            f.delete_run(&id)
        });
        barrier.wait();
        assert!(matches!(
            first.join().unwrap()?,
            DeleteRunOutcome::Deleted { .. }
        ));
        assert!(matches!(
            second.join().unwrap()?,
            DeleteRunOutcome::Deleted { .. }
        ));
        Ok(())
    })?;
    assert!(f.run_detail(&retry).is_ok());
    assert!(f.retry(&id).is_err());
    Ok(())
}
