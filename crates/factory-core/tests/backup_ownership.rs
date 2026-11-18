mod common;
#[test]
fn imported_origin_does_not_read_or_delete_local_files_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let f = common::open(root.path().into()).unwrap();
    let id = f
        .enqueue(factory_core::TriggerEvent::issue(
            "acme/repo",
            89,
            "[AI] fixture",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    f.cancel(&id).unwrap();
    let node = f.run_detail(&id).unwrap().nodes[0].id.clone();
    let sentinel = root
        .path()
        .join("sessions")
        .join(&id)
        .join(format!("{node}.log"));
    std::fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    std::fs::write(&sentinel, b"previous private local log").unwrap();
    let workspace = root.path().join("workspaces").join(&id).join("sentinel");
    std::fs::create_dir_all(workspace.parent().unwrap()).unwrap();
    std::fs::write(&workspace, b"keep workspace").unwrap();
    let db = rusqlite::Connection::open(root.path().join("factory.sqlite3")).unwrap();
    db.execute(
        "UPDATE run_file_origins SET origin='imported' WHERE run_id=?1",
        [&id],
    )
    .unwrap();
    db.execute(
        "INSERT INTO observations VALUES(?1,'invalid json must remain unread')",
        [format!("identity:{id}")],
    )
    .unwrap();
    let logs = f.logs(&id, &node, 1024).unwrap();
    assert!(logs.text.is_empty());
    assert_eq!((logs.offset, logs.next_offset, logs.eof), (0, 0, true));
    drop(f);
    for _ in 0..2 {
        let reopened = common::open(root.path().into()).unwrap();
        assert!(reopened.logs(&id, &node, 0).unwrap().text.is_empty());
        drop(reopened);
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"previous private local log"
        );
        assert_eq!(std::fs::read(&workspace).unwrap(), b"keep workspace");
    }
    let reopened = common::open(root.path().into()).unwrap();
    let retry = reopened.retry(&id).unwrap();
    assert_ne!(retry, id);
    assert_eq!(origin(root.path(), &retry), "local");
    reopened.cancel(&retry).unwrap();
    reopened.delete_run(&retry).unwrap();
    assert!(matches!(
        reopened.delete_run(&id).unwrap(),
        factory_core::DeleteRunOutcome::Deleted { .. }
    ));
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"previous private local log"
    );
    assert_eq!(std::fs::read(&workspace).unwrap(), b"keep workspace");
    let metadata: String = db
        .query_row(
            "SELECT json FROM observations WHERE key=?1",
            [format!("identity:{id}")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(metadata, "invalid json must remain unread");
}

use common::backup::*;

#[test]
fn colliding_imports_do_not_adopt_local_files_after_reopen_or_delete() {
    let source_dir = tempfile::tempdir().unwrap();
    let destination_dir = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = common::open(source_dir.path().to_owned()).unwrap();
    let ids = seed_mixed_backup_history(source_dir.path(), &source);
    let destination = common::open(destination_dir.path().to_owned()).unwrap();
    let mut sentinels = Vec::new();
    let mut metadata_keys = Vec::new();
    for id in &ids {
        let d = source.run_detail(id).unwrap();
        seed_local_collision(destination_dir.path(), &d);
        sentinels.extend(seed_local_files_and_metadata(destination_dir.path(), &d));
        metadata_keys.extend(local_metadata_keys(id));
    }
    let file_bytes = sentinels
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect::<Vec<_>>();
    let metadata = local_metadata(destination_dir.path(), &metadata_keys);
    let settings = table_rows(destination_dir.path(), "settings");
    let zip = files.path().join("collision.zip");
    source.export_backup(zip.clone(), history(), None).unwrap();
    destination.inspect_backup(zip.clone()).unwrap();
    let prepared = destination
        .prepare_backup_import(zip, history(), None)
        .unwrap();
    assert!(
        destination
            .apply_backup_import(prepared.confirmation_id, true, None)
            .unwrap()
            .committed
    );
    assert_eq!(table_rows(destination_dir.path(), "settings"), settings);
    for id in &ids {
        let node = &source.run_detail(id).unwrap().nodes[0].id;
        let logs = destination.logs(id, node, 1024).unwrap();
        assert!(logs.text.is_empty());
        assert_eq!((logs.offset, logs.next_offset, logs.eof), (0, 0, true));
        assert_eq!(origin(destination_dir.path(), id), "imported");
    }
    drop(destination);
    for _ in 0..2 {
        let reopened = common::open(destination_dir.path().to_owned()).unwrap();
        for id in &ids {
            let d = reopened.run_detail(id).unwrap();
            assert!(d.run.workspace.is_none());
            assert!(d.nodes.iter().all(|n| n.session_id.is_none()));
            assert!(reopened
                .logs(id, &d.nodes[0].id, 0)
                .unwrap()
                .text
                .is_empty());
            assert_eq!(origin(destination_dir.path(), id), "imported");
        }
        assert_eq!(
            sentinels
                .iter()
                .map(|path| std::fs::read(path).unwrap())
                .collect::<Vec<_>>(),
            file_bytes
        );
        assert_eq!(
            local_metadata(destination_dir.path(), &metadata_keys),
            metadata
        );
        drop(reopened);
    }
    let reopened = common::open(destination_dir.path().to_owned()).unwrap();
    for id in &ids {
        assert!(matches!(
            reopened.delete_run(id).unwrap(),
            factory_core::DeleteRunOutcome::Deleted { .. }
        ));
        assert!(matches!(
            reopened.run_view(id).unwrap(),
            factory_core::RunView::Missing
        ));
    }
    assert_eq!(
        sentinels
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>(),
        file_bytes
    );
    assert_eq!(
        local_metadata(destination_dir.path(), &metadata_keys),
        metadata
    );
    assert!(table_rows(destination_dir.path(), "run_file_origins").is_empty());
    assert!(table_rows(destination_dir.path(), "nodes").is_empty());
    assert!(table_rows(destination_dir.path(), "issue_node_comments").is_empty());
}

#[test]
fn history_origin_failure_rolls_back_local_ownership_and_all_rows() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = common::open(src.path().into()).unwrap();
    let ids = seed_mixed_backup_history(src.path(), &source);
    let target = common::open(dst.path().into()).unwrap();
    let mut paths = Vec::new();
    for id in &ids {
        let detail = source.run_detail(id).unwrap();
        seed_local_collision(dst.path(), &detail);
        paths.extend(seed_local_files_and_metadata(dst.path(), &detail));
    }
    let before = table_rows(dst.path(), "runs");
    let origins = table_rows(dst.path(), "run_file_origins");
    let path = files.path().join("history.zip");
    source.export_backup(path.clone(), history(), None).unwrap();
    let db = rusqlite::Connection::open(dst.path().join("factory.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER deny_origin BEFORE INSERT ON run_file_origins WHEN NEW.origin='imported' AND substr(NEW.run_id,1,7)='legacy-' BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    let ticket = target.prepare_backup_import(path, history(), None).unwrap();
    assert!(target
        .apply_backup_import(ticket.confirmation_id, true, None)
        .is_err());
    assert_eq!(table_rows(dst.path(), "runs"), before);
    assert_eq!(table_rows(dst.path(), "run_file_origins"), origins);
    for path in paths {
        assert_eq!(
            std::fs::read(path).unwrap(),
            b"LOCAL FILE: do not adopt or remove"
        );
    }
}
#[test]
fn imported_deletion_crash_recovers_without_files_and_rejects_injected_targets() {
    for corrupt in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let factory = common::open(root.path().into()).unwrap();
        let id = factory
            .enqueue(factory_core::TriggerEvent::issue(
                "acme/repo",
                89,
                "[AI] fixture",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        factory.cancel(&id).unwrap();
        let detail = factory.run_detail(&id).unwrap();
        let files = seed_local_files_and_metadata(root.path(), &detail);
        let db = rusqlite::Connection::open(root.path().join("factory.sqlite3")).unwrap();
        db.execute(
            "UPDATE run_file_origins SET origin='imported' WHERE run_id=?1",
            [&id],
        )
        .unwrap();
        db.execute(
            "INSERT INTO run_deletions VALUES(?1,'acme/repo',89,'[AI] fixture',1,'pending',NULL)",
            [&id],
        )
        .unwrap();
        if corrupt {
            db.execute(
                "INSERT INTO run_deletion_targets VALUES(?1,'workspace',0)",
                [&id],
            )
            .unwrap();
        }
        drop(factory);
        let reopened = common::open(root.path().into());
        if corrupt {
            assert!(reopened.is_err());
            db.execute("DELETE FROM run_deletion_targets WHERE run_id=?1", [&id])
                .unwrap();
            let f = common::open(root.path().into()).unwrap();
            assert!(matches!(
                f.run_view(&id).unwrap(),
                factory_core::RunView::Missing
            ));
        } else {
            assert!(matches!(
                reopened.unwrap().run_view(&id).unwrap(),
                factory_core::RunView::Missing
            ));
        }
        for file in files {
            assert_eq!(
                std::fs::read(file).unwrap(),
                b"LOCAL FILE: do not adopt or remove"
            );
        }
    }
}
