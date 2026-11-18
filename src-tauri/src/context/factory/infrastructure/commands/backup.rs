use factory_core::{
    BackupFailure, BackupInspection, BackupOutcome, BackupRecoveryStatus, BackupSelection,
    PreparedBackupImport,
};
async fn backup_blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, BackupFailure> + Send + 'static,
) -> Result<T, BackupFailure> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|_| BackupFailure::persistence_failed())?
}
#[tauri::command]
pub(crate) async fn export_backup(
    state: tauri::State<'_, crate::DesktopState>,
    path: String,
    selection: BackupSelection,
    password: Option<String>,
) -> Result<BackupOutcome, BackupFailure> {
    let factory = state.factory.clone();
    let password = password.map(zeroize::Zeroizing::new);
    backup_blocking(move || factory.export_backup(path.into(), selection, password)).await
}
#[tauri::command]
pub(crate) async fn inspect_backup(
    state: tauri::State<'_, crate::DesktopState>,
    path: String,
) -> Result<BackupInspection, BackupFailure> {
    let factory = state.factory.clone();
    backup_blocking(move || factory.inspect_backup(path.into())).await
}
#[tauri::command]
pub(crate) async fn prepare_backup_import(
    state: tauri::State<'_, crate::DesktopState>,
    path: String,
    selection: BackupSelection,
    password: Option<String>,
) -> Result<PreparedBackupImport, BackupFailure> {
    let factory = state.factory.clone();
    let password = password.map(zeroize::Zeroizing::new);
    backup_blocking(move || factory.prepare_backup_import(path.into(), selection, password)).await
}
#[tauri::command]
pub(crate) async fn apply_backup_import(
    state: tauri::State<'_, crate::DesktopState>,
    confirmation_id: String,
    confirmed: bool,
    password: Option<String>,
) -> Result<BackupOutcome, BackupFailure> {
    let factory = state.factory.clone();
    let password = password.map(zeroize::Zeroizing::new);
    backup_blocking(move || factory.apply_backup_import(confirmation_id, confirmed, password)).await
}
#[tauri::command]
pub(crate) async fn discard_backup_import(
    state: tauri::State<'_, crate::DesktopState>,
    confirmation_id: String,
) -> Result<(), BackupFailure> {
    let factory = state.factory.clone();
    backup_blocking(move || factory.discard_backup_import(confirmation_id)).await
}
#[tauri::command]
pub(crate) async fn recover_backup_import(
    state: tauri::State<'_, crate::DesktopState>,
) -> Result<(), BackupFailure> {
    let factory = state.factory.clone();
    backup_blocking(move || factory.recover_backup_import()).await
}
#[tauri::command]
pub(crate) fn get_backup_recovery_status(
    state: tauri::State<'_, crate::DesktopState>,
) -> BackupRecoveryStatus {
    state.factory.backup_recovery_status()
}
