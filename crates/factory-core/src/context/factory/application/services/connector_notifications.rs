use crate::context::factory::application::dto::connector_notification::{
    ConnectorNotificationJob, ConnectorTarget,
};
use crate::*;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

struct Worker {
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
}
pub(crate) fn targets_for_event(
    connectors: &[NotificationConnector],
    event: NotificationEvent,
) -> Vec<&NotificationConnector> {
    connectors
        .iter()
        .filter(|c| {
            c.enabled
                && c.events.contains(&event)
                && matches!(c.provider, ConnectorProvider::Telegram(_))
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
        }
    }
    pub fn enqueue(&self, job: ConnectorNotificationJob) {
        if self.stop.is_cancelled() {
            return;
        }
        let id = job.target.connector_id.clone();
        let mut workers = self.workers.lock().unwrap();
        let worker = workers.entry(id.clone()).or_insert_with(|| {
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
                        result = deliver(&factory, transport.as_ref(), job) => result,
                    };
                    if let Err(code) = result {
                        eprintln!("{}", failure_line(&id, code));
                    }
                }
            });
            Worker {
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
    pub fn reconcile(&self, connectors: &[NotificationConnector]) {
        let mut workers = self.workers.lock().unwrap();
        let removed: Vec<_> = workers
            .keys()
            .filter(|id| {
                !connectors
                    .iter()
                    .any(|c| &c.id == *id && matches!(c.provider, ConnectorProvider::Telegram(_)))
            })
            .cloned()
            .collect();
        for id in removed {
            if let Some(worker) = workers.remove(&id) {
                worker.stop.cancel();
                self.retired.lock().unwrap().push(worker.handle);
            }
        }
    }
    pub async fn shutdown(&self) {
        self.stop.cancel();
        let workers = std::mem::take(&mut *self.workers.lock().unwrap());
        let mut handles = std::mem::take(&mut *self.retired.lock().unwrap());
        handles.extend(workers.into_values().map(|w| w.handle));
        for handle in handles {
            let _ = handle.await;
        }
    }
}
async fn deliver(
    factory: &Weak<Factory>,
    transport: &dyn NotificationTransport,
    job: ConnectorNotificationJob,
) -> std::result::Result<(), DeliveryFailure> {
    let ConnectorTarget {
        connector_id,
        chat_id,
        credential_ref,
    } = job.target;
    let f = factory.upgrade().ok_or(DeliveryFailure::QueueClosed)?;
    let reader = f.clone();
    let mut credential = tokio::task::spawn_blocking(move || {
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
    transport
        .send(
            NotificationDestination::Telegram {
                chat_id,
                bot_token: token,
            },
            message,
        )
        .await
}
#[cfg(test)]
mod tests {
    use super::*;
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
                targets_for_event(&connectors, event)
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>(),
                expected,
                "{event:?}"
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
