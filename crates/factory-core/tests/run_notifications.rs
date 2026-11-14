mod common;
use common::notifications::RecordingNotifications;
use factory_core::*;
use std::sync::{Arc, Barrier};
fn setup() -> (tempfile::TempDir, Arc<Factory>, Arc<RecordingNotifications>) {
    let dir = tempfile::tempdir().unwrap();
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9").unwrap();
    let sink = Arc::new(RecordingNotifications::default());
    f.set_run_notification_sink(sink.clone());
    (dir, f, sink)
}
fn queued(f: &Factory, number: u64, title: &str) -> String {
    f.enqueue(TriggerEvent::issue("acme/repo", number, title, "alice", ""))
        .unwrap()
        .unwrap()
}
#[test]
fn notification_lifecycle_uses_persisted_status_once() -> Result<()> {
    let (_dir, f, sink) = setup();
    let id = queued(&f, 11, "[AI] Notify me");
    assert!(sink.drain().is_empty());
    f.set_run_status(&id, "running", None)?;
    f.set_run_status(&id, "running", None)?;
    let started = sink.drain();
    assert_eq!(started.len(), 1);
    assert_eq!(
        started[0],
        RunNotification {
            run_id: id.clone(),
            attempt: f.run_detail(&id)?.run.attempt,
            status: "running".into(),
            title: "AI Factory — Run started".into(),
            body: "acme/repo #11 · [AI] Notify me".into()
        }
    );
    f.set_run_status(&id, "succeeded", None)?;
    f.set_run_status(&id, "failed", Some("late external failure".into()))?;
    f.set_run_status(&id, "succeeded", None)?;
    let ended = sink.drain();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].status, "succeeded");
    assert_eq!(ended[0].title, "AI Factory — Run succeeded");
    assert_eq!(ended[0].body, "acme/repo #11 · [AI] Notify me");
    assert_eq!(f.run_detail(&id)?.run.status, "succeeded");
    Ok(())
}
#[test]
fn notification_cancelled_and_interrupted_are_silent() -> Result<()> {
    let (_dir, f, sink) = setup();
    let id = queued(&f, 1, "Queued");
    f.cancel(&id)?;
    assert!(sink.drain().is_empty());
    for (n, state) in [(2, "cancelled"), (3, "interrupted")] {
        let id = queued(&f, n, "Active");
        f.set_run_status(&id, "running", None)?;
        f.set_run_status(&id, state, None)?;
        let messages = sink.drain();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].status, "running");
    }
    Ok(())
}
#[test]
fn notification_failed_and_blocked_have_safe_details() -> Result<()> {
    let (_dir, f, sink) = setup();
    for (i, (state, error, suffix)) in [
        (
            "failed",
            Some("timed out TOKEN=PRIVATE"),
            " · Execution timed out.",
        ),
        (
            "blocked",
            Some("EXTERNAL_PRIVATE_SENTINEL"),
            " · Execution reported an error. See run details.",
        ),
        ("failed", None, ""),
        ("blocked", Some("  "), ""),
        (
            "failed",
            Some("agent exited with 1 private"),
            " · Agent process exited unsuccessfully.",
        ),
        (
            "blocked",
            Some("merge and source branch cleanup were not confirmed private"),
            " · Merge or branch cleanup was not confirmed.",
        ),
        (
            "failed",
            Some("publication was not confirmed private"),
            " · Version publication was not confirmed.",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let number = i as u64 + 1;
        let id = queued(&f, number, "Outcome");
        f.set_run_status(&id, "running", None)?;
        sink.drain();
        f.set_run_status(&id, state, error.map(String::from))?;
        let messages = sink.drain();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].title, format!("AI Factory — Run {state}"));
        assert_eq!(
            messages[0].body,
            format!("acme/repo #{number} · Outcome{suffix}")
        );
        f.set_run_status(&id, state, Some("changed error".into()))?;
        assert!(sink.drain().is_empty());
    }
    Ok(())
}
#[test]
fn notification_retry_has_independent_identity() -> Result<()> {
    let (_dir, f, sink) = setup();
    let first = queued(&f, 11, "Retry");
    f.set_run_status(&first, "running", None)?;
    f.set_run_status(&first, "failed", Some("timeout".into()))?;
    let second = f.retry(&first)?;
    assert_ne!(first, second);
    assert_eq!(
        sink.drain()
            .iter()
            .map(|m| (m.run_id.as_str(), m.status.as_str(), m.attempt))
            .collect::<Vec<_>>(),
        vec![
            (first.as_str(), "running", 1),
            (first.as_str(), "failed", 1)
        ]
    );
    f.set_run_status(&second, "running", None)?;
    f.set_run_status(&second, "succeeded", None)?;
    let messages = sink.drain();
    assert_eq!(
        messages
            .iter()
            .map(|m| (m.run_id.as_str(), m.status.as_str(), m.attempt))
            .collect::<Vec<_>>(),
        vec![
            (second.as_str(), "running", 2),
            (second.as_str(), "succeeded", 2)
        ]
    );
    assert_eq!(messages[1].body, "acme/repo #11 · Retry");
    Ok(())
}
#[test]
fn notification_snapshots_and_subscriptions_do_not_replay() -> Result<()> {
    let (_dir, f, sink) = setup();
    let id = queued(&f, 11, "History");
    f.set_run_status(&id, "running", None)?;
    f.set_run_status(&id, "succeeded", None)?;
    sink.drain();
    for _ in 0..3 {
        let _events = f.subscribe();
        f.snapshot()?;
        f.run_detail(&id)?;
    }
    assert!(sink.drain().is_empty());
    Ok(())
}
#[test]
fn notification_restart_does_not_replay_history() -> Result<()> {
    let (dir, f, sink) = setup();
    let id = queued(&f, 11, "History");
    f.set_run_status(&id, "running", None)?;
    f.set_run_status(&id, "succeeded", None)?;
    let active = queued(&f, 12, "Interrupted");
    f.set_run_status(&active, "running", None)?;
    drop(f);
    let f = common::open_with_api(dir.path().into(), "http://127.0.0.1:9")?;
    let fresh = Arc::new(RecordingNotifications::default());
    f.set_run_notification_sink(fresh.clone());
    assert_eq!(f.run_detail(&active)?.run.status, "interrupted");
    assert_eq!(f.run_detail(&id)?.run.status, "succeeded");
    assert!(fresh.drain().is_empty());
    let new = queued(&f, 13, "Fresh");
    f.set_run_status(&new, "running", None)?;
    assert_eq!(fresh.drain()[0].run_id, new);
    assert_eq!(sink.drain().len(), 3);
    Ok(())
}
#[test]
fn notification_simultaneous_runs_keep_their_metadata() -> Result<()> {
    let (_dir, f, sink) = setup();
    let a = queued(&f, 21, "Alpha");
    let b = queued(&f, 22, "Beta");
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [
        (a.clone(), "failed", "timeout"),
        (b.clone(), "blocked", "private"),
    ]
    .into_iter()
    .map(|(id, state, error)| {
        let (f, barrier) = (f.clone(), barrier.clone());
        std::thread::spawn(move || {
            barrier.wait();
            f.set_run_status(&id, "running", None).unwrap();
            f.set_run_status(&id, state, Some(error.into())).unwrap();
        })
    })
    .collect();
    for t in threads {
        t.join().unwrap();
    }
    let all = sink.drain();
    assert_eq!(all.len(), 4);
    for (id, body, state, suffix) in [
        (a, "acme/repo #21 · Alpha", "failed", "Execution timed out."),
        (
            b,
            "acme/repo #22 · Beta",
            "blocked",
            "Execution reported an error. See run details.",
        ),
    ] {
        let messages: Vec<_> = all.iter().filter(|m| m.run_id == id).collect();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].status, "running");
        assert_eq!(messages[0].body, body);
        assert_eq!(messages[1].status, state);
        assert_eq!(messages[1].body, format!("{body} · {suffix}"));
        assert_eq!(messages[1].attempt, f.run_detail(&id)?.run.attempt);
    }
    Ok(())
}
#[test]
fn notification_cached_credentials_are_redacted_without_keychain_io() -> Result<()> {
    let (dir, f, sink) = setup();
    let mut settings = f.settings()?;
    settings.token_profiles = vec![TokenProfile {
        id: "alice".into(),
        label: "Test".into(),
        account: "alice".into(),
        token: Some("PRIVATE_KNOWN_VALUE".into()),
        has_token: false,
        owners: vec!["acme".into()],
        authors: vec!["alice".into()],
        reviewers: vec![],
    }];
    common::bind_fixture_codex(&f, &mut settings);
    f.save_settings(settings)?;
    let id = queued(&f, 11, "PRIVATE_KNOWN_VALUE title");
    let credentials = common::backend(dir.path());
    credentials
        .fail_get
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let gets = credentials.gets.lock().unwrap().clone();
    f.set_run_status(&id, "running", None)?;
    let messages = sink.drain();
    assert_eq!(messages.len(), 1);
    assert!(!messages[0].body.contains("PRIVATE_KNOWN_VALUE"));
    assert_eq!(*credentials.gets.lock().unwrap(), gets);
    Ok(())
}
#[test]
fn notification_hides_telegram_shaped_tokens_before_truncation() {
    let (_dir, f, sink) = setup();
    for (n, title) in [
        "prefix 123456:abcdefghijklmnopqrstuvwxyz_0123456789".to_owned(),
        format!(
            "{}123456:abcdefghijklmnopqrstuvwxyz_0123456789",
            "x".repeat(110)
        ),
        "123456:abc\u{202e}defghijklmnopqrstuvwxyz_0123456789".into(),
    ]
    .iter()
    .enumerate()
    {
        let id = queued(&f, n as u64 + 100, title);
        f.set_run_status(&id, "running", None).unwrap();
        assert_eq!(
            sink.drain()[0].body,
            format!("acme/repo #{} · [Title hidden]", n + 100)
        );
    }
}
#[test]
fn notification_hides_credentials_obfuscated_by_directional_marks() {
    let (_dir, f, sink) = setup();
    let id = queued(
        &f,
        200,
        "123456:abc\u{200f}def\u{061c}ghijklmnopqrstuvwxyz_0123456789",
    );
    f.set_run_status(&id, "running", None).unwrap();
    assert_eq!(sink.drain()[0].body, "acme/repo #200 · [Title hidden]");
}
