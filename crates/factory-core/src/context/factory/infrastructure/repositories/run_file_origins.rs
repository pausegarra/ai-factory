use crate::*;
use rusqlite::{Connection, OptionalExtension};
pub(super) fn migrate(db: &mut Connection) -> Result<()> {
    let tx = db.transaction()?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='run_file_origins')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        tx.execute_batch("CREATE TABLE run_file_origins(run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,origin TEXT NOT NULL CHECK(origin IN ('local','imported'))); INSERT INTO run_file_origins SELECT id,'local' FROM runs;")?;
    }
    tx.commit()?;
    Ok(())
}
pub(super) fn origin_db(db: &Connection, id: &str) -> Result<Option<RunFileOrigin>> {
    let row:Option<Option<String>>=db.query_row("SELECT (SELECT origin FROM run_file_origins WHERE run_id=runs.id) FROM runs WHERE id=?1",[id],|r|r.get(0)).optional()?;
    match row {
        None => Ok(None),
        Some(Some(v)) if v == "local" => Ok(Some(RunFileOrigin::Local)),
        Some(Some(v)) if v == "imported" => Ok(Some(RunFileOrigin::Imported)),
        _ => anyhow::bail!("Invalid run file provenance"),
    }
}
impl super::store::Store {
    pub(crate) fn run_file_origin(&self, id: &str) -> Result<Option<RunFileOrigin>> {
        origin_db(&self.db.lock().unwrap(), id)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn run_file_origins_migration_backfills_once_and_never_adopts_missing_rows() {
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE runs(id TEXT PRIMARY KEY); INSERT INTO runs VALUES('old'),('legacy-0123456789abcdef0123456789abcdef');").unwrap();
        migrate(&mut db).unwrap();
        assert_eq!(
            origin_db(&db, "old").unwrap(),
            Some(crate::RunFileOrigin::Local)
        );
        db.execute(
            "UPDATE run_file_origins SET origin='imported' WHERE run_id='old'",
            [],
        )
        .unwrap();
        migrate(&mut db).unwrap();
        assert_eq!(
            origin_db(&db, "old").unwrap(),
            Some(crate::RunFileOrigin::Imported)
        );
        db.execute("DELETE FROM run_file_origins WHERE run_id='old'", [])
            .unwrap();
        migrate(&mut db).unwrap();
        assert!(origin_db(&db, "old").is_err());
        assert_eq!(origin_db(&db, "absent").unwrap(), None);
    }
}
