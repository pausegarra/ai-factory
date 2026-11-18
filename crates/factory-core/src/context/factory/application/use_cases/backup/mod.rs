pub(crate) mod registry;
#[cfg(test)]
mod tests;
use crate::context::factory::application::{
    interfaces::backup_repository::*, services::maintenance::OperationKind,
};
use crate::*;
use std::{path::PathBuf, time::Instant};
use zeroize::Zeroizing;
type Result<T> = std::result::Result<T, BackupFailure>;
impl Factory {
    pub fn export_backup(
        &self,
        path: PathBuf,
        selection: BackupSelection,
        password: Option<Zeroizing<String>>,
    ) -> Result<BackupOutcome> {
        let _operation = self.maintenance.try_enter(OperationKind::ReadOnly)?;
        let selection = crate::context::factory::application::dto::backup::selection(&selection)?;
        let parent = path
            .parent()
            .ok_or_else(BackupFailure::persistence_failed)?
            .canonicalize()
            .map_err(|_| BackupFailure::persistence_failed())?;
        if parent.starts_with(&self.root) {
            return Err(BackupFailure::invalid_archive());
        }
        let path = parent.join(
            path.file_name()
                .ok_or_else(BackupFailure::invalid_archive)?,
        );
        self.backup_archive.write(
            &path,
            &selection,
            self.store.capture(&selection)?,
            password.as_deref().map(|v| v.as_str()),
        )
    }
    pub fn inspect_backup(&self, path: PathBuf) -> Result<BackupInspection> {
        let _operation = self.maintenance.try_enter(OperationKind::ReadOnly)?;
        self.backup_archive.inspect(&path)
    }
    pub fn prepare_backup_import(
        &self,
        path: PathBuf,
        selection: BackupSelection,
        password: Option<Zeroizing<String>>,
    ) -> Result<PreparedBackupImport> {
        let _operation = self.maintenance.try_enter(OperationKind::ReadOnly)?;
        let confirmation_id = self.backup_registry.lock().unwrap().begin_preparation();
        let selection = crate::context::factory::application::dto::backup::selection(&selection)?;
        let path = path
            .canonicalize()
            .map_err(|_| BackupFailure::invalid_archive())?;
        let before = self.store.fingerprint()?;
        let archive =
            self.backup_archive
                .read(&path, &selection, password.as_deref().map(|v| v.as_str()))?;
        let sha256 = archive.inspection.archive_sha256.clone();
        let plan = self.store.prepare(archive, selection.clone())?;
        let fingerprint = self.store.fingerprint()?;
        if before != fingerprint {
            return Err(BackupFailure::new(BackupErrorCode::StalePreview));
        }
        let summary = PreparedBackupImport {
            confirmation_id,
            replace: selection.components.clone(),
            preserve: crate::context::factory::application::dto::backup::all_components()
                .into_iter()
                .filter(|c| !selection.contains(*c))
                .collect(),
            counts: crate::context::factory::application::dto::backup_format::counts(
                plan.effective_configuration
                    .as_ref()
                    .filter(|_| selection.contains(BackupComponent::Configuration)),
                plan.normalized_history.as_ref(),
                selection
                    .contains(BackupComponent::Credentials)
                    .then_some(&plan.credential_updates),
            ),
            warnings: plan.warnings,
        };
        self.backup_registry
            .lock()
            .unwrap()
            .publish(registry::BackupTicket {
                summary: summary.clone(),
                path,
                sha256,
                fingerprint,
                selection,
                created: Instant::now(),
            });
        Ok(summary)
    }
    pub fn apply_backup_import(
        &self,
        confirmation_id: String,
        confirmed: bool,
        password: Option<Zeroizing<String>>,
    ) -> Result<BackupOutcome> {
        let ticket = self
            .backup_registry
            .lock()
            .unwrap()
            .consume(&confirmation_id)?;
        if !confirmed {
            return Err(BackupFailure::new(BackupErrorCode::ConfirmationRequired));
        }
        let mut exclusive = self.maintenance.try_restore()?;
        if self.store.fingerprint()? != ticket.fingerprint {
            return Err(BackupFailure::new(BackupErrorCode::StalePreview));
        }
        let archive = self
            .backup_archive
            .read(
                &ticket.path,
                &ticket.selection,
                password.as_deref().map(|v| v.as_str()),
            )
            .map_err(|e| {
                if e.code == BackupErrorCode::InvalidPassword {
                    e
                } else {
                    BackupFailure::new(BackupErrorCode::StalePreview)
                }
            })?;
        if archive.inspection.archive_sha256 != ticket.sha256 {
            return Err(BackupFailure::new(BackupErrorCode::StalePreview));
        }
        let plan = self.store.prepare(archive, ticket.selection)?;
        let outcome = match self.store.apply(plan) {
            Ok(outcome) => outcome,
            Err(error) => {
                if error.code == BackupErrorCode::RecoveryRequired {
                    self.maintenance.require_recovery();
                }
                return Err(error);
            }
        };
        *self.github_http.lock().unwrap() = Default::default();
        self.reconcile_backup_dispatchers();
        exclusive.commit_epoch();
        drop(exclusive);
        self.emit("backup_imported", None, None);
        if outcome.components.contains(&BackupComponent::Configuration) {
            self.emit("settings_changed", None, None);
        }
        if outcome.components.contains(&BackupComponent::History) {
            self.emit("history_imported", None, None);
        }
        self.wake.notify_one();
        Ok(outcome)
    }
    pub fn discard_backup_import(&self, confirmation_id: String) -> Result<()> {
        self.backup_registry
            .lock()
            .unwrap()
            .discard(&confirmation_id);
        Ok(())
    }
    pub fn backup_recovery_status(&self) -> BackupRecoveryStatus {
        BackupRecoveryStatus {
            required: self.maintenance.recovery_required(),
        }
    }
    pub fn recover_backup_import(&self) -> Result<()> {
        let exclusive = self.maintenance.try_recover()?;
        self.store.recover()?;
        let cleanup_errors = self
            .complete_backup_recovery()
            .map_err(|_| BackupFailure::persistence_failed())?;
        self.service.lock().unwrap().last_error =
            (!cleanup_errors.is_empty()).then(|| "Workspace cleanup remains pending".into());
        exclusive.recovered();
        drop(exclusive);
        self.emit("service_changed", None, None);
        self.wake.notify_one();
        Ok(())
    }
}
