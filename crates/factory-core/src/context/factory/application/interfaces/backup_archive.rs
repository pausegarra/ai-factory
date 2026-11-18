use crate::context::factory::application::dto::backup_format::*;
use crate::*;
use std::path::Path;
pub trait BackupArchive: Send + Sync {
    fn inspect(&self, path: &Path) -> std::result::Result<BackupInspection, BackupFailure>;
    fn read(
        &self,
        path: &Path,
        selection: &BackupSelection,
        password: Option<&str>,
    ) -> std::result::Result<ValidatedBackup, BackupFailure>;
    fn write(
        &self,
        path: &Path,
        selection: &BackupSelection,
        snapshot: BackupSnapshot,
        password: Option<&str>,
    ) -> std::result::Result<BackupOutcome, BackupFailure>;
}
