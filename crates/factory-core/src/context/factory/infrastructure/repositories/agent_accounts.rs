use super::store::Store;
use crate::{AgentAccount, Result};
use anyhow::{ensure, Context};
use rusqlite::{params, OptionalExtension};

pub(super) fn migrate(db: &rusqlite::Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_accounts(id TEXT PRIMARY KEY,json TEXT NOT NULL);",
    )?;
    // Login callbacks and their URLs are process-local. Restart never revives them.
    db.execute("UPDATE agent_accounts SET json=json_set(json,'$.status','failed','$.error','Login interrupted; connect the account again') WHERE json_extract(json,'$.status')='pending'", [])?;
    Ok(())
}

impl Store {
    pub(crate) fn agent_accounts(&self) -> Result<Vec<AgentAccount>> {
        let db = self.db.lock().unwrap();
        let mut query = db.prepare("SELECT json FROM agent_accounts ORDER BY rowid")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub(crate) fn agent_account(&self, id: &str) -> Result<AgentAccount> {
        let raw: Option<String> = self
            .db
            .lock()
            .unwrap()
            .query_row("SELECT json FROM agent_accounts WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        serde_json::from_str(&raw.context("Agent account not found")?).map_err(Into::into)
    }

    pub(crate) fn save_agent_account(&self, account: &AgentAccount) -> Result<()> {
        self.db.lock().unwrap().execute("INSERT INTO agent_accounts(id,json) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET json=excluded.json", params![account.id,serde_json::to_string(account)?])?;
        Ok(())
    }

    pub(crate) fn ensure_agent_account_unused(
        &self,
        id: &str,
        active_workers: &[String],
    ) -> Result<()> {
        ensure_unused(&self.db.lock().unwrap(), id, active_workers)
    }

    pub(crate) fn delete_agent_account(
        &self,
        id: &str,
        active_workers: &[String],
        remove_home: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let _settings = self.settings_lock.lock().unwrap();
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        ensure_unused(&tx, id, active_workers)?;
        let raw: Option<String> = tx
            .query_row("SELECT json FROM settings WHERE id=1", [], |row| row.get(0))
            .optional()?;
        if let Some(raw) = raw {
            let mut value: serde_json::Value = serde_json::from_str(&raw)?;
            if let Some(bindings) = value
                .get_mut("codex_bindings")
                .and_then(|v| v.as_object_mut())
            {
                bindings.retain(|_, value| value.as_str() != Some(id));
            }
            tx.execute(
                "UPDATE settings SET json=?1 WHERE id=1",
                [serde_json::to_string(&value)?],
            )?;
        }
        ensure!(
            tx.execute("DELETE FROM agent_accounts WHERE id=?1", [id])? == 1,
            "Agent account not found"
        );
        remove_home()?;
        tx.commit()?;
        Ok(())
    }
}

fn ensure_unused(db: &rusqlite::Connection, id: &str, active_workers: &[String]) -> Result<()> {
    let used: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM runs, json_each(runs.flow,'$.nodes') AS node WHERE json_extract(runs.json,'$.status') IN ('queued','running') AND json_extract(node.value,'$.codex_account_id')=?1)", [id], |row| row.get(0))?;
    ensure!(
        !used,
        "Agent account is assigned to a queued or running run"
    );
    for worker in active_workers {
        let used: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM runs, json_each(runs.flow,'$.nodes') AS node WHERE runs.id=?2 AND json_extract(node.value,'$.codex_account_id')=?1)", params![id,worker], |row| row.get(0))?;
        ensure!(
            !used,
            "Agent account is assigned to an active worker; wait for cleanup to finish"
        );
    }
    Ok(())
}
