mod common;
use std::sync::atomic::Ordering;
#[test]
fn recovery_blocks_mutations_until_new_unreferenced_credentials_can_be_cleaned() {
    let root = tempfile::tempdir().unwrap();
    drop(common::open(root.path().into()).unwrap());
    let backend = common::backend(root.path());
    let reference = "89a5ec1c-b045-4e2b-a3aa-253d51a05e0f";
    backend
        .values
        .lock()
        .unwrap()
        .insert(reference.into(), "staged fixture".into());
    backend.fail_delete.store(true, Ordering::SeqCst);
    let db = rusqlite::Connection::open(root.path().join("factory.sqlite3")).unwrap();
    db.execute(
        "INSERT INTO backup_import_operations VALUES(?1,?2,'[\"credentials\"]','{}','staged')",
        rusqlite::params![reference, "a".repeat(64)],
    )
    .unwrap();
    db.execute(
        "INSERT INTO backup_staged_credentials VALUES(?1,?1)",
        [reference],
    )
    .unwrap();
    let f = common::open(root.path().into()).unwrap();
    assert!(f.backup_recovery_status().required);
    assert!(f.save_settings(factory_core::Settings::default()).is_err());
    assert!(f.settings().is_ok());
    assert!(f.recover_backup_import().is_err());
    backend.fail_delete.store(false, Ordering::SeqCst);
    f.recover_backup_import().unwrap();
    assert!(!f.backup_recovery_status().required);
    assert!(f.snapshot().unwrap().service.last_error.is_none());
    assert!(!backend.values.lock().unwrap().contains_key(reference));
    assert!(f.save_settings(factory_core::Settings::default()).is_ok());
}

#[test]
fn credential_gc_excludes_keys_staged_by_a_backup_operation() {
    let root = tempfile::tempdir().unwrap();
    let backend = common::backend(root.path());
    let store=factory_core::context::factory::infrastructure::repositories::store::Store::open_with_credentials(root.path(),backend.clone()).unwrap();
    let reference = "89a5ec1c-b045-4e2b-a3aa-253d51a05e0f";
    backend
        .values
        .lock()
        .unwrap()
        .insert(reference.into(), "staged fixture".into());
    {
        let db = store.db.lock().unwrap();
        db.execute(
            "INSERT INTO backup_import_operations VALUES(?1,?2,'[]','{}','staged')",
            rusqlite::params![reference, "a".repeat(64)],
        )
        .unwrap();
        db.execute(
            "INSERT INTO backup_staged_credentials VALUES(?1,?1)",
            [reference],
        )
        .unwrap();
        db.execute("INSERT INTO credential_gc VALUES(?1)", [reference])
            .unwrap();
    }
    store
        .save_settings(&factory_core::Settings::default())
        .unwrap();
    assert!(backend.values.lock().unwrap().contains_key(reference));
}

#[test]
fn keychain_write_read_verification_and_cleanup_failures_preserve_old_state() {
    use common::backup::*;
    let src = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = common::open(src.path().into()).unwrap();
    let mut config = token_settings();
    config.poll_interval_seconds = 31;
    source.save_settings(config).unwrap();
    let chosen = factory_core::BackupSelection::new(vec![
        factory_core::BackupComponent::Configuration,
        factory_core::BackupComponent::Credentials,
    ])
    .unwrap();
    let zip = files.path().join("keys.zip");
    source
        .export_backup(zip.clone(), chosen.clone(), password())
        .unwrap();
    for failure in ["set", "get", "verify", "delete"] {
        let dst = tempfile::tempdir().unwrap();
        let target = common::open(dst.path().into()).unwrap();
        let mut config = token_settings();
        config.poll_interval_seconds = 67;
        config.token_profiles[0].token = Some("fixture-old-managed-secret".into());
        target.save_settings(config).unwrap();
        let backend = common::backend(dst.path());
        let refs = table_rows(dst.path(), "credential_refs");
        let old = backend.values.lock().unwrap().clone();
        let settings = table_rows(dst.path(), "settings");
        let ticket = target
            .prepare_backup_import(zip.clone(), chosen.clone(), password())
            .unwrap();
        backend
            .fail_set
            .store(matches!(failure, "set" | "delete"), Ordering::SeqCst);
        backend.fail_get.store(failure == "get", Ordering::SeqCst);
        backend
            .corrupt_write
            .store(failure == "verify", Ordering::SeqCst);
        backend
            .fail_delete
            .store(failure == "delete", Ordering::SeqCst);
        let error = target
            .apply_backup_import(ticket.confirmation_id, true, password())
            .unwrap_err();
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("fixture-old-managed-secret"));
        assert_eq!(table_rows(dst.path(), "settings"), settings);
        assert_eq!(table_rows(dst.path(), "credential_refs"), refs);
        assert_eq!(*backend.values.lock().unwrap(), old);
        assert_eq!(
            target.backup_recovery_status().required,
            failure == "delete"
        );
        backend.fail_set.store(false, Ordering::SeqCst);
        backend.fail_get.store(false, Ordering::SeqCst);
        backend.corrupt_write.store(false, Ordering::SeqCst);
        backend.fail_delete.store(false, Ordering::SeqCst);
        if failure == "delete" {
            target.recover_backup_import().unwrap();
            assert!(!target.backup_recovery_status().required);
        }
        assert_eq!(*backend.values.lock().unwrap(), old);
    }
}
