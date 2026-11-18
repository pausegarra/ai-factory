use super::*;
use crate::context::factory::application::{
    dto::backup_format::{BackupSnapshot, ValidatedBackup},
    interfaces::backup_archive::BackupArchive,
};
use crate::context::factory::infrastructure::filesystem::backup_archive::ZipBackupArchive;
use std::{
    collections::HashMap,
    path::Path,
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

struct NoCredentials;
impl Credentials for NoCredentials {
    fn get(&self, _: &str) -> crate::Result<Option<String>> {
        panic!("unexpected credential read")
    }
    fn set(&self, _: &str, _: &str) -> crate::Result<()> {
        panic!("unexpected credential write")
    }
    fn delete(&self, _: &str) -> crate::Result<()> {
        panic!("unexpected credential deletion")
    }
}

struct ReadBarrier {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
type ReadControl = (mpsc::Receiver<()>, mpsc::Sender<()>);
fn read_barrier() -> (ReadBarrier, ReadControl) {
    let (entered, entry) = mpsc::channel();
    let (release, released) = mpsc::channel();
    (
        ReadBarrier {
            entered,
            release: released,
        },
        (entry, release),
    )
}

// Only scheduling is controlled: Factory, registry, ZIP validation, SQLite
// preparation and the eventual apply all execute their production code.
struct PausedArchive {
    reads: Mutex<HashMap<PathBuf, ReadBarrier>>,
}
impl BackupArchive for PausedArchive {
    fn inspect(&self, path: &Path) -> Result<BackupInspection> {
        ZipBackupArchive.inspect(path)
    }
    fn read(
        &self,
        path: &Path,
        selection: &BackupSelection,
        password: Option<&str>,
    ) -> Result<ValidatedBackup> {
        let barrier = self.reads.lock().unwrap().remove(path);
        if let Some(barrier) = barrier {
            barrier.entered.send(()).unwrap();
            barrier
                .release
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }
        ZipBackupArchive.read(path, selection, password)
    }
    fn write(
        &self,
        path: &Path,
        selection: &BackupSelection,
        snapshot: BackupSnapshot,
        password: Option<&str>,
    ) -> Result<BackupOutcome> {
        ZipBackupArchive.write(path, selection, snapshot, password)
    }
}

fn configuration() -> BackupSelection {
    BackupSelection::new(vec![BackupComponent::Configuration]).unwrap()
}
fn set_interval(factory: &Factory, interval: u64) {
    let mut settings = factory.settings().unwrap();
    settings.poll_interval_seconds = interval;
    factory.save_settings(settings).unwrap();
}
fn export_configuration(factory: &Factory, path: &Path, interval: u64) -> PathBuf {
    set_interval(factory, interval);
    factory
        .export_backup(path.into(), configuration(), None)
        .unwrap();
    path.canonicalize().unwrap()
}
fn preparation_order(newer_finishes_first: bool) {
    let root = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let mut factory = Factory::open_with_credentials(
        root.path().into(),
        "http://127.0.0.1:1",
        Arc::new(NoCredentials),
    )
    .unwrap();
    let a_path = export_configuration(&factory, &files.path().join("a.zip"), 31);
    let b_path = export_configuration(&factory, &files.path().join("b.zip"), 41);
    set_interval(&factory, 90);
    let (a_barrier, (a_entered, release_a)) = read_barrier();
    let (b_barrier, (b_entered, release_b)) = read_barrier();
    Arc::get_mut(&mut factory).unwrap().backup_archive = Arc::new(PausedArchive {
        reads: Mutex::new(HashMap::from([
            (a_path.clone(), a_barrier),
            (b_path.clone(), b_barrier),
        ])),
    });
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| factory.prepare_backup_import(a_path, configuration(), None));
        a_entered.recv_timeout(Duration::from_secs(10)).unwrap();
        let b = scope.spawn(|| factory.prepare_backup_import(b_path, configuration(), None));
        b_entered.recv_timeout(Duration::from_secs(10)).unwrap();
        if newer_finishes_first {
            release_b.send(()).unwrap();
            let b = b.join().unwrap();
            release_a.send(()).unwrap();
            (a.join().unwrap(), b)
        } else {
            release_a.send(()).unwrap();
            let a = a.join().unwrap();
            release_b.send(()).unwrap();
            (a, b.join().unwrap())
        }
    });
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.confirmation_id, b.confirmation_id);
    assert_eq!(factory.settings().unwrap().poll_interval_seconds, 90);
    // This is the UI's cleanup of the obsolete response, after B is displayed.
    factory.discard_backup_import(a.confirmation_id).unwrap();
    assert!(
        factory
            .apply_backup_import(b.confirmation_id.clone(), true, None)
            .unwrap()
            .committed
    );
    assert_eq!(factory.settings().unwrap().poll_interval_seconds, 41);
    assert_eq!(
        factory
            .apply_backup_import(b.confirmation_id, true, None)
            .unwrap_err()
            .code,
        BackupErrorCode::ConfirmationRequired
    );
}

#[test]
fn backup_newer_preparation_finishing_first_keeps_current_confirmation() {
    preparation_order(true);
}
#[test]
fn backup_newer_preparation_finishing_last_keeps_current_confirmation() {
    preparation_order(false);
}

#[test]
fn backup_failed_newer_preparation_does_not_revive_older_confirmation() {
    let root = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let mut factory = Factory::open_with_credentials(
        root.path().into(),
        "http://127.0.0.1:1",
        Arc::new(NoCredentials),
    )
    .unwrap();
    let path = export_configuration(&factory, &files.path().join("a.zip"), 31);
    set_interval(&factory, 90);
    let (barrier, (entered, release)) = read_barrier();
    Arc::get_mut(&mut factory).unwrap().backup_archive = Arc::new(PausedArchive {
        reads: Mutex::new(HashMap::from([(path.clone(), barrier)])),
    });
    let a = std::thread::scope(|scope| {
        let a = scope.spawn(|| factory.prepare_backup_import(path, configuration(), None));
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        let b =
            factory.prepare_backup_import(files.path().join("missing.zip"), configuration(), None);
        release.send(()).unwrap();
        let a = a.join().unwrap();
        assert_eq!(b.unwrap_err().code, BackupErrorCode::InvalidArchive);
        a.unwrap()
    });
    assert_eq!(
        factory
            .apply_backup_import(a.confirmation_id.clone(), true, None)
            .unwrap_err()
            .code,
        BackupErrorCode::ConfirmationRequired
    );
    factory.discard_backup_import(a.confirmation_id).unwrap();
    assert_eq!(factory.settings().unwrap().poll_interval_seconds, 90);
}
