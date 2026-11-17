mod common;
use factory_core::*;
use serde_json::json;
use std::sync::Arc;
fn connector(id: &str, chat: &str, enabled: bool, events: serde_json::Value) -> serde_json::Value {
    json!({"id":id,"label":id,"enabled":enabled,"events":events,"type":"telegram","config":{"chat_id":chat,"bot_token":"123456:fixture_token_abcdefghijklmnopqrstuvwxyz","triggers":["new_issue","pr_review"],"token_profile_ids":["work"]}})
}
fn configure(f: &Factory, connectors: serde_json::Value) {
    let mut v = serde_json::to_value(f.settings().unwrap()).unwrap();
    v["notification_connectors"] = connectors;
    if !v["source_connectors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == "work")
    {
        v["source_connectors"]
            .as_array_mut()
            .unwrap()
            .push(json!({"token":"fixture-source-only","owners":["acme"],"authors":["alice"],"reviewers":["alice"],"id":"work","label":"Work","type":"github_issues","account":"alice"}));
    }
    f.save_settings(serde_json::from_value(v).unwrap()).unwrap();
}
fn enqueue(f: &Factory, n: u64) -> String {
    let mut event = common::from_source(
        TriggerEvent::issue("acme/repo", n, "Safe title", "alice", ""),
        "alice",
    );
    event.origin_token_profile_id = Some("work".into());
    f.enqueue(event).unwrap().unwrap()
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
    let mut settings = f.settings().unwrap();
    settings.issue_node_comments_enabled = false;
    f.save_settings(settings).unwrap();
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
        "Trigger: New task · acme/repo #18 · Safe title · Execution timed out."
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
    let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM issue_node_comments", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        0
    );
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
        json!([{ "id":"gchat", "label":"Draft", "enabled":false, "events":["run_failed"], "type":"google_chat", "config":{} },connector("a","off",false,json!(["run_started","run_failed"])),{"id":"future","label":"Future","enabled":true,"events":["run_failed"],"type":"future","config":{}}]),
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
            (
                first.as_str(),
                1,
                "failed",
                "Trigger: New task · acme/repo #1 · Safe title"
            ),
            (
                retry.as_str(),
                2,
                "failed",
                "Trigger: New task · acme/repo #1 · Safe title"
            ),
            (
                fresh.as_str(),
                1,
                "failed",
                "Trigger: New task · acme/repo #2 · Safe title"
            ),
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

#[tokio::test]
async fn connectors_match_all_filters_and_notify_each_transition_once() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    let f = open(dir.path(), t.clone());
    let mut s = f.settings().unwrap();
    s.source_connectors.push(SourceConnector {
        token: Some("fixture-source-only".into()),
        owners: vec!["acme".into()],
        authors: vec!["alice".into()],
        reviewers: vec!["alice".into()],
        kind: factory_core::SourceKind::GithubIssues,
        id: "work".into(),
        account: "alice".into(),
        label: "Work".into(),
        ..Default::default()
    });
    s.source_connectors.push(SourceConnector {
        token: Some("fixture-source-only".into()),
        owners: vec!["acme".into()],
        authors: vec!["bob".into()],
        reviewers: vec!["bob".into()],
        kind: factory_core::SourceKind::GithubIssues,
        id: "other".into(),
        account: "bob".into(),
        label: "Other".into(),
        ..Default::default()
    });
    s.notification_connectors=serde_json::from_value(json!([
      {"id":"gchat","label":"Team","enabled":true,"events":["run_started","run_succeeded","run_failed","run_blocked"],"type":"google_chat","config":{"webhook_url":"https://chat.googleapis.com/v1/spaces/fixture/messages?key=PRIVATE_KEY&token=PRIVATE_TOKEN","triggers":["new_issue","pr_review"],"token_profile_ids":["work"]}},
      connector("tg","telegram",true,json!(["run_started","run_succeeded","run_failed","run_blocked"]))
    ])).unwrap();
    f.save_settings(s).unwrap();
    f.start_notification_delivery();
    let mut selected = Vec::new();
    for (n, trigger, origin, terminal) in [
        (1, "new_issue", Some("work"), "succeeded"),
        (2, "pr_review", Some("work"), "failed"),
        (3, "new_issue", Some("work"), "blocked"),
        (4, "new_issue", Some("other"), "failed"),
        (5, "pr_approved", Some("work"), "failed"),
        (6, "new_issue", None, "failed"),
    ] {
        let mut e = common::from_source(
            TriggerEvent::issue(
                "acme/repo",
                n,
                "PRIVATE_KEY PRIVATE_TOKEN",
                if origin == Some("other") {
                    "bob"
                } else {
                    "alice"
                },
                "",
            ),
            "alice",
        );
        e.trigger_id = trigger.into();
        e.event_id = format!("{trigger}:acme/repo#{n}");
        e.origin_token_profile_id = origin.map(str::to_owned);
        let id = if let Some(id) = f.enqueue(e.clone()).unwrap() {
            id
        } else {
            assert!(n >= 4);
            let store =
                factory_core::context::factory::infrastructure::repositories::store::Store::open(
                    dir.path(),
                )
                .unwrap();
            store
                .insert(
                    &e,
                    &f.settings()
                        .unwrap()
                        .flows
                        .iter()
                        .find(|flow| flow.id == trigger)
                        .unwrap()
                        .clone(),
                    1,
                    true,
                )
                .unwrap()
                .unwrap()
        };
        f.set_run_status(&id, "running", None).unwrap();
        f.set_run_status(&id, "running", None).unwrap();
        f.set_run_status(&id, terminal, None).unwrap();
        f.set_run_status(&id, terminal, None).unwrap();
        if n <= 3 {
            selected.push(id);
        }
    }
    // Each provider's FIFO control proves all preceding transitions were processed.
    let mut e = common::from_source(
        TriggerEvent::issue("acme/repo", 7, "Control", "alice", ""),
        "alice",
    );
    e.origin_token_profile_id = Some("work".into());
    let control = f.enqueue(e).unwrap().unwrap();
    f.set_run_status(&control, "failed", None).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let changed = t.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let chats = t.chats();
            let messages = t.messages();
            if ["google_chat", "telegram"].iter().all(|destination| {
                chats
                    .iter()
                    .zip(&messages)
                    .any(|(chat, m)| chat == destination && m.run_id == control)
            }) {
                break;
            }
            changed.await;
        }
    })
    .await
    .expect("both FIFO controls delivered");
    f.shutdown().await;
    let chats = t.chats();
    let messages = t.messages();
    assert_eq!(messages.len(), 14);
    for destination in ["google_chat", "telegram"] {
        let delivered: Vec<_> = messages
            .iter()
            .zip(&chats)
            .filter(|(_, c)| c.as_str() == destination)
            .map(|(m, _)| m)
            .collect();
        assert_eq!(delivered.len(), 7, "{destination}");
        for id in &selected {
            assert_eq!(
                delivered.iter().filter(|m| &m.run_id == id).count(),
                2,
                "{destination}"
            );
        }
        assert_eq!(delivered.iter().filter(|m| m.run_id == control).count(), 1);
        assert_eq!(delivered.last().unwrap().run_id, control);
    }
    assert!(messages
        .iter()
        .all(|m| !m.body.contains("PRIVATE_KEY") && !m.body.contains("PRIVATE_TOKEN")));
}

