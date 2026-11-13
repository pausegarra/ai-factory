use super::*;
use std::sync::Barrier;

struct NoCredentials;

impl Credentials for NoCredentials {
    fn get(&self, _: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn set(&self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }

    fn delete(&self, _: &str) -> Result<()> {
        Ok(())
    }
}

fn setup() -> (tempfile::TempDir, Arc<Factory>) {
    let dir = tempfile::tempdir().unwrap();
    let factory = Factory::open_with_credentials(
        dir.path().into(),
        "http://127.0.0.1:9",
        Arc::new(NoCredentials),
    )
    .unwrap();
    (dir, factory)
}

fn event(trigger: &str, repository: &str, actor: &str, number: u64) -> TriggerEvent {
    let mut event = TriggerEvent::issue(repository, number, "[AI] fixture", actor, "");
    event.trigger_id = trigger.into();
    event.event_id = format!("{trigger}:{repository}#{number}");
    if trigger != "new_issue" {
        event.head_sha = "a".repeat(40);
        event.url = format!("https://github.com/{repository}/pull/{number}");
    }
    event
}

fn queued(factory: &Factory, trigger: &str, number: u64) -> String {
    factory
        .enqueue(event(trigger, "acme/repo", "alice", number))
        .unwrap()
        .unwrap()
}

fn release_for_test(factory: &Factory, id: &str) {
    let _gate = factory.lifecycle_gate.lock().unwrap();
    assert!(factory.active.lock().unwrap().remove(id).is_some());
    factory.wake.notify_one();
}

#[test]
fn approval_reserves_before_running_and_keeps_global_capacity_shared() {
    let (_dir, factory) = setup();
    let first = queued(&factory, "pr_approved", 1);
    assert!(factory.register_queued_worker(&first, 4).unwrap().is_some());

    let mut settings = factory.settings().unwrap();
    let flow = settings
        .flows
        .iter_mut()
        .find(|flow| flow.id == "pr_approved")
        .unwrap();
    flow.name = "Custom approval workflow".into();
    flow.nodes[0].agent = "opencode".into();
    flow.nodes[0].access = "read".into();
    factory.save_settings(settings).unwrap();

    let second = factory
        .enqueue(event("pr_approved", "other/repo", "bob", 2))
        .unwrap()
        .unwrap();
    assert_eq!(factory.run_detail(&first).unwrap().run.status, "queued");
    assert!(factory
        .register_queued_worker(&second, 4)
        .unwrap()
        .is_none());

    for (number, trigger) in [(3, "new_issue"), (4, "pr_review"), (5, "new_issue")] {
        let id = queued(&factory, trigger, number);
        assert!(factory.register_queued_worker(&id, 4).unwrap().is_some());
    }
    assert_eq!(factory.active.lock().unwrap().len(), 4);

    let beyond_capacity = queued(&factory, "new_issue", 6);
    assert!(factory
        .register_queued_worker(&beyond_capacity, 4)
        .unwrap()
        .is_none());
    assert!(factory
        .register_queued_worker(&second, 16)
        .unwrap()
        .is_none());
    let detail = factory.run_detail(&second).unwrap();
    assert!(detail.run.started_at.is_none());
    assert!(detail.run.workspace.is_none());
    assert!(detail.nodes.iter().all(|node| node.status == "pending"));
}

#[test]
fn simultaneous_approval_registration_grants_exactly_one_reservation() {
    let (_dir, factory) = setup();
    let ids = [
        queued(&factory, "pr_approved", 1),
        queued(&factory, "pr_approved", 2),
    ];
    let barrier = Barrier::new(3);
    let granted = std::thread::scope(|scope| {
        let handles: Vec<_> = ids
            .iter()
            .map(|id| {
                let factory = &factory;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    factory.register_queued_worker(id, 4).unwrap().is_some()
                })
            })
            .collect();
        barrier.wait();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|granted| *granted)
            .count()
    });

    assert_eq!(granted, 1);
    let active = factory.active.lock().unwrap();
    assert_eq!(active.len(), 1);
    let rejected = ids.iter().find(|id| !active.contains_key(*id)).unwrap();
    drop(active);
    assert!(factory
        .register_queued_worker(rejected, 4)
        .unwrap()
        .is_none());
    assert!(ids
        .iter()
        .all(|id| factory.run_detail(id).unwrap().run.status == "queued"));
}

