// Ephemeral destinations and jobs never implement Debug or Serialize.
pub enum NotificationDestination {
    Telegram { chat_id: String, bot_token: String },
}
pub(crate) struct ConnectorTarget {
    pub connector_id: String,
    pub chat_id: String,
    pub credential_ref: String,
}
pub(crate) struct ConnectorNotificationJob {
    pub target: ConnectorTarget,
    pub transition: crate::context::factory::domain::entities::run_transition::RunTransition,
}
