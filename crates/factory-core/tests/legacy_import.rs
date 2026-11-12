mod common;
use factory_core::*;
use rusqlite::Connection;
use std::{fs, path::Path};

#[path = "../src/context/factory/infrastructure/migration/legacy.rs"]
mod legacy;

fn config(path: &Path) {
    fs::write(path.join("prompt.md"), "Build from imported prompt.").unwrap();
    fs::write(
        path.join("schema.json"),
        r#"{"type":"object","required":["approved"],"properties":{"approved":{"type":"boolean"}}}"#,
    )
    .unwrap();
    fs::write(
        path.join("config.yaml"),
        r#"
github:
  owners: [ACME]
  authors: [Alice]
  reviewers: [Bob]
poll_seconds: 5
commands: {codex: /tools/codex, gh: gh, git: git}
triggers:
  new_issue:
    type: new_issue
    workflow:
      max_parallel: 3
      nodes:
        spec:
          agent: codex
          model: gpt-test
          effort: high
          access: write
          instructions_file: prompt.md
          result_schema: &approval {type: object, properties: {approved: {type: boolean}}}
        review:
          agent: codex
          model: gpt-test
          access: read
          depends_on: [spec]
          instructions: Review it.
          result_schema: *approval
  pr_approved:
    type: pr_approved
    source: new_issue
    workflow:
      nodes:
        publish:
          agent: codex
          model: gpt-test
          access: write
          instructions: Publish it.
          result_schema: schema.json
"#,
    )
    .unwrap();
}