#[test]
fn scheduler_scan_does_not_overtake_an_older_deferred_approval() {
    let (_dir, factory) = setup();
    let active = queued(&factory, "pr_approved", 1);
    let older = queued(&factory, "pr_approved", 2);
    let other_trigger = queued(&factory, "new_issue", 3);
    let newer = queued(&factory, "pr_approved", 3);
    assert!(factory
        .register_queued_worker(&active, 4)
        .unwrap()
        .is_some());
    assert!(factory.store.claim_queued(&active).unwrap());

    for (id, created_at) in [
        (&older, "2026-11-12T10:00:00Z"),
        (&other_trigger, "2026-11-12T10:00:30Z"),
        (&newer, "2026-11-12T10:01:00Z"),
    ] {
        factory
            .store
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE runs SET created_at=?1,json=json_set(json,'$.created_at',?1) WHERE id=?2",
                rusqlite::params![created_at, id],
            )
            .unwrap();
    }
    let queued_order: Vec<_> = factory
        .store
        .runs()
        .unwrap()
        .into_iter()
        .rev()
        .filter(|run| run.status == "queued")
        .map(|run| run.id)
        .collect();
    assert_eq!(
        queued_order,
        vec![older.clone(), other_trigger.clone(), newer.clone()]
    );

    let older_rejected = Barrier::new(2);
    let continue_scan = Barrier::new(2);
    let (other_reserved, newer_reserved) = std::thread::scope(|scope| {
        let scan = scope.spawn(|| {
            let mut scan = QueuedWorkerScan::default();
            assert!(scan
                .reserve(&factory, &older, "pr_approved", 4)
                .unwrap()
                .is_none());
            older_rejected.wait();
            continue_scan.wait();
            let other_reserved = scan
                .reserve(&factory, &other_trigger, "new_issue", 4)
                .unwrap()
                .is_some();
            let newer_reserved = scan
                .reserve(&factory, &newer, "pr_approved", 4)
                .unwrap()
                .is_some();
            (other_reserved, newer_reserved)
        });

        older_rejected.wait();
        release_for_test(&factory, &active);
        continue_scan.wait();
        scan.join().unwrap()
    });

    assert!(
        other_reserved,
        "deferred approvals must not block other triggers"
    );
    assert!(
        !newer_reserved,
        "a newer approval must not take the slot released mid-scan"
    );
    release_for_test(&factory, &other_trigger);

    let mut next_scan = QueuedWorkerScan::default();
    assert!(next_scan
        .reserve(&factory, &older, "pr_approved", 4)
        .unwrap()
        .is_some());
    assert!(next_scan
        .reserve(&factory, &newer, "pr_approved", 4)
        .unwrap()
        .is_none());
}

#[test]
fn global_capacity_one_and_reduction_do_not_cancel_existing_reservations() {
    let (_dir, factory) = setup();
    let approval = queued(&factory, "pr_approved", 1);
    let issue = queued(&factory, "new_issue", 2);
    let token = factory
        .register_queued_worker(&approval, 1)
        .unwrap()
        .unwrap();
    assert!(factory.register_queued_worker(&issue, 1).unwrap().is_none());
    assert!(factory.register_queued_worker(&issue, 4).unwrap().is_some());

    let review = queued(&factory, "pr_review", 3);
    assert!(factory
        .register_queued_worker(&review, 1)
        .unwrap()
        .is_none());
    assert_eq!(factory.active.lock().unwrap().len(), 2);
    assert!(!token.is_cancelled());
}

