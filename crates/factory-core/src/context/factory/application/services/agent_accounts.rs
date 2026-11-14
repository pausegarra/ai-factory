use crate::context::factory::{
    domain::entities::agent_account::{account_label, validate_account_id},
    infrastructure::{filesystem::agent_home, processes::codex_login},
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
use tokio::sync::{oneshot, Notify};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(crate) struct AccountManager {
    pending: Mutex<Option<PendingLogin>>,
    leases: Mutex<HashMap<String, usize>>,
}
/// Keeps a managed home alive during short-lived catalog and usage queries.
pub(crate) struct CodexAccountLease<'a> {
    manager: &'a AccountManager,
    id: String,
    pub(crate) home: PathBuf,
}
impl Drop for CodexAccountLease<'_> {
    fn drop(&mut self) {
        let mut leases = self.manager.leases.lock().unwrap();
        if let Some(count) = leases.get_mut(&self.id) {
            *count -= 1;
            if *count == 0 {
                leases.remove(&self.id);
            }
        }
    }
}
struct PendingLogin {
    id: String,
    url: Option<String>,
    cancel: CancellationToken,
    completion: Arc<Completion>,
}
#[derive(Default)]
struct Completion {
    stopped: AtomicBool,
    notify: Notify,
}
impl Completion {
    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.stopped.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}
impl AccountManager {
    fn ensure_not_leased(&self, id: &str) -> Result<()> {
        ensure!(
            !self.leases.lock().unwrap().contains_key(id),
            "Agent account is in use; wait for the current query to finish"
        );
        Ok(())
    }
    pub(crate) async fn shutdown(&self) {
        let completion = self.pending.lock().unwrap().as_ref().map(|pending| {
            pending.cancel.cancel();
            pending.completion.clone()
        });
        if let Some(completion) = completion {
            completion.wait().await;
        }
    }
}
impl Drop for AccountManager {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.get_mut().unwrap().as_ref() {
            pending.cancel.cancel();
        }
    }
}

impl Factory {
    pub fn agent_accounts(&self) -> Result<Vec<AgentAccount>> {
        self.store.agent_accounts()
    }

    pub fn codex_login_url(&self, id: &str) -> Result<String> {
        validate_account_id(id)?;
        self.accounts
            .pending
            .lock()
            .unwrap()
            .as_ref()
            .filter(|pending| pending.id == id)
            .and_then(|pending| pending.url.clone())
            .context("Codex login is no longer pending")
    }

    pub(crate) fn codex_home(&self, id: &str) -> Result<PathBuf> {
        validate_account_id(id)?;
        let account = self.store.agent_account(id)?;
        ensure!(
            account.id == id && account.agent == "codex" && account.status == "ready",
            "Codex account is not connected"
        );
        agent_home::home(&self.root, id, true)
    }

