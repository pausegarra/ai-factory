use super::store::Store;
use crate::*;
use anyhow::{ensure, Context};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::json;
use std::collections::HashSet;

fn origin_is_current(settings: &Settings, event: &TriggerEvent) -> bool {
    event
        .origin_token_profile_id
        .as_ref()
        .is_none_or(|id| safe_id(id) && settings.token_profiles.iter().any(|p| &p.id == id))
}

impl Store {
    pub(crate) fn admit_review_scan(
        &self,
        owner: &str,
        reviewer: &str,
        candidates: &[TriggerEvent],
    ) -> Result<Vec<String>> {
        self.with_admission(|tx, settings| {
            let flow = settings.flows.iter().find(|f| f.id == "pr_review").context("Review flow missing")?;
            if flow.activation_for(reviewer, &settings.token_profiles) != ActivationState::Enabled {
                return Ok(Vec::new());
            }
            let flow = settings.flow_for_actor(flow, reviewer);
            let key = format!("review:{owner}:{reviewer}");
            let raw: Option<String> = tx.query_row("SELECT json FROM observations WHERE key=?1", [&key], |r| r.get(0)).optional()?;
            let mut previous: serde_json::Value = raw.map(|raw| serde_json::from_str(&raw)).transpose()?.unwrap_or(json!({}));
            let map = previous.as_object_mut().context("Invalid review state")?;
            let mut present = HashSet::new();
            let mut created = Vec::new();
            for candidate in candidates {
                if !origin_is_current(settings, candidate) { continue; }
                let identity = format!("{}#{}", candidate.repository, candidate.number);
                if !present.insert(identity.clone()) { continue; }
                let old = map.get(&identity);
                let requested = old.and_then(|v| v["requested"].as_bool()).unwrap_or(false);
                let generation = old.and_then(|v| v["generation"].as_u64()).unwrap_or(0);
                if requested { continue; }
                let generation = generation.checked_add(1).context("Review generation limit reached")?;
                let mut event = candidate.clone();
                event.event_id = format!("review:{}#{}:{reviewer}:{generation}", event.repository, event.number);
                if let Some(id) = Self::insert_in_tx(tx, &event, &flow, 1, true, None)? { created.push(id); }
                map.insert(identity, json!({"requested":true,"generation":generation}));
            }
            for (identity, value) in map.iter_mut() {
                if !present.contains(identity) { value["requested"] = json!(false); }
            }
            tx.execute("INSERT INTO observations VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json", params![key, serde_json::to_string(&previous)?])?;
            Ok(created)
        })
    }
    // Settings and all admission effects share one SQLite transaction. Never hydrate secrets here.
    fn with_admission<T>(
        &self,
        write: impl FnOnce(&Transaction<'_>, &Settings) -> Result<T>,
    ) -> Result<T> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let raw: Option<String> = tx
            .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
            .optional()?;
        let settings: Settings = raw
            .map(|raw| serde_json::from_str(&raw))
            .transpose()?
            .unwrap_or_default();
        let result = write(&tx, &settings)?;
        tx.commit()?;
        Ok(result)
    }

    pub(crate) fn admit_event(&self, event: &TriggerEvent) -> Result<Option<String>> {
        self.with_admission(|tx, settings| {
            if !origin_is_current(settings, event) {
                return Ok(None);
            }
            let flow = settings
                .flows
                .iter()
                .find(|f| f.id == event.trigger_id)
                .context("Unknown trigger")?;
            if flow.activation_for(&event.actor, &settings.token_profiles)
                != ActivationState::Enabled
            {
                return Ok(None);
            }
            let flow = settings.flow_for_actor(flow, &event.actor);
            Self::insert_in_tx(tx, event, &flow, 1, true, None)
        })
    }

    pub(crate) fn admit_approval(
        &self,
        event: &TriggerEvent,
        comments: &[u64],
        approved: bool,
    ) -> Result<Option<String>> {
        ensure!(
            event.trigger_id == "pr_approved",
            "Invalid approval trigger"
        );
        self.with_admission(|tx, settings| {
            if !origin_is_current(settings, event) {
                return Ok(None);
            }
            let flow = settings
                .flows
                .iter()
                .find(|f| f.id == "pr_approved")
                .context("Approval flow missing")?;
            if flow.activation_for(&event.actor, &settings.token_profiles)
                != ActivationState::Enabled
            {
                return Ok(None);
            }
            let flow = settings.flow_for_actor(flow, &event.actor);
            Self::insert_in_tx(tx, event, &flow, 1, true, Some((comments, approved)))
        })
    }

