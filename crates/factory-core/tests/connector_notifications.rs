mod common;
use factory_core::*;
use serde_json::json;
use std::sync::Arc;
fn connector(id: &str, chat: &str, enabled: bool, events: serde_json::Value) -> serde_json::Value {
    json!({"id":id,"label":id,"enabled":enabled,"events":events,"type":"telegram","config":{"chat_id":chat,"bot_token":"123456:fixture_token_abcdefghijklmnopqrstuvwxyz"}})
}
fn configure(f: &Factory, connectors: serde_json::Value) {
    let mut v = serde_json::to_value(f.settings().unwrap()).unwrap();
    v["notification_connectors"] = connectors;
    f.save_settings(serde_json::from_value(v).unwrap()).unwrap();
}
fn enqueue(f: &Factory, n: u64) -> String {
    f.enqueue(TriggerEvent::issue(
        "acme/repo",
        n,
        "Safe title",
        "alice",
        "",
    ))
    .unwrap()
    .unwrap()
}
fn open(dir: &std::path::Path, t: Arc<common::notifications::RecordingTransport>) -> Arc<Factory> {
    Factory::open_with_notification_transport(
        dir.into(),
        "http://127.0.0.1:9",
        Arc::new(common::MemoryCredentials::default()),
        t,
    )
    .unwrap()
}

async fn drop_factory_after_readers_drain(mut factory: Arc<Factory>) {
    // Shutdown cancels delivery, but OS reads already on the blocking pool may
    // retain Factory. Drop its last owner here so factory.lock is released before reopen.
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match Arc::try_unwrap(factory) {
                Ok(factory) => break drop(factory),
                Err(shared) => factory = shared,
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Factory readers drained before restart");
}

#[tokio::test]
async fn connector_restart_waits_for_last_factory_reader_before_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let transport = Arc::new(common::notifications::RecordingTransport::default());
    let factory = open(dir.path(), transport.clone());
    let reader = factory.clone();
    factory.shutdown().await;
    let mut closing = Box::pin(drop_factory_after_readers_drain(factory));
    let waits_for_reader = std::future::poll_fn(|cx| {
        std::task::Poll::Ready(std::future::Future::poll(closing.as_mut(), cx).is_pending())
    })
    .await;
    drop(reader);
    if waits_for_reader {
        closing.await;
    }
    assert!(
        waits_for_reader,
        "reopening must wait for outstanding Factory readers"
    );
    let reopened = open(dir.path(), transport);
    reopened.shutdown().await;
}

