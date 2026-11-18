mod common;
use factory_core::*;
use std::sync::atomic::Ordering;
fn selection(mask: u8) -> BackupSelection {
    BackupSelection::new(
        [
            BackupComponent::Configuration,
            BackupComponent::History,
            BackupComponent::Credentials,
        ]
        .into_iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, v)| v)
        .collect(),
    )
    .unwrap()
}
fn seed(root: &std::path::Path, title: &str, number: u64) -> String {
    let s= factory_core::context::factory::infrastructure::repositories::store::Store::open_with_credentials(root,common::backend(root)).unwrap();
    let mut settings = Settings {
        poll_interval_seconds: number + 30,
        ..Settings::default()
    };
    settings.token_profiles.push(TokenProfile {
        id: "profile".into(),
        label: title.into(),
        account: "alice".into(),
        token: Some(format!("fixture-managed-secret-{number}")),
        ..Default::default()
    });
    s.save_settings(&settings).unwrap();
    s.insert(
        &TriggerEvent::issue("acme/repo", number, title, "alice", "body"),
        &settings.flows[0],
        1,
        true,
    )
    .unwrap()
    .unwrap()
}
#[test]
fn seven_selections_replace_only_selected_categories_and_require_confirmation() {
    for mask in 1..8 {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let files = tempfile::tempdir().unwrap();
        let source = common::open(src.path().into()).unwrap();
        let destination = common::open(dst.path().into()).unwrap();
        let source_id = seed(src.path(), "source", 1);
        let destination_id = seed(dst.path(), "destination", 2);
        let old_settings = destination.settings().unwrap();
        let old_values = common::backend(dst.path()).values.lock().unwrap().clone();
        let path = files.path().join("test.zip");
        let chosen = selection(mask);
        let password = || {
            if mask & 4 != 0 {
                Some(zeroize::Zeroizing::new("fixture password 89".into()))
            } else {
                None
            }
        };
        source
            .export_backup(path.clone(), chosen.clone(), password())
            .unwrap();
        let inspection = destination.inspect_backup(path.clone()).unwrap();
        assert_eq!(inspection.components, chosen.components);
        let prepared = destination
            .prepare_backup_import(path.clone(), chosen.clone(), password())
            .unwrap();
        assert!(destination
            .apply_backup_import(prepared.confirmation_id, false, password())
            .is_err());
        assert_eq!(destination.snapshot().unwrap().runs[0].id, destination_id);
        let prepared = destination
            .prepare_backup_import(path, chosen, password())
            .unwrap();
        let result = destination
            .apply_backup_import(prepared.confirmation_id.clone(), true, password())
            .unwrap();
        assert!(result.committed);
        assert!(destination
            .apply_backup_import(prepared.confirmation_id, true, password())
            .is_err());
        let history = destination.snapshot().unwrap();
        assert_eq!(
            history.runs[0].id,
            if mask & 2 != 0 {
                source_id
            } else {
                destination_id
            }
        );
        if mask & 2 != 0 {
            assert_eq!(history.runs[0].status, "interrupted");
        }
        assert_eq!(
            destination.settings().unwrap().poll_interval_seconds,
            if mask & 1 != 0 {
                31
            } else {
                old_settings.poll_interval_seconds
            }
        );
        for (key, value) in old_values {
            assert_eq!(
                common::backend(dst.path()).values.lock().unwrap().get(&key),
                Some(&value)
            );
        }
    }
}
#[test]
fn incorrect_password_and_sql_failure_leave_all_categories_unchanged() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = common::open(src.path().into()).unwrap();
    let destination = common::open(dst.path().into()).unwrap();
    seed(src.path(), "source", 1);
    let before = seed(dst.path(), "destination", 2);
    let path = files.path().join("test.zip");
    source
        .export_backup(
            path.clone(),
            selection(7),
            Some(zeroize::Zeroizing::new("fixture password 89".into())),
        )
        .unwrap();
    assert!(destination
        .prepare_backup_import(
            path.clone(),
            selection(7),
            Some(zeroize::Zeroizing::new("wrong password 89".into()))
        )
        .is_err());
    let credentials = common::backend(dst.path());
    let values = credentials.values.lock().unwrap().clone();
    let db = rusqlite::Connection::open(dst.path().join("factory.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER deny_backup BEFORE INSERT ON nodes BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    let prepared = destination
        .prepare_backup_import(
            path,
            selection(7),
            Some(zeroize::Zeroizing::new("fixture password 89".into())),
        )
        .unwrap();
    assert!(destination
        .apply_backup_import(
            prepared.confirmation_id,
            true,
            Some(zeroize::Zeroizing::new("fixture password 89".into()))
        )
        .is_err());
    assert_eq!(destination.snapshot().unwrap().runs[0].id, before);
    assert_eq!(*credentials.values.lock().unwrap(), values);
    assert_eq!(destination.settings().unwrap().poll_interval_seconds, 32);
    credentials.fail_set.store(false, Ordering::SeqCst);
}
#[test]
fn changed_archive_and_changed_local_data_invalidate_preview() {
    let root = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let f = common::open(root.path().into()).unwrap();
    seed(root.path(), "source", 1);
    let path = files.path().join("test.zip");
    f.export_backup(path.clone(), selection(2), None).unwrap();
    let preview = f
        .prepare_backup_import(path.clone(), selection(2), None)
        .unwrap();
    seed(root.path(), "changed", 2);
    assert!(f
        .apply_backup_import(preview.confirmation_id, true, None)
        .is_err());
    let preview = f
        .prepare_backup_import(path.clone(), selection(2), None)
        .unwrap();
    std::fs::write(path, b"corrupt").unwrap();
    assert!(f
        .apply_backup_import(preview.confirmation_id, true, None)
        .is_err());
}

#[test]
fn mixed_modern_and_legacy_history_survives_roundtrip_and_reopen() {
    use common::backup::{history, origin, seed_mixed_backup_history, table_rows};
    use std::collections::BTreeSet;
    fn pr_origins(root: &std::path::Path) -> Vec<(String, bool)> {
        let db = rusqlite::Connection::open(root.join("factory.sqlite3")).unwrap();
        let mut query=db.prepare("SELECT json_array(repository,number,origin_key,event_id,issue_repository,issue_number,actor,trigger_id),trusted FROM tracked_pr_origins ORDER BY repository,number,origin_key").unwrap();
        let rows = query
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }
    let source_dir = tempfile::tempdir().unwrap();
    let destination_dir = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = common::open(source_dir.path().to_owned()).unwrap();
    let ids = seed_mixed_backup_history(source_dir.path(), &source);
    let expected_origins = pr_origins(source_dir.path())
        .into_iter()
        .map(|(row, _)| row)
        .collect::<Vec<_>>();
    assert!(!expected_origins.is_empty());
    let expected_ids = ids.iter().cloned().collect::<BTreeSet<_>>();
    let mut dedup = table_rows(source_dir.path(), "dedup");
    dedup.sort();
    let db = rusqlite::Connection::open(source_dir.path().join("factory.sqlite3")).unwrap();
    for sentinel in ["consumed", "legacy-consumed"] {
        assert!(db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM dedup WHERE run_id=?1)",
                [sentinel],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
    }
    drop(db);
    let mut comments = table_rows(source_dir.path(), "issue_node_comments");
    comments.sort();
    assert_eq!(comments.len(), 2);
    let zip = files.path().join("mixed.zip");
    source.export_backup(zip.clone(), history(), None).unwrap();
    let destination = common::open(destination_dir.path().to_owned()).unwrap();
    assert_eq!(
        destination.inspect_backup(zip.clone()).unwrap().components,
        history().components
    );
    let prepared = destination
        .prepare_backup_import(zip, history(), None)
        .unwrap();
    assert!(
        destination
            .apply_backup_import(prepared.confirmation_id, true, None)
            .unwrap()
            .committed
    );
    drop(destination);
    let reopened = common::open(destination_dir.path().to_owned()).unwrap();
    assert_eq!(
        reopened
            .snapshot()
            .unwrap()
            .runs
            .into_iter()
            .map(|r| r.id)
            .collect::<BTreeSet<_>>(),
        expected_ids
    );
    let mut actual_dedup = table_rows(destination_dir.path(), "dedup");
    actual_dedup.sort();
    assert_eq!(actual_dedup, dedup);
    let mut actual_comments = table_rows(destination_dir.path(), "issue_node_comments");
    actual_comments.sort();
    assert_eq!(actual_comments, comments);
    for id in &ids {
        let before = source.run_detail(id).unwrap();
        let after = reopened.run_detail(id).unwrap();
        assert_eq!(after.run.id, before.run.id);
        assert_eq!(after.run.event_id, before.run.event_id);
        assert_eq!(after.run.repository, before.run.repository);
        assert_eq!(after.run.attempt, before.run.attempt);
        let mut expected_run = before.run.clone();
        expected_run.workspace = None;
        assert_eq!(
            serde_json::to_value(&after.run).unwrap(),
            serde_json::to_value(&expected_run).unwrap()
        );
        let mut expected_flow = before.flow.clone();
        for node in &mut expected_flow.nodes {
            node.codex_account_id = None;
        }
        assert_eq!(
            serde_json::to_value(&after.flow).unwrap(),
            serde_json::to_value(&expected_flow).unwrap()
        );
        let mut expected_nodes = before.nodes.clone();
        for node in &mut expected_nodes {
            node.session_id = None;
        }
        assert_eq!(
            serde_json::to_value(&after.nodes).unwrap(),
            serde_json::to_value(&expected_nodes).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&after.event).unwrap(),
            serde_json::to_value(&before.event).unwrap()
        );
        assert_eq!(
            after.nodes.iter().map(|n| &n.id).collect::<BTreeSet<_>>(),
            before.nodes.iter().map(|n| &n.id).collect()
        );
        assert!(after.nodes.iter().all(|n| n.session_id.is_none()));
        assert_eq!(origin(destination_dir.path(), id), "imported");
    }
    let third_dir = tempfile::tempdir().unwrap();
    let third = common::open(third_dir.path().to_owned()).unwrap();
    let second_zip = files.path().join("mixed-again.zip");
    reopened
        .export_backup(second_zip.clone(), history(), None)
        .unwrap();
    third.inspect_backup(second_zip.clone()).unwrap();
    let prepared = third
        .prepare_backup_import(second_zip, history(), None)
        .unwrap();
    third
        .apply_backup_import(prepared.confirmation_id, true, None)
        .unwrap();
    assert_eq!(
        third
            .snapshot()
            .unwrap()
            .runs
            .into_iter()
            .map(|r| r.id)
            .collect::<BTreeSet<_>>(),
        expected_ids
    );
    let mut actual_dedup = table_rows(third_dir.path(), "dedup");
    actual_dedup.sort();
    assert_eq!(actual_dedup, dedup);
    let mut actual_comments = table_rows(third_dir.path(), "issue_node_comments");
    actual_comments.sort();
    assert_eq!(actual_comments, comments);
    for root in [destination_dir.path(), third_dir.path()] {
        let origins = pr_origins(root);
        assert!(origins.iter().all(|(_, trusted)| !*trusted));
        assert_eq!(
            origins.into_iter().map(|(row, _)| row).collect::<Vec<_>>(),
            expected_origins
        );
    }
}