#[tokio::test]
async fn google_chat_revokes_held_http_and_queued_jobs_on_every_lifecycle_change() {
    for action in ["rotate", "disable", "remove", "shutdown"] {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(common::notifications::RecordingTransport::default());
        *t.hold_chat.lock().unwrap() = Some("google_chat".into());
        let f = open(dir.path(), t.clone());
        let mut s = f.settings().unwrap();
        s.source_connectors.push(SourceConnector {
            token: Some("fixture-source-only".into()),
            owners: vec!["acme".into()],
            authors: vec!["alice".into()],
            reviewers: vec!["alice".into()],
            kind: factory_core::SourceKind::GithubIssues,
            id: "work".into(),
            account: "alice".into(),
            label: "Work".into(),
            ..Default::default()
        });
        let connector:NotificationConnector=serde_json::from_value(json!({"id":"chat","label":"Chat","enabled":true,"events":["run_failed"],"type":"google_chat","config":{"webhook_url":"https://fixture.example/old?token=old_fixture","triggers":["new_issue"],"token_profile_ids":["work"]}})).unwrap();
        s.notification_connectors = vec![connector.clone()];
        f.save_settings(s).unwrap();
        f.start_notification_delivery();
        let emit = |n| {
            let mut e = common::from_source(
                TriggerEvent::issue("acme/repo", n, "Fixture", "alice", ""),
                "alice",
            );
            e.origin_token_profile_id = Some("work".into());
            let id = f.enqueue(e).unwrap().unwrap();
            f.set_run_status(&id, "failed", None).unwrap();
            id
        };
        let first = emit(1);
        t.wait_for_calls(1).await;
        let queued = emit(2);
        let mut s = f.settings().unwrap();
        match action {
            "rotate" => {
                if let ConnectorProvider::GoogleChat(config) =
                    &mut s.notification_connectors[0].provider
                {
                    config.webhook_url =
                        Some("https://fixture.example/new?token=new_fixture".into());
                }
                f.save_settings(s).unwrap();
            }
            "disable" => {
                s.notification_connectors[0].enabled = false;
                f.save_settings(s).unwrap();
            }
            "remove" => {
                s.notification_connectors.clear();
                f.save_settings(s).unwrap();
            }
            _ => {
                f.shutdown().await;
            }
        }
        t.wait_for_cancelled(&first).await;
        assert!(t.completed().is_empty());
        assert!(!t.messages().iter().any(|m| m.run_id == queued));
        if action != "shutdown" {
            if action == "disable" {
                let mut s = f.settings().unwrap();
                s.notification_connectors[0].enabled = true;
                f.save_settings(s).unwrap();
            }
            if action == "remove" {
                let mut s = f.settings().unwrap();
                s.notification_connectors.push(connector);
                f.save_settings(s).unwrap();
            }
            *t.hold_chat.lock().unwrap() = None;
            t.release.add_permits(1);
            let fresh = emit(3);
            t.wait_for_completed(&fresh).await;
            f.shutdown().await;
            assert_eq!(t.completed(), vec![fresh]);
            assert_eq!(t.messages().len(), 2);
        } else {
            assert_eq!(t.messages().len(), 1);
        }
    }
}