    pub(crate) fn admit_retry(&self, id: &str) -> Result<String> {
        self.with_admission(|tx, settings| {
            let (raw_run, raw_event): (String, String) = tx
                .query_row("SELECT json,event FROM runs WHERE id=?1", [id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?
                .context("Run not found")?;
            let run: Run = serde_json::from_str(&raw_run)?;
            let event: TriggerEvent = serde_json::from_str(&raw_event)?;
            ensure!(
                !["running", "queued"].contains(&run.status.as_str()),
                "Cannot retry active run"
            );
            ensure!(
                !tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runs WHERE event_id=?1 AND json_extract(json,'$.status') IN ('queued','running'))",
                    [&event.event_id],
                    |row| row.get::<_, bool>(0),
                )?,
                "Cannot retry event with another active attempt"
            );
            let flow = settings
                .flows
                .iter()
                .find(|f| f.id == event.trigger_id)
                .context("Flow missing")?;
            let reason = match flow.activation_for(&event.actor, &settings.token_profiles) {
                ActivationState::Enabled => None,
                ActivationState::DisabledGlobally => Some("disabled globally"),
                ActivationState::DisabledForProfile => Some("disabled for this account"),
            };
            if let Some(reason) = reason {
                anyhow::bail!("Workflow {} for @{} is {}", flow.id, event.actor, reason);
            }
            let attempt = run
                .attempt
                .checked_add(1)
                .context("Run attempt limit reached")?;
            let flow = settings.flow_for_actor(flow, &event.actor);
            Self::insert_in_tx(tx, &event, &flow, attempt, false, None)?
                .context("Retry was not inserted")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    #[derive(Default)]
    struct Memory(Mutex<HashMap<String, String>>);
    impl Credentials for Memory {
        fn get(&self, key: &str) -> Result<Option<String>> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        fn set(&self, key: &str, value: &str) -> Result<()> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<()> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }
    fn setup() -> (tempfile::TempDir, Store, Settings) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_with_credentials(dir.path(), Arc::new(Memory::default())).unwrap();
        let mut s = Settings::default();
        s.token_profiles.push(TokenProfile {
            id: "work".into(),
            label: "Work".into(),
            account: "alice".into(),
            token: Some("fixture".into()),
            has_token: false,
            ..Default::default()
        });
        store.save_settings(&s).unwrap();
        (dir, store, s)
    }
    fn candidate(trigger: &str, n: u64) -> TriggerEvent {
        let mut e = TriggerEvent::issue("acme/repo", n, "fixture", "alice", "");
        e.trigger_id = trigger.into();
        e.event_id = if trigger == "pr_review" {
            String::new()
        } else {
            format!("approval:acme/repo#{n}@{}", "a".repeat(40))
        };
        e.head_sha = "a".repeat(40);
        e
    }
    fn counts(store: &Store) -> (i64, i64, i64) {
        let db = store.db.lock().unwrap();
        (
            db.query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
                .unwrap(),
            db.query_row("SELECT COUNT(*) FROM dedup", [], |r| r.get(0))
                .unwrap(),
            db.query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
                .unwrap(),
        )
    }
    #[test]
    fn suppressed_ok_comments_cannot_approve_a_later_corrected_head() {
        let (_dir, store, _) = setup();
        let approved = candidate("pr_approved", 5);
        assert!(store
            .admit_approval(&approved, &[], true)
            .unwrap()
            .is_some());
        let mut ordinary = candidate("pr_approved", 5);
        assert!(store
            .admit_approval(&ordinary, &[8], false)
            .unwrap()
            .is_none());
        assert!(store
            .observation("approval-comment:acme/repo#5:8")
            .unwrap()
            .is_some());
        ordinary.head_sha = "b".repeat(40);
        ordinary.event_id = format!("approval:acme/repo#5@{}", ordinary.head_sha);
        assert!(store
            .admit_approval(&ordinary, &[8], false)
            .unwrap()
            .is_none());
        assert!(store
            .admit_approval(&ordinary, &[9], false)
            .unwrap()
            .is_some());
    }

    #[test]
    fn consumed_ok_comment_cannot_approve_a_changed_head_without_fresh_signal() {
        let (_dir, store, _) = setup();
        let mut event = candidate("pr_approved", 5);
        assert!(store.admit_approval(&event, &[7], false).unwrap().is_some());
        event.head_sha = "b".repeat(40);
        event.event_id = format!("approval:acme/repo#5@{}", event.head_sha);
        assert!(store.admit_approval(&event, &[7], false).unwrap().is_none());
        assert!(store
            .admit_approval(&event, &[7, 8], false)
            .unwrap()
            .is_some());
        assert!(store
            .observation("approval-comment:acme/repo#5:8")
            .unwrap()
            .is_some());
    }

    #[test]
    fn approval_disabled_and_sql_failure_do_not_consume_comment() {
        let (_dir, store, mut s) = setup();
        let e = candidate("pr_approved", 5);
        for global in [false, true] {
            s.flows[1].enabled = global;
            s.flows[1].profile_activation.insert("work".into(), false);
            store.save_settings(&s).unwrap();
            assert!(store.admit_approval(&e, &[7], false).unwrap().is_none());
            assert_eq!(counts(&store), (0, 0, 0));
        }
        s.flows[1].profile_activation.clear();
        store.save_settings(&s).unwrap();
        store.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_nodes BEFORE INSERT ON nodes BEGIN SELECT RAISE(ABORT,'fixture write denied'); END;").unwrap();
        assert!(store.admit_approval(&e, &[7], false).is_err());
        assert_eq!(counts(&store), (0, 0, 0));
        store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER deny_nodes")
            .unwrap();
        assert!(store.admit_approval(&e, &[7], false).unwrap().is_some());
        assert!(store.admit_approval(&e, &[7], false).unwrap().is_none());
        assert_eq!(counts(&store), (1, 1, 1));
    }
    #[test]
    fn review_scan_rising_edges_disabled_scans_and_rollback() {
        let (_dir, store, mut s) = setup();
        let e = candidate("pr_review", 5);
        s.flows[2].profile_activation.insert("work".into(), false);
        store.save_settings(&s).unwrap();
        assert!(store
            .admit_review_scan("acme", "alice", std::slice::from_ref(&e))
            .unwrap()
            .is_empty());
        assert_eq!(counts(&store), (0, 0, 0));
        s.flows[2].profile_activation.clear();
        store.save_settings(&s).unwrap();
        assert_eq!(
            store
                .admit_review_scan("acme", "alice", &[e.clone(), e.clone()])
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .admit_review_scan("acme", "alice", std::slice::from_ref(&e))
            .unwrap()
            .is_empty());
        let before = store.observation("review:acme:alice").unwrap();
        assert_eq!(
            before.as_ref().unwrap()["acme/repo#5"]["generation"],
            json!(1)
        );
        s.flows[2].enabled = false;
        store.save_settings(&s).unwrap();
        store.admit_review_scan("acme", "alice", &[]).unwrap();
        assert_eq!(store.observation("review:acme:alice").unwrap(), before);
        s.flows[2].enabled = true;
        store.save_settings(&s).unwrap();
        store.admit_review_scan("acme", "alice", &[]).unwrap();
        store.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_observation BEFORE UPDATE ON observations BEGIN SELECT RAISE(ABORT,'fixture write denied'); END;").unwrap();
        let baseline = counts(&store);
        assert!(store
            .admit_review_scan("acme", "alice", &[e.clone(), candidate("pr_review", 6)])
            .is_err());
        assert_eq!(counts(&store), baseline);
        assert_eq!(
            store.observation("review:acme:alice").unwrap().unwrap()["acme/repo#5"]["requested"],
            json!(false)
        );
        store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER deny_observation")
            .unwrap();
        assert_eq!(
            store
                .admit_review_scan("acme", "alice", &[e, candidate("pr_review", 6)])
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store.observation("review:acme:alice").unwrap().unwrap()["acme/repo#5"]["generation"],
            json!(2)
        );
    }
    #[test]
    fn review_scan_recovers_observation_for_existing_dedup() {
        let (_dir, store, s) = setup();
        let mut e = candidate("pr_review", 5);
        e.event_id = "review:acme/repo#5:alice:1".into();
        store.insert(&e, &s.flows[2], 1, true).unwrap().unwrap();
        e.event_id.clear();
        assert!(store
            .admit_review_scan("acme", "alice", &[e])
            .unwrap()
            .is_empty());
        assert_eq!(counts(&store), (1, 1, 1));
        assert_eq!(
            store.observation("review:acme:alice").unwrap().unwrap()["acme/repo#5"],
            json!({"requested":true,"generation":1})
        );
    }
}
