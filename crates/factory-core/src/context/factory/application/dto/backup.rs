use crate::{BackupComponent, BackupSelection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackupErrorCode {
    InvalidSelection,
    InvalidArchive,
    UnsupportedVersion,
    LimitExceeded,
    InvalidPassword,
    CredentialUnavailable,
    CredentialTargetMissing,
    OperationBusy,
    CleanupBusy,
    StalePreview,
    ConfirmationRequired,
    PersistenceFailed,
    DestinationExists,
    RecoveryRequired,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupFailure {
    pub code: BackupErrorCode,
    pub category: Option<BackupComponent>,
    pub message: String,
}
impl BackupFailure {
    pub fn new(code: BackupErrorCode) -> Self {
        let message = match code {
            BackupErrorCode::InvalidSelection => "Choose available backup categories",
            BackupErrorCode::InvalidArchive => "Invalid backup archive",
            BackupErrorCode::UnsupportedVersion => "Unsupported backup format version",
            BackupErrorCode::LimitExceeded => "Backup exceeds supported limits",
            BackupErrorCode::InvalidPassword => {
                "Cannot unlock credentials; check the backup password"
            }
            BackupErrorCode::CredentialUnavailable => "Unlock the credential store and retry",
            BackupErrorCode::CredentialTargetMissing => {
                "Backup credentials do not match local configuration"
            }
            BackupErrorCode::OperationBusy => "Wait for active work or cancel it, then retry",
            BackupErrorCode::CleanupBusy => "Finish pending run deletion before restoring history",
            BackupErrorCode::StalePreview => {
                "Backup or local data changed; review the import again"
            }
            BackupErrorCode::ConfirmationRequired => {
                "Review and confirm the replacement before importing"
            }
            BackupErrorCode::PersistenceFailed => "Cannot persist backup data",
            BackupErrorCode::DestinationExists => "Choose a new destination filename",
            BackupErrorCode::RecoveryRequired => "Unlock the credential store and retry recovery",
        };
        Self {
            code,
            category: match code {
                BackupErrorCode::InvalidPassword
                | BackupErrorCode::CredentialUnavailable
                | BackupErrorCode::CredentialTargetMissing
                | BackupErrorCode::RecoveryRequired => Some(BackupComponent::Credentials),
                BackupErrorCode::CleanupBusy => Some(BackupComponent::History),
                _ => None,
            },
            message: message.into(),
        }
    }
    pub fn invalid_archive() -> Self {
        Self::new(BackupErrorCode::InvalidArchive)
    }
    pub fn invalid_password() -> Self {
        Self::new(BackupErrorCode::InvalidPassword)
    }
    pub fn limit_exceeded() -> Self {
        Self::new(BackupErrorCode::LimitExceeded)
    }
    pub fn persistence_failed() -> Self {
        Self::new(BackupErrorCode::PersistenceFailed)
    }
}
impl std::fmt::Display for BackupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for BackupFailure {}
impl From<crate::context::factory::domain::entities::backup::BackupRuleError> for BackupFailure {
    fn from(_: crate::context::factory::domain::entities::backup::BackupRuleError) -> Self {
        Self::new(BackupErrorCode::InvalidSelection)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupWarning {
    pub code: String,
    pub category: BackupComponent,
    pub entity_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupInspection {
    pub archive_sha256: String,
    pub archive_id: String,
    pub components: Vec<BackupComponent>,
    pub encrypted_credentials: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedBackupImport {
    pub confirmation_id: String,
    pub replace: Vec<BackupComponent>,
    pub preserve: Vec<BackupComponent>,
    pub counts: BTreeMap<String, u64>,
    pub warnings: Vec<BackupWarning>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupOutcome {
    pub committed: bool,
    pub components: Vec<BackupComponent>,
    pub path: Option<String>,
    pub counts: BTreeMap<String, u64>,
    pub warnings: Vec<BackupWarning>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupRecoveryStatus {
    pub required: bool,
}
pub(crate) fn warning(code: &str, category: BackupComponent) -> BackupWarning {
    BackupWarning {
        code: code.into(),
        category,
        entity_id: None,
    }
}
pub(crate) fn all_components() -> Vec<BackupComponent> {
    vec![
        BackupComponent::Configuration,
        BackupComponent::History,
        BackupComponent::Credentials,
    ]
}
pub(crate) fn selection(selection: &BackupSelection) -> Result<BackupSelection, BackupFailure> {
    Ok(BackupSelection::new(selection.components.clone())?)
}
