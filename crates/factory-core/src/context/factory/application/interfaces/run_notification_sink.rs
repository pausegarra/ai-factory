use crate::RunNotification;
/// Output port: enqueue without waiting for native I/O, panicking, or propagating errors.
pub trait RunNotificationSink: Send + Sync {
    fn enqueue(&self, notification: RunNotification);
}
#[derive(Default)]
pub struct NoopRunNotificationSink;
impl RunNotificationSink for NoopRunNotificationSink {
    fn enqueue(&self, _: RunNotification) {}
}