#[test]
fn credentials_only_preserves_unrelated_references_and_agent_bindings() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = common::open(src.path().into()).unwrap();
    let target = common::open(dst.path().into()).unwrap();
    seed(src.path(), "source", 1);
    seed(dst.path(), "target", 2);
    let db = rusqlite::Connection::open(dst.path().join("factory.sqlite3")).unwrap();
    db.execute("UPDATE settings SET json=json_set(json,'$.codex_bindings.profile','89a5ec1c-b045-4e2b-a3aa-253d51a05e0f')",[]).unwrap();
    db.execute("INSERT INTO credential_refs VALUES('unrelated','other','91a5ec1c-b045-4e2b-a3aa-253d51a05e0f')",[]).unwrap();
    let before = target.settings().unwrap();
    let path = files.path().join("credentials.zip");
    source
        .export_backup(
            path.clone(),
            selection(4),
            Some(zeroize::Zeroizing::new("fixture password 89".into())),
        )
        .unwrap();
    let ticket = target
        .prepare_backup_import(
            path,
            selection(4),
            Some(zeroize::Zeroizing::new("fixture password 89".into())),
        )
        .unwrap();
    target
        .apply_backup_import(
            ticket.confirmation_id,
            true,
            Some(zeroize::Zeroizing::new("fixture password 89".into())),
        )
        .unwrap();
    assert_eq!(
        target.settings().unwrap().codex_bindings,
        before.codex_bindings
    );
    assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM credential_refs WHERE profile_id='unrelated' AND reference='91a5ec1c-b045-4e2b-a3aa-253d51a05e0f')",[],|r|r.get::<_,bool>(0)).unwrap());
    let reference: String = db
        .query_row(
            "SELECT reference FROM credential_refs WHERE profile_id='profile'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        common::backend(dst.path())
            .values
            .lock()
            .unwrap()
            .get(&reference)
            .unwrap(),
        "fixture-managed-secret-1"
    );
}

