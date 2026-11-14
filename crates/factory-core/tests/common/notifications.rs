use factory_core::{RunNotification, RunNotificationSink};
use std::sync::Mutex;
#[derive(Default)]
pub struct RecordingNotifications {
    messages: Mutex<Vec<RunNotification>>,
}
impl RunNotificationSink for RecordingNotifications {
    fn enqueue(&self, notification: RunNotification) {
        self.messages.lock().unwrap().push(notification);
    }
}
impl RecordingNotifications {
    pub fn drain(&self) -> Vec<RunNotification> {
        std::mem::take(&mut *self.messages.lock().unwrap())
    }
}

pub struct RecordingTransport {
    calls: Mutex<Vec<(String, RunNotification)>>,
    pub changed: tokio::sync::Notify,
    completed: Mutex<Vec<String>>,
    cancelled: Mutex<Vec<String>>,
    pub hold_chat: Mutex<Option<String>>,
    pub release: tokio::sync::Semaphore,
}
impl RecordingTransport {
    pub fn messages(&self) -> Vec<RunNotification> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, m)| m.clone())
            .collect()
    }
    pub fn chats(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(c, _)| c.clone())
            .collect()
    }
    pub fn completed(&self) -> Vec<String> {
        self.completed.lock().unwrap().clone()
    }
    pub async fn wait_for_cancelled(&self, id: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let changed = self.changed.notified();
                if self.cancelled.lock().unwrap().iter().any(|r| r == id) {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("cancelled HTTP future dropped");
    }
    pub async fn wait_for_completed(&self, id: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let changed = self.changed.notified();
                if self.completed.lock().unwrap().iter().any(|r| r == id) {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("HTTP future completed");
    }
    pub async fn wait_for_calls(&self, n: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let changed = self.changed.notified();
                if self.calls.lock().unwrap().len() >= n {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("notification delivery did not progress");
    }
}
impl factory_core::NotificationTransport for RecordingTransport {
    fn send(
        &self,
        destination: factory_core::NotificationDestination,
        notification: RunNotification,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), factory_core::DeliveryFailure>> + Send + '_,
        >,
    > {
        Box::pin(async move {
            let chat_id = match destination {
                factory_core::NotificationDestination::Telegram { chat_id, .. } => chat_id,
                factory_core::NotificationDestination::GoogleChat { .. } => "google_chat".into(),
            };
            let mut guard = RecordingSend {
                transport: self,
                run_id: notification.run_id.clone(),
                finished: false,
            };
            let hold = self.hold_chat.lock().unwrap().as_ref() == Some(&chat_id);
            self.calls
                .lock()
                .unwrap()
                .push((chat_id.clone(), notification));
            self.changed.notify_waiters();
            if hold {
                self.release.acquire().await.unwrap().forget();
            }
            guard.finished = true;
            self.completed.lock().unwrap().push(guard.run_id.clone());
            self.changed.notify_waiters();
            if chat_id == "fail" {
                Err(factory_core::DeliveryFailure::HttpRejected)
            } else {
                Ok(())
            }
        })
    }
}

impl Default for RecordingTransport {
    fn default() -> Self {
        Self {
            calls: Mutex::default(),
            changed: tokio::sync::Notify::new(),
            completed: Mutex::default(),
            cancelled: Mutex::default(),
            hold_chat: Mutex::default(),
            release: tokio::sync::Semaphore::new(0),
        }
    }
}

struct RecordingSend<'a> {
    transport: &'a RecordingTransport,
    run_id: String,
    finished: bool,
}
impl Drop for RecordingSend<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.transport
                .cancelled
                .lock()
                .unwrap()
                .push(self.run_id.clone());
            self.transport.changed.notify_waiters();
        }
    }
}
