use super::*;
use crate::context::factory::application::interfaces::startup_registration::StartupRegistration;
use std::sync::mpsc;
use std::time::Duration;

struct NoCredentials;
impl Credentials for NoCredentials {
    fn get(&self, _: &str) -> Result<Option<String>> {
        panic!("unexpected credential read")
    }
    fn set(&self, _: &str, _: &str) -> Result<()> {
        panic!("unexpected credential write")
    }
    fn delete(&self, _: &str) -> Result<()> {
        panic!("unexpected credential deletion")
    }
}
fn open_factory(root: &std::path::Path) -> Arc<Factory> {
    Factory::open_with_credentials(root.into(), "http://127.0.0.1:1", Arc::new(NoCredentials))
        .unwrap()
}

#[derive(Default)]
struct FakeStartup {
    enabled: bool,
    reads: usize,
    changes: Vec<bool>,
    fail_read: bool,
    fail_set_at: Option<usize>,
}
impl StartupRegistration for FakeStartup {
    fn is_enabled(&mut self) -> Result<bool> {
        self.reads += 1;
        ensure!(!self.fail_read, "read denied");
        Ok(self.enabled)
    }
    fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        self.changes.push(enabled);
        ensure!(
            self.fail_set_at != Some(self.changes.len()),
            "setter denied"
        );
        self.enabled = enabled;
        Ok(())
    }
}
fn desired(factory: &Factory) -> Settings {
    let mut settings = factory.settings().unwrap();
    settings.start_at_login = true;
    settings
}
fn assert_denied(factory: &Factory, settings: Settings, code: BackupErrorCode) {
    let mut startup = FakeStartup::default();
    let error = factory
        .save_settings_with_startup(settings, &mut startup)
        .unwrap_err();
    assert_eq!(error.downcast_ref::<BackupFailure>().unwrap().code, code);
    assert_eq!(startup.reads, 0);
    assert!(startup.changes.is_empty());
}
#[test]
fn settings_update_rejects_before_reading_or_setting_startup_during_restore_recovery_and_stopping()
{
    let dir = tempfile::tempdir().unwrap();
    let factory = open_factory(dir.path());
    // Even saving another preference must not reconcile an out-of-sync OS entry.
    factory.save_settings(desired(&factory)).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.poll_interval_seconds += 1;
    let exclusive = factory.maintenance.try_restore().unwrap();
    assert_denied(&factory, settings.clone(), BackupErrorCode::OperationBusy);
    drop(exclusive);
    factory.maintenance.require_recovery();
    assert_denied(
        &factory,
        settings.clone(),
        BackupErrorCode::RecoveryRequired,
    );
    factory.maintenance.try_recover().unwrap().recovered();
    factory.maintenance.request_shutdown();
    assert_denied(&factory, settings, BackupErrorCode::OperationBusy);
}
#[test]
fn settings_update_persists_registration_and_unchanged_os_state_without_rewriting_it() {
    let dir = tempfile::tempdir().unwrap();
    let factory = open_factory(dir.path());
    let mut settings = desired(&factory);
    let mut startup = FakeStartup::default();
    factory
        .save_settings_with_startup(settings.clone(), &mut startup)
        .unwrap();
    assert!(startup.enabled);
    assert_eq!(startup.changes, vec![true]);
    assert!(factory.settings().unwrap().start_at_login);
    startup.changes.clear();
    settings.poll_interval_seconds = 90;
    factory
        .save_settings_with_startup(settings, &mut startup)
        .unwrap();
    assert!(startup.changes.is_empty());
    assert_eq!(factory.settings().unwrap().poll_interval_seconds, 90);
}
#[test]
fn settings_update_does_not_persist_after_startup_query_or_set_failure() {
    let dir = tempfile::tempdir().unwrap();
    let factory = open_factory(dir.path());
    let settings = desired(&factory);
    let before = serde_json::to_value(factory.settings().unwrap()).unwrap();
    for fail_read in [true, false] {
        let mut startup = FakeStartup {
            fail_read,
            fail_set_at: Some(1),
            ..Default::default()
        };
        assert!(factory
            .save_settings_with_startup(settings.clone(), &mut startup)
            .is_err());
        assert_eq!(startup.changes.len(), usize::from(!fail_read));
        assert_eq!(
            serde_json::to_value(factory.settings().unwrap()).unwrap(),
            before
        );
        assert!(factory.maintenance.try_restore().is_ok());
    }
}
fn fail_persistence(factory: &Factory) {
    factory.store.db.lock().unwrap().execute_batch(
        "CREATE TRIGGER reject_settings_update BEFORE INSERT ON settings BEGIN SELECT RAISE(ABORT,'fixture disk full'); END;"
    ).unwrap();
}
#[test]
fn settings_update_rolls_back_startup_and_reports_both_errors_if_compensation_fails() {
    for fail_rollback in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let factory = open_factory(dir.path());
        let settings = desired(&factory);
        let before = serde_json::to_value(factory.settings().unwrap()).unwrap();
        fail_persistence(&factory);
        let mut startup = FakeStartup {
            fail_set_at: fail_rollback.then_some(2),
            ..Default::default()
        };
        let error = factory
            .save_settings_with_startup(settings, &mut startup)
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("fixture disk full"));
        assert_eq!(
            message.contains("Could not restore login setting: setter denied"),
            fail_rollback
        );
        assert_eq!(startup.changes, vec![true, false]);
        assert_eq!(startup.enabled, fail_rollback);
        assert_eq!(
            serde_json::to_value(factory.settings().unwrap()).unwrap(),
            before
        );
        assert!(factory.maintenance.try_restore().is_ok());
    }
}
struct SuspendedStartup<'a> {
    factory: &'a Factory,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
    suspend_at: usize,
    changes: Vec<bool>,
}
impl StartupRegistration for SuspendedStartup<'_> {
    fn is_enabled(&mut self) -> Result<bool> {
        Ok(false)
    }
    fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        self.changes.push(enabled);
        if self.changes.len() == self.suspend_at {
            self.entered.send(()).unwrap();
            self.release.recv_timeout(Duration::from_secs(10)).unwrap();
            // A previously admitted operation must finish after shutdown is requested.
            self.factory.maintenance.request_shutdown();
        }
        Ok(())
    }
}
#[test]
fn settings_update_blocks_restore_through_suspended_setter_and_compensation_without_readmission() {
    for suspend_at in [1, 2] {
        let dir = tempfile::tempdir().unwrap();
        let factory = open_factory(dir.path());
        let settings = desired(&factory);
        if suspend_at == 2 {
            fail_persistence(&factory);
        }
        let history = BackupSelection::new(vec![BackupComponent::History]).unwrap();
        let files = tempfile::tempdir().unwrap();
        let archive = files.path().join("history.zip");
        factory
            .export_backup(archive.clone(), history.clone(), None)
            .unwrap();
        let ticket = factory
            .prepare_backup_import(archive, history, None)
            .unwrap();
        let (entered, entry) = mpsc::channel();
        let (release, released) = mpsc::channel();
        std::thread::scope(|scope| {
            let factory_ref = &factory;
            let saving = scope.spawn(move || {
                let factory = factory_ref;
                let mut startup = SuspendedStartup {
                    factory,
                    entered,
                    release: released,
                    suspend_at,
                    changes: Vec::new(),
                };
                let result = factory.save_settings_with_startup(settings, &mut startup);
                (result, startup.changes)
            });
            entry.recv_timeout(Duration::from_secs(10)).unwrap();
            let attempted = factory.apply_backup_import(ticket.confirmation_id, true, None);
            // Always release the worker before asserting to avoid hanging on failure.
            release.send(()).unwrap();
            let (result, changes) = saving.join().unwrap();
            assert!(matches!(
                attempted,
                Err(BackupFailure {
                    code: BackupErrorCode::OperationBusy,
                    ..
                })
            ));
            if suspend_at == 1 {
                result.unwrap();
                assert_eq!(changes, vec![true]);
                assert!(factory.settings().unwrap().start_at_login);
            } else {
                assert!(format!("{:#}", result.unwrap_err()).contains("fixture disk full"));
                assert_eq!(changes, vec![true, false]);
                assert!(!factory.settings().unwrap().start_at_login);
            }
        });
    }
}
