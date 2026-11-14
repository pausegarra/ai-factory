use crate::context::factory::application::dto::connector_notification::{
    ConnectorNotificationJob, ConnectorTarget, ConnectorTargetConfig, ConnectorWorkerKey,
};
use crate::*;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{
    sync::{mpsc, Semaphore},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

// Weak entries preserve identity while an old blocking read still owns its permit.
// Lock order: workers, then read_gates. No dispatcher lock crosses an await or OS call.
type ReadGates = Mutex<HashMap<String, Weak<Semaphore>>>;
fn read_gate_for(gates: &ReadGates, id: &str) -> Arc<Semaphore> {
    let mut gates = gates.lock().unwrap();
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = gates.get(id).and_then(Weak::upgrade) {
        return gate;
    }
    let gate = Arc::new(Semaphore::new(1));
    gates.insert(id.into(), Arc::downgrade(&gate));
    gate
}
fn prune_read_gates(gates: &ReadGates) {
    gates
        .lock()
        .unwrap()
        .retain(|_, gate| gate.strong_count() > 0);
}
struct Worker {
    key: ConnectorWorkerKey,
    _read_gate: Arc<Semaphore>,
    sender: mpsc::Sender<ConnectorNotificationJob>,
    stop: CancellationToken,
    handle: JoinHandle<()>,
}
pub(crate) struct ConnectorDispatcher {
    runtime: tokio::runtime::Handle,
    factory: Weak<Factory>,
    transport: Arc<dyn NotificationTransport>,
    stop: CancellationToken,
    workers: Mutex<HashMap<String, Worker>>,
    retired: Mutex<Vec<JoinHandle<()>>>,
    read_gates: ReadGates,
    #[cfg(test)]
    read_probe: Mutex<Option<mpsc::UnboundedSender<ReadProbeEvent>>>,
}
pub(crate) fn targets_for_event<'a>(
    connectors: &'a [NotificationConnector],
    event: NotificationEvent,
    run: &Run,
    profiles: &[TokenProfile],
) -> Vec<&'a NotificationConnector> {
    connectors
        .iter()
        .filter(|c| {
            c.enabled
                && c.events.contains(&event)
                && match &c.provider {
                    ConnectorProvider::Telegram(_) => true,
                    ConnectorProvider::GoogleChat(config) => {
                        config.triggers.contains(&run.trigger_id)
                            && run.origin_token_profile_id.as_ref().is_some_and(|id| {
                                config.token_profile_ids.contains(id)
                                    && profiles.iter().any(|p| &p.id == id)
                            })
                    }
                    _ => false,
                }
        })
        .collect()
}
pub(crate) fn failure_line(id: &str, code: DeliveryFailure) -> String {
    // Defense in depth: diagnostic identities cannot inject data or additional lines.
    let id = if safe_id(id) { id } else { "invalid" };
    format!(
        "notification_delivery_failed connector={id} reason={}",
        code.code()
    )
}
impl ConnectorDispatcher {
    pub fn start(
        factory: Weak<Factory>,
        transport: Arc<dyn NotificationTransport>,
        stop: CancellationToken,
    ) -> Self {
        Self {
            runtime: tokio::runtime::Handle::current(),
            factory,
            transport,
            stop,
            workers: Mutex::default(),
            retired: Mutex::default(),
            read_gates: ReadGates::default(),
            #[cfg(test)]
            read_probe: Mutex::default(),
        }
    }
    pub fn enqueue(&self, job: ConnectorNotificationJob) {
        if self.stop.is_cancelled() {
            return;
        }
        let id = job.target.connector_id.clone();
        let mut workers = self.workers.lock().unwrap();
        prune_read_gates(&self.read_gates);
        self.retired
            .lock()
            .unwrap()
            .retain(|handle| !handle.is_finished());
        let key = job.target.worker_key();
        if workers.get(&id).is_some_and(|w| w.key != key) {
            let old = workers.remove(&id).unwrap();
            old.stop.cancel();
            self.retired.lock().unwrap().push(old.handle);
        }
        let worker = workers.entry(id.clone()).or_insert_with(|| {
            let read_gate=read_gate_for(&self.read_gates,&id);
            let task_gate=read_gate.clone();
            #[cfg(test)]
            let probe=self.read_probe.lock().unwrap().clone();
            let (sender, mut receiver) = mpsc::channel::<ConnectorNotificationJob>(64);
            let stop = self.stop.child_token();
            let task_stop = stop.clone();
            let factory = self.factory.clone();
            let transport = self.transport.clone();
            let handle = self.runtime.spawn(async move {
                loop {
                    let job = tokio::select! {
                        biased;
                        _ = task_stop.cancelled() => break,
                        job = receiver.recv() => match job {
                            Some(job) => job,
                            None => break,
                        }
                    };
                    let id = job.target.connector_id.clone();
                    let result = tokio::select! {
                        biased;
                        _ = task_stop.cancelled() => break,
                        result = deliver(&factory, transport.as_ref(), job, task_gate.clone(), &task_stop, #[cfg(test)] probe.clone()) => result,
                    };
                    if let Err(code) = result {
                        eprintln!("{}", failure_line(&id, code));
                    }
                }
            });
            Worker {
                key,
                _read_gate: read_gate,
                sender,
                stop,
                handle,
            }
        });
        if let Err(err) = worker.sender.try_send(job) {
            let code = match err {
                mpsc::error::TrySendError::Full(_) => DeliveryFailure::QueueFull,
                mpsc::error::TrySendError::Closed(_) => DeliveryFailure::QueueClosed,
            };
            eprintln!("{}", failure_line(&id, code));
        }
    }
    pub fn reconcile(&self, live: &HashMap<String, ConnectorWorkerKey>) {
        let mut workers = self.workers.lock().unwrap();
        let removed: Vec<_> = workers
            .iter()
            .filter(|(id, w)| live.get(*id) != Some(&w.key))
            .map(|(id, _)| id.clone())
            .collect();
        for id in removed {
            let worker = workers.remove(&id).unwrap();
            worker.stop.cancel();
            self.retired.lock().unwrap().push(worker.handle);
        }
        prune_read_gates(&self.read_gates);
        self.retired
            .lock()
            .unwrap()
            .retain(|handle| !handle.is_finished());
    }
    #[cfg(test)]
    pub(super) fn observe_reads_for_test(&self, sender: mpsc::UnboundedSender<ReadProbeEvent>) {
        *self.read_probe.lock().unwrap() = Some(sender);
    }
    pub async fn shutdown(&self) {
        self.stop.cancel();
        let workers = std::mem::take(&mut *self.workers.lock().unwrap());
        let mut handles = std::mem::take(&mut *self.retired.lock().unwrap());
        handles.extend(workers.into_values().map(|w| w.handle));
        for handle in handles {
            let _ = handle.await;
        }
        self.read_gates.lock().unwrap().clear();
    }
}
async fn deliver(
    factory: &Weak<Factory>,
    transport: &dyn NotificationTransport,
    job: ConnectorNotificationJob,
    read_gate: Arc<Semaphore>,
    stop: &CancellationToken,
    #[cfg(test)] probe: Option<mpsc::UnboundedSender<ReadProbeEvent>>,
) -> std::result::Result<(), DeliveryFailure> {
    let ConnectorTarget {
        connector_id,
        config,
        credential_ref,
    } = job.target;
    #[cfg(test)]
    let event = ReadProbeEvent {
        connector_id: connector_id.clone(),
        run_id: job.transition.run.id.clone(),
        phase: ReadProbePhase::Started,
    };
    let acquire = read_gate.acquire_owned();
    tokio::pin!(acquire);
    #[cfg(test)]
    let mut waiting = false;
    let permit = tokio::select! {
        biased;
        _ = stop.cancelled() => return Err(DeliveryFailure::QueueClosed),
        result=std::future::poll_fn(|cx| {
        let result=std::future::Future::poll(acquire.as_mut(),cx);
        #[cfg(test)]
        if result.is_pending() && !waiting {
            waiting=true;
            if let Some(probe)=&probe {let mut e=event.clone();e.phase=ReadProbePhase::Waiting;let _=probe.send(e);}
        }
        result
        }) => result.map_err(|_| DeliveryFailure::QueueClosed)?,
    };
    if stop.is_cancelled() {
        return Err(DeliveryFailure::QueueClosed);
    }
    let f = factory.upgrade().ok_or(DeliveryFailure::QueueClosed)?;
    let reader = f.clone();
    let read_stop = stop.clone();
    let mut credential = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if read_stop.is_cancelled() {
            return Err(anyhow::anyhow!("Notification credential unavailable"));
        }
        #[cfg(test)]
        let _probe = ReadProbeGuard::start(probe, event);
        reader
            .store
            .notification_token_for(&connector_id, &credential_ref)
    });
    let token = match tokio::time::timeout(Duration::from_secs(3), &mut credential).await {
        Ok(result) => result
            .map_err(|_| DeliveryFailure::CredentialUnavailable)?
            .map_err(|_| DeliveryFailure::CredentialUnavailable)?,
        Err(_) => {
            // OS calls cannot be cancelled. Wait before accepting another read on this consumer.
            // The outer cancellation select can discard this result without a late HTTP send.
            let _ = credential.await;
            return Err(DeliveryFailure::Timeout);
        }
    };
    let message =
        super::run_notifications::notification_for(&job.transition, |s| f.store.redact_cached(s))
            .ok_or(DeliveryFailure::InvalidResponse)?;
    drop(f);
    let send = transport.send(
        match config {
            ConnectorTargetConfig::Telegram { chat_id } => NotificationDestination::Telegram {
                chat_id,
                bot_token: token,
            },
            ConnectorTargetConfig::GoogleChat => {
                NotificationDestination::GoogleChat { webhook_url: token }
            }
        },
        message,
    );
    tokio::select! {
        biased;
        _=stop.cancelled()=>Err(DeliveryFailure::QueueClosed),
        result=send=>result,
    }
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadProbePhase {
    Waiting,
    Started,
    Finished,
}
#[cfg(test)]
#[derive(Clone, Debug)]
pub(super) struct ReadProbeEvent {
    pub connector_id: String,
    pub run_id: String,
    pub phase: ReadProbePhase,
}
#[cfg(test)]
struct ReadProbeGuard {
    sender: Option<mpsc::UnboundedSender<ReadProbeEvent>>,
    event: ReadProbeEvent,
}
#[cfg(test)]
impl ReadProbeGuard {
    fn start(sender: Option<mpsc::UnboundedSender<ReadProbeEvent>>, event: ReadProbeEvent) -> Self {
        if let Some(sender) = &sender {
            let _ = sender.send(event.clone());
        }
        Self { sender, event }
    }
}
#[cfg(test)]
impl Drop for ReadProbeGuard {
    fn drop(&mut self) {
        if let Some(sender) = &self.sender {
            self.event.phase = ReadProbePhase::Finished;
            let _ = sender.send(self.event.clone());
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn notification_read_gate_survives_removed_generation_until_owner_returns() {
        let gates = ReadGates::default();
        let old = read_gate_for(&gates, "same-id");
        let weak = Arc::downgrade(&old);
        let owner = old.clone().acquire_owned().await.unwrap();
        drop(old);
        prune_read_gates(&gates);
        let recreated = read_gate_for(&gates, "same-id");
        assert!(Arc::ptr_eq(&recreated, &weak.upgrade().unwrap()));
        assert!(recreated.clone().try_acquire_owned().is_err());
        assert!(read_gate_for(&gates, "other-id")
            .try_acquire_owned()
            .is_ok());
        drop(recreated);
        drop(owner);
        prune_read_gates(&gates);
        assert!(weak.upgrade().is_none());
        assert!(!gates.lock().unwrap().contains_key("same-id"));
        assert!(read_gate_for(&gates, "same-id").try_acquire_owned().is_ok());
    }
    fn selection_run() -> Run {
        serde_json::from_value(serde_json::json!({"id":"fixture","event_id":"fixture","trigger_id":"new_issue","repository":"acme/repo","number":1,"title":"fixture","actor":"alice","url":"","status":"running","current_step":null,"created_at":"now","started_at":null,"finished_at":null,"error":null,"attempt":1,"workspace":null})).unwrap()
    }
    #[test]
    fn notification_selection_excludes_disabled_unknown_and_unselected_events() {
        let connectors: Vec<NotificationConnector> = serde_json::from_value(serde_json::json!([
            {"id":"selected", "label":"Selected", "enabled":true,
             "events":["run_failed"], "type":"telegram", "config":{"chat_id":"a"}},
            {"id":"disabled", "label":"Disabled", "enabled":false,
             "events":["run_started","run_succeeded","run_failed","run_blocked"],
             "type":"telegram", "config":{"chat_id":"b"}},
            {"id":"unknown", "label":"Unknown", "enabled":true,
             "events":["run_started","run_succeeded","run_failed","run_blocked"],
             "type":"future", "config":{}},
            {"id":"unselected", "label":"Unselected", "enabled":true,
             "events":[], "type":"telegram", "config":{"chat_id":"c"}}
        ]))
        .unwrap();
        for (event, expected) in [
            (NotificationEvent::RunStarted, vec![]),
            (NotificationEvent::RunSucceeded, vec![]),
            (NotificationEvent::RunFailed, vec!["selected"]),
            (NotificationEvent::RunBlocked, vec![]),
        ] {
            assert_eq!(
                targets_for_event(&connectors, event, &selection_run(), &[])
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>(),
                expected,
                "{event:?}"
            );
        }
    }
    #[test]
    fn notification_selection_google_chat_ands_dimensions_ors_ids_and_rejects_orphan_origin() {
        let connectors:Vec<NotificationConnector>=serde_json::from_value(serde_json::json!([
            {"id":"chat","label":"Team","enabled":true,"events":["run_started","run_failed"],"type":"google_chat","config":{"triggers":["new_issue","pr_review"],"token_profile_ids":["work","personal"]}}
        ])).unwrap();
        let profiles = vec![
            TokenProfile {
                id: "work".into(),
                ..Default::default()
            },
            TokenProfile {
                id: "personal".into(),
                ..Default::default()
            },
        ];
        for (event, trigger, origin, exists, expected) in [
            (
                NotificationEvent::RunStarted,
                "new_issue",
                Some("work"),
                true,
                1,
            ),
            (
                NotificationEvent::RunFailed,
                "pr_review",
                Some("personal"),
                true,
                1,
            ),
            (
                NotificationEvent::RunSucceeded,
                "new_issue",
                Some("work"),
                true,
                0,
            ),
            (
                NotificationEvent::RunStarted,
                "pr_approved",
                Some("work"),
                true,
                0,
            ),
            (
                NotificationEvent::RunStarted,
                "new_issue",
                Some("other"),
                true,
                0,
            ),
            (NotificationEvent::RunStarted, "new_issue", None, true, 0),
            (
                NotificationEvent::RunStarted,
                "new_issue",
                Some("work"),
                false,
                0,
            ),
        ] {
            let mut run = selection_run();
            run.trigger_id = trigger.into();
            run.origin_token_profile_id = origin.map(str::to_owned);
            run.actor = "work".into();
            assert_eq!(
                targets_for_event(
                    &connectors,
                    event,
                    &run,
                    if exists { &profiles } else { &[] }
                )
                .len(),
                expected
            );
        }
    }
    #[test]
    fn notification_diagnostics_only_contain_valid_identity_and_closed_code() {
        assert_eq!(
            failure_line("team", DeliveryFailure::HttpRejected),
            "notification_delivery_failed connector=team reason=http_rejected"
        );
        assert_eq!(
            failure_line("secret\nhttp://bot-token", DeliveryFailure::Timeout),
            "notification_delivery_failed connector=invalid reason=timeout"
        );
    }
}