#[tokio::test]
async fn google_chat_queue_saturation_is_bounded_and_does_not_change_runs() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    *t.hold_chat.lock().unwrap() = Some("google_chat".into());
    let f = open(dir.path(), t.clone());
    let mut s = f.settings().unwrap();
    s.source_connectors.push(SourceConnector {
        token: Some("fixture-source-only".into()),
        owners: vec!["acme".into()],
        authors: vec!["alice".into()],
        reviewers: vec!["alice".into()],
        kind: factory_core::SourceKind::GithubIssues,
        id: "work".into(),
        account: "alice".into(),
        label: "Work".into(),
        ..Default::default()
    });
    s.notification_connectors=serde_json::from_value(json!([{"id":"chat","label":"Chat","enabled":true,"events":["run_failed"],"type":"google_chat","config":{"webhook_url":"https://fixture.example/messages?token=fixture_queue","triggers":["new_issue"],"token_profile_ids":["work"]}}])).unwrap();
    f.save_settings(s).unwrap();
    f.start_notification_delivery();
    let emit = |n| {
        let mut e = common::from_source(
            TriggerEvent::issue("acme/repo", n, "Fixture", "alice", ""),
            "alice",
        );
        e.origin_token_profile_id = Some("work".into());
        let id = f.enqueue(e).unwrap().unwrap();
        f.set_run_status(&id, "failed", None).unwrap();
        id
    };
    let first = emit(1);
    t.wait_for_calls(1).await;
    let mut accepted_last = String::new();
    let mut dropped = Vec::new();
    for n in 2..=70 {
        let id = emit(n);
        if n == 65 {
            accepted_last = id.clone();
        }
        if n > 65 {
            dropped.push(id);
        }
    }
    assert_eq!(t.messages().len(), 1);
    for id in &dropped {
        assert_eq!(f.run_detail(id).unwrap().run.status, "failed");
    }
    *t.hold_chat.lock().unwrap() = None;
    t.release.add_permits(1);
    t.wait_for_completed(&accepted_last).await;
    f.shutdown().await;
    assert_eq!(t.messages().len(), 65);
    assert_eq!(t.completed().len(), 65);
    assert_eq!(t.messages()[0].run_id, first);
    assert!(t.messages().iter().all(|m| !dropped.contains(&m.run_id)));
    assert!(f.snapshot().unwrap().service.last_error.is_none());
}