#[test]
fn terminal_status_and_retry_do_not_release_a_live_reservation() {
    for status in ["succeeded", "failed", "blocked", "cancelled", "interrupted"] {
        let (_dir, factory) = setup();
        let active = queued(&factory, "pr_approved", 1);
        let queued_approval = queued(&factory, "pr_approved", 2);
        let token = factory.register_queued_worker(&active, 4).unwrap().unwrap();
        if status == "cancelled" {
            factory.cancel(&active).unwrap();
            assert!(token.is_cancelled());
        } else {
            factory.set_run_status(&active, status, None).unwrap();
        }

        let retry = factory.retry(&active).unwrap();
        assert!(factory
            .register_queued_worker(&queued_approval, 4)
            .unwrap()
            .is_none());
        assert!(factory.register_queued_worker(&retry, 4).unwrap().is_none());
        factory.cancel(&queued_approval).unwrap();
        assert_eq!(factory.active.lock().unwrap().len(), 1);

        release_for_test(&factory, &active);
        assert!(factory
            .register_queued_worker(&queued_approval, 4)
            .unwrap()
            .is_none());
        assert!(factory.register_queued_worker(&retry, 4).unwrap().is_some());

        let mut settings = factory.settings().unwrap();
        settings
            .flows
            .iter_mut()
            .find(|flow| flow.id == "pr_approved")
            .unwrap()
            .enabled = false;
        factory.save_settings(settings).unwrap();
        assert!(factory.retry(&active).is_err());
    }
}

struct GatedCleanup {
    id: String,
    entered: Notify,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    first: AtomicBool,
    fail: bool,
}

impl RunCleanup for GatedCleanup {
    fn remove(&self, _: &str, id: &str, _: &CleanupTarget) -> Result<()> {
        if id != self.id {
            return Ok(());
        }
        if !self.first.swap(true, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.lock().unwrap().recv()?;
        }
        anyhow::ensure!(!self.fail, "fixture cleanup failure");
        Ok(())
    }
}

