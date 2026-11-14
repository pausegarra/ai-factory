use crate::context::factory::application::dto::connector_notification::ConnectorNotificationJob;
use crate::context::factory::application::interfaces::run_cleanup::RunCleanup;
use crate::context::factory::domain::entities::run_transition::RunTransition;
use crate::context::factory::infrastructure::filesystem::run_cleanup::LocalRunCleanup;
use crate::context::factory::infrastructure::{
    processes::agents, repositories::store::Store, rest_clients::github::Github,
};
use crate::*;
use anyhow::{ensure, Context};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{broadcast, Notify};
use tokio_util::sync::CancellationToken;

pub(crate) struct ActiveWorker {
    event_id: String,
    trigger_id: String,
    cancel: CancellationToken,
}

#[derive(Default)]
struct QueuedWorkerScan {
    approval_deferred: bool,
}

impl QueuedWorkerScan {
    fn reserve(
        &mut self,
        factory: &Factory,
        id: &str,
        trigger_id: &str,
    ) -> Result<Option<CancellationToken>> {
        if trigger_id == "pr_approved" && self.approval_deferred {
            return Ok(None);
        }

        let reservation = factory.register_queued_worker(id);
        if trigger_id == "pr_approved" && !matches!(reservation.as_ref(), Ok(Some(_))) {
            // Preserve FIFO among approvals for the rest of this queue snapshot.
            // Other triggers must still be considered for any free global slots.
            self.approval_deferred = true;
        }
        reservation
    }
}

pub struct Factory {
    pub(crate) usage: super::agent_usage::UsageManager,
    pub(crate) accounts: super::agent_accounts::AccountManager,
    _process_lock: std::fs::File,
    pub(crate) root: PathBuf,
    pub(crate) github_base: String,
    pub(crate) store: Store,
    pub(crate) events: broadcast::Sender<FactoryEvent>,
    pub(crate) wake: Notify,
    pub(crate) stop: CancellationToken,
    pub(crate) started: AtomicBool,
    pub(crate) active: Mutex<HashMap<String, ActiveWorker>>,
    pub(crate) service: Mutex<ServiceStatus>,
    poll_lock: tokio::sync::Mutex<()>,
    pub(crate) lifecycle_gate: Mutex<()>,
    pub(crate) deletion_locks: Mutex<HashMap<String, std::sync::Weak<Mutex<()>>>>,
    pub(crate) cleanup: Arc<dyn RunCleanup>,
    connector_dispatcher: Mutex<Option<super::connector_notifications::ConnectorDispatcher>>,
    issue_node_comment_dispatcher:
        Mutex<Option<super::issue_node_comments::IssueNodeCommentDispatcher>>,
    notification_transport: Arc<dyn NotificationTransport>,
    run_notification_sink: Mutex<Arc<dyn RunNotificationSink>>,
}

#[cfg(test)]
mod worker_tests;