#[tokio::test]
async fn connector_transition_filters_and_preserves_native_sink() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    let f = open(dir.path(), t.clone());
    configure(
        &f,
        json!([
            connector("a", "a", true, json!(["run_failed"])),
            connector("b", "b", false, json!(["run_started", "run_failed"]))
        ]),
    );
    let native = Arc::new(common::notifications::RecordingNotifications::default());
    f.set_run_notification_sink(native.clone());
    f.start_notification_delivery();
    let id = enqueue(&f, 18);
    f.set_run_status(&id, "running", None).unwrap();
    f.set_run_status(&id, "running", None).unwrap();
    f.set_run_status(&id, "failed", Some("timeout PRIVATE_AGENT_TEXT".into()))
        .unwrap();
    f.set_run_status(&id, "failed", Some("late PRIVATE_AGENT_TEXT".into()))
        .unwrap();
    t.wait_for_calls(1).await;
    f.shutdown().await;
    assert_eq!(t.chats(), vec!["a"]);
    assert_eq!(
        t.messages()[0].body,
        "acme/repo #18 · Safe title · Execution timed out."
    );
    assert_eq!(
        native
            .drain()
            .iter()
            .map(|m| m.status.as_str())
            .collect::<Vec<_>>(),
        vec!["running", "failed"]
    );
    assert_eq!(f.run_detail(&id).unwrap().run.status, "failed");
}
#[tokio::test]
async fn connector_all_events_are_filtered_and_non_events_are_silent() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    let f = open(dir.path(), t.clone());
    configure(
        &f,
        json!([connector(
            "a",
            "a",
            true,
            json!(["run_started", "run_succeeded", "run_failed", "run_blocked"])
        )]),
    );
    f.start_notification_delivery();
    f.start_notification_delivery();
    let id = enqueue(&f, 1);
    f.set_run_status(&id, "running", None).unwrap();
    f.set_run_status(&id, "succeeded", None).unwrap();
    for (n, status) in [
        (2, "failed"),
        (3, "blocked"),
        (4, "cancelled"),
        (5, "interrupted"),
    ] {
        let id = enqueue(&f, n);
        f.set_run_status(&id, status, None).unwrap();
    }
    t.wait_for_calls(4).await;
    f.shutdown().await;
    assert_eq!(
        t.messages()
            .iter()
            .map(|m| m.status.as_str())
            .collect::<Vec<_>>(),
        vec!["running", "succeeded", "failed", "blocked"]
    );
}
#[tokio::test]
async fn connector_slow_queue_does_not_block_other_connector_or_run_transitions() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    *t.hold_chat.lock().unwrap() = Some("slow".into());
    let f = open(dir.path(), t.clone());
    configure(
        &f,
        json!([
            connector("a", "slow", true, json!(["run_failed"])),
            connector("b", "fast", true, json!(["run_failed"]))
        ]),
    );
    f.start_notification_delivery();
    let id = enqueue(&f, 1);
    f.set_run_status(&id, "failed", None).unwrap();
    t.wait_for_calls(2).await;
    for n in 2..72 {
        let id = enqueue(&f, n);
        f.set_run_status(&id, "failed", None).unwrap();
        tokio::task::yield_now().await;
    }
    // Fast consumer must reach the last run despite a full independent slow queue.
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let changed = t.changed.notified();
            if t.messages().iter().any(|m| m.body.contains("#71 ")) {
                break;
            }
            changed.await;
        }
    })
    .await
    .unwrap();
    f.shutdown().await;
    assert_eq!(t.chats().iter().filter(|c| c.as_str() == "slow").count(), 1);
    assert!(f.snapshot().unwrap().service.last_error.is_none());
}
#[tokio::test]
async fn connector_snapshots_destination_and_preferences_and_never_replays() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    let f = open(dir.path(), t.clone());
    configure(
        &f,
        json!([connector("a", "old", true, json!(["run_started"]))]),
    );
    let historic = enqueue(&f, 1);
    f.set_run_status(&historic, "succeeded", None).unwrap();
    f.start_notification_delivery();
    let id = enqueue(&f, 2);
    f.set_run_status(&id, "running", None).unwrap();
    t.wait_for_calls(1).await;
    let mut settings = f.settings().unwrap();
    settings.notification_connectors[0].events = vec![NotificationEvent::RunSucceeded];
    if let ConnectorProvider::Telegram(c) = &mut settings.notification_connectors[0].provider {
        c.chat_id = "new".into();
    }
    f.save_settings(settings).unwrap();
    f.set_run_status(&id, "succeeded", None).unwrap();
    t.wait_for_calls(2).await;
    let mut settings = f.settings().unwrap();
    settings.notification_connectors[0].enabled = false;
    f.save_settings(settings).unwrap();
    let id = enqueue(&f, 3);
    f.set_run_status(&id, "succeeded", None).unwrap();
    f.shutdown().await;
    assert_eq!(t.chats(), vec!["old", "new"]);
}
#[tokio::test]
async fn connector_http_failure_does_not_change_run_or_other_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    let f = open(dir.path(), t.clone());
    configure(
        &f,
        json!([
            connector("a", "fail", true, json!(["run_failed"])),
            connector("b", "ok", true, json!(["run_failed"]))
        ]),
    );
    let native = Arc::new(common::notifications::RecordingNotifications::default());
    f.set_run_notification_sink(native.clone());
    f.start_notification_delivery();
    let id = enqueue(&f, 1);
    f.set_run_status(&id, "failed", None).unwrap();
    t.wait_for_calls(2).await;
    f.shutdown().await;
    assert_eq!(f.run_detail(&id).unwrap().run.status, "failed");
    assert_eq!(native.drain().len(), 1);
    assert!(f.snapshot().unwrap().service.last_error.is_none());
}
#[tokio::test]
async fn connector_pending_jobs_keep_transition_destination_when_preferences_change() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    *t.hold_chat.lock().unwrap() = Some("old".into());
    let f = open(dir.path(), t.clone());
    configure(
        &f,
        json!([connector("a", "old", true, json!(["run_failed"]))]),
    );
    f.start_notification_delivery();
    let first = enqueue(&f, 1);
    f.set_run_status(&first, "failed", None).unwrap();
    t.wait_for_calls(1).await;
    let second = enqueue(&f, 2);
    f.set_run_status(&second, "failed", None).unwrap();
    let mut settings = f.settings().unwrap();
    if let ConnectorProvider::Telegram(c) = &mut settings.notification_connectors[0].provider {
        c.chat_id = "new".into();
    }
    f.save_settings(settings).unwrap();
    let third = enqueue(&f, 3);
    f.set_run_status(&third, "failed", None).unwrap();
    let mut settings = f.settings().unwrap();
    settings.notification_connectors[0].enabled = false;
    f.save_settings(settings).unwrap();
    *t.hold_chat.lock().unwrap() = None;
    t.release.add_permits(1);
    t.wait_for_calls(3).await;
    f.shutdown().await;
    assert_eq!(t.chats(), vec!["old", "old", "new"]);
    assert!(t.messages()[1].body.contains("#2 "));
    assert!(t.messages()[2].body.contains("#3 "));
}
#[tokio::test]
async fn connector_rotation_invalidates_queued_credentials_instead_of_reusing_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    *t.hold_chat.lock().unwrap() = Some("held".into());
    let f = open(dir.path(), t.clone());
    configure(
        &f,
        json!([connector("a", "held", true, json!(["run_failed"]))]),
    );
    f.start_notification_delivery();
    let id = enqueue(&f, 1);
    f.set_run_status(&id, "failed", None).unwrap();
    t.wait_for_calls(1).await;
    let id = enqueue(&f, 2);
    f.set_run_status(&id, "failed", None).unwrap();
    configure(
        &f,
        json!([connector("a", "replacement", true, json!(["run_failed"]))]),
    );
    let id = enqueue(&f, 3);
    f.set_run_status(&id, "failed", None).unwrap();
    *t.hold_chat.lock().unwrap() = None;
    t.release.add_permits(1);
    t.wait_for_calls(2).await;
    f.shutdown().await;
    assert_eq!(t.chats(), vec!["held", "replacement"]);
    assert!(t.messages()[1].body.contains("#3 "));
}
#[tokio::test]
async fn connector_removal_cancels_pending_work_and_allows_a_new_same_id_destination() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    *t.hold_chat.lock().unwrap() = Some("held".into());
    let f = open(dir.path(), t.clone());
    configure(
        &f,
        json!([connector("a", "held", true, json!(["run_failed"]))]),
    );
    f.start_notification_delivery();
    let id = enqueue(&f, 1);
    f.set_run_status(&id, "failed", None).unwrap();
    t.wait_for_calls(1).await;
    let id = enqueue(&f, 2);
    f.set_run_status(&id, "failed", None).unwrap();
    configure(&f, json!([]));
    configure(
        &f,
        json!([connector("a", "new", true, json!(["run_failed"]))]),
    );
    let id = enqueue(&f, 3);
    f.set_run_status(&id, "failed", None).unwrap();
    t.wait_for_calls(2).await;
    f.shutdown().await;
    assert_eq!(t.chats(), vec!["held", "new"]);
}
#[tokio::test]
async fn connector_missing_credential_isolated_from_native_other_connector_and_run() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    let c = Arc::new(common::MemoryCredentials::default());
    let f = Factory::open_with_notification_transport(
        dir.path().into(),
        "http://127.0.0.1:9",
        c.clone(),
        t.clone(),
    )
    .unwrap();
    let mut a = connector("a", "missing", true, json!(["run_failed"]));
    a["config"]["bot_token"] = json!("123456:missing_fixture");
    configure(
        &f,
        json!([a, connector("b", "ok", true, json!(["run_failed"]))]),
    );
    c.values
        .lock()
        .unwrap()
        .retain(|_, v| v != "123456:missing_fixture");
    let native = Arc::new(common::notifications::RecordingNotifications::default());
    f.set_run_notification_sink(native.clone());
    f.start_notification_delivery();
    let id = enqueue(&f, 1);
    f.set_run_status(&id, "failed", None).unwrap();
    t.wait_for_calls(1).await;
    f.shutdown().await;
    assert_eq!(t.chats(), vec!["ok"]);
    assert_eq!(native.drain().len(), 1);
    assert_eq!(f.run_detail(&id).unwrap().run.status, "failed");
    assert!(f.snapshot().unwrap().service.last_error.is_none());
}
#[tokio::test]
async fn connector_disabled_and_unknown_providers_do_not_read_credentials_or_deliver() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    let c = Arc::new(common::MemoryCredentials::default());
    let f = Factory::open_with_notification_transport(
        dir.path().into(),
        "http://127.0.0.1:9",
        c.clone(),
        t.clone(),
    )
    .unwrap();
    configure(
        &f,
        json!([connector("a","off",false,json!(["run_started","run_failed"])),{"id":"future","label":"Future","enabled":true,"events":["run_failed"],"type":"future","config":{}}]),
    );
    c.gets.lock().unwrap().clear();
    f.start_notification_delivery();
    let id = enqueue(&f, 1);
    f.set_run_status(&id, "running", None).unwrap();
    f.set_run_status(&id, "failed", None).unwrap();
    f.shutdown().await;
    assert!(c.gets.lock().unwrap().is_empty());
    assert!(t.messages().is_empty());
}
#[tokio::test]
async fn connector_retry_is_a_new_attempt_and_restart_does_not_replay_history() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    let c = Arc::new(common::MemoryCredentials::default());
    let f = Factory::open_with_notification_transport(
        dir.path().into(),
        "http://127.0.0.1:9",
        c.clone(),
        t.clone(),
    )
    .unwrap();
    configure(
        &f,
        json!([connector("a", "a", true, json!(["run_failed"]))]),
    );
    f.start_notification_delivery();
    let first = enqueue(&f, 1);
    f.set_run_status(&first, "failed", None).unwrap();
    t.wait_for_calls(1).await;
    let retry = f.retry(&first).unwrap();
    f.set_run_status(&retry, "failed", None).unwrap();
    t.wait_for_calls(2).await;
    f.shutdown().await;
    drop_factory_after_readers_drain(f).await;
    let f = Factory::open_with_notification_transport(
        dir.path().into(),
        "http://127.0.0.1:9",
        c,
        t.clone(),
    )
    .unwrap();
    f.start_notification_delivery();
    let _ = f.settings().unwrap();
    let _ = f.snapshot().unwrap();
    let fresh = enqueue(&f, 2);
    f.set_run_status(&fresh, "failed", None).unwrap();
    // The same connector consumes FIFO. Seeing this new transition proves that
    // its restarted consumer has processed any history incorrectly queued ahead
    // of it; shutting down first could silently discard that history.
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let changed = t.changed.notified();
            if t.messages().iter().any(|message| message.run_id == fresh) {
                break;
            }
            changed.await;
        }
    })
    .await
    .expect("restarted connector did not deliver the fresh transition");
    let messages = t.messages();
    assert_eq!(t.chats(), vec!["a", "a", "a"]);
    assert_eq!(
        messages
            .iter()
            .map(|m| (
                m.run_id.as_str(),
                m.attempt,
                m.status.as_str(),
                m.body.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            (first.as_str(), 1, "failed", "acme/repo #1 · Safe title"),
            (retry.as_str(), 2, "failed", "acme/repo #1 · Safe title"),
            (fresh.as_str(), 1, "failed", "acme/repo #2 · Safe title"),
        ]
    );
    f.shutdown().await;
}
struct BlockedCredentials {
    memory: common::MemoryCredentials,
    blocking: std::sync::atomic::AtomicBool,
    reads: std::sync::atomic::AtomicUsize,
    gate: (std::sync::Mutex<bool>, std::sync::Condvar),
    entered: tokio::sync::Notify,
    finished: tokio::sync::Notify,
}
impl Default for BlockedCredentials {
    fn default() -> Self {
        Self {
            memory: Default::default(),
            blocking: false.into(),
            reads: 0.into(),
            gate: (false.into(), std::sync::Condvar::new()),
            entered: tokio::sync::Notify::new(),
            finished: tokio::sync::Notify::new(),
        }
    }
}
impl Credentials for BlockedCredentials {
    fn get(&self, key: &str) -> factory_core::Result<Option<String>> {
        if self.blocking.load(std::sync::atomic::Ordering::SeqCst) {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.entered.notify_one();
            let (lock, condition) = &self.gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = condition.wait(open).unwrap();
            }
            self.finished.notify_one();
        }
        self.memory.get(key)
    }
    fn set(&self, key: &str, value: &str) -> factory_core::Result<()> {
        self.memory.set(key, value)
    }
    fn delete(&self, key: &str) -> factory_core::Result<()> {
        self.memory.delete(key)
    }
}
struct ReleaseCredential(Arc<BlockedCredentials>);
impl Drop for ReleaseCredential {
    fn drop(&mut self) {
        *self.0.gate.0.lock().unwrap() = true;
        self.0.gate.1.notify_all();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connector_slow_os_read_is_bounded_per_consumer_and_cannot_send_after_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    let c = Arc::new(BlockedCredentials::default());
    let release = ReleaseCredential(c.clone());
    let f = Factory::open_with_notification_transport(
        dir.path().into(),
        "http://127.0.0.1:9",
        c.clone(),
        t.clone(),
    )
    .unwrap();
    configure(
        &f,
        json!([connector("a", "a", true, json!(["run_failed"]))]),
    );
    let native = Arc::new(common::notifications::RecordingNotifications::default());
    f.set_run_notification_sink(native.clone());
    c.blocking.store(true, std::sync::atomic::Ordering::SeqCst);
    f.start_notification_delivery();
    let id = enqueue(&f, 1);
    f.set_run_status(&id, "failed", None).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), c.entered.notified())
        .await
        .unwrap();
    for n in 2..5 {
        let id = enqueue(&f, n);
        f.set_run_status(&id, "failed", None).unwrap();
    }
    // Cross the documented 3 s deadline: the blocked OS call must not spawn a new read.
    tokio::time::sleep(std::time::Duration::from_millis(3100)).await;
    assert_eq!(c.reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(native.drain().len(), 4);
    tokio::time::timeout(std::time::Duration::from_secs(1), f.shutdown())
        .await
        .unwrap();
    drop(release);
    tokio::time::timeout(std::time::Duration::from_secs(1), c.finished.notified())
        .await
        .unwrap();
    assert!(t.messages().is_empty());
}
#[tokio::test]
async fn connector_transition_from_synchronous_thread_uses_started_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    let f = open(dir.path(), t.clone());
    configure(
        &f,
        json!([connector("a", "a", true, json!(["run_failed"]))]),
    );
    f.start_notification_delivery();
    let id = enqueue(&f, 1);
    let caller = f.clone();
    std::thread::spawn(move || caller.set_run_status(&id, "failed", None).unwrap())
        .join()
        .unwrap();
    t.wait_for_calls(1).await;
    f.shutdown().await;
    assert_eq!(t.messages().len(), 1);
}
