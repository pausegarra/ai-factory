mod common;
use common::backup::*;
use factory_core::*;
#[test]
fn stale_confirmation_does_not_consume_current_preview() {
    let root = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let f = common::open(root.path().into()).unwrap();
    let mut settings = f.settings().unwrap();
    settings.poll_interval_seconds = 31;
    f.save_settings(settings.clone()).unwrap();
    let path = files.path().join("configuration.zip");
    f.export_backup(path.clone(), configuration(), None)
        .unwrap();
    settings.poll_interval_seconds = 90;
    f.save_settings(settings).unwrap();
    let obsolete = f
        .prepare_backup_import(path.clone(), configuration(), None)
        .unwrap();
    f.discard_backup_import(obsolete.confirmation_id.clone())
        .unwrap();
    let current = f
        .prepare_backup_import(path, configuration(), None)
        .unwrap();
    assert_ne!(obsolete.confirmation_id, current.confirmation_id);
    assert_eq!(
        f.apply_backup_import(obsolete.confirmation_id, true, None)
            .unwrap_err()
            .code,
        BackupErrorCode::StalePreview
    );
    assert_eq!(f.settings().unwrap().poll_interval_seconds, 90);
    assert!(
        f.apply_backup_import(current.confirmation_id.clone(), true, None)
            .unwrap()
            .committed
    );
    assert_eq!(f.settings().unwrap().poll_interval_seconds, 31);
    assert_eq!(
        f.apply_backup_import(current.confirmation_id, true, None)
            .unwrap_err()
            .code,
        BackupErrorCode::ConfirmationRequired
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_is_busy_until_an_admitted_poll_finishes() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (entered, entry) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut bytes = [0; 8192];
        let _ = socket.read(&mut bytes).unwrap();
        entered.send(()).unwrap();
        released
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let body = r#"{"login":"alice"}"#;
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let f = common::open_with_api(root.path().into(), &format!("http://{addr}")).unwrap();
    let store=factory_core::context::factory::infrastructure::repositories::store::Store::open_with_credentials(root.path(),common::backend(root.path())).unwrap();
    let mut settings = Settings::default();
    settings.token_profiles.push(TokenProfile {
        id: "work".into(),
        account: "alice".into(),
        label: "Work".into(),
        owners: vec!["acme".into()],
        authors: vec!["alice".into()],
        token: Some("fixture-managed-token".into()),
        ..Default::default()
    });
    store.save_settings(&settings).unwrap();
    drop(store);
    let path = files.path().join("history.zip");
    f.export_backup(path.clone(), history(), None).unwrap();
    let ticket = f
        .prepare_backup_import(path.clone(), history(), None)
        .unwrap();
    let polling = {
        let f = f.clone();
        tokio::spawn(async move { f.poll_now().await })
    };
    tokio::task::spawn_blocking(move || {
        entry
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
    })
    .await
    .unwrap();
    let error = f
        .apply_backup_import(ticket.confirmation_id, true, None)
        .unwrap_err();
    assert_eq!(error.code, BackupErrorCode::OperationBusy);
    release.send(()).unwrap();
    let _ = polling.await.unwrap();
    server.join().unwrap();
    let ticket = f.prepare_backup_import(path, history(), None).unwrap();
    assert!(
        f.apply_backup_import(ticket.confirmation_id, true, None)
            .unwrap()
            .committed
    );
    f.shutdown().await;
}
