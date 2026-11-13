use factory_core::{Factory, RunNotification, RunNotificationSink};
use std::sync::Arc;
use tauri_plugin_notification::{NotificationExt, PermissionState};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
struct QueuedNotifications(UnboundedSender<RunNotification>);
impl RunNotificationSink for QueuedNotifications {
    fn enqueue(&self, message: RunNotification) {
        let _ = self.0.send(message);
    }
}
trait NotificationBackend {
    fn permitted(&self) -> Result<bool, ()>;
    fn show(&self, message: &RunNotification) -> Result<(), ()>;
}
fn deliver(backend: &impl NotificationBackend, message: &RunNotification) {
    if matches!(backend.permitted(), Ok(true)) {
        let _ = backend.show(message);
    }
}
async fn consume(
    mut receiver: UnboundedReceiver<RunNotification>,
    backend: impl NotificationBackend,
) {
    while let Some(message) = receiver.recv().await {
        deliver(&backend, &message);
    }
}
struct TauriBackend(tauri::AppHandle);
impl NotificationBackend for TauriBackend {
    fn permitted(&self) -> Result<bool, ()> {
        self.0
            .notification()
            .permission_state()
            .map(|state| state == PermissionState::Granted)
            .map_err(|_| ())
    }
    fn show(&self, message: &RunNotification) -> Result<(), ()> {
        self.0
            .notification()
            .builder()
            .title(&message.title)
            .body(&message.body)
            .show()
            .map_err(|_| ())
    }
}
pub(crate) fn install(app: tauri::AppHandle, factory: &Factory) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    factory.set_run_notification_sink(Arc::new(QueuedNotifications(sender)));
    tauri::async_runtime::spawn(consume(receiver, TauriBackend(app)));
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    struct FakeBackend {
        permission: Result<bool, ()>,
        attempted: Mutex<Vec<RunNotification>>,
        failures: AtomicUsize,
    }
    impl FakeBackend {
        fn new(permission: Result<bool, ()>, failures: usize) -> Arc<Self> {
            Arc::new(Self {
                permission,
                attempted: Mutex::new(vec![]),
                failures: AtomicUsize::new(failures),
            })
        }
    }
    impl NotificationBackend for Arc<FakeBackend> {
        fn permitted(&self) -> Result<bool, ()> {
            self.permission
        }
        fn show(&self, message: &RunNotification) -> Result<(), ()> {
            assert_eq!(
                self.permission,
                Ok(true),
                "must not call show without permission"
            );
            self.attempted.lock().unwrap().push(message.clone());
            if self
                .failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                Err(())
            } else {
                Ok(())
            }
        }
    }
    fn message(id: &str, attempt: u32, state: &str, body: &str) -> RunNotification {
        RunNotification {
            run_id: id.into(),
            attempt,
            status: state.into(),
            title: format!("AI Factory — Run {state}"),
            body: body.into(),
        }
    }
    #[tokio::test]
    async fn notification_backend_sends_exact_message_without_window() {
        let backend = FakeBackend::new(Ok(true), 0);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let first = message("first-run", 1, "succeeded", "acme/a #1 · First");
        QueuedNotifications(tx.clone()).enqueue(first.clone());
        drop(tx);
        consume(rx, backend.clone()).await;
        assert_eq!(*backend.attempted.lock().unwrap(), vec![first]);
    }
    #[tokio::test]
    async fn notification_backend_denial_and_permission_error_are_silent() {
        for permission in [Ok(false), Err(())] {
            let backend = FakeBackend::new(permission, 0);
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            tx.send(message("run", 1, "failed", "acme/a #1 · First"))
                .unwrap();
            drop(tx);
            consume(rx, backend.clone()).await;
            assert!(backend.attempted.lock().unwrap().is_empty());
        }
    }
    #[tokio::test]
    async fn notification_backend_show_failure_does_not_stop_queue() {
        let backend = FakeBackend::new(Ok(true), 1);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let first = message("first-run", 1, "failed", "acme/a #1 · First");
        let second = message("second-run", 2, "succeeded", "acme/b #2 · Second");
        tx.send(first.clone()).unwrap();
        tx.send(second.clone()).unwrap();
        drop(tx);
        consume(rx, backend.clone()).await;
        assert_eq!(*backend.attempted.lock().unwrap(), vec![first, second]);
        assert_eq!(backend.failures.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn notification_queue_keeps_start_before_fast_result() {
        let backend = FakeBackend::new(Ok(true), 0);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let start = RunNotification {
            title: "AI Factory — Run started".into(),
            ..message("fast", 2, "running", "acme/a #1 · Fast")
        };
        let result = message("fast", 2, "succeeded", "acme/a #1 · Fast");
        let sink = QueuedNotifications(tx);
        sink.enqueue(start.clone());
        sink.enqueue(result.clone());
        drop(sink);
        consume(rx, backend.clone()).await;
        assert_eq!(*backend.attempted.lock().unwrap(), vec![start, result]);
    }
    #[test]
    fn notification_closed_receiver_is_harmless() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        QueuedNotifications(tx).enqueue(message("run", 1, "failed", "acme/a #1 · First"));
    }
}
