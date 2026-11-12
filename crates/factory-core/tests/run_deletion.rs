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
    assert!(f.enqueue(event)?.is_none());
    assert!(matches!(
        f.delete_run(&id)?,
        DeleteRunOutcome::Deleted { .. }
    ));
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
