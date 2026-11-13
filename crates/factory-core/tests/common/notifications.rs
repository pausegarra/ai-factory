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
