use super::store::Store;
use crate::context::factory::domain::entities::pull_request_review::PullRequestReviewState;
use crate::*;
use anyhow::{ensure, Context};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::HashSet;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ReviewObservation {
    requested: bool,
    generation: u64,
}
fn review_generation(
    global: Option<&ReviewObservation>,
    local: Option<&ReviewObservation>,
) -> Result<(u64, bool)> {
    let generation = global.map_or(0, |v| v.generation);
    ensure!(
        local.is_none_or(|v| v.generation <= generation),
        "Invalid review generation state"
    );
    let advance = generation == 0
        || global.is_some_and(|v| !v.requested)
        || local.is_some_and(|v| !v.requested && v.generation == generation);
    Ok((
        if advance {
            generation
                .checked_add(1)
                .context("Review generation limit reached")?
        } else {
            generation
        },
        advance,
    ))
}
fn origin_is_current(settings: &Settings, event: &TriggerEvent) -> bool {
    settings.source_for_event(event).is_ok()
}
impl Store {
    pub(crate) fn admit_review_scan(
        &self,
        source_id: &str,
        owner: &str,
        reviewer: &str,
        candidates: &[TriggerEvent],
    ) -> Result<Vec<String>> {
        self.with_admission(|tx, settings| {
            ensure!(safe_id(source_id) && valid_login(owner) && valid_login(reviewer), "Invalid review scan identity");
            let Some(source) = settings.source_by_id(source_id) else { return Ok(Vec::new()); };
            let template = settings.flows.iter().find(|f| f.id == "pr_review").context("Review flow missing")?;
            if !source.account.eq_ignore_ascii_case(reviewer) || !source.allows_owner(owner) || !source.allows_actor("pr_review")
                || template.activation_for_source(source_id, &settings.source_connectors) != ActivationState::Enabled { return Ok(Vec::new()); }
            for e in candidates {
                ensure!(e.trigger_id == "pr_review" && e.number > 0 && valid_repository(&e.repository)
                    && e.origin_token_profile_id.as_deref() == Some(source_id) && origin_is_current(settings, e)
                    && e.repository.split('/').next().is_some_and(|v| v.eq_ignore_ascii_case(owner)), "Invalid review candidate identity");
            }
            let flow = settings.flow_for_source(template, source_id)?;
            let owner = owner.to_lowercase(); let reviewer = reviewer.to_lowercase();
            let global_key = format!("review:{owner}:{reviewer}");
            let local_key = format!("review-source:{source_id}:{owner}:{reviewer}");
            let read = |key: &str| -> Result<std::collections::BTreeMap<String, ReviewObservation>> {
                let raw: Option<String> = tx.query_row("SELECT json FROM observations WHERE key=?1", [key], |r| r.get(0)).optional()?;
                Ok(raw.map(|raw| serde_json::from_str(&raw)).transpose()?.unwrap_or_default())
            };
            let mut global = read(&global_key)?; let mut local = read(&local_key)?;
            // Recover exact numeric generations even after run deletion/import or older observation loss.
            let mut q = tx.prepare("SELECT event_id FROM dedup WHERE substr(event_id,1,7)='review:'")?;
            for row in q.query_map([], |r| r.get::<_,String>(0))? {
                let id = row?;
                let Some((prefix, generation)) = id.rsplit_once(':') else { continue; };
                let Some((identity, actor)) = prefix.strip_prefix("review:").and_then(|v| v.rsplit_once(':')) else { continue; };
                let Some((repo, number)) = identity.rsplit_once('#') else { continue; };
                if actor != reviewer || repo.split('/').next() != Some(owner.as_str()) || !valid_repository(repo) || number.parse::<u64>().ok().is_none_or(|n| n==0) { continue; }
                let Ok(generation) = generation.parse::<u64>() else { continue; };
                let key = identity.to_lowercase();
                if generation > global.get(&key).map_or(0, |v| v.generation) { global.insert(key, ReviewObservation { requested:true, generation }); }
            }
            let mut present = HashSet::new(); let mut created = Vec::new();
            for candidate in candidates {
                // Polling may have awaited HTTP while this Source's author list was saved.
                // Recheck inside the settings transaction before consuming any review edge.
                if !source.allows_pull_request_author(candidate.payload["pull_request"]["user"]["login"].as_str()) {
                    continue;
                }
                let feedback = candidate.payload.get("review_feedback")
                    .map(|v| serde_json::from_value::<PullRequestReviewState>(v.clone())).transpose()?;
                if let Some(feedback) = &feedback {
                    let requested = candidate.payload["pull_request"]["requested_reviewers"].as_array().is_some_and(|reviewers| {
                        reviewers.iter().any(|r| r["login"].as_str().is_some_and(|login| login.eq_ignore_ascii_case(&reviewer)))
                    });
                    if !feedback.allows_review(&candidate.head_sha, source.review_all_open_pull_requests, requested) {
                        continue;
                    }
                }
                let identity = format!("{}#{}", candidate.repository.to_lowercase(), candidate.number);
                if !present.insert(identity.clone()) { continue; }
                let (generation, advance) = review_generation(global.get(&identity), local.get(&identity))?;
                let updated_review = feedback.as_ref().is_some_and(|f| f.needs_updated_review(&candidate.head_sha));
                if advance || updated_review {
                    let mut event = candidate.clone();
                    if updated_review {
                        ensure!(candidate.head_sha.len() == 40 && candidate.head_sha.bytes().all(|c| c.is_ascii_hexdigit()), "Invalid review head");
                        event.event_id = format!("review-head:{identity}:{reviewer}:{}", candidate.head_sha.to_ascii_lowercase());
                    } else {
                        event.event_id = format!("review:{identity}:{reviewer}:{generation}");
                    }
                    // Keep automatic head consumption independent of disposable
                    // attempts, just like consumed approval-comment identities.
                    let fresh_head = !updated_review || tx.execute(
                        "INSERT OR IGNORE INTO observations(key,json) VALUES(?1,'true')", [&event.event_id]
                    )? == 1;
                    // Old numeric-generation attempts may already own this head.
                    // Keep them authoritative when upgrading and across Sources.
                    let existing_head = updated_review && fresh_head && tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM runs WHERE json_extract(json,'$.trigger_id')='pr_review' AND lower(json_extract(json,'$.repository'))=?1 AND json_extract(json,'$.number')=?2 AND lower(json_extract(json,'$.actor'))=?3 AND lower(json_extract(event,'$.head_sha'))=?4)",
                        params![candidate.repository.to_lowercase(), i64::try_from(candidate.number)?, reviewer, candidate.head_sha.to_ascii_lowercase()], |row| row.get::<_, bool>(0)
                    )?;
                    if fresh_head && !existing_head {
                        if let Some(id) = Self::insert_in_tx(tx, &event, &flow, 1, true, None)? { created.push(id); }
                    }
                }
                global.insert(identity.clone(), ReviewObservation { requested:true, generation });
                local.insert(identity, ReviewObservation { requested:true, generation });
            }
            let identities: HashSet<_> = global.keys().chain(local.keys()).cloned().collect();
            for identity in identities {
                if !present.contains(&identity) {
                    local.insert(identity.clone(), ReviewObservation { requested:false, generation:global.get(&identity).map_or(0, |v| v.generation) });
                }
            }
            for (key, map) in [(global_key, global), (local_key, local)] {
                tx.execute("INSERT INTO observations VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json", params![key, serde_json::to_string(&map)?])?;
            }
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
            if flow.activation_for_source(
                event.origin_token_profile_id.as_deref().unwrap_or(""),
                &settings.source_connectors,
            ) != ActivationState::Enabled
            {
                return Ok(None);
            }
            let flow = settings.flow_for_source(
                flow,
                event
                    .origin_token_profile_id
                    .as_deref()
                    .context("Source connector unavailable")?,
            )?;
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
            if flow.activation_for_source(
                event.origin_token_profile_id.as_deref().unwrap_or(""),
                &settings.source_connectors,
            ) != ActivationState::Enabled
            {
                return Ok(None);
            }
            let flow = settings.flow_for_source(
                flow,
                event
                    .origin_token_profile_id
                    .as_deref()
                    .context("Source connector unavailable")?,
            )?;
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
            settings.source_for_event(&event)?;
            let flow = settings.flows.iter().find(|f| f.id == event.trigger_id).context("Flow missing")?;
            let reason = match flow.activation_for_source(event.origin_token_profile_id.as_deref().unwrap_or(""), &settings.source_connectors) {
                ActivationState::Enabled => None,
                ActivationState::DisabledGlobally => Some("disabled globally"),
                ActivationState::DisabledForProfile => Some("disabled for this source"),
                ActivationState::UnavailableSource => Some("source connector unavailable"),
            };
            if let Some(reason) = reason {
                anyhow::bail!("Workflow {} for @{} is {}", flow.id, event.actor, reason);
            }
            let attempt = run
                .attempt
                .checked_add(1)
                .context("Run attempt limit reached")?;
            let flow = settings.flow_for_source(flow, event.origin_token_profile_id.as_deref().context("Source connector unavailable")?)?;
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
        s.source_connectors.push(SourceConnector {
            review_all_open_pull_requests: false,
            pull_request_authors: vec![],
            kind: SourceKind::GithubIssues,
            id: "work".into(),
            label: "Work".into(),
            account: "alice".into(),
            owners: vec!["acme".into()],
            authors: vec!["alice".into()],
            reviewers: vec!["alice".into()],
            token: Some("fixture".into()),
            has_token: false,
        });
        store.save_settings(&s).unwrap();
        (dir, store, s)
    }
    fn candidate(trigger: &str, n: u64) -> TriggerEvent {
        let mut e = TriggerEvent::issue("acme/repo", n, "fixture", "alice", "");
        e.origin_token_profile_id = Some("work".into());
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
            .admit_review_scan("work", "acme", "alice", std::slice::from_ref(&e))
            .unwrap()
            .is_empty());
        assert_eq!(counts(&store), (0, 0, 0));
        s.flows[2].profile_activation.clear();
        store.save_settings(&s).unwrap();
        assert_eq!(
            store
                .admit_review_scan("work", "acme", "alice", &[e.clone(), e.clone()])
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .admit_review_scan("work", "acme", "alice", std::slice::from_ref(&e))
            .unwrap()
            .is_empty());
        let before = store.observation("review:acme:alice").unwrap();
        assert_eq!(
            before.as_ref().unwrap()["acme/repo#5"]["generation"],
            json!(1)
        );
        s.flows[2].enabled = false;
        store.save_settings(&s).unwrap();
        store
            .admit_review_scan("work", "acme", "alice", &[])
            .unwrap();
        assert_eq!(store.observation("review:acme:alice").unwrap(), before);
        s.flows[2].enabled = true;
        store.save_settings(&s).unwrap();
        store
            .admit_review_scan("work", "acme", "alice", &[])
            .unwrap();
        store.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_observation BEFORE UPDATE ON observations BEGIN SELECT RAISE(ABORT,'fixture write denied'); END;").unwrap();
        let baseline = counts(&store);
        assert!(store
            .admit_review_scan(
                "work",
                "acme",
                "alice",
                &[e.clone(), candidate("pr_review", 6)]
            )
            .is_err());
        assert_eq!(counts(&store), baseline);
        assert_eq!(
            store
                .observation("review-source:work:acme:alice")
                .unwrap()
                .unwrap()["acme/repo#5"]["requested"],
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
                .admit_review_scan("work", "acme", "alice", &[e, candidate("pr_review", 6)])
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
            .admit_review_scan("work", "acme", "alice", &[e])
            .unwrap()
            .is_empty());
        assert_eq!(counts(&store), (1, 1, 2));
        assert_eq!(
            store.observation("review:acme:alice").unwrap().unwrap()["acme/repo#5"],
            json!({"requested":true,"generation":1})
        );
    }
    fn source_pair() -> (tempfile::TempDir, Store, Settings) {
        let (dir, store, mut settings) = setup();
        let mut a = settings.source_connectors[0].clone();
        a.id = "src-a".into();
        a.account = "alice".into();
        a.owners = vec!["acme".into()];
        a.authors.clear();
        a.reviewers = vec!["alice".into()];
        a.token = Some("fixture-a".into());
        let mut b = a.clone();
        b.id = "src-b".into();
        b.token = Some("fixture-b".into());
        settings.source_connectors = vec![a, b];
        settings
            .flows
            .iter_mut()
            .find(|f| f.id == "pr_review")
            .unwrap()
            .enabled = true;
        store.save_settings(&settings).unwrap();
        (dir, store, settings)
    }
    fn source_review_candidate(source_id: &str) -> TriggerEvent {
        let mut e = candidate("pr_review", 5);
        e.origin_token_profile_id = Some(source_id.into());
        e
    }

    #[test]
    fn review_sources_keep_global_generation_after_disable_and_removal() {
        for catch_up_before_removal in [false, true] {
            let (dir, store, mut settings) = source_pair();
            let a = source_review_candidate("src-a");
            let b = source_review_candidate("src-b");
            assert_eq!(
                store
                    .admit_review_scan("src-a", "acme", "alice", std::slice::from_ref(&a))
                    .unwrap()
                    .len(),
                1
            );
            assert!(store
                .admit_review_scan("src-b", "acme", "alice", std::slice::from_ref(&b))
                .unwrap()
                .is_empty());
            let b_before = store.observation("review-source:src-b:acme:alice").unwrap();
            settings
                .flows
                .iter_mut()
                .find(|f| f.id == "pr_review")
                .unwrap()
                .profile_activation
                .insert("src-b".into(), false);
            store.save_settings(&settings).unwrap();
            assert!(store
                .admit_review_scan("src-b", "acme", "alice", &[])
                .unwrap()
                .is_empty());
            assert!(store
                .admit_review_scan("src-b", "acme", "alice", std::slice::from_ref(&b))
                .unwrap()
                .is_empty());
            assert_eq!(
                store.observation("review-source:src-b:acme:alice").unwrap(),
                b_before
            );
            store
                .admit_review_scan("src-a", "acme", "alice", &[])
                .unwrap();
            assert_eq!(
                store
                    .admit_review_scan("src-a", "acme", "alice", &[a])
                    .unwrap()
                    .len(),
                1
            );
            settings
                .flows
                .iter_mut()
                .find(|f| f.id == "pr_review")
                .unwrap()
                .profile_activation
                .remove("src-b");
            if catch_up_before_removal {
                store.save_settings(&settings).unwrap();
                assert!(store
                    .admit_review_scan("src-b", "acme", "alice", std::slice::from_ref(&b))
                    .unwrap()
                    .is_empty());
            }
            settings
                .source_connectors
                .retain(|source| source.id != "src-a");
            store.save_settings(&settings).unwrap();
            let a_before = store.observation("review-source:src-a:acme:alice").unwrap();
            store
                .admit_review_scan("src-b", "acme", "alice", &[])
                .unwrap();
            let ids = store
                .admit_review_scan("src-b", "acme", "alice", std::slice::from_ref(&b))
                .unwrap();
            assert_eq!(ids.len(), 1, "new request must not reuse generation 2");
            let detail = store.detail(&ids[0]).unwrap();
            assert_eq!(detail.event.event_id, "review:acme/repo#5:alice:3");
            assert_eq!(
                detail.event.origin_token_profile_id.as_deref(),
                Some("src-b")
            );
            assert!(store
                .admit_review_scan("src-b", "acme", "alice", std::slice::from_ref(&b))
                .unwrap()
                .is_empty());
            assert_eq!(
                store.observation("review-source:src-a:acme:alice").unwrap(),
                a_before
            );
            let mut events: Vec<_> = store
                .runs()
                .unwrap()
                .into_iter()
                .map(|r| r.event_id)
                .collect();
            events.sort();
            assert_eq!(
                events,
                (1..=3)
                    .map(|g| format!("review:acme/repo#5:alice:{g}"))
                    .collect::<Vec<_>>()
            );
            drop(store);
            let reopened =
                Store::open_with_credentials(dir.path(), Arc::new(Memory::default())).unwrap();
            assert!(reopened
                .admit_review_scan("src-b", "acme", "alice", &[b])
                .unwrap()
                .is_empty());
            assert_eq!(reopened.runs().unwrap().len(), 3);
        }
    }

    #[test]
    fn review_generation_joins_current_and_advances_only_from_current_absence() {
        let state = |requested, generation| ReviewObservation {
            requested,
            generation,
        };
        let cases = [
            (None, None, (1, true)),
            (Some(state(true, 2)), None, (2, false)),
            (Some(state(true, 2)), Some(state(true, 1)), (2, false)),
            (Some(state(true, 2)), Some(state(false, 1)), (2, false)),
            (Some(state(true, 2)), Some(state(false, 2)), (3, true)),
            (Some(state(true, 2)), Some(state(true, 2)), (2, false)),
            (Some(state(false, 2)), None, (3, true)),
        ];
        for (global, local, expected) in cases {
            assert_eq!(
                review_generation(global.as_ref(), local.as_ref()).unwrap(),
                expected
            );
        }
        assert!(review_generation(Some(&state(true, 2)), Some(&state(false, 3))).is_err());
        assert!(
            review_generation(Some(&state(true, u64::MAX)), Some(&state(false, u64::MAX))).is_err()
        );
    }

    #[test]
    fn review_sources_staggered_absence_joins_existing_cycle() {
        for (leader, follower) in [("src-a", "src-b"), ("src-b", "src-a")] {
            let (_dir, store, _) = source_pair();
            let first = source_review_candidate(leader);
            let second = source_review_candidate(follower);
            assert_eq!(
                store
                    .admit_review_scan(leader, "acme", "alice", std::slice::from_ref(&first))
                    .unwrap()
                    .len(),
                1
            );
            assert!(store
                .admit_review_scan(follower, "acme", "alice", std::slice::from_ref(&second))
                .unwrap()
                .is_empty());
            store
                .admit_review_scan(follower, "acme", "alice", &[])
                .unwrap();
            store
                .admit_review_scan(leader, "acme", "alice", &[])
                .unwrap();
            assert_eq!(
                store
                    .admit_review_scan(leader, "acme", "alice", &[first])
                    .unwrap()
                    .len(),
                1
            );
            assert!(store
                .admit_review_scan(follower, "acme", "alice", &[second])
                .unwrap()
                .is_empty());
            let mut ids: Vec<_> = store
                .runs()
                .unwrap()
                .into_iter()
                .map(|r| r.event_id)
                .collect();
            ids.sort();
            assert_eq!(
                ids,
                vec!["review:acme/repo#5:alice:1", "review:acme/repo#5:alice:2"]
            );
            let detail = store.detail(&store.runs().unwrap()[0].id).unwrap();
            assert_eq!(
                detail.event.origin_token_profile_id.as_deref(),
                Some(leader)
            );
        }
    }
    #[test]
    fn review_history_numeric_bootstrap_and_deletion_never_replay_the_current_cycle() {
        for historical_false in [false, true] {
            let (_dir, store, settings) = source_pair();
            let mut e = source_review_candidate("src-a");
            for generation in [9, 10] {
                e.event_id = format!("review:acme/repo#5:alice:{generation}");
                store.insert(&e, &settings.flows[2], 1, true).unwrap();
            }
            if historical_false {
                store
                    .observe(
                        "review:acme:alice",
                        &json!({"acme/repo#5":{"requested":false,"generation":10}}),
                    )
                    .unwrap();
            }
            e.event_id.clear();
            let ids = store
                .admit_review_scan("src-a", "acme", "alice", std::slice::from_ref(&e))
                .unwrap();
            assert_eq!(ids.len(), usize::from(historical_false));
            let generation = if historical_false { 11 } else { 10 };
            assert_eq!(
                store.observation("review:acme:alice").unwrap().unwrap()["acme/repo#5"]
                    ["generation"],
                json!(generation)
            );
            store
                .db
                .lock()
                .unwrap()
                .execute_batch("DELETE FROM nodes;DELETE FROM runs;DELETE FROM dedup;")
                .unwrap();
            let b = source_review_candidate("src-b");
            assert!(store
                .admit_review_scan("src-b", "acme", "alice", std::slice::from_ref(&b))
                .unwrap()
                .is_empty());
            store
                .admit_review_scan("src-b", "acme", "alice", &[])
                .unwrap();
            let ids = store
                .admit_review_scan("src-b", "acme", "alice", &[b])
                .unwrap();
            assert_eq!(ids.len(), 1);
            assert_eq!(
                store.detail(&ids[0]).unwrap().event.event_id,
                format!("review:acme/repo#5:alice:{}", generation + 1)
            );
        }
    }
    #[test]
    fn review_local_write_failure_and_overflow_rollback_every_admission_effect() {
        let (_dir, store, _) = source_pair();
        let e = source_review_candidate("src-a");
        store
            .admit_review_scan("src-a", "acme", "alice", std::slice::from_ref(&e))
            .unwrap();
        store
            .admit_review_scan("src-a", "acme", "alice", &[])
            .unwrap();
        let global = store.observation("review:acme:alice").unwrap();
        let local = store.observation("review-source:src-a:acme:alice").unwrap();
        let before = counts(&store);
        store.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_local BEFORE UPDATE ON observations WHEN NEW.key='review-source:src-a:acme:alice' BEGIN SELECT RAISE(ABORT,'local denied'); END;").unwrap();
        assert!(store
            .admit_review_scan("src-a", "acme", "alice", std::slice::from_ref(&e))
            .is_err());
        assert_eq!(counts(&store), before);
        assert_eq!(store.observation("review:acme:alice").unwrap(), global);
        assert_eq!(
            store.observation("review-source:src-a:acme:alice").unwrap(),
            local
        );
        store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_local")
            .unwrap();
        store
            .observe(
                "review:acme:alice",
                &json!({"acme/repo#5":{"requested":false,"generation":u64::MAX}}),
            )
            .unwrap();
        assert!(store
            .admit_review_scan("src-a", "acme", "alice", &[e])
            .is_err());
        assert_eq!(counts(&store), before);
        assert_eq!(
            store.observation("review:acme:alice").unwrap().unwrap()["acme/repo#5"]["generation"],
            json!(u64::MAX)
        );
    }
}
