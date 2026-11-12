use super::store::Store;
use crate::*;
use anyhow::{ensure, Context};
use rusqlite::{params, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrackedPrOrigin {
    pub repository: String,
    pub number: u64,
    pub event_id: String,
    pub issue_repository: String,
    pub issue_number: u64,
    pub actor: String,
}

fn trusted_event(repo: &str, event: &TriggerEvent) -> bool {
    valid_repository(repo)
        && event.repository.eq_ignore_ascii_case(repo)
        && valid_repository(&event.repository)
        && event.trigger_id == "new_issue"
        && event.number > 0
        && i64::try_from(event.number).is_ok()
        && !event.event_id.is_empty()
        && valid_login(&event.actor)
}

pub(super) fn project(
    tx: &Transaction<'_>,
    repo: &str,
    number: u64,
    source: &str,
    event: Option<&TriggerEvent>,
) -> Result<()> {
    project_number(tx, repo, i64::try_from(number)?, source, event)
}

fn project_number(
    tx: &Transaction<'_>,
    repo: &str,
    number: i64,
    source: &str,
    event: Option<&TriggerEvent>,
) -> Result<()> {
    let valid = number > 0 && event.is_some_and(|e| trusted_event(repo, e));
    let event = event.filter(|_| valid);
    tx.execute(
        // Conflict evidence can revoke trust but later imports cannot restore it.
        "INSERT INTO tracked_pr_origins VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(repository,number,origin_key) DO UPDATE SET
         event_id=NULL,issue_repository=NULL,issue_number=NULL,actor=NULL,trigger_id=NULL,trusted=0
         WHERE excluded.trusted=0",
        params![
            repo.to_lowercase(),
            number,
            format!("{:x}", Sha256::digest(source.as_bytes())),
            event.map(|e| e.event_id.as_str()),
            event.map(|e| e.repository.to_lowercase()),
            event.map(|e| i64::try_from(e.number)).transpose()?,
            event.map(|e| e.actor.to_lowercase()),
            event.map(|e| e.trigger_id.as_str()),
            valid
        ],
    )?;
    Ok(())
}

pub(super) fn backfill(tx: &Transaction<'_>) -> Result<()> {
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='tracked_prs')",
        [],
        |r| r.get(0),
    )?;
    if exists {
        let rows = {
            let mut q = tx.prepare("SELECT repository,number,source_run FROM tracked_prs")?;
            let rows = q.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (repo, number, source) in rows {
            let raw: Option<(String, String)> = tx
                .query_row("SELECT json,event FROM runs WHERE id=?1", [&source], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            let event = raw.and_then(|(run, event)| {
                let run: Run = serde_json::from_str(&run).ok()?;
                let event: TriggerEvent = serde_json::from_str(&event).ok()?;
                (run.id == source
                    && run.repository.eq_ignore_ascii_case(&repo)
                    && run.event_id == event.event_id
                    && run.trigger_id == event.trigger_id
                    && run.number == event.number
                    && run.actor.eq_ignore_ascii_case(&event.actor)
                    && run.repository == event.repository)
                    .then_some(event)
            });
            project_number(tx, &repo, number, &source, event.as_ref())?;
        }
        tx.execute_batch("DROP TABLE tracked_prs;")?;
    }
    let receipts = {
        let mut q =
            tx.prepare("SELECT key,json FROM observations WHERE substr(key,1,7)='action:'")?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (key, raw) in receipts {
        if receipt_key(&key) {
            let value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
            tx.execute(
                "UPDATE observations SET json=?2 WHERE key=?1",
                params![key, serde_json::to_string(&compact_receipt(&value))?],
            )?;
        }
    }
    Ok(())
}
fn receipt_key(key: &str) -> bool {
    key.strip_prefix("action:")
        .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
fn compact_receipt(receipt: &serde_json::Value) -> serde_json::Value {
    let mut value = serde_json::json!({"completed":true});
    if let Some(id) = receipt
        .get("remote_id")
        .or_else(|| receipt.get("id"))
        .filter(|id| id.is_i64() || id.is_u64())
    {
        value["remote_id"] = id.clone();
    }
    value
}
impl Store {
    pub fn track_pr(&self, repo: &str, number: u64, source_run: &str) -> Result<()> {
        ensure!(valid_repository(repo) && number > 0, "Invalid PR identity");
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let detail = super::run_deletions::detail_db(&tx, source_run)?;
        ensure!(
            detail.run.id == source_run
                && detail.run.repository == detail.event.repository
                && detail.run.event_id == detail.event.event_id
                && detail.run.trigger_id == detail.event.trigger_id
                && detail.run.number == detail.event.number
                && detail.run.actor.eq_ignore_ascii_case(&detail.event.actor)
                && trusted_event(repo, &detail.event),
            "Invalid PR origin"
        );
        project(&tx, repo, number, source_run, Some(&detail.event))?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn tracked_prs(&self) -> Result<Vec<TrackedPrOrigin>> {
        let db = self.db.lock().unwrap();
        let mut q=db.prepare("SELECT repository,number,event_id,issue_repository,issue_number,actor FROM tracked_pr_origins GROUP BY repository,number HAVING MIN(trusted)=1 AND COUNT(DISTINCT json_array(event_id,issue_repository,issue_number,actor,trigger_id))=1 AND COUNT(event_id)=COUNT(*) AND COUNT(issue_repository)=COUNT(*) AND COUNT(issue_number)=COUNT(*) AND COUNT(actor)=COUNT(*) AND MIN(trigger_id)='new_issue'")?;
        let rows = q.query_map([], |r| {
            Ok(TrackedPrOrigin {
                repository: r.get(0)?,
                number: r
                    .get::<_, i64>(1)?
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                event_id: r.get(2)?,
                issue_repository: r.get(3)?,
                issue_number: r
                    .get::<_, i64>(4)?
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                actor: r.get(5)?,
            })
        })?;
        let origins = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(origins
            .into_iter()
            .filter(|o| {
                valid_repository(&o.repository)
                    && o.number > 0
                    && o.issue_number > 0
                    && o.issue_repository == o.repository
                    && !o.actor.is_empty()
                    && !o.event_id.is_empty()
            })
            .collect())
    }
    pub(crate) fn record_action_receipt(
        &self,
        key: &str,
        receipt: &serde_json::Value,
    ) -> Result<()> {
        ensure!(receipt_key(key), "Invalid action receipt key");
        self.observe(key, &compact_receipt(receipt))
            .context("Could not store action receipt")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v0(root: &std::path::Path) -> Result<rusqlite::Connection> {
        let db = rusqlite::Connection::open(root.join("factory.sqlite3"))?;
        db.execute_batch("CREATE TABLE runs(id TEXT PRIMARY KEY,event_id TEXT NOT NULL,json TEXT NOT NULL,event TEXT NOT NULL,flow TEXT NOT NULL,created_at TEXT NOT NULL);
            CREATE TABLE tracked_prs(repository TEXT NOT NULL,number INTEGER NOT NULL,source_run TEXT NOT NULL,PRIMARY KEY(repository,number,source_run));
            CREATE TABLE observations(key TEXT PRIMARY KEY,json TEXT NOT NULL);")?;
        Ok(db)
    }
    fn source(db: &rusqlite::Connection, id: &str, number: u64, actor: &str) -> Result<()> {
        let e = TriggerEvent::issue(
            "acme/repo",
            number,
            "[AI] private title",
            actor,
            "private-body",
        );
        let r = serde_json::json!({"id":id,"event_id":e.event_id,"trigger_id":"new_issue","repository":"acme/repo","number":number,"title":e.title,"actor":actor,"url":e.url,"status":"succeeded","current_step":null,"created_at":now(),"started_at":null,"finished_at":null,"error":null,"attempt":1,"workspace":null});
        db.execute(
            "INSERT INTO runs VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                id,
                e.event_id,
                r.to_string(),
                serde_json::to_string(&e)?,
                serde_json::to_string(&Settings::default().flows[0])?,
                now()
            ],
        )?;
        Ok(())
    }
    #[test]
    fn v0_upgrade_preserves_all_origins_compacts_receipts_and_reopens_once() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = v0(dir.path())?;
        source(&db, "one", 1, "Alice")?;
        source(&db, "retry", 1, "alice")?;
        source(&db, "other", 2, "alice")?;
        source(&db, "malformed", 3, "alice")?;
        db.execute_batch("INSERT INTO tracked_prs VALUES('acme/repo',5,'one'),('acme/repo',5,'retry'),('acme/repo',6,'one'),('acme/repo',6,'other'),('acme/repo',7,'one'),('acme/repo',7,'missing'),('acme/repo',8,'malformed');UPDATE runs SET event='not json' WHERE id='malformed';")?;
        let key = format!("action:{}", "b".repeat(64));
        let no_id = format!("action:{}", "c".repeat(64));
        for (key, value) in [
            (
                &key,
                serde_json::json!({"id":123,"body":"private-response","user":{"login":"private"}}),
            ),
            (&no_id, serde_json::json!({"body":"private-response"})),
        ] {
            db.execute(
                "INSERT INTO observations VALUES(?1,?2)",
                params![key, value.to_string()],
            )?;
        }
        db.execute(
            "INSERT INTO observations VALUES('review:acme:alice','{\"keep\":true}')",
            [],
        )?;
        drop(db);
        let store = Store::open(dir.path())?;
        let origins = store.tracked_prs()?;
        assert_eq!(origins.len(), 1);
        assert_eq!(origins[0].number, 5);
        assert_eq!(origins[0].actor, "alice");
        assert_eq!(
            store.observation(&key)?,
            Some(serde_json::json!({"completed":true,"remote_id":123}))
        );
        assert_eq!(
            store.observation(&no_id)?,
            Some(serde_json::json!({"completed":true}))
        );
        assert_eq!(
            store.observation("review:acme:alice")?,
            Some(serde_json::json!({"keep":true}))
        );
        {
            let db = store.db.lock().unwrap();
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                db.query_row("SELECT COUNT(*) FROM tracked_pr_origins", [], |r| r
                    .get::<_, i64>(0))?,
                7
            );
            assert_eq!(
                db.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name='tracked_prs'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            db.execute("DELETE FROM runs", [])?;
        }
        assert_eq!(store.tracked_prs()?, origins);
        drop(store);
        let store = Store::open(dir.path())?;
        assert_eq!(store.tracked_prs()?, origins);
        Ok(())
    }
    #[test]
    fn migration_failure_rolls_back_schema_origins_and_receipts() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = v0(dir.path())?;
        source(&db, "one", 1, "alice")?;
        db.execute("INSERT INTO tracked_prs VALUES('acme/repo',5,'one')", [])?;
        db.execute(
            "INSERT INTO observations VALUES(?1,'{\"body\":\"keep until commit\"}')",
            [format!("action:{}", "a".repeat(64))],
        )?;
        db.execute_batch("CREATE TRIGGER stop_upgrade BEFORE UPDATE ON observations BEGIN SELECT RAISE(ABORT,'injected upgrade failure'); END;")?;
        assert!(Store::open(dir.path()).is_err());
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
            0
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM tracked_prs", [], |r| r
                .get::<_, i64>(0))?,
            1
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='run_deletions'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        db.execute_batch("DROP TRIGGER stop_upgrade;")?;
        drop(db);
        assert!(Store::open(dir.path()).is_ok());
        Ok(())
    }
    #[test]
    fn invalid_v0_identity_is_retained_as_untrusted_not_promoted_or_dropped() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = v0(dir.path())?;
        source(&db, "wrong-repo", 1, "alice")?;
        db.execute_batch("INSERT INTO tracked_prs VALUES('other/repo',5,'wrong-repo'),('acme/repo',-1,'wrong-repo'),('acme/repo',0,'wrong-repo');")?;
        drop(db);
        let store = Store::open(dir.path())?;
        assert!(store.tracked_prs()?.is_empty());
        assert_eq!(
            store.db.lock().unwrap().query_row(
                "SELECT COUNT(*) FROM tracked_pr_origins WHERE trusted=0",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            3
        );
        Ok(())
    }
    #[test]
    fn fresh_action_receipts_keep_only_completion_and_integer_id() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let key = format!("action:{}", "a".repeat(64));
        store.record_action_receipt(
            &key,
            &serde_json::json!({"id":42,"body":"secret","user":{"token":"secret"}}),
        )?;
        assert_eq!(
            store.observation(&key)?,
            Some(serde_json::json!({"completed":true,"remote_id":42}))
        );
        store.record_action_receipt(
            &key,
            &serde_json::json!({"id":"not integer","body":"secret"}),
        )?;
        assert_eq!(
            store.observation(&key)?,
            Some(serde_json::json!({"completed":true}))
        );
        assert!(store
            .record_action_receipt("action:bad", &serde_json::json!({}))
            .is_err());
        Ok(())
    }
}