#[test]
fn legacy_config_resets_profile_activation() {
    let tmp = tempfile::tempdir().unwrap();
    config(tmp.path());
    let mut current = Settings {
        token_profiles: ["alice", "bob"]
            .into_iter()
            .map(|account| TokenProfile {
                id: account.into(),
                label: account.into(),
                account: account.into(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    for flow in &mut current.flows {
        flow.profile_activation.insert("work".into(), false);
        flow.profile_activation.insert("orphan".into(), false);
    }
    let batch = legacy::read_legacy(
        Some(&tmp.path().join("config.yaml")),
        None,
        &tmp.path().join("app"),
        &current,
    )
    .unwrap();
    let imported = batch.settings.unwrap();
    assert!(imported
        .flows
        .iter()
        .all(|f| f.profile_activation.is_empty()));
    for profile in &imported.token_profiles {
        assert_eq!(profile.owners, vec!["acme"]);
        assert_eq!(
            profile.authors,
            if profile.account == "alice" {
                vec!["alice"]
            } else {
                vec![]
            }
        );
        assert_eq!(
            profile.reviewers,
            if profile.account == "bob" {
                vec!["bob"]
            } else {
                vec![]
            }
        );
        assert!(!profile.has_token);
        assert!(profile.token.is_none());
    }
}

#[test]
fn imports_mapping_nodes_yaml_anchors_and_relative_files_without_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    config(tmp.path());
    let batch = legacy::read_legacy(
        Some(&tmp.path().join("config.yaml")),
        None,
        &tmp.path().join("app"),
        &Settings::default(),
    )
    .unwrap();
    let settings = batch.settings.unwrap();
    let canonical = serde_json::to_value(&settings).unwrap();
    for field in ["owners", "authors", "reviewers"] {
        assert!(canonical.get(field).is_none());
    }
    assert_eq!(settings.max_concurrent, 4);
    assert!(settings.token_profiles.is_empty());
    let issue = settings.flows.iter().find(|f| f.id == "new_issue").unwrap();
    assert_eq!(issue.nodes.len(), 2);
    assert_eq!(issue.max_parallel, 3);
    assert_eq!(
        issue.nodes.iter().find(|n| n.id == "spec").unwrap().prompt,
        "Build from imported prompt."
    );
    assert_eq!(issue.nodes[0].result_schema, issue.nodes[1].result_schema);
    assert!(
        !settings
            .flows
            .iter()
            .find(|f| f.id == "pr_review")
            .unwrap()
            .enabled
    );
    assert_eq!(
        settings
            .flows
            .iter()
            .find(|f| f.id == "pr_approved")
            .unwrap()
            .nodes[0]
            .result_schema["required"][0],
        "approved"
    );
    settings.validate().unwrap();
}

fn old_database(path: &Path) {
    let c = Connection::open(path).unwrap();
    c.execute_batch(r#"
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
 "#).unwrap();
}

#[test]
fn imports_readonly_history_interrupts_live_runs_and_preserves_dedup() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("old.sqlite");
    old_database(&path);
    let before = fs::read(&path).unwrap();
    let batch = legacy::read_legacy(
        None,
        Some(&path),
        &tmp.path().join("app"),
        &Settings::default(),
    )
    .unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(batch.runs.len(), 1);
    assert_eq!(batch.runs[0].detail.run.status, "interrupted");
    assert_eq!(batch.runs[0].detail.run.attempt, 2);
    assert_eq!(batch.runs[0].detail.run.created_at, "2026-01-01T00:00:00Z");
    assert!(batch.runs[0].detail.nodes.is_empty());
    assert!(batch
        .consumed_events
        .contains(&"issue:acme/repo#7".to_owned()));
    assert!(batch
        .consumed_events
        .contains(&"acme/repo#9@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned()));
    assert_eq!(batch.approval_comments[0].comment_id, 44);
    assert_eq!(batch.reviewer_requests[0].generation, 3);
    assert!(batch.reviewer_requests[0].requested);
    assert_eq!(batch.registered_prs[0].source_issue.number, 7);
    assert_eq!(batch.registered_prs[0].number, 9);
    assert!(batch.runs[0].raw["task"].is_object());
}

#[test]
fn imports_sessions_and_copies_accessible_logs_into_application_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("old.sqlite");
    old_database(&path);
    let log = tmp.path().join("events.jsonl");
    fs::write(&log, "session output legacy-secret\n").unwrap();
    let result = tmp.path().join("result.json");
    fs::write(
        &result,
        r#"{"status":"succeeded","summary":"done","data":{},"artifacts":[]}"#,
    )
    .unwrap();
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TABLE workflow_nodes(workflow_key TEXT,node_id TEXT,state TEXT,session_id TEXT,started_at TEXT,finished_at TEXT,last_error TEXT); INSERT INTO workflow_nodes VALUES('acme/repo#7','builder','running','session-1','2026-01-01T00:01:00Z',NULL,NULL); CREATE TABLE workflow_sessions(id TEXT,workflow_key TEXT,node_id TEXT,provider TEXT,event_path TEXT,result_path TEXT,prompt_path TEXT);").unwrap();
    c.execute("INSERT INTO workflow_sessions VALUES('session-1','acme/repo#7','builder','codex',?1,?2,NULL)",rusqlite::params![log.to_str(),result.to_str()]).unwrap();
    drop(c);
    let app = tmp.path().join("app");
    let batch = legacy::read_legacy(None, Some(&path), &app, &Settings::default()).unwrap();
    assert_eq!(batch.runs[0].detail.nodes[0].status, "interrupted");
    assert!(batch.runs[0].log_text.contains("session output"));
    assert_eq!(
        batch.runs[0].detail.nodes[0].result.as_ref().unwrap()["summary"],
        "done"
    );
    let copied = batch.runs[0].raw["copied_files"].as_array().unwrap();
    assert!(copied
        .iter()
        .any(|p| Path::new(p.as_str().unwrap()).starts_with(&app)));
    assert!(log.exists());
    let factory = common::open(app).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.token_profiles.push(factory_core::TokenProfile {
        id: "secret".into(),
        label: "Secret".into(),
        account: "alice".into(),
        token: Some("legacy-secret".into()),
        has_token: false,
        ..Default::default()
    });
    factory.save_settings(settings).unwrap();
    factory.import_legacy(None, Some(path)).unwrap();
    let id = &factory.snapshot().unwrap().runs[0].id;
    assert!(factory
        .logs(id, "builder", 0)
        .unwrap()
        .text
        .contains("session output"));
    assert!(!factory
        .logs(id, "builder", 0)
        .unwrap()
        .text
        .contains("legacy-secret"));
}

#[test]
fn rejects_absent_inputs_and_invalid_graph_without_changing_current_settings() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(legacy::read_legacy(None, None, tmp.path(), &Settings::default()).is_err());
    config(tmp.path());
    let path = tmp.path().join("config.yaml");
    let bad = fs::read_to_string(&path)
        .unwrap()
        .replace("depends_on: [spec]", "depends_on: [missing]");
    fs::write(&path, bad).unwrap();
    assert!(legacy::read_legacy(Some(&path), None, tmp.path(), &Settings::default()).is_err());
}

#[test]
fn restores_immutable_legacy_flow_snapshot_and_copies_project_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("old.sqlite");
    old_database(&db);
    let memory = tmp.path().join("old-memory");
    fs::create_dir(&memory).unwrap();
    fs::write(memory.join("context.md"), "Project memory preserved.").unwrap();
    let snapshot = tmp.path().join("workflow.json");
    fs::write(&snapshot,r#"{"max_parallel":2,"nodes":[{"id":"legacy_step","kind":"agent","access":"read","agent":"codex","model":"old-model","instructions":"Pinned old prompt.","depends_on":[],"result_schema":{"type":"object"}}]}"#).unwrap();
    let c = Connection::open(&db).unwrap();
    c.execute_batch("ALTER TABLE workflows ADD COLUMN definition_path TEXT; ALTER TABLE workflows ADD COLUMN documentation TEXT;").unwrap();
    c.execute(
        "UPDATE workflows SET definition_path=?1,documentation=?2",
        rusqlite::params![
            snapshot.to_str(),
            serde_json::json!({"memory_dir":memory}).to_string()
        ],
    )
    .unwrap();
    drop(c);
    let app = tmp.path().join("app");
    let first = legacy::read_legacy(None, Some(&db), &app, &Settings::default()).unwrap();
    assert_eq!(first.runs[0].detail.flow.nodes[0].id, "legacy_step");
    assert_eq!(
        first.runs[0].detail.flow.nodes[0].prompt,
        "Pinned old prompt."
    );
    assert_eq!(first.runs[0].detail.flow.max_parallel, 2);
    let copied = first.runs[0].raw["copied_files"].as_array().unwrap();
    assert!(copied
        .iter()
        .any(|p| fs::read_to_string(p.as_str().unwrap())
            .is_ok_and(|s| s == "Project memory preserved.")));
    assert!(first
        .archive_path
        .as_ref()
        .is_some_and(|p| Path::new(p).starts_with(&app) && Path::new(p).is_file()));
    let archive: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(first.archive_path.unwrap()).unwrap()).unwrap();
    assert_eq!(archive["reviewer_requests"][0]["generation"], 3);
    let second = legacy::read_legacy(None, Some(&db), &app, &Settings::default()).unwrap();
    assert_eq!(first.runs[0].detail.run.id, second.runs[0].detail.run.id);
}

#[test]
fn preserves_pending_event_dedup_without_queuing_legacy_work() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("old.sqlite");
    old_database(&db);
    let c = Connection::open(&db).unwrap();
    c.execute("INSERT INTO trigger_events VALUES('pr_review:acme/repo#9:bob:3','pr_review','pending','{}',NULL)",[]).unwrap();
    drop(c);
    let batch = legacy::read_legacy(
        None,
        Some(&db),
        &tmp.path().join("app"),
        &Settings::default(),
    )
    .unwrap();
    assert!(batch
        .consumed_events
        .contains(&"review:acme/repo#9:bob:3".into()));
    assert!(batch
        .consumed_events
        .contains(&"approval:acme/repo#9@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()));
    assert!(batch
        .runs
        .iter()
        .all(|r| r.detail.run.status != "queued" && r.detail.run.status != "running"));
}

#[test]
fn refuses_ambiguous_legacy_pull_request_provenance() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("old.sqlite");
    old_database(&db);
    let c = Connection::open(&db).unwrap();
    c.execute_batch("INSERT INTO tasks SELECT 'acme/repo#8',repository,8,replace(issue,'old','other'),'completed','finished',1,detected_at,launched_at,finished_at,last_error FROM tasks; INSERT INTO workflows SELECT 'acme/repo#8',phase,trigger_name,pull_requests,created_at,updated_at FROM workflows;").unwrap();
    drop(c);
    let batch = legacy::read_legacy(
        None,
        Some(&db),
        &tmp.path().join("app"),
        &Settings::default(),
    )
    .unwrap();
    assert!(batch.registered_prs.is_empty());
    assert_eq!(batch.pr_conflicts.len(), 2);
    assert!(batch.warnings.iter().any(|w| w.contains("ambiguous")));
}

#[test]
fn factory_ingests_legacy_history_idempotently_and_keeps_secrets_private() {
    let source = tempfile::tempdir().unwrap();
    let database = source.path().join("old.sqlite3");
    let c = rusqlite::Connection::open(&database).unwrap();
    c.execute_batch("CREATE TABLE tasks(key TEXT PRIMARY KEY,repository TEXT,issue_number INTEGER,issue TEXT,status TEXT,attempts INTEGER,detected_at TEXT);").unwrap();
    c.execute("INSERT INTO tasks VALUES(?1,?2,?3,?4,?5,1,?6)",rusqlite::params!["acme/repo#8","acme/repo",8,serde_json::json!({"repository":"acme/repo","number":8,"title":"[AI] old","body":"body","url":"https://github.com/acme/repo/issues/8","author":"alice","state":"open"}).to_string(),"succeeded","2026-01-01T00:00:00Z"]).unwrap();
    drop(c);
    let root = tempfile::tempdir().unwrap();
    let f = common::open(root.path().into()).unwrap();
    let mut settings = f.settings().unwrap();
    settings.flows[0]
        .profile_activation
        .insert("orphan".into(), false);
    f.save_settings(settings).unwrap();
    let local = f
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            99,
            "[AI] local",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    let before = serde_json::to_value(f.run_detail(&local).unwrap()).unwrap();
    let report = f.import_legacy(None, Some(database.clone())).unwrap();
    assert_eq!(report.runs_imported, 1);
    assert!(report.archive_path.is_some());
    assert_eq!(f.snapshot().unwrap().metrics.total, 2);
    assert_eq!(
        f.settings().unwrap().flows[0]
            .profile_activation
            .get("orphan"),
        Some(&false)
    );
    assert_eq!(
        serde_json::to_value(f.run_detail(&local).unwrap()).unwrap(),
        before
    );
    let imported = f
        .snapshot()
        .unwrap()
        .runs
        .into_iter()
        .find(|r| r.id != local)
        .unwrap();
    assert!(f
        .run_detail(&imported.id)
        .unwrap()
        .flow
        .profile_activation
        .is_empty());
    let again = f.import_legacy(None, Some(database)).unwrap();
    assert_eq!(again.runs_imported, 0);
    assert_eq!(f.snapshot().unwrap().metrics.total, 2);
    assert!(f
        .enqueue(factory_core::TriggerEvent::issue(
            "acme/repo",
            8,
            "[AI] old",
            "alice",
            ""
        ))
        .unwrap()
        .is_none());
}

fn legacy_with_local_files(root: &Path) -> (std::path::PathBuf, Vec<std::path::PathBuf>) {
    let db = root.join("old.sqlite");
    old_database(&db);
    let log = root.join("events.jsonl");
    fs::write(&log, "legacy session output\n").unwrap();
    let memory = root.join("old-memory");
    fs::create_dir(&memory).unwrap();
    let context = memory.join("context.md");
    fs::write(&context, "legacy project memory").unwrap();
    let c = Connection::open(&db).unwrap();
    c.execute_batch("CREATE TABLE workflow_nodes(workflow_key TEXT,node_id TEXT,state TEXT,session_id TEXT,started_at TEXT,finished_at TEXT,last_error TEXT); INSERT INTO workflow_nodes VALUES('acme/repo#7','builder','running','session-1',NULL,NULL,NULL); CREATE TABLE workflow_sessions(id TEXT,workflow_key TEXT,node_id TEXT,provider TEXT,event_path TEXT); ALTER TABLE workflows ADD COLUMN documentation TEXT;").unwrap();
    c.execute(
        "INSERT INTO workflow_sessions VALUES('session-1','acme/repo#7','builder','codex',?1)",
        [log.to_str()],
    )
    .unwrap();
    c.execute(
        "UPDATE workflows SET documentation=?1",
        [serde_json::json!({"memory_dir": memory}).to_string()],
    )
    .unwrap();
    drop(c);
    (db.clone(), vec![db, log, context])
}

#[test]
fn deleting_imported_run_preserves_sources_and_shared_archive_and_allows_explicit_reimport() {
    let source = tempfile::tempdir().unwrap();
    let (database, files) = legacy_with_local_files(source.path());
    let before: Vec<_> = files.iter().map(|p| fs::read(p).unwrap()).collect();
    let app = tempfile::tempdir().unwrap();
    let f = common::open(app.path().into()).unwrap();
    let report = f.import_legacy(None, Some(database.clone())).unwrap();
    assert_eq!(report.runs_imported, 1);
    let id = f.snapshot().unwrap().runs[0].id.clone();
    let archive = report.archive_path.unwrap();
    let archive_before = fs::read(&archive).unwrap();
    let sessions = app.path().join("sessions").join(&id);
    let legacy = app.path().join("legacy").join(&id);
    let memory = app.path().join("memory/acme/repo/runs").join(&id);
    assert!(sessions.join("builder.log").is_file());
    assert!(legacy.is_dir());
    assert!(fs::read_dir(&legacy).unwrap().any(|entry| {
        fs::read_to_string(entry.unwrap().path()).is_ok_and(|text| text == "legacy project memory")
    }));
    // Imported memory copies live in legacy/<id>; newer run-local memory is owned too.
    fs::create_dir_all(&memory).unwrap();
    fs::write(memory.join("context.md"), "run-local memory").unwrap();
    let shared = app.path().join("memory/acme/repo/shared/context.md");
    fs::create_dir_all(shared.parent().unwrap()).unwrap();
    fs::write(&shared, "shared project memory").unwrap();

    assert!(matches!(
        f.delete_run(&id).unwrap(),
        DeleteRunOutcome::Deleted { .. }
    ));
    assert!(f.snapshot().unwrap().runs.is_empty());
    assert!(!sessions.exists());
    assert!(!legacy.exists());
    assert!(!memory.exists());
    assert_eq!(fs::read(&archive).unwrap(), archive_before);
    assert_eq!(
        fs::read_to_string(&shared).unwrap(),
        "shared project memory"
    );
    for (file, bytes) in files.iter().zip(&before) {
        assert_eq!(&fs::read(file).unwrap(), bytes);
    }

    let again = f.import_legacy(None, Some(database)).unwrap();
    assert_eq!(again.runs_imported, 1);
    assert_eq!(f.snapshot().unwrap().runs[0].id, id);
    assert!(sessions.join("builder.log").is_file());
    assert!(legacy.is_dir());
}

#[test]
fn pending_legacy_deletion_is_excluded_before_session_and_memory_copies() {
    let source = tempfile::tempdir().unwrap();
    let (database, files) = legacy_with_local_files(source.path());
    let before: Vec<_> = files.iter().map(|p| fs::read(p).unwrap()).collect();
    let baseline = tempfile::tempdir().unwrap();
    let batch =
        legacy::read_legacy(None, Some(&database), baseline.path(), &Settings::default()).unwrap();
    let id = batch.runs[0].detail.run.id.clone();
    assert!(baseline.path().join("sessions").join(&id).is_dir());
    assert!(baseline.path().join("legacy").join(&id).is_dir());

    let app = tempfile::tempdir().unwrap();
    let batch = legacy::read_legacy_excluding(
        None,
        Some(&database),
        app.path(),
        &Settings::default(),
        &std::collections::HashSet::from([id.clone()]),
    )
    .unwrap();
    assert!(batch.runs.is_empty());
    assert!(batch.consumed_events.contains(&"issue:acme/repo#7".into()));
    assert!(!app.path().join("sessions").join(&id).exists());
    assert!(!app.path().join("legacy").join(&id).exists());
    assert!(!app.path().join("memory/acme/repo/runs").join(&id).exists());
    for (file, bytes) in files.iter().zip(&before) {
        assert_eq!(&fs::read(file).unwrap(), bytes);
    }
}

#[test]
fn ambiguous_provenance_survives_pending_deletion_reimport_and_reopen() -> Result<()> {
    let source = tempfile::tempdir()?;
    let (database, files) = legacy_with_local_files(source.path());
    let legacy_db = Connection::open(&database)?;
    legacy_db.execute_batch("INSERT INTO tasks SELECT 'acme/repo#8',repository,8,replace(issue,'old','other'),'completed','finished',1,detected_at,launched_at,finished_at,last_error FROM tasks; INSERT INTO workflows SELECT 'acme/repo#8',phase,trigger_name,pull_requests,created_at,updated_at,documentation FROM workflows;")?;
    drop(legacy_db);
    let before: Vec<_> = files.iter().map(|p| fs::read(p).unwrap()).collect();
    let app = tempfile::tempdir()?;
    let f = common::open(app.path().into())?;
    assert_eq!(
        f.import_legacy(None, Some(database.clone()))?.runs_imported,
        2
    );
    let id = f
        .snapshot()?
        .runs
        .iter()
        .find(|r| r.number == 7)
        .unwrap()
        .id
        .clone();
    let db = Connection::open(app.path().join("factory.sqlite3"))?;
    let assert_ineligible = || -> Result<()> {
        let eligible: i64 = db.query_row(
            "SELECT COUNT(*) FROM (SELECT repository,number FROM tracked_pr_origins WHERE repository='acme/repo' AND number=9 GROUP BY repository,number HAVING MIN(trusted)=1 AND COUNT(DISTINCT json_array(event_id,issue_repository,issue_number,actor,trigger_id))=1)",
            [], |r| r.get(0),
        )?;
        assert_eq!(
            eligible, 0,
            "ambiguous PR must remain ineligible for approval automation"
        );
        Ok(())
    };
    assert_ineligible()?;
    // Fail finalization after all physical cleanup: portable, deterministic partial deletion.
    db.execute_batch("CREATE TRIGGER fail_legacy_final BEFORE DELETE ON runs BEGIN SELECT RAISE(ABORT,'fixture final failure'); END;")?;
    assert!(f.delete_run(&id).is_err());
    assert!(matches!(f.run_view(&id)?, RunView::Deleting { .. }));
    let sessions = app.path().join("sessions").join(&id);
    let legacy = app.path().join("legacy").join(&id);
    assert!(!sessions.exists());
    assert!(!legacy.exists());
    assert_eq!(
        f.import_legacy(None, Some(database.clone()))?.runs_imported,
        0
    );
    assert!(!sessions.exists(), "pending deletion must not restore logs");
    assert!(
        !legacy.exists(),
        "pending deletion must not restore memory copies"
    );
    assert_ineligible()?;
    db.execute_batch("DROP TRIGGER fail_legacy_final;")?;
    assert!(matches!(
        f.delete_run(&id)?,
        DeleteRunOutcome::Deleted { .. }
    ));
    assert_ineligible()?;
    drop(f);
    let f = common::open(app.path().into())?;
    assert!(matches!(f.run_view(&id)?, RunView::Missing));
    assert_ineligible()?;
    // Explicit reimport after completed deletion may restore history, never trust.
    assert_eq!(f.import_legacy(None, Some(database))?.runs_imported, 1);
    assert_ineligible()?;
    for (file, bytes) in files.iter().zip(&before) {
        assert_eq!(&fs::read(file)?, bytes);
    }
    Ok(())
}

#[test]
fn newly_discovered_legacy_conflict_revokes_prior_trust_durably() -> Result<()> {
    let source = tempfile::tempdir()?;
    let database = source.path().join("old.sqlite");
    old_database(&database);
    let app = tempfile::tempdir()?;
    let f = common::open(app.path().into())?;
    f.import_legacy(None, Some(database.clone()))?;
    let db = Connection::open(app.path().join("factory.sqlite3"))?;
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM tracked_pr_origins WHERE trusted=1",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        1
    );
    let legacy_db = Connection::open(&database)?;
    legacy_db.execute_batch("INSERT INTO tasks SELECT 'acme/repo#8',repository,8,replace(issue,'old','other'),'completed','finished',1,detected_at,launched_at,finished_at,last_error FROM tasks; INSERT INTO workflows SELECT 'acme/repo#8',phase,trigger_name,pull_requests,created_at,updated_at FROM workflows;")?;
    f.import_legacy(None, Some(database.clone()))?;
    let assert_conflicts = || -> Result<()> {
        let counts: (i64, i64) = db.query_row(
            "SELECT COUNT(*),SUM(trusted) FROM tracked_pr_origins WHERE repository='acme/repo' AND number=9 AND event_id IS NULL AND issue_repository IS NULL AND issue_number IS NULL AND actor IS NULL AND trigger_id IS NULL",
            [], |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(
            counts,
            (2, 0),
            "retain only minimal, untrusted conflict identities"
        );
        Ok(())
    };
    assert_conflicts()?;
    // An incomplete later source cannot erase earlier ambiguity or promote trust.
    legacy_db.execute("DELETE FROM workflows WHERE key='acme/repo#8'", [])?;
    f.import_legacy(None, Some(database))?;
    assert_conflicts()?;
    for run in f.snapshot()?.runs {
        f.delete_run(&run.id)?;
    }
    drop(f);
    let f = common::open(app.path().into())?;
    assert!(f.snapshot()?.runs.is_empty());
    assert_conflicts()?;
    Ok(())
}
