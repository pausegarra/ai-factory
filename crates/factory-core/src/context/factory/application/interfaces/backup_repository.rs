use crate::context::factory::application::dto::backup_format::*;
use crate::*;
pub trait BackupRepository {
    fn capture(
        &self,
        selection: &BackupSelection,
    ) -> std::result::Result<BackupSnapshot, BackupFailure>;
    fn fingerprint(&self) -> std::result::Result<String, BackupFailure>;
    fn prepare(
        &self,
        archive: ValidatedBackup,
        selection: BackupSelection,
    ) -> std::result::Result<RestorePlan, BackupFailure>;
}
pub trait BackupRestoreRepository: BackupRepository {
    fn apply(&self, plan: RestorePlan) -> std::result::Result<BackupOutcome, BackupFailure>;
    fn recover(&self) -> std::result::Result<(), BackupFailure>;
}
