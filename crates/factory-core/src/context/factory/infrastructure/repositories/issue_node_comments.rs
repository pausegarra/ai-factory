use super::store::Store;
use crate::context::factory::application::dto::issue_node_comment::*;
use crate::context::factory::application::services::issue_node_comments::{
    marker_for, retry_delay, terminal_for_issue_comment,
};
use crate::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
pub(super) fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS issue_node_comments (key TEXT PRIMARY KEY,run_id TEXT NOT NULL,node_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),snapshot TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('pending','delivered','exhausted')),failures INTEGER NOT NULL DEFAULT 0 CHECK(failures>=0),next_at TEXT NOT NULL,last_error TEXT,receipt TEXT,UNIQUE(run_id,node_id,revision));CREATE INDEX IF NOT EXISTS issue_node_comments_due ON issue_node_comments(state,next_at);")?;
    Ok(())
}
// Read with the caller's SQLite lock so preference and outbox decisions share a snapshot.
fn comments_enabled(db: &Connection) -> Result<bool> {
    let raw: Option<String> = db
        .query_row("SELECT json FROM settings WHERE id=1", [], |row| row.get(0))
        .optional()?;
    let settings: Settings = raw
        .map(|value| serde_json::from_str(&value))
        .transpose()?
        .unwrap_or_default();
    Ok(settings.issue_node_comments_enabled)
}
pub(super) fn capture(
    tx: &Transaction<'_>,
    store: &Store,
    run: &str,
    previous: &NodeRun,
    node: &NodeRun,
) -> Result<()> {
    if !matches!(previous.status.as_str(), "pending" | "running")
        || !terminal_for_issue_comment(&node.status)
    {
        return Ok(());
    }
    if !comments_enabled(tx)? {
        return Ok(());
    }
    let (event, state): (String, String) =
        tx.query_row("SELECT event,json FROM runs WHERE id=?1", [run], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    let event: TriggerEvent = serde_json::from_str(&event)?;
    let state: Run = serde_json::from_str(&state)?;
    if event.trigger_id != "new_issue"
        || !valid_repository(&event.repository)
        || event.number == 0
        || !valid_login(&event.actor)
    {
        return Ok(());
    }
    let revision:i64=tx.query_row("SELECT COALESCE(MAX(revision),0)+1 FROM issue_node_comments WHERE run_id=?1 AND node_id=?2",params![run,node.id],|r|r.get(0))?;
    let revision = u32::try_from(revision)?;
    let snapshot = IssueNodeCommentSnapshot {
        run_id: run.into(),
        node_id: node.id.clone(),
        node_name: store.redact_cached(&node.name),
        repository: event.repository,
        number: event.number,
        actor: event.actor,
        run_attempt: state.attempt,
        revision,
        status: node.status.clone(),
        summary: node
            .result
            .as_ref()
            .and_then(|r| r["summary"].as_str())
            .map(|s| store.redact_cached(s)),
        error: node.error.as_ref().map(|e| store.redact_cached(e)),
    };
    let marker = marker_for(run, &node.id, revision);
    let key = marker
        .trim_start_matches("<!-- ai-factory:")
        .trim_end_matches(" -->");
    tx.execute("INSERT INTO issue_node_comments(key,run_id,node_id,revision,snapshot,state,next_at) VALUES(?1,?2,?3,?4,?5,'pending',?6)",params![key,run,node.id,revision,serde_json::to_string(&snapshot)?,now()])?;
    Ok(())
}
const ALLOWED:&str="EXISTS(SELECT 1 FROM runs WHERE id=issue_node_comments.run_id) AND NOT EXISTS(SELECT 1 FROM run_deletions WHERE run_id=issue_node_comments.run_id)";
impl Store {
    pub(crate) fn due_issue_node_comment(&self, at: &str) -> Result<Option<IssueNodeCommentJob>> {
        let db = self.db.lock().unwrap();
        if !comments_enabled(&db)? {
            return Ok(None);
        }
        let row:Option<(String,String,u32)>=db.query_row(&format!("SELECT key,snapshot,failures FROM issue_node_comments WHERE state='pending' AND julianday(next_at)<=julianday(?1) AND {ALLOWED} ORDER BY julianday(next_at),rowid LIMIT 1"),[at],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        row.map(|(key, snapshot, failures)| {
            Ok(IssueNodeCommentJob {
                key,
                snapshot: serde_json::from_str(&snapshot)?,
                failures,
            })
        })
        .transpose()
    }
    pub(crate) fn issue_node_comment_allowed(&self, key: &str) -> Result<bool> {
        let db = self.db.lock().unwrap();
        if !comments_enabled(&db)? {
            return Ok(false);
        }
        Ok(db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM issue_node_comments WHERE key=?1 AND state='pending' AND {ALLOWED})"),[key],|r|r.get(0))?)
    }
    pub(crate) fn issue_node_comment_last_error(&self, key: &str) -> Result<Option<String>> {
        let db = self.db.lock().unwrap();
        Ok(db.query_row(
            &format!("SELECT last_error FROM issue_node_comments WHERE key=?1 AND state='pending' AND {ALLOWED}"),
            [key], |row| row.get::<_, Option<String>>(0),
        ).optional()?.flatten())
    }
    pub(crate) fn confirm_issue_node_comment(
        &self,
        key: &str,
        receipt: &IssueNodeCommentReceipt,
    ) -> Result<bool> {
        Ok(self.db.lock().unwrap().execute(&format!("UPDATE issue_node_comments SET state='delivered',receipt=?2 WHERE key=?1 AND state='pending' AND {ALLOWED}"),params![key,serde_json::to_string(receipt)?])?==1)
    }
    pub(crate) fn fail_issue_node_comment(&self, key: &str, code: &str, at: &str) -> Result<bool> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let failures:Option<u32>=tx.query_row(&format!("SELECT failures FROM issue_node_comments WHERE key=?1 AND state='pending' AND {ALLOWED}"),[key],|r|r.get(0)).optional()?;
        let Some(failures) = failures else {
            return Ok(false);
        };
        let failures = failures + 1;
        let at = chrono::DateTime::parse_from_rfc3339(at)?;
        let delay = retry_delay(failures);
        let next = at + chrono::Duration::from_std(delay.unwrap_or_default())?;
        tx.execute("UPDATE issue_node_comments SET state=?2,failures=?3,next_at=?4,last_error=?5 WHERE key=?1",params![key,if delay.is_some(){"pending"}else{"exhausted"},failures,next.to_rfc3339(),code])?;
        tx.commit()?;
        Ok(true)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::factory::application::services::issue_node_comments::request_for;
    fn setup() -> (tempfile::TempDir, Store, String) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path()).unwrap();
        let mut flow = Settings::default().flows[0].clone();
        flow.nodes.truncate(1);
        let id = s
            .insert(
                &TriggerEvent::issue("acme/repo", 41, "[AI] test", "alice", ""),
                &flow,
                1,
                true,
            )
            .unwrap()
            .unwrap();
        (d, s, id)
    }
    fn terminal(s: &Store, id: &str, status: &str) {
        let mut n = s.nodes(id).unwrap().remove(0);
        n.status = status.into();
        n.result = Some(serde_json::json!({"summary":"done","data":{"repository":"evil/repo"}}));
        s.save_node(id, &n).unwrap();
    }
    fn count(s: &Store) -> u32 {
        s.db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM issue_node_comments", [], |r| r.get(0))
            .unwrap()
    }
    fn set_comments_enabled(s: &Store, enabled: bool) {
        let mut settings = s.public_settings().unwrap();
        settings.issue_node_comments_enabled = enabled;
        s.save_settings(&settings).unwrap();
    }

    #[test]
    fn issue_node_comment_disabled_capture_commits_node_without_backfill() {
        let (_d, s, id) = setup();
        set_comments_enabled(&s, false);
        terminal(&s, &id, "succeeded");
        assert_eq!(s.nodes(&id).unwrap()[0].status, "succeeded");
        assert_eq!(count(&s), 0);
        set_comments_enabled(&s, true);
        terminal(&s, &id, "succeeded");
        assert_eq!(count(&s), 0);
        assert!(s.due_issue_node_comment(&now()).unwrap().is_none());
    }

    #[test]
    fn issue_node_comment_disabled_pending_survives_reopen_and_resumes() {
        let (d, s, id) = setup();
        terminal(&s, &id, "failed");
        let job = s
            .due_issue_node_comment("2099-01-01T00:00:00.000Z")
            .unwrap()
            .unwrap();
        assert!(s
            .fail_issue_node_comment(&job.key, "http_rejected", "2000-01-01T00:00:00.000Z")
            .unwrap());
        let before: (String, u32, String, Option<String>, Option<String>) = s.db.lock().unwrap().query_row(
        "SELECT snapshot,failures,next_at,last_error,receipt FROM issue_node_comments WHERE key=?1",
        [&job.key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    ).unwrap();
        set_comments_enabled(&s, false);
        assert!(s
            .due_issue_node_comment("2099-01-01T00:00:00.000Z")
            .unwrap()
            .is_none());
        assert!(!s.issue_node_comment_allowed(&job.key).unwrap());
        drop(s);
        let s = Store::open(d.path()).unwrap();
        assert!(!s.issue_node_comment_allowed(&job.key).unwrap());
        assert!(s
            .due_issue_node_comment("2099-01-01T00:00:00.000Z")
            .unwrap()
            .is_none());
        let after: (String, u32, String, Option<String>, Option<String>) = s.db.lock().unwrap().query_row(
        "SELECT snapshot,failures,next_at,last_error,receipt FROM issue_node_comments WHERE key=?1",
        [&job.key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    ).unwrap();
        assert_eq!(before, after);
        set_comments_enabled(&s, true);
        let resumed = s
            .due_issue_node_comment("2099-01-01T00:00:00.000Z")
            .unwrap()
            .unwrap();
        assert_eq!(resumed.key, job.key);
        assert_eq!(resumed.failures, 1);
        assert_eq!(resumed.snapshot.revision, job.snapshot.revision);
        assert!(s.issue_node_comment_allowed(&job.key).unwrap());
        assert_eq!(count(&s), 1);
    }

    #[test]
    fn issue_node_comment_inflight_receipt_can_commit_while_disabled() {
        let (_d, s, id) = setup();
        terminal(&s, &id, "succeeded");
        let job = s.due_issue_node_comment(&now()).unwrap().unwrap();
        set_comments_enabled(&s, false);
        assert!(!s.issue_node_comment_allowed(&job.key).unwrap());
        assert!(s
            .confirm_issue_node_comment(
                &job.key,
                &IssueNodeCommentReceipt {
                    comment_id: 9001,
                    html_url: None,
                }
            )
            .unwrap());
        set_comments_enabled(&s, true);
        assert!(s
            .due_issue_node_comment("2099-01-01T00:00:00.000Z")
            .unwrap()
            .is_none());
        assert!(!s.issue_node_comment_allowed(&job.key).unwrap());
    }
    #[test]
    fn issue_node_comment_captures_only_committed_transition_and_frozen_target() {
        let (_d, s, id) = setup();
        let mut n = s.nodes(&id).unwrap().remove(0);
        n.status = "running".into();
        s.save_node(&id, &n).unwrap();
        s.set_node_result(&id, &n.id, &serde_json::json!({"summary":"provisional"}))
            .unwrap();
        assert!(s.due_issue_node_comment(&now()).unwrap().is_none());
        terminal(&s, &id, "succeeded");
        terminal(&s, &id, "succeeded");
        let j = s
            .due_issue_node_comment(&now())
            .unwrap()
            .expect("terminal notification");
        assert_eq!(j.snapshot.repository, "acme/repo");
        assert_eq!(j.snapshot.number, 41);
        assert_eq!(j.snapshot.revision, 1);
        assert_eq!(j.snapshot.summary.as_deref(), Some("done"));
        assert_eq!(count(&s), 1);
    }
    #[test]
    fn issue_node_comment_filters_states_and_stale_completion() {
        for status in ["pending", "running", "cancelled", "interrupted", "unknown"] {
            let (_d, s, id) = setup();
            terminal(&s, &id, status);
            assert!(s.due_issue_node_comment(&now()).unwrap().is_none());
            if status == "cancelled" {
                terminal(&s, &id, "succeeded");
                assert!(s.due_issue_node_comment(&now()).unwrap().is_none());
            }
        }
        for trigger in ["pr_review", "pr_approved"] {
            let d = tempfile::tempdir().unwrap();
            let s = Store::open(d.path()).unwrap();
            let mut e = TriggerEvent::issue("acme/repo", 41, "[AI] test", "alice", "");
            e.trigger_id = trigger.into();
            let flow = Settings::default()
                .flows
                .into_iter()
                .find(|f| f.id == trigger)
                .unwrap();
            let id = s.insert(&e, &flow, 1, true).unwrap().unwrap();
            terminal(&s, &id, "failed");
            assert!(s.due_issue_node_comment(&now()).unwrap().is_none());
        }
    }
    #[test]
    fn issue_node_comment_skipped_snapshot_does_not_invent_cause() {
        for reason in [None, Some("Dependency did not succeed")] {
            let (_d, s, id) = setup();
            let mut r = s.detail(&id).unwrap().run;
            r.error = Some("global dependency problem".into());
            s.save_run(&r).unwrap();
            let mut n = s.nodes(&id).unwrap().remove(0);
            n.status = "skipped".into();
            n.error = reason.map(str::to_owned);
            s.save_node(&id, &n).unwrap();
            let j = s.due_issue_node_comment(&now()).unwrap().unwrap();
            assert_eq!(j.snapshot.error.as_deref(), reason);
            let body = request_for(&j.snapshot, None, str::to_owned).body;
            assert!(body.contains("Step was not executed."));
            assert_eq!(
                body.contains("Dependency did not succeed"),
                reason.is_some()
            );
            assert!(!body.contains("global dependency"));
        }
    }
    #[test]
    fn issue_node_comment_atomic_rollback() {
        let (_d, s, id) = setup();
        s.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_comment BEFORE INSERT ON issue_node_comments BEGIN SELECT RAISE(ABORT,'fixture rollback'); END;").unwrap();
        let mut n = s.nodes(&id).unwrap().remove(0);
        n.status = "succeeded".into();
        assert!(s.save_node(&id, &n).is_err());
        assert_eq!(s.nodes(&id).unwrap()[0].status, "pending");
        assert_eq!(count(&s), 0);
    }
    #[test]
    fn issue_node_comment_retry_receipt_reopen_and_no_replay() {
        let (d, s, id) = setup();
        terminal(&s, &id, "failed");
        let job = s
            .due_issue_node_comment("2099-01-01T00:00:00.000Z")
            .unwrap()
            .unwrap();
        for f in 1..=5 {
            assert!(s
                .fail_issue_node_comment(&job.key, "http_rejected", "2000-01-01T00:00:00.000Z")
                .unwrap());
            if f < 5 {
                assert_eq!(
                    s.due_issue_node_comment("2099-01-01T00:00:00.000Z")
                        .unwrap()
                        .unwrap()
                        .failures,
                    f
                );
            }
        }
        assert!(s
            .due_issue_node_comment("2099-01-01T00:00:00.000Z")
            .unwrap()
            .is_none());
        drop(s);
        let s = Store::open(d.path()).unwrap();
        assert_eq!(count(&s), 1);
        assert!(s
            .due_issue_node_comment("2099-01-01T00:00:00.000Z")
            .unwrap()
            .is_none());
        let id2 = s
            .insert(
                &TriggerEvent::issue("acme/repo", 42, "[AI] next", "alice", ""),
                &Settings::default().flows[0],
                1,
                true,
            )
            .unwrap()
            .unwrap();
        terminal(&s, &id2, "succeeded");
        let j = s.due_issue_node_comment(&now()).unwrap().unwrap();
        assert_ne!(j.key, job.key);
        assert!(s
            .confirm_issue_node_comment(
                &j.key,
                &IssueNodeCommentReceipt {
                    comment_id: 9001,
                    html_url: None
                }
            )
            .unwrap());
        drop(s);
        let s = Store::open(d.path()).unwrap();
        assert!(s
            .due_issue_node_comment("2099-01-01T00:00:00.000Z")
            .unwrap()
            .is_none());
        assert_eq!(count(&s), 2);
    }
    #[test]
    fn issue_node_comment_deletion_does_not_resurrect_rows() {
        let (_d, s, id) = setup();
        terminal(&s, &id, "succeeded");
        let j = s.due_issue_node_comment(&now()).unwrap().unwrap();
        let mut r = s.detail(&id).unwrap().run;
        r.status = "succeeded".into();
        s.save_run(&r).unwrap();
        let record = s.prepare_deletion(&id).unwrap().unwrap();
        assert!(!s.issue_node_comment_allowed(&j.key).unwrap());
        assert!(s.due_issue_node_comment(&now()).unwrap().is_none());
        assert!(!s
            .fail_issue_node_comment(&j.key, "transport", &now())
            .unwrap());
        for t in record.targets {
            s.mark_target_complete(&id, &t).unwrap();
        }
        s.finish_deletion(&id).unwrap();
        assert_eq!(count(&s), 0);
        assert!(!s
            .confirm_issue_node_comment(
                &j.key,
                &IssueNodeCommentReceipt {
                    comment_id: 1,
                    html_url: None
                }
            )
            .unwrap());
    }
    #[test]
    fn issue_node_comment_real_requeue_preserves_identical_results_and_revision() {
        use crate::context::factory::application::services::reviews::ReviewRework;
        let (_d, s, id) = setup();
        let mut run = s.detail(&id).unwrap().run;
        run.status = "running".into();
        s.save_run(&run).unwrap();
        terminal(&s, &id, "succeeded");
        let first = s.due_issue_node_comment(&now()).unwrap().unwrap();
        let rework = ReviewRework {
            authors: [first.snapshot.node_id.clone()].into_iter().collect(),
            affected: [first.snapshot.node_id.clone()].into_iter().collect(),
            feedback: vec![serde_json::json!({"approved":false})],
        };
        assert!(s.requeue_review(&id, &rework).unwrap());
        terminal(&s, &id, "succeeded");
        terminal(&s, &id, "succeeded");
        let db = s.db.lock().unwrap();
        let mut q = db
            .prepare("SELECT snapshot,key FROM issue_node_comments ORDER BY revision")
            .unwrap();
        let rows = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        let a: IssueNodeCommentSnapshot = serde_json::from_str(&rows[0].0).unwrap();
        let b: IssueNodeCommentSnapshot = serde_json::from_str(&rows[1].0).unwrap();
        assert_eq!(a.summary, b.summary);
        assert_eq!(b.revision, 2);
        assert_ne!(rows[0].1, rows[1].1);
    }
    #[test]
    fn issue_node_comment_summary_contract_and_migration_without_backfill() {
        for summary in [
            serde_json::Value::Null,
            serde_json::json!(15),
            serde_json::json!("  "),
        ] {
            let (d, s, id) = setup();
            let mut n = s.nodes(&id).unwrap().remove(0);
            n.status = "succeeded".into();
            n.result = Some(
                serde_json::json!({"summary":summary,"data":{"private":"DO_NOT_PUBLISH"},"artifacts":[{"path":"PRIVATE"}]}),
            );
            s.save_node(&id, &n).unwrap();
            let j = s.due_issue_node_comment(&now()).unwrap().unwrap();
            let body = request_for(&j.snapshot, None, str::to_owned).body;
            assert!(body.contains("no summary available"));
            assert!(!body.contains("PRIVATE"));
            s.db.lock()
                .unwrap()
                .execute_batch("DROP TABLE issue_node_comments")
                .unwrap();
            drop(s);
            let s = Store::open(d.path()).unwrap();
            assert_eq!(count(&s), 0);
        }
    }
}
