use crate::*;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
pub(crate) struct BackupTicket {
    pub summary: PreparedBackupImport,
    pub path: PathBuf,
    pub sha256: String,
    pub fingerprint: String,
    pub selection: BackupSelection,
    pub created: Instant,
}
#[derive(Default)]
pub(crate) struct BackupImportRegistry {
    ticket: Option<BackupTicket>,
    latest_preparation: Option<String>,
}
impl BackupImportRegistry {
    pub fn begin_preparation(&mut self) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.latest_preparation = Some(id.clone());
        self.ticket = None;
        id
    }
    pub fn publish(&mut self, ticket: BackupTicket) {
        // A newer request supersedes this one even if it fails or finishes first.
        // Keep its generation after consume/discard so an older read cannot revive.
        if self.latest_preparation.as_ref() == Some(&ticket.summary.confirmation_id) {
            self.ticket = Some(ticket);
        }
    }
    pub fn consume(&mut self, id: &str) -> std::result::Result<BackupTicket, BackupFailure> {
        let current = self
            .ticket
            .as_ref()
            .ok_or_else(|| BackupFailure::new(BackupErrorCode::ConfirmationRequired))?;
        if current.summary.confirmation_id != id {
            return Err(BackupFailure::new(BackupErrorCode::StalePreview));
        }
        let ticket = self.ticket.take().unwrap();
        if ticket.created.elapsed() > Duration::from_secs(300) {
            return Err(BackupFailure::new(BackupErrorCode::StalePreview));
        }
        Ok(ticket)
    }
    pub fn discard(&mut self, id: &str) {
        if self
            .ticket
            .as_ref()
            .is_some_and(|t| t.summary.confirmation_id == id)
        {
            self.ticket = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ticket() -> BackupTicket {
        BackupTicket {
            summary: PreparedBackupImport {
                confirmation_id: "fixture".into(),
                replace: vec![BackupComponent::History],
                preserve: vec![],
                counts: Default::default(),
                warnings: vec![],
            },
            path: "/fixture".into(),
            sha256: "a".repeat(64),
            fingerprint: "b".repeat(64),
            selection: BackupSelection::new(vec![BackupComponent::History]).unwrap(),
            created: Instant::now(),
        }
    }
    #[test]
    fn stale_confirmation_does_not_consume_the_current_ticket() {
        let mut registry = BackupImportRegistry {
            ticket: Some(ticket()),
            ..Default::default()
        };
        assert_eq!(
            registry.consume("obsolete").err().unwrap().code,
            BackupErrorCode::StalePreview
        );
        assert!(registry.consume("fixture").is_ok());
        assert_eq!(
            registry.consume("fixture").err().unwrap().code,
            BackupErrorCode::ConfirmationRequired
        );
    }
    #[test]
    fn backup_confirmation_is_single_use_expires_and_can_be_discarded() {
        let mut registry = BackupImportRegistry {
            ticket: Some(ticket()),
            ..Default::default()
        };
        assert!(registry.consume("fixture").is_ok());
        assert!(registry.consume("fixture").is_err());
        let mut old = ticket();
        old.created -= Duration::from_secs(301);
        registry.ticket = Some(old);
        assert!(registry.consume("fixture").is_err());
        registry.ticket = Some(ticket());
        registry.discard("other");
        assert!(registry.ticket.is_some());
        registry.discard("fixture");
        registry.discard("fixture");
        assert!(registry.ticket.is_none());
    }
}
