pub fn history() -> factory_core::BackupSelection {
    factory_core::BackupSelection::new(vec![factory_core::BackupComponent::History]).unwrap()
}
pub fn configuration() -> factory_core::BackupSelection {
    factory_core::BackupSelection::new(vec![factory_core::BackupComponent::Configuration]).unwrap()
}
pub fn all() -> factory_core::BackupSelection {
    factory_core::BackupSelection::new(vec![
        factory_core::BackupComponent::Configuration,
        factory_core::BackupComponent::History,
        factory_core::BackupComponent::Credentials,
    ])
    .unwrap()
}
pub fn password() -> Option<zeroize::Zeroizing<String>> {
    Some(zeroize::Zeroizing::new("fixture password 89".to_owned()))
}
pub fn token_settings() -> factory_core::Settings {
    let mut s = factory_core::Settings::default();
    s.token_profiles.push(factory_core::TokenProfile {
        id: "backup-profile".into(),
        label: "Backup test".into(),
        account: "pausegarra".into(),
        token: Some("fixture-managed-token-89".into()),
        has_token: false,
        ..Default::default()
    });
    s
}
pub fn seed_run(root: &std::path::Path, title: &str) -> String {
    let store = factory_core::context::factory::infrastructure::repositories::store::Store::open_with_credentials(
        root, super::backend(root)).unwrap();
    let event =
        factory_core::TriggerEvent::issue("pausegarra/ai-factory", 89, title, "pausegarra", "");
    store
        .insert(&event, &factory_core::Settings::default().flows[0], 1, true)
        .unwrap()
        .unwrap()
}
pub fn table_rows(root: &std::path::Path, table: &str) -> Vec<String> {
    assert!([
        "settings",
        "runs",
        "nodes",
        "credential_refs",
        "notification_credential_refs",
        "dedup",
        "observations",
        "issue_node_comments",
        "tracked_pr_origins",
        "run_file_origins"
    ]
    .contains(&table));
    let db = rusqlite::Connection::open(root.join("factory.sqlite3")).unwrap();
    let mut stmt = db
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap();
    let count = stmt.column_count();
    let rows = stmt
        .query_map([], |row| {
            Ok((0..count)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows
}
pub fn seed_mixed_backup_history(
    root: &std::path::Path,
    factory: &factory_core::Factory,
) -> Vec<String> {
    use rusqlite::{params, Connection};
    use sha2::{Digest, Sha256};
    let modern = seed_run(root, "[AI] modern backup fixture");
    let legacy_dir = tempfile::tempdir().unwrap();
    let legacy_db = legacy_dir.path().join("old.sqlite");
    let old = Connection::open(&legacy_db).unwrap();
    old.execute_batch(r#"
      CREATE TABLE tasks(key TEXT PRIMARY KEY,repository TEXT,issue_number INTEGER,issue TEXT,status TEXT,launch_phase TEXT,attempts INTEGER,detected_at TEXT,launched_at TEXT,finished_at TEXT,last_error TEXT);
      INSERT INTO tasks VALUES('acme/repo#7','acme/repo',7,'{"repository":"acme/repo","number":7,"title":"[AI] old","body":"body","author":"alice","url":"https://github.com/acme/repo/issues/7"}','running','confirmed',2,'2026-01-01T00:00:00Z','2026-01-01T00:01:00Z',NULL,NULL);
      CREATE TABLE workflows(key TEXT,phase TEXT,trigger_name TEXT,pull_requests TEXT,created_at TEXT,updated_at TEXT);
      INSERT INTO workflows VALUES('acme/repo#7','dag','new_issue','[{"repository":"acme/repo","number":9,"url":"https://github.com/acme/repo/pull/9","author":"alice","head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]','2026-01-01T00:00:00Z','2026-01-01T00:01:00Z');
      CREATE TABLE trigger_events(event_id TEXT,trigger_name TEXT,state TEXT,payload TEXT,run_key TEXT);
      INSERT INTO trigger_events VALUES('acme/repo#9@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','pr_approved','claimed','{}','old');
      CREATE TABLE pr_approval_comments(repository TEXT,pull_number INTEGER,comment_id INTEGER,event_id TEXT);
      INSERT INTO pr_approval_comments VALUES('acme/repo',9,44,'old');
      CREATE TABLE reviewer_requests(repository TEXT,pull_number INTEGER,reviewer TEXT,requested INTEGER,generation INTEGER);
      INSERT INTO reviewer_requests VALUES('acme/repo',9,'bob',1,3);
      CREATE TABLE workflow_nodes(workflow_key TEXT,node_id TEXT,state TEXT,session_id TEXT,started_at TEXT,finished_at TEXT,last_error TEXT);
      INSERT INTO workflow_nodes VALUES('acme/repo#7','builder','running','session-1','2026-01-01T00:01:00Z',NULL,NULL);
      CREATE TABLE workflow_sessions(id TEXT,workflow_key TEXT,node_id TEXT,provider TEXT,event_path TEXT,result_path TEXT,prompt_path TEXT);
      INSERT INTO workflow_sessions VALUES('session-1','acme/repo#7','builder','codex',NULL,NULL,NULL);
    "#).unwrap();
    drop(old);
    assert_eq!(
        factory
            .import_legacy(None, Some(legacy_db))
            .unwrap()
            .runs_imported,
        1
    );
    let legacy = factory
        .snapshot()
        .unwrap()
        .runs
        .into_iter()
        .find(|r| r.id.starts_with("legacy-"))
        .unwrap()
        .id;
    let ids = vec![modern, legacy];
    let db = Connection::open(root.join("factory.sqlite3")).unwrap();
    // Fixture terminal sin callbacks: evita iniciar workers o transportes.
    db.execute(
        "INSERT INTO dedup VALUES('fixture:already-consumed','consumed')",
        [],
    )
    .unwrap();
    for id in &ids {
        db.execute("UPDATE runs SET json=json_set(json,'$.status','succeeded','$.current_step',NULL,'$.finished_at','2026-01-01T00:02:00Z') WHERE id=?1", [id]).unwrap();
        db.execute("UPDATE nodes SET json=json_set(json,'$.status','succeeded','$.finished_at','2026-01-01T00:02:00Z') WHERE run_id=?1", [id]).unwrap();
        let d = factory.run_detail(id).unwrap();
        let n = &d.nodes[0];
        let digest = Sha256::digest(
            serde_json::to_vec(&serde_json::json!(["node-result:v1", id, n.id, 1])).unwrap(),
        );
        let key = format!("node-result:v1:{digest:x}");
        let snapshot = serde_json::json!({"run_id":id,"node_id":n.id,"node_name":n.name,
            "repository":d.run.repository,"number":d.run.number,"actor":d.run.actor,
            "run_attempt":d.run.attempt,"revision":1,"status":"succeeded","summary":null,"error":null});
        db.execute("INSERT INTO issue_node_comments(key,run_id,node_id,revision,snapshot,state,failures,next_at,last_error,receipt) VALUES(?1,?2,?3,1,?4,'delivered',0,'2026-01-01T00:02:00Z',NULL,?5)",
            params![key,id,n.id,snapshot.to_string(),r#"{"comment_id":89,"html_url":null}"#]).unwrap();
    }
    ids
}

pub fn origin(root: &std::path::Path, run: &str) -> String {
    rusqlite::Connection::open(root.join("factory.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT origin FROM run_file_origins WHERE run_id=?1",
            [run],
            |r| r.get(0),
        )
        .unwrap()
}

pub fn seed_local_collision(root: &std::path::Path, detail: &factory_core::RunDetail) {
    use rusqlite::params;
    let mut db = rusqlite::Connection::open(root.join("factory.sqlite3")).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    let tx = db.transaction().unwrap();
    let mut run = detail.run.clone();
    run.title = "[AI] previous local destination run".into();
    tx.execute(
        "INSERT INTO runs VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            run.id,
            run.event_id,
            serde_json::to_string(&run).unwrap(),
            serde_json::to_string(&detail.event).unwrap(),
            serde_json::to_string(&detail.flow).unwrap(),
            run.created_at
        ],
    )
    .unwrap();
    tx.execute("INSERT INTO run_file_origins VALUES(?1,'local')", [&run.id])
        .unwrap();
    for n in &detail.nodes {
        tx.execute(
            "INSERT INTO nodes VALUES(?1,?2,?3)",
            params![run.id, n.id, serde_json::to_string(n).unwrap()],
        )
        .unwrap();
    }
    tx.commit().unwrap();
}

pub fn local_metadata_keys(run: &str) -> Vec<String> {
    [
        "identity",
        "pr-workspace",
        "version-workspace",
        "release-workspace",
        "pr-release",
    ]
    .into_iter()
    .map(|prefix| format!("{prefix}:{run}"))
    .chain([
        format!("review-feedback:{run}:builder"),
        format!("review-history:{run}:fixture"),
    ])
    .collect()
}

pub fn local_metadata(root: &std::path::Path, keys: &[String]) -> Vec<String> {
    let db = rusqlite::Connection::open(root.join("factory.sqlite3")).unwrap();
    keys.iter()
        .map(|key| {
            db.query_row("SELECT json FROM observations WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .unwrap()
        })
        .collect()
}

pub fn seed_local_files_and_metadata(
    root: &std::path::Path,
    d: &factory_core::RunDetail,
) -> Vec<std::path::PathBuf> {
    let (owner, repo) = d.run.repository.split_once('/').unwrap();
    let id = &d.run.id;
    let paths = vec![
        root.join("sessions")
            .join(id)
            .join(format!("{}.log", d.nodes[0].id)),
        root.join("memory")
            .join(owner)
            .join(repo)
            .join("runs")
            .join(id)
            .join("sentinel"),
        root.join("workspaces").join(id).join("sentinel"),
        root.join("legacy").join(id).join("sentinel"),
    ];
    for path in &paths {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"LOCAL FILE: do not adopt or remove").unwrap();
    }
    let db = rusqlite::Connection::open(root.join("factory.sqlite3")).unwrap();
    for key in local_metadata_keys(id) {
        db.execute(
            "INSERT INTO observations VALUES(?1,?2)",
            rusqlite::params![key, r#"{"path":"/fixture/local-private","keep":true}"#],
        )
        .unwrap();
    }
    paths
}
