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

pub struct Factory {
    _process_lock: std::fs::File,
    pub(crate) root: PathBuf,
    pub(crate) github_base: String,
    pub(crate) store: Store,
    pub(crate) events: broadcast::Sender<FactoryEvent>,
    pub(crate) wake: Notify,
    pub(crate) stop: CancellationToken,
    pub(crate) started: AtomicBool,
    pub(crate) active: Mutex<HashMap<String, CancellationToken>>,
    pub(crate) service: Mutex<ServiceStatus>,
    poll_lock: tokio::sync::Mutex<()>,
}
impl Drop for Factory {
    fn drop(&mut self) {
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
        let cleanup_errors =
            crate::context::factory::infrastructure::processes::workspaces::recover(&root, &store)?;
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
        }))
    }
    pub fn settings(&self) -> Result<Settings> {
        self.store.public_settings()
    }
    pub async fn agent_models(&self, agent: &str) -> Result<Vec<AgentModel>> {
        let settings = self.store.public_settings()?;
        let executable = match agent {
            "codex" => &settings.commands.codex,
            "opencode" => &settings.commands.opencode,
            _ => anyhow::bail!("Unsupported agent"),
        };
        tokio::select! {
            _ = self.stop.cancelled() => anyhow::bail!("Factory is stopping"),
            result = crate::context::factory::infrastructure::processes::agent_models::discover(agent, executable, &self.root) => result,
        }
    }
    pub fn save_settings(&self, mut s: Settings) -> Result<Settings> {
        for values in [&mut s.owners, &mut s.authors, &mut s.reviewers] {
            let mut seen = std::collections::HashSet::new();
            for v in values.iter_mut() {
                *v = v.to_lowercase();
            }
            values.retain(|v| seen.insert(v.clone()));
        }
        for p in &mut s.token_profiles {
            p.account = p.account.to_lowercase();
        }
        for p in &mut s.projects {
            p.repository = p.repository.to_lowercase();
        }
        s.validate()?;
        self.store.save_settings(&s)?;
        self.emit("settings_changed", None, None);
        self.wake.notify_one();
        self.settings()
    }
    pub fn import_legacy(
        &self,
        config_path: Option<PathBuf>,
        database_path: Option<PathBuf>,
    ) -> Result<ImportReport> {
        let active = self.active.lock().unwrap();
        ensure!(
            active.is_empty(),
            "Stop active runs before importing history"
        );
        let batch = crate::context::factory::infrastructure::migration::legacy::read_legacy(
            config_path.as_deref(),
            database_path.as_deref(),
            &self.root,
            &self.store.settings()?,
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
        let runs = self.store.runs()?;
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
            metrics,
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
        let s = self.store.public_settings()?;
        let flow = s
            .flows
            .iter()
            .find(|f| f.id == e.trigger_id)
            .context("Unknown trigger")?;
        if !flow.enabled {
            return Ok(None);
        }
        let flow = flow.for_actor(&e.actor, &s.token_profiles);
        let id = self.store.insert(&e, &flow, 1, true)?;
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
        let s = self.store.public_settings()?;
        let flow = s
            .flows
            .iter()
            .find(|f| f.id == "pr_approved")
            .context("Approval flow missing")?;
        if !flow.enabled {
            return Ok(None);
        }
        let flow = flow.for_actor(&e.actor, &s.token_profiles);
        let id = self.store.insert_approval(&e, &flow, comments, approved)?;
        if let Some(id) = &id {
            self.emit("run_created", Some(id), None);
            self.wake.notify_one();
        }
        Ok(id)
    }
    pub(crate) fn github(&self, token: &str) -> Result<Github> {
        Github::with_base(token, &self.github_base)
    }
    pub fn set_run_status(&self, id: &str, status: &str, error: Option<String>) -> Result<()> {
        let mut r = self.store.detail(id)?.run;
        r.status = status.into();
        r.error = error;
        if status == "running" {
            r.started_at = Some(now());
        } else if status != "queued" {
            r.finished_at = Some(now());
            r.current_step = None;
        }
        self.store.save_run(&r)?;
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
        let d = self.store.detail(id)?;
        ensure!(
            ["queued", "running"].contains(&d.run.status.as_str()),
            "Run is already terminal"
        );
        if let Some(c) = self.active.lock().unwrap().get(id) {
            c.cancel();
        }
        self.set_run_status(id, "cancelled", None)?;
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
        let d = self.store.detail(id)?;
        ensure!(
            !["running", "queued"].contains(&d.run.status.as_str()),
            "Cannot retry active run"
        );
        let settings = self.store.public_settings()?;
        let f = settings
            .flows
            .iter()
            .find(|f| f.id == d.flow.id)
            .context("Flow missing")?
            .for_actor(&d.event.actor, &settings.token_profiles);
        let id = self
            .store
            .insert(&d.event, &f, d.run.attempt + 1, false)?
            .unwrap();
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
        Ok(LogChunk {
            text,
            offset,
            next_offset: next,
            eof: next == total && !["running", "pending"].contains(&n.status.as_str()),
        })
    }
    pub fn start(self: &Arc<Self>) {
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
                    for r in runs.into_iter().rev().filter(|r| r.status == "queued") {
                        let mut active = f.active.lock().unwrap();
                        if active.len() >= s.max_concurrent {
                            break;
                        }
                        if active.contains_key(&r.id) {
                            continue;
                        }
                        let c = CancellationToken::new();
                        active.insert(r.id.clone(), c.clone());
                        drop(active);
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
                            worker.active.lock().unwrap().remove(&r.id);
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
        self.stop.cancel();
        for c in self.active.lock().unwrap().values() {
            c.cancel();
        }
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
