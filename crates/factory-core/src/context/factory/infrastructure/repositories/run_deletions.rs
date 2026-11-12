use super::store::Store;
use crate::*;
use anyhow::{ensure, Context};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

pub(super) fn migrate(db: &mut Connection) -> Result<()> {
    let version: u32 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(version <= 1, "Unsupported database schema");
    if version == 1 {
        return Ok(());
    }
    let tx = db.transaction()?;
    tx.execute_batch("CREATE TABLE run_deletions (run_id TEXT PRIMARY KEY,repository TEXT NOT NULL,number INTEGER NOT NULL,title TEXT NOT NULL,attempt INTEGER NOT NULL,state TEXT NOT NULL CHECK(state IN ('pending','failed')),last_error TEXT);
        CREATE TABLE run_deletion_targets (run_id TEXT NOT NULL,target TEXT NOT NULL CHECK(target IN ('sessions','memory','workspace','legacy')),done INTEGER NOT NULL DEFAULT 0 CHECK(done IN (0,1)),PRIMARY KEY(run_id,target),FOREIGN KEY(run_id) REFERENCES run_deletions(run_id) ON DELETE CASCADE);
        CREATE TABLE tracked_pr_origins (repository TEXT NOT NULL,number INTEGER NOT NULL,origin_key TEXT NOT NULL,event_id TEXT,issue_repository TEXT,issue_number INTEGER,actor TEXT,trigger_id TEXT,trusted INTEGER NOT NULL CHECK(trusted IN (0,1)),PRIMARY KEY(repository,number,origin_key));")?;
    super::pr_origins::backfill(&tx)?;
    tx.execute_batch("PRAGMA user_version=1;")?;
    tx.commit()?;
    Ok(())
}

pub(super) fn detail_db(db: &Connection, id: &str) -> Result<RunDetail> {
    let row: (String,String,String) = db.query_row("SELECT json,event,flow FROM runs WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM run_deletions WHERE run_id=runs.id)", [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?.context("Run not found or deleting")?;
    let mut q = db.prepare("SELECT json FROM nodes WHERE run_id=?1 ORDER BY rowid")?;
    let nodes = q
        .query_map([id], |r| r.get::<_, String>(0))?
        .map(|r| Ok(serde_json::from_str(&r?)?))
        .collect::<Result<Vec<_>>>()?;
    Ok(RunDetail {
        run: serde_json::from_str(&row.0)?,
        event: serde_json::from_str(&row.1)?,
        flow: serde_json::from_str(&row.2)?,
        nodes,
    })
}

fn record(db: &Connection, id: &str) -> Result<Option<DeletionRecord>> {
    let public = db.query_row("SELECT run_id,repository,number,title,attempt,state,last_error FROM run_deletions WHERE run_id=?1", [id], |r| Ok(RunDeletion {run_id:r.get(0)?,repository:r.get(1)?,number:r.get::<_,i64>(2)?.try_into().map_err(|_|rusqlite::Error::InvalidQuery)?,title:r.get(3)?,attempt:r.get(4)?,state:r.get(5)?,last_error:r.get(6)?})).optional()?;
    let Some(public) = public else {
        return Ok(None);
    };
    ensure!(
        valid_deletion_id(id) && valid_repository(&public.repository),
        "Invalid deletion identity"
    );
    let mut q = db.prepare("SELECT target,done FROM run_deletion_targets WHERE run_id=?1")?;
    let rows = q.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut targets = Vec::new();
    let mut completed = Vec::new();
    for row in rows {
        let (target, done) = row?;
        let target: CleanupTarget = serde_json::from_value(serde_json::json!(target))?;
        ensure!(done == 0 || done == 1, "Invalid target progress");
        if done == 1 {
            completed.push(target.clone());
        }
        targets.push(target);
    }
    let expected = deletion_targets(id);
    ensure!(
        targets.len() == expected.len() && expected.iter().all(|t| targets.contains(t)),
        "Invalid deletion targets"
    );
    Ok(Some(DeletionRecord {
        public,
        targets: expected,
        completed,
    }))
}

fn required(tx: &Transaction<'_>, id: &str) -> Result<DeletionRecord> {
    record(tx, id)?.context("Deletion not found")
}

impl Store {
    pub(crate) fn prepare_deletion(&self, id: &str) -> Result<Option<DeletionRecord>> {
        ensure!(valid_deletion_id(id), "Invalid run ID");
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        if let Some(existing) = record(&tx, id)? {
            return Ok(Some(existing));
        }
        let raw: Option<(String, String)> = tx
            .query_row("SELECT json,event FROM runs WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let Some((run, event)) = raw else {
            return Ok(None);
        };
        let r: Run = serde_json::from_str(&run)?;
        let event: TriggerEvent = serde_json::from_str(&event)?;
        ensure!(
            r.id == id && valid_repository(&r.repository) && r.repository == event.repository,
            "Invalid run identity"
        );
        ensure!(
            !matches!(r.status.as_str(), "queued" | "running"),
            "Cancel this run before deleting it."
        );
        ensure!(
            terminal_for_deletion(&r.status),
            "This run cannot be deleted in its current state."
        );
        tx.execute(
            "INSERT INTO run_deletions VALUES(?1,?2,?3,?4,?5,'pending',NULL)",
            params![
                id,
                r.repository,
                i64::try_from(r.number)?,
                r.title,
                r.attempt
            ],
        )?;
        for target in deletion_targets(id) {
            tx.execute(
                "INSERT INTO run_deletion_targets(run_id,target) VALUES(?1,?2)",
                params![id, serde_json::to_value(target)?.as_str().unwrap()],
            )?;
        }
        let result = required(&tx, id)?;
        tx.commit()?;
        Ok(Some(result))
    }
    pub(crate) fn pending_deletions(&self) -> Result<Vec<DeletionRecord>> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let result = records(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    pub(crate) fn deletion(&self, id: &str) -> Result<Option<DeletionRecord>> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let result = record(&tx, id)?;
        tx.commit()?;
        Ok(result)
    }
    pub(crate) fn mark_target_complete(&self, id: &str, target: &CleanupTarget) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let r = required(&tx, id)?;
        ensure!(r.targets.contains(target), "Unexpected cleanup target");
        ensure!(
            tx.execute(
                "UPDATE run_deletion_targets SET done=1 WHERE run_id=?1 AND target=?2",
                params![id, serde_json::to_value(target)?.as_str().unwrap()]
            )? == 1,
            "Target not found"
        );
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn fail_deletion(&self, id: &str, message: &str) -> Result<()> {
        self.deletion_state(id, "failed", Some(message))
    }
    pub(crate) fn resume_deletion(&self, id: &str) -> Result<()> {
        self.deletion_state(id, "pending", None)
    }
    fn deletion_state(&self, id: &str, state: &str, error: Option<&str>) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        required(&tx, id)?;
        tx.execute(
            "UPDATE run_deletions SET state=?2,last_error=?3 WHERE run_id=?1",
            params![id, state, error],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn finish_deletion(&self, id: &str) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let record = required(&tx, id)?;
        ensure!(
            record.targets.iter().all(|t| record.completed.contains(t)),
            "Cleanup targets incomplete"
        );
        let event_id: String =
            tx.query_row("SELECT event_id FROM runs WHERE id=?1", [id], |r| r.get(0))?;
        tx.execute("UPDATE dedup SET run_id='consumed' WHERE run_id=?1", [id])?;
        tx.execute("DELETE FROM nodes WHERE run_id=?1", [id])?;
        tx.execute("DELETE FROM observations WHERE key IN (?1,?2,?3) OR substr(key,1,length(?4))=?4 OR substr(key,1,length(?5))=?5",params![format!("identity:{id}"),format!("pr-workspace:{id}"),format!("pr-update:{id}"),format!("review-feedback:{id}:"),format!("review-history:{id}:")])?;
        tx.execute("DELETE FROM runs WHERE id=?1", [id])?;
        // Keep the event consumed while any attempt still owns it, including
        // attempts whose cleanup is pending. Release it with the last deletion.
        tx.execute(
            "DELETE FROM dedup WHERE event_id=?1 AND NOT EXISTS(SELECT 1 FROM runs WHERE event_id=?1)",
            [&event_id],
        )?;
        tx.execute("DELETE FROM run_deletions WHERE run_id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn run_view(&self, id: &str) -> Result<RunView> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let view = if let Some(r) = record(&tx, id)? {
            RunView::Deleting { deletion: r.public }
        } else {
            let exists: bool =
                tx.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1)", [id], |r| {
                    r.get(0)
                })?;
            if exists {
                RunView::Available {
                    detail: detail_db(&tx, id)?,
                }
            } else {
                RunView::Missing
            }
        };
        tx.commit()?;
        Ok(view)
    }
    pub(crate) fn snapshot_history(&self) -> Result<(Vec<Run>, Vec<RunDeletion>)> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let runs = {
            let mut q=tx.prepare("SELECT json FROM runs WHERE NOT EXISTS(SELECT 1 FROM run_deletions d WHERE d.run_id=runs.id) ORDER BY created_at DESC,id DESC")?;
            let rows = q.query_map([], |r| r.get::<_, String>(0))?;
            rows.map(|r| Ok(serde_json::from_str(&r?)?))
                .collect::<Result<Vec<_>>>()?
        };
        let deletions = records(&tx)?.into_iter().map(|r| r.public).collect();
        tx.commit()?;
        Ok((runs, deletions))
    }
    pub(crate) fn previous_successful(
        &self,
        repository: &str,
        except: &str,
    ) -> Result<serde_json::Value> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let ids = {
            let mut q=tx.prepare("SELECT id FROM runs WHERE json_extract(json,'$.repository')=?1 AND id!=?2 AND json_extract(json,'$.status')='succeeded' AND NOT EXISTS(SELECT 1 FROM run_deletions d WHERE d.run_id=runs.id) ORDER BY created_at DESC,id DESC LIMIT 12")?;
            let rows = q.query_map(params![repository, except], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut previous = Vec::new();
        for id in ids {
            let Ok(d) = detail_db(&tx, &id) else { continue };
            previous.push(serde_json::json!({"run_id":d.run.id,"title":d.run.title,"nodes":d.nodes.into_iter().map(|n|serde_json::json!({"id":n.id,"summary":n.result.as_ref().and_then(|v|v["summary"].as_str())})).collect::<Vec<_>>()}));
        }
        tx.commit()?;
        Ok(serde_json::json!(previous))
    }
}
fn records(db: &Connection) -> Result<Vec<DeletionRecord>> {
    let mut q = db.prepare("SELECT run_id FROM run_deletions ORDER BY run_id")?;
    let ids = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ids.into_iter()
        .map(|id| record(db, &id)?.context("Deletion missing"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn terminal(store: &Store, number: u64, status: &str) -> Result<String> {
        let id = store
            .insert(
                &TriggerEvent::issue("acme/repo", number, "[AI] test", "alice", "private"),
                &Settings::default().flows[0],
                1,
                true,
            )?
            .unwrap();
        let mut run = store.detail(&id)?.run;
        run.status = status.into();
        store.save_run(&run)?;
        Ok(id)
    }
    #[test]
    fn terminal_identity_and_absence_contracts() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        for (i, status) in [
            "succeeded",
            "failed",
            "blocked",
            "cancelled",
            "interrupted",
            "queued",
            "running",
            "unknown",
        ]
        .iter()
        .enumerate()
        {
            let id = terminal(&store, i as u64 + 1, status)?;
            assert_eq!(
                store.prepare_deletion(&id).is_ok(),
                terminal_for_deletion(status),
                "{status}"
            );
        }
        for id in [
            "../escape",
            "legacy-abc",
            "AAAAAAAA-AAAA-4AAA-AAAA-AAAAAAAAAAAA",
            "00000000000000000000000000000000",
            "",
        ] {
            assert!(store.prepare_deletion(id).is_err());
        }
        let missing = uuid::Uuid::new_v4().to_string();
        assert!(store.prepare_deletion(&missing)?.is_none());
        assert!(store.fail_deletion(&missing, "failed").is_err());
        assert!(store.resume_deletion(&missing).is_err());
        assert!(store
            .mark_target_complete(&missing, &CleanupTarget::Sessions)
            .is_err());
        assert!(store.finish_deletion(&missing).is_err());
        Ok(())
    }
    #[test]
    fn recovery_validates_targets_and_mutations_cannot_restore_content() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let id = terminal(&store, 1, "succeeded")?;
        let old = store.detail(&id)?;
        let record = store.prepare_deletion(&id)?.unwrap();
        store.mark_target_complete(&id, &CleanupTarget::Sessions)?;
        store.fail_deletion(&id, "cleanup failed: sessions io")?;
        assert_eq!(store.deletion(&id)?.unwrap().public.state, "failed");
        assert!(store.save_run(&old.run).is_err());
        assert!(store.save_node(&id, &old.nodes[0]).is_err());
        assert!(store.set_workspace(&id, Some("outside")).is_err());
        assert!(store
            .set_node_result(&id, &old.nodes[0].id, &serde_json::json!({}))
            .is_err());
        assert!(!store.claim_queued(&id)?);
        assert!(store
            .observe(
                &format!("identity:{id}"),
                &serde_json::json!({"late":"writer"})
            )
            .is_err());
        assert!(store
            .observe(
                &format!("review-feedback:{id}:builder"),
                &serde_json::json!({"late":"writer"})
            )
            .is_err());
        for prefix in ["pr-workspace", "pr-update"] {
            assert!(store
                .observe(
                    &format!("{prefix}:{id}"),
                    &serde_json::json!({"late":"writer"})
                )
                .is_err());
        }
        let (runs, deletions) = store.snapshot_history()?;
        assert!(runs.is_empty());
        assert_eq!(deletions.len(), 1);
        assert_eq!(
            store.previous_successful("acme/repo", "other")?,
            serde_json::json!([])
        );
        drop(store);
        let store = Store::open(dir.path())?;
        assert_eq!(store.pending_deletions()?.len(), 1);
        let resumed = store.prepare_deletion(&id)?.unwrap();
        assert_eq!(resumed.targets, record.targets);
        assert_eq!(resumed.completed, vec![CleanupTarget::Sessions]);
        store.resume_deletion(&id)?;
        assert_eq!(store.deletion(&id)?.unwrap().public.last_error, None);
        store.db.lock().unwrap().execute(
            "DELETE FROM run_deletion_targets WHERE run_id=?1 AND target='memory'",
            [&id],
        )?;
        assert!(store.deletion(&id).is_err());
        assert!(store.finish_deletion(&id).is_err());
        assert!(store.pending_deletions().is_err());
        Ok(())
    }
    #[test]
    fn finalization_is_atomic_and_preserves_prefix_neighbors_and_shared_records() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let id = terminal(&store, 1, "failed")?;
        let keep = [
            format!("identity:{id}-neighbor"),
            format!("pr-workspace:{id}-neighbor"),
            format!("pr-update:{id}-neighbor"),
            format!("review-feedback:{id}-neighbor:n"),
            format!("review-history:{id}-neighbor:n"),
            "approval-comment:acme/repo#1:7".into(),
            "review:acme:alice".into(),
            format!("action:{}", "a".repeat(64)),
        ];
        for key in &keep {
            store.db.lock().unwrap().execute(
                "INSERT INTO observations VALUES(?1,'{\"keep\":true}')",
                [key],
            )?;
        }
        let record = store.prepare_deletion(&id)?.unwrap();
        for t in record.targets {
            store.mark_target_complete(&id, &t)?;
        }
        store.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_delete BEFORE DELETE ON runs BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
        assert!(store.finish_deletion(&id).is_err());
        let consumed: String =
            store
                .db
                .lock()
                .unwrap()
                .query_row("SELECT run_id FROM dedup", [], |r| r.get(0))?;
        assert_eq!(consumed, id);
        assert!(store.deletion(&id)?.is_some());
        store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_delete;")?;
        store.finish_deletion(&id)?;
        for key in keep {
            assert!(store.observation(&key)?.is_some());
        }
        Ok(())
    }
    #[test]
    fn independent_origin_stays_ambiguous_after_one_source_is_deleted() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let first = terminal(&store, 1, "succeeded")?;
        let second = terminal(&store, 2, "succeeded")?;
        store.track_pr("acme/repo", 5, &first)?;
        assert_eq!(store.tracked_prs()?.len(), 1);
        store.track_pr("acme/repo", 5, &second)?;
        assert!(store.tracked_prs()?.is_empty());
        let record = store.prepare_deletion(&first)?.unwrap();
        for t in record.targets {
            store.mark_target_complete(&first, &t)?;
        }
        store.finish_deletion(&first)?;
        assert!(store.tracked_prs()?.is_empty());
        assert_eq!(
            store.db.lock().unwrap().query_row(
                "SELECT COUNT(*) FROM tracked_pr_origins",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            2
        );
        Ok(())
    }
    #[test]
    fn legacy_targets_and_run_event_identity_are_validated() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let id = terminal(&store, 1, "interrupted")?;
        let legacy = format!("legacy-{}", "a".repeat(32));
        store.db.lock().unwrap().execute(
            "UPDATE runs SET id=?2,json=json_set(json,'$.id',?2) WHERE id=?1",
            params![id, legacy],
        )?;
        let record = store.prepare_deletion(&legacy)?.unwrap();
        assert_eq!(
            record.targets,
            vec![
                CleanupTarget::Sessions,
                CleanupTarget::Memory,
                CleanupTarget::Legacy
            ]
        );
        assert!(store
            .mark_target_complete(&legacy, &CleanupTarget::Workspace)
            .is_err());
        for (number, column, field, value) in [
            (2, "event", "repository", "other/repo"),
            (3, "json", "id", "different-id"),
            (4, "json", "repository", "../escape"),
        ] {
            let id = terminal(&store, number, "failed")?;
            store.db.lock().unwrap().execute(
                &format!("UPDATE runs SET {column}=json_set({column},'$.{field}',?2) WHERE id=?1"),
                params![id, value],
            )?;
            assert!(store.prepare_deletion(&id).is_err());
            assert!(store.deletion(&id)?.is_none());
        }
        Ok(())
    }
    #[test]
    fn failed_prepare_rolls_back_intention_and_all_targets() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let id = terminal(&store, 1, "failed")?;
        store.db.lock().unwrap().execute_batch("CREATE TRIGGER target_failure BEFORE INSERT ON run_deletion_targets WHEN NEW.target='memory' BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
        assert!(store.prepare_deletion(&id).is_err());
        assert!(store.deletion(&id)?.is_none());
        assert!(store.detail(&id).is_ok());
        assert_eq!(
            store.db.lock().unwrap().query_row(
                "SELECT COUNT(*) FROM run_deletion_targets",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    }
    #[test]
    fn prior_successful_summary_is_bounded_and_excludes_pending_runs() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let mut ids = Vec::new();
        for n in 1..=15 {
            ids.push(terminal(&store, n, "succeeded")?);
        }
        let excluded = ids.pop().unwrap();
        store.prepare_deletion(&excluded)?;
        let except = ids.pop().unwrap();
        let summaries = store.previous_successful("acme/repo", &except)?;
        let rows = summaries.as_array().unwrap();
        assert_eq!(rows.len(), 12);
        for row in rows {
            assert_ne!(row["run_id"], excluded);
            assert_ne!(row["run_id"], except);
            assert_eq!(row.as_object().unwrap().len(), 3);
            assert!(row["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|n| n.as_object().unwrap().len() == 2));
        }
        assert_eq!(
            store.previous_successful("other/repo", "")?,
            serde_json::json!([])
        );
        Ok(())
    }
    #[test]
    fn import_cannot_restore_pending_run_but_explicit_import_after_completion_can() -> Result<()> {
        use crate::context::factory::infrastructure::migration::legacy::{LegacyImport, LegacyRun};
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let id = terminal(&store, 1, "succeeded")?;
        let detail = store.detail(&id)?;
        let batch = LegacyImport {
            runs: vec![LegacyRun {
                detail,
                log_text: String::new(),
                raw: serde_json::Value::Null,
            }],
            ..LegacyImport::default()
        };
        let record = store.prepare_deletion(&id)?.unwrap();
        assert_eq!(store.import_legacy(&batch)?, 0);
        assert!(store.detail(&id).is_err());
        for t in record.targets {
            store.mark_target_complete(&id, &t)?;
        }
        store.finish_deletion(&id)?;
        assert_eq!(store.import_legacy(&batch)?, 1);
        assert!(store.detail(&id).is_ok());
        Ok(())
    }
    #[test]
    fn intent_hides_content_and_finalize_removes_only_owned_rows() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let event = TriggerEvent::issue("acme/repo", 1, "[AI] deletion", "alice", "payload-marker");
        let flow = Settings::default().flows.remove(0);
        let id = store.insert(&event, &flow, 1, true)?.unwrap();
        let sibling = store.insert(&event, &flow, 2, false)?.unwrap();
        let mut run = store.detail(&id)?.run;
        run.status = "succeeded".into();
        store.save_run(&run)?;
        let owned = [
            format!("identity:{id}"),
            format!("pr-workspace:{id}"),
            format!("pr-update:{id}"),
            format!("review-feedback:{id}:builder"),
            format!("review-history:{id}:round1"),
        ];
        for key in &owned {
            store.observe(key, &serde_json::json!({"secret_result":true}))?;
        }
        let other = format!("review-feedback:{sibling}:builder");
        store.observe(&other, &serde_json::json!({"keep":true}))?;
        let record = store.prepare_deletion(&id)?.unwrap();
        assert!(store.runs()?.iter().all(|r| r.id != id));
        assert!(matches!(store.run_view(&id)?, RunView::Deleting { .. }));
        assert!(store.detail(&id).is_err());
        assert!(store.finish_deletion(&id).is_err());
        for target in &record.targets {
            store.mark_target_complete(&id, target)?;
        }
        store.finish_deletion(&id)?;
        assert!(matches!(store.run_view(&id)?, RunView::Missing));
        assert!(store.nodes(&id)?.is_empty());
        for key in owned {
            assert!(store.observation(&key)?.is_none());
        }
        assert!(store.observation(&other)?.is_some());
        assert!(store.detail(&sibling).is_ok());
        assert!(store.insert(&event, &flow, 1, true)?.is_none());
        Ok(())
    }
}
