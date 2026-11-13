use crate::{NotificationDestination, RunNotification};
use std::{future::Future, pin::Pin};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryFailure {
    CredentialUnavailable,
    Timeout,
    Transport,
    HttpRejected,
    ApiRejected,
    InvalidResponse,
    ResponseTooLarge,
    QueueFull,
    QueueClosed,
}
impl DeliveryFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::CredentialUnavailable => "credential_unavailable",
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::HttpRejected => "http_rejected",
            Self::ApiRejected => "api_rejected",
            Self::InvalidResponse => "invalid_response",
            Self::ResponseTooLarge => "response_too_large",
            Self::QueueFull => "queue_full",
            Self::QueueClosed => "queue_closed",
        }
    }
}
pub trait NotificationTransport: Send + Sync {
    fn send(
        &self,
        destination: NotificationDestination,
        notification: RunNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), DeliveryFailure>> + Send + '_>>;
}