#[tokio::test]
async fn connectors_reject_deleted_profiles_and_suppress_historical_orphan_origins() {
    let dir = tempfile::tempdir().unwrap();
    let t = Arc::new(common::notifications::RecordingTransport::default());
    let f = open(dir.path(), t.clone());
    configure(
        &f,
        json!([
            connector("tg","telegram",true,json!(["run_failed"])),
            {"id":"chat","label":"Chat","enabled":true,"events":["run_failed"],"type":"google_chat","config":{"webhook_url":"https://fixture.example/messages?token=fixture","triggers":["new_issue"],"token_profile_ids":["work"]}}
        ]),
    );
    let original = f.settings().unwrap();
    let orphan = enqueue(&f, 71);
    let mut removed = original.clone();
    removed.source_connectors.clear();
    f.save_settings(removed).unwrap();
    assert!(f.settings().unwrap().source_connectors.is_empty());
    let db = rusqlite::Connection::open(dir.path().join("factory.sqlite3")).unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(
        &db.query_row("SELECT json FROM settings", [], |r| r.get::<_, String>(0))
            .unwrap(),
    )
    .unwrap();
    raw["source_connectors"] = json!([]);
    db.execute("UPDATE settings SET json=?1", [raw.to_string()])
        .unwrap();
    f.start_notification_delivery();
    f.set_run_status(&orphan, "failed", None).unwrap();
    configure(
        &f,
        json!([
            connector("tg","telegram",true,json!(["run_failed"])),
            {"id":"chat","label":"Chat","enabled":true,"events":["run_failed"],"type":"google_chat","config":{"webhook_url":"https://fixture.example/messages?token=fixture","triggers":["new_issue"],"token_profile_ids":["work"]}}
        ]),
    );
    let control = enqueue(&f, 72);
    f.set_run_status(&control, "failed", None).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let changed = t.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let messages = t.messages();
            let chats = t.chats();
            if ["telegram", "google_chat"].iter().all(|dest| {
                chats
                    .iter()
                    .zip(&messages)
                    .any(|(c, m)| c == dest && m.run_id == control)
            }) {
                break;
            }
            changed.await;
        }
    })
    .await
    .expect("both eligible controls delivered");
    f.shutdown().await;
    assert_eq!(
        t.messages()
            .iter()
            .map(|m| m.run_id.as_str())
            .collect::<Vec<_>>(),
        vec![control.as_str(), control.as_str()]
    );
}

#[tokio::test]
async fn connector_trigger_label_is_shared_by_native_telegram_and_google_chat() {
    let dir = tempfile::tempdir().unwrap();
    let transport = Arc::new(common::notifications::RecordingTransport::default());
    let f = open(dir.path(), transport.clone());
    let mut settings = f.settings().unwrap();
    settings.source_connectors.push(SourceConnector {
        token: Some("fixture-source-only".into()),
        owners: vec!["acme".into()],
        authors: vec!["alice".into()],
        reviewers: vec!["alice".into()],
        kind: factory_core::SourceKind::GithubIssues,
        id: "work".into(),
        account: "alice".into(),
        label: "Work".into(),
        ..Default::default()
    });
    let mut telegram = connector(
        "tg",
        "telegram",
        true,
        json!(["run_started", "run_succeeded", "run_failed", "run_blocked"]),
    );
    telegram["config"]["triggers"] = json!(["new_issue", "pr_approved", "pr_review", "release"]);
    settings.notification_connectors = serde_json::from_value(json!([
        telegram,
        {"id":"gc","label":"Chat","enabled":true,
         "events":["run_started","run_succeeded","run_failed","run_blocked"],
         "type":"google_chat","config":{
             "webhook_url":"https://fixture.example/messages?token=fixture_only",
             "triggers":["new_issue","pr_approved","pr_review","release"],"token_profile_ids":["work"]}}
    ]))
    .unwrap();
    settings
        .flows
        .iter_mut()
        .find(|f| f.id == "release")
        .unwrap()
        .enabled = true;
    f.save_settings(settings).unwrap();
    let native = Arc::new(common::notifications::RecordingNotifications::default());
    f.set_run_notification_sink(native.clone());
    f.start_notification_delivery();
    let mut expected = Vec::new();
    let mut number = 0;
    for (trigger, label) in [
        ("new_issue", "New task"),
        ("pr_approved", "PR approved"),
        ("pr_review", "Review requested"),
        ("release", "Release"),
    ] {
        for status in ["running", "succeeded", "failed", "blocked"] {
            number += 1;
            let mut event = common::from_source(
                TriggerEvent::issue("acme/repo", number, "Shared title", "alice", ""),
                "alice",
            );
            event.trigger_id = trigger.into();
            event.event_id = format!("{trigger}:acme/repo#{number}");
            event.origin_token_profile_id = Some("work".into());
            let id = f.enqueue(event).unwrap().unwrap();
            f.set_run_status(&id, status, Some("timeout PRIVATE_ERROR".into()))
                .unwrap();
            let suffix = if matches!(status, "failed" | "blocked") {
                " · Execution timed out."
            } else {
                ""
            };
            expected.push((
                id,
                status,
                format!("Trigger: {label} · acme/repo #{number} · Shared title{suffix}"),
            ));
        }
    }
    transport.wait_for_calls(32).await;
    f.shutdown().await;
    let native_messages = native.drain();
    let messages = transport.messages();
    let chats = transport.chats();
    assert_eq!(native_messages.len(), 16);
    assert_eq!(messages.len(), 32);
    assert_eq!(chats.len(), 32);
    for (id, status, body) in expected {
        let native = native_messages.iter().find(|m| m.run_id == id).unwrap();
        assert_eq!(native.status, status);
        assert_eq!(native.body, body);
        assert!(!native.body.contains("PRIVATE_ERROR"));
        for chat in ["telegram", "google_chat"] {
            let delivered: Vec<_> = messages
                .iter()
                .zip(&chats)
                .filter(|(m, c)| m.run_id == id && c.as_str() == chat)
                .map(|(m, _)| m)
                .collect();
            assert_eq!(delivered.len(), 1);
            assert_eq!(delivered[0], native);
        }
    }
}

