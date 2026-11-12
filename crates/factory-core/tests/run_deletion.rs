mod common;
use factory_core::*;

#[test]
fn deletes_one_attempt_offline_and_keeps_retry_and_shared_data() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let f = common::open(dir.path().into())?;
    let event = TriggerEvent::issue("acme/repo", 4, "[AI] deletion", "alice", "body");
    let id = f.enqueue(event.clone())?.unwrap();
    f.cancel(&id)?;
    let sibling = f.retry(&id)?;
    for relative in [
        format!("sessions/{id}/nested/output.log"),
        format!("memory/acme/repo/runs/{id}/artifacts/spec/report.md"),
        format!("workspaces/{id}/unpublished.txt"),
    ] {
        let path = dir.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, b"run-owned")?;
    }
    std::fs::create_dir_all(dir.path().join("memory/acme/repo/shared"))?;
    std::fs::write(dir.path().join("memory/acme/repo/shared/keep"), b"shared")?;
    assert!(matches!(
        f.delete_run(&id)?,
        DeleteRunOutcome::Deleted { .. }
    ));
    assert!(f.run_detail(&id).is_err());
    assert!(f.run_detail(&sibling).is_ok());
    assert!(f.logs(&id, "spec", 0).is_err());
    assert!(!dir.path().join("sessions").join(&id).exists());
    assert!(!dir.path().join("memory/acme/repo/runs").join(&id).exists());
    assert!(!dir.path().join("workspaces").join(&id).exists());
    assert_eq!(
        std::fs::read(dir.path().join("memory/acme/repo/shared/keep"))?,
        b"shared"
    );
    assert!(f.enqueue(event.clone())?.is_none());
    f.cancel(&sibling)?;
    f.delete_run(&sibling)?;
    let next = f
        .enqueue(event.clone())?
        .expect("last deletion releases event");
    assert_ne!(next, id);
    assert_ne!(next, sibling);
    assert!(matches!(
        f.delete_run(&id)?,
        DeleteRunOutcome::Deleted { .. }
    ));
    assert!(
        f.enqueue(event)?.is_none(),
        "repeated deletion preserves new run"
    );
    Ok(())
}

#[test]
fn deleting_last_run_releases_only_its_event_across_restart() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let f = common::open(dir.path().into())?;
    let event = TriggerEvent::issue("acme/repo", 4, "[AI] deletion", "alice", "body");
    let other = TriggerEvent::issue("acme/repo", 5, "[AI] other", "alice", "body");
    let keep = f.enqueue(other.clone())?.unwrap();
    let id = f.enqueue(event.clone())?.unwrap();
    f.set_run_status(&id, "failed", Some("Agent exited".into()))?;
    f.delete_run(&id)?;
    drop(f);

    let f = common::open(dir.path().into())?;
    assert!(f.enqueue(event.clone())?.is_some());
    assert!(f.enqueue(event)?.is_none());
    assert!(f.enqueue(other)?.is_none());
    assert!(f.run_detail(&keep).is_ok());
    Ok(())
}

#[test]
fn deleting_retry_before_original_preserves_event_until_last_attempt() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let f = common::open(dir.path().into())?;
    let event = TriggerEvent::issue("acme/repo", 4, "[AI] deletion", "alice", "body");
    let id = f.enqueue(event.clone())?.unwrap();
    f.cancel(&id)?;
    let retry = f.retry(&id)?;
    f.cancel(&retry)?;
    f.delete_run(&retry)?;
    assert!(f.enqueue(event.clone())?.is_none());
    f.delete_run(&id)?;
    assert!(f.enqueue(event)?.is_some());
    Ok(())
}

#[test]
fn rejects_queued_and_invalid_but_never_guesses_absent_ownership() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let f = common::open(dir.path().into())?;
    let id = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            4,
            "[AI] deletion",
            "alice",
            "body",
        ))?
        .unwrap();
    assert!(f
        .delete_run(&id)
        .unwrap_err()
        .to_string()
        .contains("Cancel"));
    for invalid in ["..", "../sessions", "/root", "legacy-x"] {
        assert!(f.delete_run(invalid).is_err());
    }
    let absent = uuid::Uuid::new_v4().to_string();
    let sentinel = dir.path().join("sessions").join(&absent);
    std::fs::create_dir_all(&sentinel)?;
    std::fs::write(sentinel.join("keep"), b"keep")?;
    f.delete_run(&absent)?;
    assert!(sentinel.join("keep").exists());
    Ok(())
}
