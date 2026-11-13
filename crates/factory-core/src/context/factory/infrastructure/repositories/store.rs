use crate::*;
use anyhow::{ensure, Context};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

pub struct Store {
    pub db: Mutex<Connection>,
    pub(super) credentials: Arc<dyn Credentials>,
    pub(super) settings_lock: Mutex<()>,
    pub(super) secret_cache: Arc<Mutex<HashMap<String, String>>>,
}

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with_credentials(root, Arc::new(crate::context::factory::infrastructure::security::native_credentials::NativeCredentials))
    }
    pub fn open_with_credentials(root: &Path, credentials: Arc<dyn Credentials>) -> Result<Self> {
        let mut db = Connection::open(root.join("factory.sqlite3"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA secure_delete=ON; PRAGMA busy_timeout=5000;
   CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS runs (id TEXT PRIMARY KEY,event_id TEXT NOT NULL,json TEXT NOT NULL,event TEXT NOT NULL,flow TEXT NOT NULL,created_at TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS dedup (event_id TEXT PRIMARY KEY,run_id TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS nodes (run_id TEXT NOT NULL,node_id TEXT NOT NULL,json TEXT NOT NULL,PRIMARY KEY(run_id,node_id));
   CREATE TABLE IF NOT EXISTS observations (key TEXT PRIMARY KEY,json TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS credential_refs (profile_id TEXT PRIMARY KEY,account TEXT NOT NULL,reference TEXT NOT NULL UNIQUE);
   CREATE TABLE IF NOT EXISTS credential_gc (reference TEXT PRIMARY KEY);")?;
        super::run_deletions::migrate(&mut db)?;
        super::agent_accounts::migrate(&db)?;
        let s = Self {
            db: Mutex::new(db),
            credentials,
            settings_lock: Mutex::new(()),
            secret_cache: Arc::new(Mutex::new(HashMap::new())),
        };
        s.migrate_credentials()?;
        for mut r in s.runs()? {
            if r.status == "running" {
                r.status = "interrupted".into();
                r.finished_at = Some(now());
                r.error=Some("Application stopped while run was executing; retry explicitly to avoid duplicate side effects".into());
                r.current_step = None;
                s.save_run(&r)?;
                for mut n in s.nodes(&r.id)? {
                    if n.status == "running" {
                        n.status = "interrupted".into();
                        n.finished_at = Some(now());
                        s.save_node(&r.id, &n)?;
                    }
                }
            }
        }
        Ok(s)
    }
    pub(super) fn raw_settings(&self) -> Result<Settings> {
        let raw: Option<String> = self
            .db
            .lock()
            .unwrap()
            .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
            .optional()?;
        Ok(raw
            .map(|r| serde_json::from_str(&r))
            .transpose()?
            .unwrap_or_default())
    }
    pub fn insert(
        &self,
        event: &TriggerEvent,
        flow: &Flow,
        attempt: u32,
        dedup: bool,
    ) -> Result<Option<String>> {
        self.insert_inner(event, flow, attempt, dedup, None)
    }
    pub fn insert_approval(
        &self,
        event: &TriggerEvent,
        flow: &Flow,
        comments: &[u64],
        approved: bool,
    ) -> Result<Option<String>> {
        self.insert_inner(event, flow, 1, true, Some((comments, approved)))
    }
    fn insert_inner(
        &self,
        event: &TriggerEvent,
        flow: &Flow,
        attempt: u32,
        dedup: bool,
        approval: Option<(&[u64], bool)>,
    ) -> Result<Option<String>> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let id = Self::insert_in_tx(&tx, event, flow, attempt, dedup, approval)?;
        tx.commit()?;
        Ok(id)
    }
    pub(super) fn insert_in_tx(
        tx: &rusqlite::Transaction<'_>,
        event: &TriggerEvent,
        flow: &Flow,
        attempt: u32,
        dedup: bool,
        approval: Option<(&[u64], bool)>,
    ) -> Result<Option<String>> {
        let flow = &flow.for_event(event)?;
        let id = uuid::Uuid::new_v4().to_string();
        let created = now();
        let run = Run {
            id: id.clone(),
            event_id: event.event_id.clone(),
            trigger_id: event.trigger_id.clone(),
            repository: event.repository.clone(),
            number: event.number,
            title: event.title.clone(),
            actor: event.actor.clone(),
            url: event.url.clone(),
            status: "queued".into(),
            current_step: None,
            created_at: created.clone(),
            started_at: None,
            finished_at: None,
            error: None,
            attempt,
            workspace: None,
        };
        if let Some((comments, approved)) = approval {
            let release_admitted: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM observations WHERE key=?1)",
                [format!(
                    "release-approved:{}#{}@{}",
                    event.repository, event.number, event.head_sha
                )],
                |row| row.get(0),
            )?;
            if !event.release_requested() && release_admitted {
                // Bind already observed OK comments to this head even when a
                // release supersedes the normal approval. They cannot approve
                // a later conflict correction.
                for comment in comments {
                    tx.execute(
                        "INSERT OR IGNORE INTO observations VALUES(?1,'true')",
                        [format!(
                            "approval-comment:{}#{}:{comment}",
                            event.repository, event.number
                        )],
                    )?;
                }
                return Ok(None);
            }
            if event.release_requested() {
                let selected = event.payload["approval"]["comment_id"].as_u64().unwrap();
                let consumed: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM observations WHERE key=?1)",
                    [format!(
                        "approval-comment:{}#{}:{selected}",
                        event.repository, event.number
                    )],
                    |row| row.get(0),
                )?;
                if !comments.contains(&selected) || consumed {
                    return Ok(None);
                }
            }
            let mut fresh = false;
            for id in comments {
                fresh |= tx.execute(
                    "INSERT OR IGNORE INTO observations VALUES(?1,?2)",
                    params![
                        format!(
                            "approval-comment:{}#{}:{id}",
                            event.repository, event.number
                        ),
                        "true"
                    ],
                )? == 1;
            }
            if !approved && !fresh {
                return Ok(None);
            }
        }
        if dedup
            && tx.execute(
                "INSERT OR IGNORE INTO dedup(event_id,run_id) VALUES(?1,?2)",
                params![event.event_id, id],
            )? == 0
        {
            return Ok(None);
        }
        if event.release_requested() {
            tx.execute(
                "INSERT OR IGNORE INTO observations VALUES(?1,'true')",
                [format!(
                    "release-approved:{}#{}@{}",
                    event.repository, event.number, event.head_sha
                )],
            )?;
        }
        tx.execute(
            "INSERT INTO runs VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                id,
                event.event_id,
                serde_json::to_string(&run)?,
                serde_json::to_string(event)?,
                serde_json::to_string(flow)?,
                created
            ],
        )?;
        for n in &flow.nodes {
            let state = NodeRun {
                id: n.id.clone(),
                name: n.name.clone(),
                agent: n.agent.clone(),
                status: "pending".into(),
                started_at: None,
                finished_at: None,
                error: None,
                result: None,
                session_id: None,
            };
            tx.execute(
                "INSERT INTO nodes VALUES(?1,?2,?3)",
                params![id, n.id, serde_json::to_string(&state)?],
            )?;
        }
        Ok(Some(id))
    }
    pub fn import_legacy(
        &self,
        batch: &crate::context::factory::infrastructure::migration::legacy::LegacyImport,
    ) -> Result<usize> {
        self.write_with_settings(batch.settings.as_ref(), |tx| {
        let mut imported = 0;
        for row in &batch.runs {
            let d = &row.detail;
            let r = &d.run;
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM run_deletions WHERE run_id=?1)",[&r.id],|r|r.get::<_,bool>(0))? {continue;}
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO runs VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    r.id,
                    r.event_id,
                    serde_json::to_string(r)?,
                    serde_json::to_string(&d.event)?,
                    serde_json::to_string(&d.flow)?,
                    r.created_at
                ],
            )?;
            imported += inserted;
            if inserted == 1 {
                for n in &d.nodes {
                    tx.execute(
                        "INSERT OR IGNORE INTO nodes VALUES(?1,?2,?3)",
                        params![r.id, n.id, serde_json::to_string(n)?],
                    )?;
                }
            }
        }
        for id in &batch.consumed_events {
            let source = batch
                .runs
                .iter()
                .find(|r| r.detail.run.event_id == *id)
                .map(|r| r.detail.run.id.as_str())
                .unwrap_or("legacy-consumed");
            tx.execute(
                "INSERT OR IGNORE INTO dedup VALUES(?1,?2)",
                params![id, source],
            )?;
        }
        for c in &batch.approval_comments {
            tx.execute(
                "INSERT OR IGNORE INTO observations VALUES(?1,'true')",
                [format!(
                    "approval-comment:{}#{}:{}",
                    c.repository, c.number, c.comment_id
                )],
            )?;
        }
        let mut reviews = std::collections::HashMap::<String, serde_json::Value>::new();
        for r in &batch.reviewer_requests {
            let owner = r.repository.split('/').next().unwrap_or("");
            let key = format!("review:{owner}:{}", r.reviewer);
            let value = reviews.entry(key).or_insert(serde_json::json!({}));
            value[format!("{}#{}", r.repository, r.number)] =
                serde_json::json!({"requested":r.requested,"generation":r.generation});
        }
        for (key, mut value) in reviews {
            let existing: Option<String> = tx
                .query_row("SELECT json FROM observations WHERE key=?1", [&key], |r| {
                    r.get(0)
                })
                .optional()?;
            if let Some(existing) = existing {
                if let Ok(existing) = serde_json::from_str::<serde_json::Value>(&existing) {
                    for (k, v) in existing.as_object().into_iter().flatten() {
                        value[k] = v.clone();
                    }
                }
            }
            tx.execute("INSERT INTO observations VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json",params![key,serde_json::to_string(&value)?])?;
        }
        for pr in &batch.registered_prs {
            super::pr_origins::project(tx,&pr.repository,pr.number,&pr.source_run_id,Some(&pr.source_issue))?;
        }
        for conflict in &batch.pr_conflicts {
            super::pr_origins::project(tx,&conflict.repository,conflict.number,&conflict.source_run_id,None)?;
        }
        Ok(imported)
        })
    }
    pub fn runs(&self) -> Result<Vec<Run>> {
        let db = self.db.lock().unwrap();
        let mut q = db.prepare("SELECT json FROM runs WHERE NOT EXISTS(SELECT 1 FROM run_deletions d WHERE d.run_id=runs.id) ORDER BY created_at DESC,id DESC")?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn nodes(&self, id: &str) -> Result<Vec<NodeRun>> {
        let db = self.db.lock().unwrap();
        let mut q = db.prepare("SELECT json FROM nodes WHERE run_id=?1 AND NOT EXISTS(SELECT 1 FROM run_deletions d WHERE d.run_id=nodes.run_id) ORDER BY rowid")?;
        let rows = q.query_map([id], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn detail(&self, id: &str) -> Result<RunDetail> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let detail = super::run_deletions::detail_db(&tx, id)?;
        tx.commit()?;
        Ok(detail)
    }
    pub fn claim_queued(&self, id: &str) -> Result<bool> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let raw: Option<String> = tx
            .query_row("SELECT json FROM runs WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM run_deletions WHERE run_id=runs.id)", [id], |r| r.get(0))
            .optional()?;
        let Some(raw) = raw else { return Ok(false) };
        let mut r: Run = serde_json::from_str(&raw)?;
        if r.status != "queued" {
            return Ok(false);
        }
        r.status = "running".into();
        r.started_at = Some(now());
        tx.execute(
            "UPDATE runs SET json=?1 WHERE id=?2",
            params![serde_json::to_string(&r)?, id],
        )?;
        tx.commit()?;
        Ok(true)
    }
    pub fn save_run(&self, r: &Run) -> Result<()> {
        let mut connection = self.db.lock().unwrap();
        let db = connection.transaction()?;
        let raw: Option<String> = db
            .query_row("SELECT json FROM runs WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM run_deletions WHERE run_id=runs.id)", [&r.id], |r| r.get(0))
            .optional()?;
        let previous: Run = serde_json::from_str(&raw.context("Run not found")?)?;
        if !["queued", "running"].contains(&previous.status.as_str()) && r.status != previous.status
        {
            return Ok(());
        }
        // Workspace lifecycle has its own atomic update; stale status snapshots must not restore it.
        let mut r = r.clone();
        r.workspace = previous.workspace;
        ensure!(
            db.execute(
                "UPDATE runs SET json=?1 WHERE id=?2",
                params![serde_json::to_string(&r)?, r.id]
            )? == 1,
            "Run not found"
        );
        db.commit()?;
        Ok(())
    }
    pub(crate) fn set_workspace(&self, id: &str, workspace: Option<&str>) -> Result<()> {
        ensure!(
            self.db.lock().unwrap().execute(
                "UPDATE runs SET json=json_set(json,'$.workspace',?2) WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM run_deletions WHERE run_id=runs.id)",
                params![id, workspace]
            )? == 1,
            "Run not found"
        );
        Ok(())
    }
    pub(crate) fn set_node_result(
        &self,
        run: &str,
        node: &str,
        result: &serde_json::Value,
    ) -> Result<()> {
        ensure!(self.db.lock().unwrap().execute("UPDATE nodes SET json=json_set(json,'$.result',json(?3)) WHERE run_id=?1 AND node_id=?2 AND EXISTS(SELECT 1 FROM runs WHERE id=nodes.run_id) AND NOT EXISTS(SELECT 1 FROM run_deletions d WHERE d.run_id=nodes.run_id)", params![run, node, serde_json::to_string(result)?])? == 1, "Node not found or deleting");
        Ok(())
    }
    pub fn save_node(&self, run: &str, n: &NodeRun) -> Result<()> {
        let mut n = n.clone();
        let mut connection = self.db.lock().unwrap();
        let db = connection.transaction()?;
        ensure!(
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM run_deletions WHERE run_id=runs.id))",
                [run],
                |r| r.get::<_, bool>(0)
            )?,
            "Run not found or deleting"
        );
        let raw: Option<String> = db
            .query_row(
                "SELECT json FROM nodes WHERE run_id=?1 AND node_id=?2",
                params![run, n.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(raw) = raw {
            let previous: NodeRun = serde_json::from_str(&raw)?;
            if !["pending", "running"].contains(&previous.status.as_str())
                && n.status != previous.status
            {
                return Ok(());
            }
            if n.result.is_none() {
                n.result = previous.result;
            }
        }
        ensure!(
            db.execute(
                "UPDATE nodes SET json=?1 WHERE run_id=?2 AND node_id=?3",
                params![serde_json::to_string(&n)?, run, n.id],
            )? == 1,
            "Node not found"
        );
        db.commit()?;
        Ok(())
    }
    /// This is the only transition from terminal nodes back to pending. Archive
    /// evidence and feedback in the same transaction, without reviving cancelled runs.
    pub(crate) fn requeue_review(
        &self,
        run: &str,
        rework: &crate::context::factory::application::services::reviews::ReviewRework,
    ) -> Result<bool> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let raw: String = tx.query_row("SELECT json FROM runs WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM run_deletions WHERE run_id=runs.id)", [run], |r| r.get(0))?;
        let state: Run = serde_json::from_str(&raw)?;
        if state.status != "running" {
            return Ok(false);
        }
        let previous: Vec<NodeRun> = {
            let mut query = tx.prepare("SELECT json FROM nodes WHERE run_id=?1 ORDER BY rowid")?;
            let rows = query.query_map([run], |r| r.get::<_, String>(0))?;
            rows.map(|row| Ok(serde_json::from_str::<NodeRun>(&row?)?))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .filter(|n| rework.affected.contains(&n.id))
                .collect()
        };
        ensure!(
            previous.iter().all(|n| n.status != "running"),
            "Cannot requeue active review work"
        );
        tx.execute(
            "INSERT INTO observations VALUES(?1,?2)",
            params![
                format!("review-history:{run}:{}", uuid::Uuid::new_v4()),
                serde_json::to_string(
                    &serde_json::json!({"nodes":previous,"feedback":rework.feedback})
                )?
            ],
        )?;
        for mut node in previous {
            if rework.authors.contains(&node.id) {
                let feedback =
                    serde_json::json!({"previous_result":node.result,"reviews":rework.feedback});
                tx.execute("INSERT INTO observations VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json", params![
                    format!("review-feedback:{run}:{}", node.id), serde_json::to_string(&feedback)?
                ])?;
            } else {
                // A downstream author may have feedback from an obsolete round.
                tx.execute(
                    "DELETE FROM observations WHERE key=?1",
                    [format!("review-feedback:{run}:{}", node.id)],
                )?;
            }
            node.status = "pending".into();
            node.started_at = None;
            node.finished_at = None;
            node.error = None;
            node.result = None;
            node.session_id = None;
            tx.execute(
                "UPDATE nodes SET json=?1 WHERE run_id=?2 AND node_id=?3",
                params![serde_json::to_string(&node)?, run, node.id],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }
    pub fn observation(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let v: Option<String> = self
            .db
            .lock()
            .unwrap()
            .query_row("SELECT json FROM observations WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(v.map(|s| serde_json::from_str(&s)).transpose()?)
    }
    pub fn observe(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let owner = key
            .strip_prefix("identity:")
            .or_else(|| key.strip_prefix("pr-workspace:"))
            .or_else(|| key.strip_prefix("pr-update:"))
            .or_else(|| key.strip_prefix("pr-merge:"))
            .or_else(|| key.strip_prefix("release-workspace:"))
            .or_else(|| key.strip_prefix("pr-release:"))
            .or_else(|| {
                key.strip_prefix("review-feedback:")
                    .and_then(|s| s.split(':').next())
            })
            .or_else(|| {
                key.strip_prefix("review-history:")
                    .and_then(|s| s.split(':').next())
            });
        if let Some(id) = owner {
            ensure!(tx.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM run_deletions WHERE run_id=runs.id))",[id],|r|r.get::<_,bool>(0))?,"Run not found or deleting");
        }
        tx.execute("INSERT INTO observations VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json",params![key,serde_json::to_string(value)?])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn stale_cancel_snapshot_keeps_archived_result() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let settings = Settings::default();
        let id = store
            .insert(
                &TriggerEvent::issue("acme/repo", 1, "[AI] fixture", "alice", ""),
                &settings.flows[0],
                1,
                true,
            )
            .unwrap()
            .unwrap();
        let mut stale = store.nodes(&id).unwrap()[0].clone();
        stale.status = "cancelled".into();
        let partial = serde_json::json!({"status":"failed","summary":"evidence retained","artifacts":[{"path":"persisted/report.txt","sha256":"fixture","kind":"report"}],"data":{}});
        store.set_node_result(&id, &stale.id, &partial).unwrap();
        store.save_node(&id, &stale).unwrap();
        let updated = store.nodes(&id).unwrap().remove(0);
        assert_eq!(updated.status, "cancelled");
        assert_eq!(updated.result, Some(partial));
    }

    #[test]
    fn review_requeue_cannot_revive_cancelled_run_or_reset_active_work() {
        use crate::context::factory::application::services::reviews::ReviewRework;
        use std::collections::HashSet;
        for cancelled in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(dir.path()).unwrap();
            let mut flow = Settings::default().flows.remove(0);
            flow.nodes.truncate(1);
            let id = store
                .insert(
                    &TriggerEvent::issue("acme/repo", 1, "[AI] fixture", "alice", ""),
                    &flow,
                    1,
                    true,
                )
                .unwrap()
                .unwrap();
            store.claim_queued(&id).unwrap();
            let mut node = store.nodes(&id).unwrap().remove(0);
            node.status = "running".into();
            store.save_node(&id, &node).unwrap();
            if cancelled {
                let mut run = store.detail(&id).unwrap().run;
                run.status = "cancelled".into();
                store.save_run(&run).unwrap();
            }
            let rework = ReviewRework {
                authors: HashSet::from([node.id.clone()]),
                affected: HashSet::from([node.id.clone()]),
                feedback: vec![],
            };
            let result = store.requeue_review(&id, &rework);
            if cancelled {
                assert!(!result.unwrap());
            } else {
                assert!(result.is_err());
            }
            assert_eq!(store.nodes(&id).unwrap()[0].status, "running");
            let count: i64 = store
                .db
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "rejected reset must leave no history or feedback");
        }
    }
}
