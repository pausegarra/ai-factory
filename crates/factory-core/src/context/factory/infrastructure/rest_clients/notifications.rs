use super::{google_chat::GoogleChatTransport, telegram::TelegramTransport};
use crate::{DeliveryFailure, NotificationDestination, NotificationTransport, RunNotification};
use std::{future::Future, pin::Pin};

pub struct ConnectorTransports {
    telegram: TelegramTransport,
    google_chat: GoogleChatTransport,
}
impl ConnectorTransports {
    pub fn new() -> Result<Self, DeliveryFailure> {
        Ok(Self::with_transports(
            TelegramTransport::new()?,
            GoogleChatTransport::new()?,
        ))
    }
    pub fn with_transports(telegram: TelegramTransport, google_chat: GoogleChatTransport) -> Self {
        Self {
            telegram,
            google_chat,
        }
    }
}
impl NotificationTransport for ConnectorTransports {
    fn send(
        &self,
        destination: NotificationDestination,
        notification: RunNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), DeliveryFailure>> + Send + '_>> {
        match &destination {
            NotificationDestination::Telegram { .. } => {
                self.telegram.send(destination, notification)
            }
            NotificationDestination::GoogleChat { .. } => {
                self.google_chat.send(destination, notification)
            }
        }
    }
}