#[test]
fn configuration_reference_matching_is_exact_and_missing_credentials_are_visible() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = common::open(root.path().into()).unwrap();
    let target = common::open(other.path().into()).unwrap();
    seed(root.path(), "source", 1);
    seed(other.path(), "other", 2);
    let path = files.path().join("configuration.zip");
    source
        .export_backup(path.clone(), selection(1), None)
        .unwrap();
    let refs = common::backup::table_rows(root.path(), "credential_refs");
    let ticket = source
        .prepare_backup_import(path.clone(), selection(1), None)
        .unwrap();
    source
        .apply_backup_import(ticket.confirmation_id, true, None)
        .unwrap();
    assert!(source.settings().unwrap().token_profiles[0].has_token);
    assert_eq!(
        common::backup::table_rows(root.path(), "credential_refs"),
        refs
    );
    let ticket = target
        .prepare_backup_import(path, selection(1), None)
        .unwrap();
    target
        .apply_backup_import(ticket.confirmation_id, true, None)
        .unwrap();
    assert!(!target.settings().unwrap().token_profiles[0].has_token);
    assert!(common::backup::table_rows(other.path(), "credential_refs").is_empty());
}
#[test]
fn all_managed_credential_providers_roundtrip_and_public_variants_are_redacted() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = common::open(src.path().into()).unwrap();
    let target = common::open(dst.path().into()).unwrap();
    common::backend(dst.path()).values.lock().unwrap().insert(
        "unrelated-native-reference".into(),
        "fixture-unrelated-secret".into(),
    );
    let mut value = serde_json::to_value(common::backup::token_settings()).unwrap();
    value["notification_connectors"] = serde_json::json!([
        {"id":"telegram","label":"Telegram","enabled":true,"events":["run_failed"],"type":"telegram","config":{"chat_id":"-1001","bot_token":"123456:fixture_telegram_secret_abcdefghijklmnopqrstuvwxyz","triggers":["new_issue"],"token_profile_ids":["backup-profile"]}},
        {"id":"chat","label":"Chat","enabled":true,"events":["run_failed"],"type":"google_chat","config":{"webhook_url":"https://chat.googleapis.com/v1/spaces/fixture/messages?key=PRIVATE_KEY&token=PRIVATE_TOKEN","triggers":["new_issue"],"token_profile_ids":["backup-profile"]}}
    ]);
    value["token_profiles"][0]["token"] = "fixture-managed-token-89".into();
    value["flows"][0]["nodes"][0]["prompt"] = "url parts PRIVATE_KEY and PRIVATE_TOKEN".into();
    source
        .save_settings(serde_json::from_value(value).unwrap())
        .unwrap();
    let path = files.path().join("all.zip");
    source
        .export_backup(
            path.clone(),
            common::backup::all(),
            common::backup::password(),
        )
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    for secret in [
        "fixture-managed-token-89",
        "123456:fixture_telegram_secret_abcdefghijklmnopqrstuvwxyz",
        "PRIVATE_KEY",
        "PRIVATE_TOKEN",
    ] {
        assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()));
    }
    let ticket = target
        .prepare_backup_import(path, common::backup::all(), common::backup::password())
        .unwrap();
    target
        .apply_backup_import(ticket.confirmation_id, true, common::backup::password())
        .unwrap();
    let settings = target.settings().unwrap();
    assert!(settings.token_profiles[0].has_token);
    assert!(settings.notification_connectors.iter().all(|c| c.enabled));
    fn assert_secrets_by_identity(root: &std::path::Path) {
        let db = rusqlite::Connection::open(root.join("factory.sqlite3")).unwrap();
        let github: String = db.query_row(
            "SELECT reference FROM credential_refs WHERE profile_id='backup-profile' AND account='pausegarra'",
            [], |row| row.get(0)
        ).unwrap();
        let telegram: String = db
            .query_row(
                "SELECT reference FROM notification_credential_refs WHERE connector_id='telegram'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let chat: String = db
            .query_row(
                "SELECT reference FROM notification_credential_refs WHERE connector_id='chat'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(github, telegram);
        assert_ne!(github, chat);
        assert_ne!(telegram, chat);
        let backend = common::backend(root);
        let secrets = backend.values.lock().unwrap();
        assert_eq!(
            secrets.get(&github).map(String::as_str),
            Some("fixture-managed-token-89")
        );
        assert_eq!(
            secrets.get(&telegram).map(String::as_str),
            Some("123456:fixture_telegram_secret_abcdefghijklmnopqrstuvwxyz")
        );
        assert_eq!(secrets.get(&chat).map(String::as_str), Some("https://chat.googleapis.com/v1/spaces/fixture/messages?key=PRIVATE_KEY&token=PRIVATE_TOKEN"));
        assert_eq!(
            secrets
                .get("unrelated-native-reference")
                .map(String::as_str),
            Some("fixture-unrelated-secret")
        );
        assert_eq!(secrets.len(), 4);
    }
    assert_secrets_by_identity(dst.path());
    drop(target);
    let reopened = common::open(dst.path().into()).unwrap();
    assert!(reopened.settings().unwrap().token_profiles[0].has_token);
    assert!(reopened
        .settings()
        .unwrap()
        .notification_connectors
        .iter()
        .all(|c| c.enabled));
    assert_secrets_by_identity(dst.path());
}

#[test]
fn history_only_restores_while_destination_keychain_is_locked() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = common::open(src.path().into()).unwrap();
    let target = common::open(dst.path().into()).unwrap();
    let source_id = common::backup::seed_run(src.path(), "[AI] source");
    seed(dst.path(), "target", 2);
    let before = common::backup::table_rows(dst.path(), "credential_refs");
    let path = files.path().join("history.zip");
    source
        .export_backup(path.clone(), selection(2), None)
        .unwrap();
    common::backend(dst.path())
        .fail_get
        .store(true, Ordering::SeqCst);
    let ticket = target
        .prepare_backup_import(path, selection(2), None)
        .unwrap();
    target
        .apply_backup_import(ticket.confirmation_id, true, None)
        .unwrap();
    assert_eq!(target.snapshot().unwrap().runs[0].id, source_id);
    assert_eq!(
        common::backup::table_rows(dst.path(), "credential_refs"),
        before
    );
}

#[test]
fn imported_agent_binding_requires_a_ready_local_account_with_valid_home() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = common::open(src.path().into()).unwrap();
    let target = common::open(dst.path().into()).unwrap();
    seed(src.path(), "source", 1);
    let account = "89a5ec1c-b045-4e2b-a3aa-253d51a05e0f";
    common::seed_codex_account(&source, account);
    common::seed_codex_account(&target, account);
    std::fs::remove_file(
        dst.path()
            .join("agent-accounts/codex")
            .join(account)
            .join("auth.json"),
    )
    .unwrap();
    let mut settings = source.settings().unwrap();
    settings
        .codex_bindings
        .insert("profile".into(), account.into());
    source.save_settings(settings).unwrap();
    let path = files.path().join("config.zip");
    source
        .export_backup(path.clone(), selection(1), None)
        .unwrap();
    let ticket = target
        .prepare_backup_import(path, selection(1), None)
        .unwrap();
    target
        .apply_backup_import(ticket.confirmation_id, true, None)
        .unwrap();
    assert!(target.settings().unwrap().codex_bindings.is_empty());
}
