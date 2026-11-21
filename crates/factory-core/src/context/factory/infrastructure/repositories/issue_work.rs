use super::store::Store;
use crate::context::factory::domain::entities::issue_work::{
    IssuePublication, IssueScope, IssueWork,
};
use crate::{
    IssueRepositoryStatus, IssueRepositorySummary, IssueWorkSummary, Result, TriggerEvent,
};
use anyhow::ensure;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::PathBuf;

pub(super) fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS issue_work (
            event_id TEXT PRIMARY KEY,
            scope TEXT NOT NULL,
            publication TEXT,
            closed INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS issue_work_bundles (
            event_id TEXT NOT NULL,
            repository TEXT NOT NULL,
            bundle BLOB NOT NULL,
            PRIMARY KEY(event_id,repository),
            FOREIGN KEY(event_id) REFERENCES issue_work(event_id) ON DELETE CASCADE
        );",
    )?;
    Ok(())
}

impl Store {
    pub(crate) fn issue_work_summary(
        &self,
        event: &TriggerEvent,
    ) -> Result<Option<IssueWorkSummary>> {
        summary(&self.db.lock().unwrap(), event)
    }
    pub(crate) fn issue_work(&self, event: &TriggerEvent) -> Result<Option<IssueWork>> {
        Ok(match self.issue_scope(event)? {
            Some(IssueScope::SubIssues(scope)) => Some(scope),
            _ => None,
        })
    }

