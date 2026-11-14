// Ephemeral destinations and jobs never implement Debug or Serialize.
pub enum NotificationDestination {
    Telegram { chat_id: String, bot_token: String },
    GoogleChat { webhook_url: String },
}
pub(crate) enum ConnectorTargetConfig {
    Telegram { chat_id: String },
    GoogleChat,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ConnectorWorkerKey {
    Telegram,
    GoogleChat { credential_ref: String },
}
pub(crate) struct ConnectorTarget {
    pub connector_id: String,
    pub credential_ref: String,
    pub config: ConnectorTargetConfig,
}
impl ConnectorTarget {
    pub fn worker_key(&self) -> ConnectorWorkerKey {
        match self.config {
            ConnectorTargetConfig::Telegram { .. } => ConnectorWorkerKey::Telegram,
            ConnectorTargetConfig::GoogleChat => ConnectorWorkerKey::GoogleChat {
                credential_ref: self.credential_ref.clone(),
            },
        }
    }
}
pub(crate) struct ConnectorNotificationJob {
    pub target: ConnectorTarget,
    pub transition: crate::context::factory::domain::entities::run_transition::RunTransition,
}