async fn wait_until(factory: &Factory, id: &str, status: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if factory.run_detail(id).unwrap().run.status == status {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_worker_holds_approval_until_cleanup_returns_even_on_cleanup_error() {
    for fail in [false, true] {
        let (_dir, mut factory) = setup();
        assert!(factory.settings().unwrap().token_profiles.is_empty());
        let active = queued(&factory, "pr_approved", 1);
        let (release, receive) = std::sync::mpsc::channel();
        let cleaner = Arc::new(GatedCleanup {
            id: active.clone(),
            entered: Notify::new(),
            release: Mutex::new(receive),
            first: AtomicBool::new(false),
            fail,
        });
        Arc::get_mut(&mut factory).unwrap().cleanup = cleaner.clone();
        factory.start();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            cleaner.entered.notified(),
        )
        .await
        .unwrap();

        let next = queued(&factory, "pr_approved", 2);
        factory.cancel(&active).unwrap();
        let retry = factory.retry(&active).unwrap();
        assert_eq!(factory.run_detail(&active).unwrap().run.status, "cancelled");
        assert!(factory.register_queued_worker(&next, 4).unwrap().is_none());
        assert!(factory.register_queued_worker(&retry, 4).unwrap().is_none());
        assert!(factory.active.lock().unwrap().contains_key(&active));
        factory.cancel(&retry).unwrap();

        release.send(()).unwrap();
        wait_until(&factory, &next, "failed").await;
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !factory.active.lock().unwrap().is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(factory.run_detail(&active).unwrap().run.status, "cancelled");
        if fail {
            assert!(factory.run_detail(&active).unwrap().run.error.is_some());
        }
        factory.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_persistence_error_does_not_leak_approval_reservation() {
    let (_dir, mut factory) = setup();
    let active = queued(&factory, "pr_approved", 1);
    let (release, receive) = std::sync::mpsc::channel();
    let cleaner = Arc::new(GatedCleanup {
        id: active.clone(),
        entered: Notify::new(),
        release: Mutex::new(receive),
        first: AtomicBool::new(false),
        fail: false,
    });
    Arc::get_mut(&mut factory).unwrap().cleanup = cleaner.clone();
    factory.start();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        cleaner.entered.notified(),
    )
    .await
    .unwrap();
    factory
        .store
        .db
        .lock()
        .unwrap()
        .execute_batch(&format!(
            "CREATE TRIGGER deny_terminal BEFORE UPDATE ON runs WHEN OLD.id='{active}' AND json_extract(NEW.json,'$.status') IN ('failed','cancelled') BEGIN SELECT RAISE(ABORT,'fixture terminal write denied'); END;"
        ))
        .unwrap();

    let next = queued(&factory, "pr_approved", 2);
    release.send(()).unwrap();
    wait_until(&factory, &next, "failed").await;
    assert!(!factory.active.lock().unwrap().contains_key(&active));
    assert_eq!(factory.run_detail(&active).unwrap().run.status, "running");
    factory
        .store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER deny_terminal")
        .unwrap();
    factory.shutdown().await;
}

#[tokio::test]
async fn shutdown_prevents_new_reservations_after_existing_worker_drains() {
    let (_dir, factory) = setup();
    let active = queued(&factory, "pr_approved", 1);
    let next = queued(&factory, "pr_approved", 2);
    let token = factory.register_queued_worker(&active, 4).unwrap().unwrap();
    let stopping = factory.clone();
    let task = tokio::spawn(async move { stopping.shutdown().await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !factory.stop.is_cancelled() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(token.is_cancelled());
    assert!(factory.register_queued_worker(&next, 4).unwrap().is_none());
    release_for_test(&factory, &active);
    task.await.unwrap();
    assert!(factory.stop.is_cancelled());
    assert!(factory.register_queued_worker(&next, 4).unwrap().is_none());
    assert_eq!(factory.run_detail(&next).unwrap().run.status, "queued");
}

#[tokio::test]
async fn occupied_lifecycle_gate_rejects_registration_without_waiting() {
    let (_dir, factory) = setup();
    let id = queued(&factory, "pr_approved", 1);
    let gate = factory.lifecycle_gate.lock().unwrap();
    let registered = std::thread::scope(|scope| {
        let worker = scope.spawn(|| factory.register_queued_worker(&id, 4).unwrap().is_some());
        worker.join().unwrap()
    });
    drop(gate);
    assert!(!registered);
    factory.shutdown().await;
    assert!(factory.register_queued_worker(&id, 4).unwrap().is_none());
}

#[test]
fn reopening_keeps_queued_snapshots_order_and_consumed_approval_signals() {
    let (dir, factory) = setup();
    let active = queued(&factory, "pr_approved", 1);
    assert!(factory
        .register_queued_worker(&active, 4)
        .unwrap()
        .is_some());
    assert!(factory.store.claim_queued(&active).unwrap());

    let approval_event = event("pr_approved", "acme/repo", "alice", 2);
    let first_queued = factory
        .enqueue_approval(approval_event.clone(), &[700], true)
        .unwrap()
        .unwrap();
    let second_queued = queued(&factory, "pr_approved", 3);
    let saved_flow = serde_json::to_value(factory.run_detail(&first_queued).unwrap().flow).unwrap();
    let old_attempt = queued(&factory, "pr_approved", 4);
    factory.cancel(&old_attempt).unwrap();
    let retry = factory.retry(&old_attempt).unwrap();

    for id in [&first_queued, &second_queued] {
        factory
            .store
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE runs SET created_at=?1,json=json_set(json,'$.created_at',?1) WHERE id=?2",
                rusqlite::params!["2026-11-12T10:00:00Z", id],
            )
            .unwrap();
    }
    factory
        .store
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE runs SET created_at=?1,json=json_set(json,'$.created_at',?1) WHERE id=?2",
            rusqlite::params!["2026-11-12T10:01:00Z", &retry],
        )
        .unwrap();
    let queued_snapshot = serde_json::to_value(factory.run_detail(&first_queued).unwrap()).unwrap();
    drop(factory);

    let reopened = Factory::open_with_credentials(
        dir.path().into(),
        "http://127.0.0.1:9",
        Arc::new(NoCredentials),
    )
    .unwrap();
    assert_eq!(
        reopened.run_detail(&active).unwrap().run.status,
        "interrupted"
    );
    assert!(reopened.active.lock().unwrap().is_empty());
    assert_eq!(
        serde_json::to_value(reopened.run_detail(&first_queued).unwrap()).unwrap(),
        queued_snapshot
    );
    assert_eq!(
        reopened.run_detail(&first_queued).unwrap().event.head_sha,
        approval_event.head_sha
    );
    assert_eq!(
        serde_json::to_value(reopened.run_detail(&first_queued).unwrap().flow).unwrap(),
        saved_flow
    );

    let order: Vec<_> = reopened
        .store
        .runs()
        .unwrap()
        .into_iter()
        .rev()
        .filter(|run| run.status == "queued")
        .map(|run| run.id)
        .collect();
    let mut tied = vec![first_queued.clone(), second_queued.clone()];
    tied.sort();
    tied.push(retry.clone());
    assert_eq!(order, tied);

    assert!(reopened
        .enqueue_approval(approval_event.clone(), &[700], true)
        .unwrap()
        .is_none());
    let mut changed_head = approval_event;
    changed_head.event_id.push_str(":changed-head");
    changed_head.head_sha = "b".repeat(40);
    assert!(reopened
        .enqueue_approval(changed_head, &[700], false)
        .unwrap()
        .is_none());
    assert!(reopened
        .register_queued_worker(&tied[0], 4)
        .unwrap()
        .is_some());
    assert!(reopened
        .register_queued_worker(&tied[1], 4)
        .unwrap()
        .is_none());
}

#[derive(Default)]
struct NotificationRecorder(Mutex<Vec<RunNotification>>);
impl RunNotificationSink for NotificationRecorder {
    fn enqueue(&self, message: RunNotification) {
        self.0.lock().unwrap().push(message);
    }
}
#[test]
fn notification_commit_failure_and_ui_events_do_not_emit() {
    let (_dir, f) = setup();
    let sink = Arc::new(NotificationRecorder::default());
    f.set_run_notification_sink(sink.clone());
    let id = queued(&f, "new_issue", 11);
    f.store.db.lock().unwrap().execute_batch("CREATE TEMP TRIGGER reject_notification BEFORE UPDATE ON runs BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
    assert!(f.set_run_status(&id, "running", None).is_err());
    assert!(f.claim_run_for_execution(&id).is_err());
    assert_eq!(f.run_detail(&id).unwrap().run.status, "queued");
    assert!(sink.0.lock().unwrap().is_empty());
    f.store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_notification")
        .unwrap();
    assert!(f.claim_run_for_execution(&id).unwrap());
    assert!(!f.claim_run_for_execution(&id).unwrap());
    assert_eq!(sink.0.lock().unwrap().len(), 1);
    sink.0.lock().unwrap().clear();
    for kind in [
        "resync",
        "run_changed",
        "node_changed",
        "logs_changed",
        "run_created",
    ] {
        f.emit(kind, Some(&id), None);
    }
    assert!(sink.0.lock().unwrap().is_empty());
}

#[test]
fn notification_committed_result_precedes_node_write_failure() {
    let (_dir, f) = setup();
    let sink = Arc::new(NotificationRecorder::default());
    f.set_run_notification_sink(sink.clone());
    let id = queued(&f, "new_issue", 11);
    f.store.db.lock().unwrap().execute_batch("CREATE TEMP TRIGGER reject_node BEFORE UPDATE ON nodes BEGIN SELECT RAISE(ABORT, 'fixture node failure'); END;").unwrap();
    assert!(f
        .set_run_status(&id, "failed", Some("timeout".into()))
        .is_err());
    assert_eq!(f.run_detail(&id).unwrap().run.status, "failed");
    let messages = sink.0.lock().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].status, "failed");
    assert_eq!(
        messages[0].body,
        "acme/repo #11 · [AI] fixture · Execution timed out."
    );
    drop(messages);
    f.store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_node")
        .unwrap();
    f.set_run_status(&id, "failed", None).unwrap();
    assert_eq!(sink.0.lock().unwrap().len(), 1);
}