#[tokio::test]
async fn release_routes_each_edge_only_to_matching_trigger_and_profile() {
    for kind in ["telegram", "google_chat"] {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(common::notifications::RecordingTransport::default());
        let f = open(dir.path(), t.clone());
        let mut settings = f.settings().unwrap();
        settings.source_connectors = vec![
            SourceConnector {
                token: Some("fixture-source-only".into()),
                owners: vec!["acme".into()],
                authors: vec!["alice".into()],
                reviewers: vec!["alice".into()],
                kind: factory_core::SourceKind::GithubIssues,
                id: "work".into(),
                account: "alice".into(),
                label: "Work".into(),
                ..Default::default()
            },
            SourceConnector {
                token: Some("fixture-source-only".into()),
                owners: vec!["acme".into()],
                authors: vec!["bob".into()],
                reviewers: vec!["bob".into()],
                kind: factory_core::SourceKind::GithubIssues,
                id: "other".into(),
                account: "bob".into(),
                label: "Other".into(),
                ..Default::default()
            },
        ];
        settings
            .flows
            .iter_mut()
            .find(|f| f.id == "release")
            .unwrap()
            .enabled = true;
        let mut connectors = vec![];
        for (id, trigger, profile, enabled) in [
            ("selected", "release", "work", true),
            ("other_trigger", "new_issue", "work", true),
            ("other_profile", "release", "other", true),
            ("disabled", "release", "work", false),
        ] {
            connectors.push(json!({"id":id,"label":id,"enabled":enabled,"events":["run_started","run_succeeded","run_failed","run_blocked"],"type":kind,"config":{"chat_id":id,"bot_token":"123456:fixture_token_abcdefghijklmnopqrstuvwxyz","webhook_url":"https://fixture.example/messages?token=fixture","triggers":[trigger],"token_profile_ids":[profile]}}));
        }
        settings.notification_connectors = serde_json::from_value(json!(connectors)).unwrap();
        f.save_settings(settings).unwrap();
        f.start_notification_delivery();
        for (n, terminal) in [(91, "succeeded"), (92, "failed"), (93, "blocked")] {
            let mut event = common::from_source(
                TriggerEvent::release_issue("acme/repo", n, "[Release] routing", "alice", ""),
                "alice",
            );
            event.origin_token_profile_id = Some("work".into());
            let id = f.enqueue(event).unwrap().unwrap();
            assert_eq!(
                f.run_detail(&id)
                    .unwrap()
                    .run
                    .origin_token_profile_id
                    .as_deref(),
                Some("work")
            );
            for state in ["running", "running", terminal, terminal] {
                f.set_run_status(&id, state, Some("timeout PRIVATE_ERROR".into()))
                    .unwrap();
            }
        }
        t.wait_for_calls(6).await;
        f.shutdown().await;
        assert_eq!(t.messages().len(), 6);
        assert!(t
            .messages()
            .iter()
            .all(|m| m.body.starts_with("Trigger: Release") && !m.body.contains("PRIVATE_ERROR")));
        assert!(t.chats().iter().all(|c| c
            == if kind == "telegram" {
                "selected"
            } else {
                "google_chat"
            }));
    }
}