    pub(crate) fn lease_codex_account(&self, id: &str) -> Result<CodexAccountLease<'_>> {
        let _gate = self.lifecycle_gate.lock().unwrap();
        ensure!(!self.stop.is_cancelled(), "Factory is stopping");
        let home = self.codex_home(id)?;
        *self
            .accounts
            .leases
            .lock()
            .unwrap()
            .entry(id.into())
            .or_default() += 1;
        Ok(CodexAccountLease {
            manager: &self.accounts,
            id: id.into(),
            home,
        })
    }

    pub(crate) fn validate_codex_bindings(&self, settings: &Settings) -> Result<()> {
        for (profile_id, account_id) in &settings.codex_bindings {
            ensure!(
                settings
                    .token_profiles
                    .iter()
                    .any(|profile| &profile.id == profile_id),
                "Codex binding references an unknown GitHub profile"
            );
            self.codex_home(account_id)?;
        }
        Ok(())
    }

    pub async fn start_codex_login(self: &Arc<Self>, label: String) -> Result<CodexLogin> {
        let label = account_label(&label)?;
        let (account, home, executable, cancel, completion) = {
            let _gate = self.lifecycle_gate.lock().unwrap();
            ensure!(!self.stop.is_cancelled(), "Factory is stopping");
            let mut pending = self.accounts.pending.lock().unwrap();
            ensure!(pending.is_none(), "Another Codex login is already pending");
            let executable = self.store.public_settings()?.commands.codex;
            let account = AgentAccount {
                id: uuid::Uuid::new_v4().to_string(),
                agent: "codex".into(),
                label,
                email: None,
                status: "pending".into(),
                error: None,
            };
            let home = agent_home::create(&self.root, &account.id)?;
            self.store.save_agent_account(&account)?;
            let cancel = self.stop.child_token();
            let completion = Arc::new(Completion::default());
            *pending = Some(PendingLogin {
                id: account.id.clone(),
                url: None,
                cancel: cancel.clone(),
                completion: completion.clone(),
            });
            (account, home, executable, cancel, completion)
        };
        self.emit("agent_accounts_changed", None, None);
        let (sender, receiver) = oneshot::channel();
        let factory = Arc::downgrade(self);
        let root = self.root.clone();
        let mut updated = account.clone();
        tokio::spawn(async move {
            let mut sender = Some(sender);
            let result = codex_login::login(&executable, &home, cancel, &mut sender, |url| {
                if let Some(factory) = factory.upgrade() {
                    let mut pending = factory.accounts.pending.lock().unwrap();
                    if let Some(pending) =
                        pending.as_mut().filter(|pending| pending.id == updated.id)
                    {
                        pending.url = Some(url.into());
                    }
                }
            })
            .await
            .and_then(|email| {
                agent_home::home(&root, &updated.id, true)?;
                Ok(email)
            });
            match result {
                Ok(email) => {
                    updated.email = email;
                    updated.status = "ready".into();
                }
                Err(error) => {
                    updated.status = "failed".into();
                    updated.error = Some(error.to_string());
                }
            }
            if let Some(factory) = factory.upgrade() {
                {
                    let mut pending = factory.accounts.pending.lock().unwrap();
                    let _ = factory.store.save_agent_account(&updated);
                    if pending
                        .as_ref()
                        .is_some_and(|pending| pending.id == updated.id)
                    {
                        *pending = None;
                    }
                }
                factory.emit("agent_accounts_changed", None, None);
            }
            completion.stop();
            if let Some(sender) = sender.take() {
                let _ = sender.send(Err(anyhow::anyhow!(updated
                    .error
                    .unwrap_or_else(|| "Codex login process stopped".into()))));
            }
        });
        let auth_url = receiver.await.context("Codex login process stopped")??;
        Ok(CodexLogin { account, auth_url })
    }

    pub async fn remove_agent_account(&self, id: &str) -> Result<()> {
        validate_account_id(id)?;
        let completion = {
            let _gate = self.lifecycle_gate.lock().unwrap();
            self.store.agent_account(id)?;
            let active_workers: Vec<String> = self.active.lock().unwrap().keys().cloned().collect();
            self.store
                .ensure_agent_account_unused(id, &active_workers)?;
            self.accounts.ensure_not_leased(id)?;
            self.accounts
                .pending
                .lock()
                .unwrap()
                .as_ref()
                .filter(|pending| pending.id == id)
                .map(|pending| {
                    pending.cancel.cancel();
                    pending.completion.clone()
                })
        };
        if let Some(completion) = completion {
            completion.wait().await;
        }
        {
            let _gate = self.lifecycle_gate.lock().unwrap();
            let active_workers: Vec<String> = self.active.lock().unwrap().keys().cloned().collect();
            self.store
                .ensure_agent_account_unused(id, &active_workers)?;
            self.accounts.ensure_not_leased(id)?;
            self.store
                .delete_agent_account(id, &active_workers, || agent_home::remove(&self.root, id))?;
        }
        self.emit("agent_accounts_changed", None, None);
        self.emit("settings_changed", None, None);
        self.wake.notify_one();
        Ok(())
    }
}

#[cfg(test)]
mod lease_tests {
    use super::*;
    struct NoCredentials;
    impl Credentials for NoCredentials {
        fn get(&self, _: &str) -> Result<Option<String>> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<()> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn deletion_waits_until_all_account_usage_leases_have_been_released() {
        let dir = tempfile::tempdir().unwrap();
        let factory = Factory::open_with_credentials(
            dir.path().join("data"),
            "https://api.github.com",
            Arc::new(NoCredentials),
        )
        .unwrap();
        let account = AgentAccount {
            id: uuid::Uuid::new_v4().to_string(),
            agent: "codex".into(),
            label: "Fixture".into(),
            email: None,
            status: "ready".into(),
            error: None,
        };
        let home = agent_home::create(&factory.root, &account.id).unwrap();
        std::fs::write(home.join("auth.json"), "fixture-credentials").unwrap();
        factory.store.save_agent_account(&account).unwrap();
        let first = factory.lease_codex_account(&account.id).unwrap();
        let second = factory.lease_codex_account(&account.id).unwrap();
        assert_eq!(first.home, home);
        assert!(factory.remove_agent_account(&account.id).await.is_err());
        drop(first);
        assert!(factory.remove_agent_account(&account.id).await.is_err());
        drop(second);
        factory.remove_agent_account(&account.id).await.unwrap();
        assert!(!home.exists());
    }
    #[tokio::test]
    async fn cancelled_worker_keeps_its_account_until_worker_cleanup_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let factory = Factory::open_with_credentials(
            dir.path().join("data"),
            "https://api.github.com",
            Arc::new(NoCredentials),
        )
        .unwrap();
        let account = AgentAccount {
            id: uuid::Uuid::new_v4().to_string(),
            agent: "codex".into(),
            label: "Fixture".into(),
            email: None,
            status: "ready".into(),
            error: None,
        };
        let home = agent_home::create(&factory.root, &account.id).unwrap();
        std::fs::write(home.join("auth.json"), "fixture-credentials").unwrap();
        factory.store.save_agent_account(&account).unwrap();
        let mut flow = Settings::default().flows.remove(0);
        for node in &mut flow.nodes {
            node.codex_account_id = Some(account.id.clone());
        }
        let event = TriggerEvent::issue("acme/repo", 1, "[AI] Fixture", "octocat", "");
        let run = factory
            .store
            .insert(&event, &flow, 1, false)
            .unwrap()
            .unwrap();
        factory.register_queued_worker(&run).unwrap().unwrap();
        factory.cancel(&run).unwrap();
        assert!(factory.remove_agent_account(&account.id).await.is_err());
        factory.active.lock().unwrap().remove(&run);
        factory.remove_agent_account(&account.id).await.unwrap();
    }
}