impl Drop for Factory {
    fn drop(&mut self) {
        self.stop.cancel();
        // Explicit unlock also releases descriptors briefly inherited by a fork
        // in another worker before that worker's exec closes them.
        let _ = fs2::FileExt::unlock(&self._process_lock);
    }
}
impl Factory {
    pub fn open(root: PathBuf) -> Result<Arc<Self>> {
        Self::open_with_api(root, "https://api.github.com")
    }
    pub fn open_with_api(root: PathBuf, base: &str) -> Result<Arc<Self>> {
        Self::open_with_credentials(root, base, Arc::new(crate::context::factory::infrastructure::security::native_credentials::NativeCredentials))
    }
    pub fn open_with_credentials(
        root: PathBuf,
        base: &str,
        credentials: Arc<dyn Credentials>,
    ) -> Result<Arc<Self>> {
        let transport = crate::context::factory::infrastructure::rest_clients::telegram::TelegramTransport::new()
            .map_err(|_| anyhow::anyhow!("Cannot initialize notification transport"))?;
        Self::open_with_notification_transport(root, base, credentials, Arc::new(transport))
    }
    pub fn open_with_notification_transport(
        root: PathBuf,
        base: &str,
        credentials: Arc<dyn Credentials>,
        transport: Arc<dyn NotificationTransport>,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        for d in ["workspaces", "memory", "sessions"] {
            std::fs::create_dir_all(root.join(d))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        }
        let process_lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("factory.lock"))?;
        fs2::FileExt::try_lock_exclusive(&process_lock)
            .context("AI Factory is already using this application data directory")?;
        let store = Store::open_with_credentials(&root, credentials)?;
        let cleanup = Arc::new(LocalRunCleanup::open(&root)?);
        for record in store.pending_deletions()? {
            crate::context::factory::application::use_cases::delete_run::resume(
                &store,
                cleanup.as_ref(),
                &record.public.run_id,
            )?;
        }
        let cleanup_errors =
            crate::context::factory::infrastructure::processes::workspaces::recover(
                &root,
                &store,
                cleanup.as_ref(),
            )?;
        let (events, _) = broadcast::channel(4096);
        let status = ServiceStatus {
            running: false,
            polling: false,
            last_poll_at: None,
            last_error: (!cleanup_errors.is_empty()).then(|| cleanup_errors.join("; ")),
            app_data_dir: root.to_string_lossy().into(),
        };
        Ok(Arc::new(Self {
            _process_lock: process_lock,
            accounts: super::agent_accounts::AccountManager::default(),
            usage: super::agent_usage::UsageManager::new(Arc::new(
                crate::context::factory::infrastructure::processes::agent_usage::CliUsageSource,
            )),
            root,
            github_base: base.into(),
            store,
            events,
            wake: Notify::new(),
            stop: CancellationToken::new(),
            started: AtomicBool::new(false),
            active: Mutex::new(HashMap::new()),
            service: Mutex::new(status),
            poll_lock: tokio::sync::Mutex::new(()),
            lifecycle_gate: Mutex::new(()),
            deletion_locks: Mutex::new(HashMap::new()),
            cleanup,
            connector_dispatcher: Mutex::new(None),
            issue_node_comment_dispatcher: Mutex::new(None),
            notification_transport: transport,
            run_notification_sink: Mutex::new(Arc::new(NoopRunNotificationSink)),
        }))
    }
    pub fn start_notification_delivery(self: &Arc<Self>) {
        let _gate = self.lifecycle_gate.lock().unwrap();
        if self.stop.is_cancelled() {
            return;
        }
        let mut issue_dispatcher = self.issue_node_comment_dispatcher.lock().unwrap();
        if issue_dispatcher.is_none() {
            *issue_dispatcher = Some(
                super::issue_node_comments::IssueNodeCommentDispatcher::start(
                    Arc::downgrade(self),
                    self.stop.child_token(),
                ),
            );
        }
        drop(issue_dispatcher);
        let mut dispatcher = self.connector_dispatcher.lock().unwrap();
        if dispatcher.is_none() {
            *dispatcher = Some(super::connector_notifications::ConnectorDispatcher::start(
                Arc::downgrade(self),
                self.notification_transport.clone(),
                self.stop.child_token(),
            ));
        }
    }
    /// Install an output port for future transitions; existing history is never replayed.
    pub fn set_run_notification_sink(&self, sink: Arc<dyn RunNotificationSink>) {
        *self.run_notification_sink.lock().unwrap() = sink;
    }
    fn publish_run_transition(&self, transition: Option<RunTransition>) {
        let Some(transition) = transition else { return };
        let Some(notification) = super::run_notifications::notification_for(&transition, |text| {
            self.store.redact_cached(text)
        }) else {
            return;
        };
        let sink = self.run_notification_sink.lock().unwrap().clone();
        sink.enqueue(notification);
        if let Some(dispatcher) = self.connector_dispatcher.lock().unwrap().as_ref() {
            if let Some(event) = NotificationEvent::from_run_status(&transition.run.status) {
                match self.store.notification_targets(event) {
                    Ok(targets) => {
                        for target in targets {
                            dispatcher.enqueue(ConnectorNotificationJob {
                                target,
                                transition: transition.clone(),
                            });
                        }
                    }
                    Err(_) => eprintln!("notification_delivery_failed connector=unavailable reason=invalid_response"),
                }
            }
        }
    }
    pub(crate) fn claim_run_for_execution(&self, id: &str) -> Result<bool> {
        let _gate = self.lifecycle_gate.lock().unwrap();
        let transition = self.store.claim_queued_transition(id)?;
        let claimed = transition.is_some();
        self.publish_run_transition(transition);
        Ok(claimed)
    }
    pub fn settings(&self) -> Result<Settings> {
        self.store.public_settings()
    }
    pub async fn agent_models(&self, agent: &str) -> Result<Vec<AgentModel>> {
        self.agent_models_for_profile(agent, None).await
    }
    pub async fn agent_models_for_profile(
        &self,
        agent: &str,
        token_profile_id: Option<&str>,
    ) -> Result<Vec<AgentModel>> {
        let settings = self.store.public_settings()?;
        let executable = match agent {
            "codex" => &settings.commands.codex,
            "opencode" => &settings.commands.opencode,
            _ => anyhow::bail!("Unsupported agent"),
        };
        let lease = if agent == "codex" {
            let account = match token_profile_id {
                Some(profile) => settings.codex_bindings.get(profile),
                None => settings
                    .token_profiles
                    .iter()
                    .find_map(|profile| settings.codex_bindings.get(&profile.id)),
            }
            .context("Assign a Codex account in Settings → Agents to load models")?;
            Some(self.lease_codex_account(account)?)
        } else {
            None
        };
        tokio::select! {
            _ = self.stop.cancelled() => anyhow::bail!("Factory is stopping"),
            result = crate::context::factory::infrastructure::processes::agent_models::discover_with_home(agent, executable, &self.root, lease.as_ref().map(|lease| lease.home.as_path())) => result,
        }
    }
    pub fn save_settings(&self, mut s: Settings) -> Result<Settings> {
        let _gate = self.lifecycle_gate.lock().unwrap();
        for profile in &mut s.token_profiles {
            profile.account = profile.account.to_lowercase();
            for values in [
                &mut profile.owners,
                &mut profile.authors,
                &mut profile.reviewers,
            ] {
                let mut seen = std::collections::HashSet::new();
                for value in values.iter_mut() {
                    *value = value.to_lowercase();
                }
                values.retain(|value| seen.insert(value.clone()));
            }
        }
        for p in &mut s.projects {
            p.repository = p.repository.to_lowercase();
        }
        s.validate()?;
        self.validate_codex_bindings(&s)?;
        self.store.save_settings(&s)?;
        if let Some(dispatcher) = self.connector_dispatcher.lock().unwrap().as_ref() {
            dispatcher.reconcile(&s.notification_connectors);
        }
        self.emit("settings_changed", None, None);
        self.wake.notify_one();
        self.settings()
    }
    pub fn import_legacy(
        &self,
        config_path: Option<PathBuf>,
        database_path: Option<PathBuf>,
    ) -> Result<ImportReport> {
        let _gate = self.lifecycle_gate.lock().unwrap();
        ensure!(
            self.active.lock().unwrap().is_empty(),
            "Stop active runs before importing history"
        );
        let excluded = self
            .store
            .pending_deletions()?
            .into_iter()
            .map(|r| r.public.run_id)
            .collect();
        let batch =
            crate::context::factory::infrastructure::migration::legacy::read_legacy_excluding(
                config_path.as_deref(),
                database_path.as_deref(),
                &self.root,
                &self.store.settings()?,
                &excluded,
            )?;
        let count = self.store.import_legacy(&batch)?;
        self.emit("history_imported", None, None);
        self.wake.notify_one();
        Ok(ImportReport {
            runs_imported: count,
            settings_imported: batch.settings.is_some(),
            archive_path: batch.archive_path,
            warnings: batch.warnings,
        })
    }
    pub fn subscribe(&self) -> broadcast::Receiver<FactoryEvent> {
        self.events.subscribe()
    }
    pub(crate) fn emit(&self, kind: &str, run: Option<&str>, node: Option<&str>) {
        let _ = self.events.send(FactoryEvent {
            kind: kind.into(),
            run_id: run.map(String::from),
            node_id: node.map(String::from),
            timestamp: now(),
        });
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        let _gate = self.lifecycle_gate.lock().unwrap();
        let (runs, deletions) = self.store.snapshot_history()?;
        let settings = self.store.public_settings()?;
        let capacity = {
            let active = self.active.lock().unwrap();
            let by_trigger = ["new_issue", "pr_review", "pr_approved"]
                .into_iter()
                .map(|id| {
                    Ok((
                        id.to_string(),
                        CapacityUsage {
                            active: active
                                .values()
                                .filter(|worker| worker.trigger_id == id)
                                .count(),
                            limit: settings.concurrency_for(id)?,
                        },
                    ))
                })
                .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
            ExecutionCapacity {
                global: CapacityUsage {
                    active: active.len(),
                    limit: settings.max_concurrent,
                },
                by_trigger,
            }
        };
        let mut metrics = Metrics {
            total: runs.len() as u64,
            ..Metrics::default()
        };
        for r in &runs {
            match r.status.as_str() {
                "queued" => metrics.queued += 1,
                "running" => metrics.running += 1,
                "succeeded" => metrics.succeeded += 1,
                "cancelled" => metrics.cancelled += 1,
                "interrupted" => metrics.interrupted += 1,
                _ => metrics.failed += 1,
            }
        }
        Ok(Snapshot {
            runs,
            deletions,
            metrics,
            capacity,
            service: self.service.lock().unwrap().clone(),
        })
    }
    pub fn run_detail(&self, id: &str) -> Result<RunDetail> {
        self.store.detail(id)
    }
    pub fn enqueue(&self, e: TriggerEvent) -> Result<Option<String>> {
        ensure!(
            valid_repository(&e.repository) && e.number > 0 && valid_login(&e.actor),
            "Invalid event identity"
        );
        let id = self.store.admit_event(&e)?;
        if let Some(id) = &id {
            self.emit("run_created", Some(id), None);
            self.wake.notify_one();
        }
        Ok(id)
    }
    pub(crate) fn enqueue_approval(
        &self,
        e: TriggerEvent,
        comments: &[u64],
        approved: bool,
    ) -> Result<Option<String>> {
        ensure!(
            valid_repository(&e.repository) && e.number > 0 && valid_login(&e.actor),
            "Invalid event identity"
        );
        let id = self.store.admit_approval(&e, comments, approved)?;
        if let Some(id) = &id {
            self.emit("run_created", Some(id), None);
            self.wake.notify_one();
        }
        Ok(id)
    }
    pub(crate) fn github(&self, token: &str) -> Result<Github> {
        Github::with_base(token, &self.github_base)
    }
    pub(crate) fn workflow_enabled_for(&self, trigger: &str, actor: &str) -> Result<bool> {
        let settings = self.store.public_settings()?;
        let flow = settings
            .flows
            .iter()
            .find(|f| f.id == trigger)
            .context("Unknown trigger")?;
        Ok(flow.activation_for(actor, &settings.token_profiles) == ActivationState::Enabled)
    }
    pub(crate) fn enqueue_review_scan(
        &self,
        owner: &str,
        reviewer: &str,
        candidates: &[TriggerEvent],
    ) -> Result<Vec<String>> {
        ensure!(
            valid_login(owner) && valid_login(reviewer),
            "Invalid review scan identity"
        );
        for e in candidates {
            ensure!(
                e.trigger_id == "pr_review"
                    && valid_repository(&e.repository)
                    && e.number > 0
                    && e.actor.eq_ignore_ascii_case(reviewer)
                    && e.repository
                        .split('/')
                        .next()
                        .is_some_and(|v| v.eq_ignore_ascii_case(owner)),
                "Invalid review candidate identity"
            );
        }
        let ids = self.store.admit_review_scan(
            &owner.to_lowercase(),
            &reviewer.to_lowercase(),
            candidates,
        )?;
        for id in &ids {
            self.emit("run_created", Some(id), None);
        }
        if !ids.is_empty() {
            self.wake.notify_one();
        }
        Ok(ids)
    }
    pub fn set_run_status(&self, id: &str, status: &str, error: Option<String>) -> Result<()> {
        let _gate = self.lifecycle_gate.lock().unwrap();
        self.set_run_status_inner(id, status, error)
    }
    fn set_run_status_inner(&self, id: &str, status: &str, error: Option<String>) -> Result<()> {
        let mut r = self.store.detail(id)?.run;
        r.status = status.into();
        r.error = error;
        if status == "running" {
            r.started_at = Some(now());
        } else if status != "queued" {
            r.finished_at = Some(now());
            r.current_step = None;
        }
        let transition = self.store.save_run_transition(&r)?;
        self.publish_run_transition(transition);
        if !["queued", "running"].contains(&status) {
            for mut n in self.store.nodes(id)? {
                if ["pending", "running"].contains(&n.status.as_str()) {
                    n.status = if ["cancelled", "interrupted"].contains(&status)
                        && n.status == "running"
                    {
                        status.into()
                    } else if status == "cancelled" {
                        "cancelled".into()
                    } else {
                        "skipped".into()
                    };
                    n.finished_at = Some(now());
                    self.store.save_node(id, &n)?;
                }
            }
        }
        self.emit("run_changed", Some(id), None);
        Ok(())
    }
    pub fn cancel(&self, id: &str) -> Result<()> {
        let _gate = self.lifecycle_gate.lock().unwrap();
        let d = self.store.detail(id)?;
        ensure!(
            ["queued", "running"].contains(&d.run.status.as_str()),
            "Run is already terminal"
        );
        if let Some(worker) = self.active.lock().unwrap().get(id) {
            worker.cancel.cancel();
        }
        self.set_run_status_inner(id, "cancelled", None)?;
        for mut n in d.nodes {
            if ["pending", "running"].contains(&n.status.as_str()) {
                n.status = "cancelled".into();
                n.finished_at = Some(now());
                self.store.save_node(id, &n)?;
            }
        }
        self.emit("run_changed", Some(id), None);
        Ok(())
    }
    pub fn retry(&self, id: &str) -> Result<String> {
        let _gate = self.lifecycle_gate.lock().unwrap();
        // Pending deletions are hidden by detail; keep that check under the gate.
        self.store.detail(id)?;
        let id = self.store.admit_retry(id)?;
        self.emit("run_created", Some(&id), None);
        self.wake.notify_one();
        Ok(id)
    }
    pub fn logs(&self, run: &str, node: &str, offset: u64) -> Result<LogChunk> {
        self.store.load_redaction_secrets()?;
        use std::io::{Read, Seek, SeekFrom};
        let d = self.store.detail(run)?;
        let n = d
            .nodes
            .iter()
            .find(|n| n.id == node)
            .context("Node not found")?;
        let path = self
            .root
            .join("sessions")
            .join(run)
            .join(format!("{node}.log"));
        let (text, offset, next, total) = if path.exists() {
            let mut file = std::fs::File::open(path)?;
            let total = file.metadata()?.len();
            let offset = offset.min(total);
            let overlap = self.store.redaction_overlap() as u64;
            let start = offset.saturating_sub(overlap);
            file.seek(SeekFrom::Start(start))?;
            let mut bytes = vec![];
            file.take(256 * 1024 + 2 * overlap)
                .read_to_end(&mut bytes)?;
            let main_start = (offset - start) as usize;
            let main_end = (main_start + 256 * 1024).min(bytes.len());
            let main = &bytes[main_start..main_end];
            let usable = match std::str::from_utf8(main) {
                Ok(_) => main.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(_) => main.len(),
            };
            let next = offset + usable as u64;
            (
                self.store
                    .redact_window(&bytes, main_start..main_start + usable),
                offset,
                next,
                total,
            )
        } else {
            (String::new(), 0, 0, 0)
        };
        self.store.detail(run)?;
        Ok(LogChunk {
            text,
            offset,
            next_offset: next,
            eof: next == total && !["running", "pending"].contains(&n.status.as_str()),
        })
    }
    pub(crate) fn register_queued_worker(&self, id: &str) -> Result<Option<CancellationToken>> {
        let Ok(_gate) = self.lifecycle_gate.try_lock() else {
            return Ok(None);
        };
        if self.stop.is_cancelled() {
            return Ok(None);
        }
        let Ok(detail) = self.store.detail(id) else {
            return Ok(None);
        };
        if detail.run.status != "queued" {
            return Ok(None);
        }
        let settings = self.store.public_settings()?;
        let trigger_limit = settings.concurrency_for(&detail.run.trigger_id)?;
        let mut active = self.active.lock().unwrap();
        let used = active
            .values()
            .filter(|worker| worker.trigger_id == detail.run.trigger_id)
            .count();
        // A cancelled/terminal worker keeps its event reserved until process and
        // workspace cleanup finish, even when global capacity has spare slots.
        if active.contains_key(id)
            || active
                .values()
                .any(|worker| worker.event_id == detail.run.event_id)
            || active.len() >= settings.max_concurrent
            || used >= trigger_limit
        {
            return Ok(None);
        }
        let cancel = CancellationToken::new();
        active.insert(
            id.into(),
            ActiveWorker {
                event_id: detail.run.event_id,
                trigger_id: detail.run.trigger_id,
                cancel: cancel.clone(),
            },
        );
        drop(active);
        self.emit("capacity_changed", Some(id), None);
        Ok(Some(cancel))
    }
    pub fn start(self: &Arc<Self>) {
        self.start_notification_delivery();
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        self.service.lock().unwrap().running = true;
        self.emit("service_changed", None, None);
        let f = self.clone();
        tokio::spawn(async move {
            let mut last = std::time::Instant::now() - std::time::Duration::from_secs(86400);
            loop {
                if f.stop.is_cancelled() {
                    break;
                }
                let s = match f.store.public_settings() {
                    Ok(s) => s,
                    Err(_) => break,
                };
                if last.elapsed().as_secs() >= s.poll_interval_seconds
                    && !f.service.lock().unwrap().polling
                {
                    let p = f.clone();
                    tokio::spawn(async move {
                        let _ = p.poll_now().await;
                    });
                    last = std::time::Instant::now();
                }
                if let Ok(runs) = f.store.runs() {
                    let mut scan = QueuedWorkerScan::default();
                    for r in runs.into_iter().rev().filter(|r| r.status == "queued") {
                        let Ok(Some(c)) = scan.reserve(&f, &r.id, &r.trigger_id) else {
                            continue;
                        };
                        let worker = f.clone();
                        tokio::spawn(async move {
                            let result = worker.execute(&r.id, c).await;
                            if let Err(e) = result {
                                let status = worker.store.detail(&r.id).ok().map(|d| d.run.status);
                                if status.as_deref() != Some("cancelled") {
                                    let _ = worker.set_run_status(
                                        &r.id,
                                        "failed",
                                        Some(worker.redact(&format!("{e:#}"))),
                                    );
                                }
                            }
                            if worker
                                .store
                                .detail(&r.id)
                                .is_ok_and(|d| d.run.status == "running")
                            {
                                let status = if worker.stop.is_cancelled() {
                                    "interrupted"
                                } else {
                                    "cancelled"
                                };
                                let _ = worker.set_run_status(
                                    &r.id,
                                    status,
                                    Some("Execution stopped before completion".into()),
                                );
                            }
                            {
                                let _gate = worker.lifecycle_gate.lock().unwrap();
                                let removed = worker.active.lock().unwrap().remove(&r.id).is_some();
                                if removed {
                                    worker.emit("capacity_changed", Some(&r.id), None);
                                }
                            }
                            worker.wake.notify_one();
                        });
                    }
                }
                tokio::select! {_=f.stop.cancelled()=>break,_=f.wake.notified()=>{},_=tokio::time::sleep(std::time::Duration::from_millis(300))=>{}}
            }
            f.service.lock().unwrap().running = false;
            f.emit("service_changed", None, None);
        });
    }
    pub async fn shutdown(&self) {
        {
            let _gate = self.lifecycle_gate.lock().unwrap();
            self.stop.cancel();
            for worker in self.active.lock().unwrap().values() {
                worker.cancel.cancel();
            }
        }
        self.usage.shutdown().await;
        let dispatcher = self.connector_dispatcher.lock().unwrap().take();
        if let Some(dispatcher) = dispatcher {
            dispatcher.shutdown().await;
        }
        let issue_dispatcher = self.issue_node_comment_dispatcher.lock().unwrap().take();
        if let Some(dispatcher) = issue_dispatcher {
            dispatcher.shutdown().await;
        }
        self.accounts.shutdown().await;
        for _ in 0..100 {
            if self.active.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
    pub(crate) fn redact(&self, s: &str) -> String {
        self.store.redact_cached(s)
    }
    pub async fn poll_now(&self) -> Result<()> {
        let _guard = self.poll_lock.lock().await;
        self.service.lock().unwrap().polling = true;
        self.emit("service_changed", None, None);
        let result = Github::poll(self).await;
        {
            let mut status = self.service.lock().unwrap();
            status.polling = false;
            status.last_poll_at = Some(now());
            status.last_error = result
                .as_ref()
                .err()
                .map(|e| self.redact(&format!("{e:#}")));
        }
        self.emit("service_changed", None, None);
        result
    }
    pub(crate) async fn execute(
        self: &Arc<Self>,
        id: &str,
        cancel: CancellationToken,
    ) -> Result<()> {
        agents::execute(self, id, cancel).await
    }
}