    pub(crate) fn issue_scope(&self, event: &TriggerEvent) -> Result<Option<IssueScope>> {
        if event.trigger_id != "new_issue" {
            return Ok(None);
        }
        let raw: Option<String> = self
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT scope FROM issue_work WHERE event_id=?1",
                [&event.event_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let scope: Option<IssueWork> = serde_json::from_str(&raw)?;
        match scope {
            Some(scope) => {
                ensure!(
                    scope.matches(event),
                    "Stored issue scope does not match the event"
                );
                Ok(Some(IssueScope::SubIssues(scope)))
            }
            None => Ok(Some(IssueScope::SingleRepository)),
        }
    }

    pub(crate) fn save_issue_scope(
        &self,
        run: &str,
        event: &TriggerEvent,
        scope: Option<&IssueWork>,
    ) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let detail = super::run_deletions::detail_db(&tx, run)?;
        ensure!(
            detail.run.status == "running"
                && detail.event.event_id == event.event_id
                && scope.is_none_or(|scope| scope.matches(&detail.event)),
            "Run no longer active or authorized to resolve scope"
        );
        tx.execute(
            "INSERT INTO issue_work(event_id,scope) VALUES(?1,?2)",
            params![event.event_id, serde_json::to_string(&scope)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn issue_publication(&self, scope: &IssueWork) -> Result<Option<IssuePublication>> {
        issue_publication(&self.db.lock().unwrap(), &scope.event_id)
    }

    pub(crate) fn stage_issue_publication(
        &self,
        scope: &IssueWork,
        publication: &IssuePublication,
        bundles: Vec<PathBuf>,
        run: &str,
    ) -> Result<()> {
        ensure!(
            publication.targets.len() == bundles.len(),
            "Incomplete publication bundles"
        );
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let verified = super::run_deletions::detail_db(&tx, run)?;
        ensure!(
            scope.matches(&verified.event)
                && verified.run.status == "running"
                && verified.nodes.iter().all(|node| node.status == "succeeded"),
            "All workflow steps must succeed before staging publication"
        );
        ensure!(
            tx.execute(
                "UPDATE issue_work SET publication=?2 WHERE event_id=?1 AND publication IS NULL",
                params![scope.event_id, serde_json::to_string(publication)?],
            )? == 1,
            "Issue publication was already prepared"
        );
        for (target, path) in publication.targets.iter().zip(bundles) {
            tx.execute(
                "INSERT INTO issue_work_bundles VALUES(?1,?2,?3)",
                params![scope.event_id, target.repository, std::fs::read(path)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn issue_bundle(&self, scope: &IssueWork, repo: &str) -> Result<Vec<u8>> {
        Ok(self.db.lock().unwrap().query_row(
            "SELECT bundle FROM issue_work_bundles WHERE event_id=?1 AND repository=?2",
            params![scope.event_id, repo],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn record_issue_pr(
        &self,
        scope: &IssueWork,
        event: &TriggerEvent,
        run: &str,
        repo: &str,
        number: u64,
    ) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let detail = super::run_deletions::detail_db(&tx, run)?;
        ensure!(
            scope.matches(event) && scope.matches(&detail.event) && detail.run.status == "running",
            "Publication run no longer active or authorized"
        );
        let mut publication = issue_publication(&tx, &scope.event_id)?
            .ok_or_else(|| anyhow::anyhow!("Issue publication not prepared"))?;
        let target = publication
            .targets
            .iter_mut()
            .find(|target| target.repository == repo)
            .ok_or_else(|| anyhow::anyhow!("Repository is outside publication scope"))?;
        ensure!(
            target.number.is_none_or(|previous| previous == number),
            "PR identity changed"
        );
        target.number = Some(number);
        super::pr_origins::project_issue_work(&tx, repo, number, run, event)?;
        tx.execute(
            "UPDATE issue_work SET publication=?2 WHERE event_id=?1",
            params![scope.event_id, serde_json::to_string(&publication)?],
        )?;
        tx.execute(
            "DELETE FROM issue_work_bundles WHERE event_id=?1 AND repository=?2",
            params![scope.event_id, repo],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn pending_issue_closures(&self) -> Result<Vec<(IssueWork, IssuePublication)>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT scope,publication FROM issue_work WHERE closed=0 AND publication IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut pending = Vec::new();
        for row in rows {
            let (scope, publication) = row?;
            let publication: IssuePublication = serde_json::from_str(&publication)?;
            if publication
                .targets
                .iter()
                .all(|target| target.number.is_some())
            {
                pending.push((serde_json::from_str(&scope)?, publication));
            }
        }
        Ok(pending)
    }

    pub(crate) fn mark_issue_work_closed(&self, scope: &IssueWork) -> Result<Vec<String>> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "UPDATE issue_work SET closed=1 WHERE event_id=?1",
            [&scope.event_id],
        )?;
        let ids: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM runs WHERE event_id=?1")?;
            let rows = stmt.query_map([&scope.event_id], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if ids.is_empty() {
            purge_unowned(&tx, &scope.event_id)?;
        }
        tx.commit()?;
        Ok(ids)
    }
}

pub(super) fn purge_unowned(tx: &Transaction<'_>, event_id: &str) -> Result<()> {
    let orphaned: Option<(String, bool)> = tx.query_row(
        "SELECT scope,closed FROM issue_work WHERE event_id=?1 AND NOT EXISTS(SELECT 1 FROM runs WHERE event_id=?1)",
        [event_id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    let Some((raw_scope, closed)) = orphaned else {
        return Ok(());
    };
    let publication = issue_publication(tx, event_id)?;
    if let Some(mut publication) = publication.filter(|publication| {
        !closed && {
            !publication.targets.is_empty()
                && publication
                    .targets
                    .iter()
                    .all(|target| target.number.is_some())
        }
    }) {
        // Retain only the operational information needed to reconcile the parent.
        let mut scope: IssueWork = serde_json::from_str(&raw_scope)?;
        for issue in scope
            .repositories
            .iter_mut()
            .flat_map(|repo| &mut repo.sub_issues)
        {
            issue.title.clear();
            issue.body.clear();
        }
        publication.result = None;
        publication.flow = None;
        for target in &mut publication.targets {
            target.title.clear();
            target.body.clear();
        }
        tx.execute(
            "UPDATE issue_work SET scope=?2,publication=?3 WHERE event_id=?1",
            params![
                event_id,
                serde_json::to_string(&scope)?,
                serde_json::to_string(&publication)?
            ],
        )?;
    } else {
        tx.execute("DELETE FROM issue_work WHERE event_id=?1", [event_id])?;
    }
    Ok(())
}

pub(super) fn summary(db: &Connection, event: &TriggerEvent) -> Result<Option<IssueWorkSummary>> {
    if event.trigger_id != "new_issue" {
        return Ok(None);
    }
    let row: Option<(String, Option<String>, bool)> = db
        .query_row(
            "SELECT scope,publication,closed FROM issue_work WHERE event_id=?1",
            [&event.event_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((scope, publication, parent_closed)) = row else {
        return Ok(None);
    };
    let Some(scope) = serde_json::from_str::<Option<IssueWork>>(&scope)? else {
        return Ok(None);
    };
    if !scope.matches(event) {
        return Ok(None);
    }
    let publication: Option<IssuePublication> = publication
        .map(|raw| serde_json::from_str(&raw))
        .transpose()?;
    let repositories = scope
        .repositories
        .into_iter()
        .map(|repo| {
            let target = publication.as_ref().and_then(|publication| {
                publication
                    .targets
                    .iter()
                    .find(|target| target.repository == repo.repository)
            });
            let pull_request = target.and_then(|target| target.number);
            let status = match (publication.is_some(), target.is_some(), pull_request) {
                (_, _, Some(_)) => IssueRepositoryStatus::Published,
                (_, true, None) => IssueRepositoryStatus::Pending,
                (true, false, None) => IssueRepositoryStatus::NoChanges,
                (false, false, None) => IssueRepositoryStatus::Selected,
            };
            IssueRepositorySummary {
                repository: repo.repository,
                sub_issues: repo
                    .sub_issues
                    .into_iter()
                    .map(|issue| issue.number)
                    .collect(),
                pull_request,
                status,
            }
        })
        .collect();
    Ok(Some(IssueWorkSummary {
        repositories,
        parent_closed,
    }))
}

pub(super) fn issue_publication(
    db: &Connection,
    event_id: &str,
) -> Result<Option<IssuePublication>> {
    let raw: Option<String> = db
        .query_row(
            "SELECT publication FROM issue_work WHERE event_id=?1",
            [event_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    Ok(raw.map(|raw| serde_json::from_str(&raw)).transpose()?)
}
