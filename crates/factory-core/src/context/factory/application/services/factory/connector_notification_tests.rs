use super::*;
use crate::context::factory::application::services::connector_notifications::{
    ReadProbeEvent, ReadProbePhase,
};
use std::{collections::HashMap, sync::Condvar, time::Duration};
use tokio::sync::{mpsc, Notify};

#[derive(Default)]
struct SelectiveCredentials {
    values: Mutex<HashMap<String, String>>,
    armed: Mutex<Option<String>>,
    reached: Notify,
    released: (Mutex<bool>, Condvar),
}
impl Credentials for SelectiveCredentials {
    fn get(&self, key: &str) -> Result<Option<String>> {
        let value = self.values.lock().unwrap().get(key).cloned();
        let block = {
            let mut armed = self.armed.lock().unwrap();
            if armed.as_deref() == Some(key) {
                armed.take();
                true
            } else {
                false
            }
        };
        if block {
            self.reached.notify_one();
            let (lock, changed) = &self.released;
            let _guard = changed
                .wait_while(lock.lock().unwrap(), |released| !*released)
                .unwrap();
        }
        Ok(value)
    }
    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.values.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}
struct ReleaseRead(Arc<SelectiveCredentials>);
impl ReleaseRead {
    fn release(&self) {
        let (lock, changed) = &self.0.released;
        *lock.lock().unwrap() = true;
        changed.notify_all();
    }
}
impl Drop for ReleaseRead {
    fn drop(&mut self) {
        self.release();
    }
}
#[derive(Default)]
struct Transport {
    started: Mutex<Vec<String>>,
    completed: Mutex<Vec<String>>,
    changed: Notify,
}
impl NotificationTransport for Transport {
    fn send(
        &self,
        _: NotificationDestination,
        m: RunNotification,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<(), DeliveryFailure>> + Send + '_>,
    > {
        Box::pin(async move {
            self.started.lock().unwrap().push(m.run_id.clone());
            self.completed.lock().unwrap().push(m.run_id);
            self.changed.notify_one();
            Ok(())
        })
    }
}
impl Transport {
    async fn wait(&self, id: &str) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let changed = self.changed.notified();
                if self.completed.lock().unwrap().iter().any(|r| r == id) {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("HTTP control completed");
    }
}
#[derive(Default)]
struct Native(Mutex<Vec<String>>);
impl RunNotificationSink for Native {
    fn enqueue(&self, m: RunNotification) {
        self.0.lock().unwrap().push(m.run_id);
    }
}
struct ReadProbe {
    receiver: mpsc::UnboundedReceiver<ReadProbeEvent>,
    backlog: Vec<ReadProbeEvent>,
    active: HashMap<String, usize>,
    started: HashMap<String, usize>,
    max_active: HashMap<String, usize>,
}
impl ReadProbe {
    fn record(&mut self, event: &ReadProbeEvent) {
        match event.phase {
            ReadProbePhase::Started => {
                *self.started.entry(event.connector_id.clone()).or_default() += 1;
                let active = self.active.entry(event.connector_id.clone()).or_default();
                *active += 1;
                let max = self
                    .max_active
                    .entry(event.connector_id.clone())
                    .or_default();
                *max = (*max).max(*active);
            }
            ReadProbePhase::Finished => {
                *self.active.entry(event.connector_id.clone()).or_default() -= 1
            }
            _ => {}
        }
    }
    async fn wait(&mut self, id: &str, run: &str, phase: ReadProbePhase) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(i) = self
                    .backlog
                    .iter()
                    .position(|e| e.connector_id == id && e.run_id == run && e.phase == phase)
                {
                    self.backlog.remove(i);
                    return;
                }
                let e = self.receiver.recv().await.expect("probe open");
                self.record(&e);
                eprintln!(
                    "read_probe connector={} run={} phase={:?} max_active={:?}",
                    e.connector_id, e.run_id, e.phase, self.max_active
                );
                assert!(
                    !(phase == ReadProbePhase::Waiting
                        && e.connector_id == id
                        && e.run_id == run
                        && e.phase == ReadProbePhase::Started),
                    "successor started an overlapping OS read instead of Waiting; max_active={:?}",
                    self.max_active
                );
                if e.connector_id == id && e.run_id == run && e.phase == phase {
                    return;
                }
                self.backlog.push(e);
            }
        })
        .await
        .expect("read probe phase reached");
    }
}
fn connector(id: &str, url: &str, trigger: &str) -> NotificationConnector {
    serde_json::from_value(serde_json::json!({"id":id,"label":id,"enabled":true,"events":["run_failed"],"type":"google_chat","config":{"webhook_url":url,"triggers":[trigger],"token_profile_ids":["work"]}})).unwrap()
}
fn emit(f: &Factory, n: u64, trigger: &str) -> String {
    let mut e = TriggerEvent::issue("acme/repo", n, "Fixture", "alice", "");
    e.trigger_id = trigger.into();
    e.event_id = format!("{trigger}:acme/repo#{n}");
    e.origin_token_profile_id = Some("work".into());
    let id = f.enqueue(e).unwrap().unwrap();
    f.set_run_status(&id, "failed", None).unwrap();
    id
}
async fn save_checked(f: Arc<Factory>, s: Settings) -> Settings {
    tokio::time::timeout(
        Duration::from_secs(2),
        tokio::task::spawn_blocking(move || f.save_settings(s)),
    )
    .await
    .expect("save must progress with old OS read held")
    .unwrap()
    .unwrap()
}
async fn scenario(mode: &str, shutdown_held: bool) {
    let dir = tempfile::tempdir().unwrap();
    let c = Arc::new(SelectiveCredentials::default());
    let release = ReleaseRead(c.clone());
    let t = Arc::new(Transport::default());
    let f = Factory::open_with_notification_transport(
        dir.path().into(),
        "http://127.0.0.1:9",
        c.clone(),
        t.clone(),
    )
    .unwrap();
    let native = Arc::new(Native::default());
    f.set_run_notification_sink(native.clone());
    let mut s = f.settings().unwrap();
    s.token_profiles.push(TokenProfile {
        id: "work".into(),
        account: "alice".into(),
        label: "Work".into(),
        ..Default::default()
    });
    s.notification_connectors = vec![
        connector(
            "slow",
            "https://fixture.example/old?token=old_fixture",
            "new_issue",
        ),
        connector(
            "fast",
            "https://fixture.example/fast?token=fast_fixture",
            "pr_review",
        ),
    ];
    save_checked(f.clone(), s).await;
    let reference: String = f
        .store
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT reference FROM notification_credential_refs WHERE connector_id='slow'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    *c.armed.lock().unwrap() = Some(reference);
    f.start_notification_delivery();
    let (sender, receiver) = mpsc::unbounded_channel();
    f.connector_dispatcher
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .observe_reads_for_test(sender);
    let mut probe = ReadProbe {
        receiver,
        backlog: vec![],
        active: HashMap::new(),
        started: HashMap::new(),
        max_active: HashMap::new(),
    };
    let old = emit(&f, 1, "new_issue");
    probe.wait("slow", &old, ReadProbePhase::Started).await;
    tokio::time::timeout(Duration::from_secs(2), c.reached.notified())
        .await
        .unwrap();
    let mut last = String::new();
    let mut revoked = vec![old.clone()];
    for i in 0..4 {
        let mut s = f.settings().unwrap();
        match mode {
            "rotate" => {
                if let ConnectorProvider::GoogleChat(config) =
                    &mut s.notification_connectors[0].provider
                {
                    config.webhook_url =
                        Some(format!("https://fixture.example/new?token=fixture_{i}"));
                }
                save_checked(f.clone(), s).await;
            }
            "disable" => {
                s.notification_connectors[0].enabled = false;
                s = save_checked(f.clone(), s).await;
                s.notification_connectors[0].enabled = true;
                save_checked(f.clone(), s).await;
            }
            "remove" => {
                s.notification_connectors.retain(|c| c.id != "slow");
                s = save_checked(f.clone(), s).await;
                if i == 1 {
                    s.notification_connectors.insert(
                        0,
                        NotificationConnector {
                            id: "slow".into(),
                            label: "slow".into(),
                            enabled: true,
                            events: vec![NotificationEvent::RunFailed],
                            provider: ConnectorProvider::Telegram(TelegramConnectorConfig {
                                chat_id: "fixture".into(),
                                bot_token: Some("123:fixture".into()),
                                has_token: false,
                            }),
                        },
                    );
                    s = save_checked(f.clone(), s).await;
                    let crossed = emit(&f, 90, "new_issue");
                    probe.wait("slow", &crossed, ReadProbePhase::Waiting).await;
                    revoked.push(crossed);
                    s.notification_connectors.retain(|c| c.id != "slow");
                    s = save_checked(f.clone(), s).await;
                }
                s.notification_connectors.insert(
                    0,
                    connector(
                        "slow",
                        &format!("https://fixture.example/recreated?token=fixture_{i}"),
                        "new_issue",
                    ),
                );
                save_checked(f.clone(), s).await;
            }
            _ => unreachable!(),
        }
        if !last.is_empty() {
            revoked.push(last);
        }
        last = emit(&f, 10 + i, "new_issue");
        probe.wait("slow", &last, ReadProbePhase::Waiting).await;
        assert_eq!(probe.active.get("slow"), Some(&1));
        assert_eq!(probe.started.get("slow"), Some(&1));
        assert_eq!(probe.max_active.get("slow"), Some(&1));
    }
    let fast = emit(&f, 99, "pr_review");
    t.wait(&fast).await;
    assert_eq!(f.run_detail(&fast).unwrap().run.status, "failed");
    assert!(native.0.lock().unwrap().contains(&fast));
    if shutdown_held {
        tokio::time::timeout(Duration::from_secs(1), f.shutdown())
            .await
            .expect("shutdown returns before OS release");
        assert_eq!(*t.started.lock().unwrap(), vec![fast.clone()]);
        release.release();
        probe.wait("slow", &old, ReadProbePhase::Finished).await;
    } else {
        release.release();
        probe.wait("slow", &old, ReadProbePhase::Finished).await;
        probe.wait("slow", &last, ReadProbePhase::Started).await;
        probe.wait("slow", &last, ReadProbePhase::Finished).await;
        t.wait(&last).await;
        f.shutdown().await;
    }
    assert_eq!(probe.max_active.get("slow"), Some(&1));
    assert_eq!(probe.active.get("slow"), Some(&0));
    for id in revoked {
        assert!(!t.started.lock().unwrap().contains(&id));
        assert!(!t.completed.lock().unwrap().contains(&id));
    }
    if shutdown_held {
        assert!(!t.started.lock().unwrap().contains(&last));
    }
    // Wait for the blocking reader's final Factory owner before releasing its files.
    tokio::time::timeout(Duration::from_secs(2), async {
        while Arc::strong_count(&f) > 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_read_gate_rotations_share_one_os_call() {
    scenario("rotate", false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_read_gate_disable_enable_reuses_pending_control() {
    scenario("disable", false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_read_gate_remove_recreate_keeps_pending_control() {
    scenario("remove", false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_read_gate_shutdown_does_not_wait_for_old_os_call() {
    for mode in ["rotate", "disable", "remove"] {
        scenario(mode, true).await;
    }
}
