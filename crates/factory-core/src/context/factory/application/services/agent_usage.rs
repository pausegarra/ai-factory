use crate::context::factory::application::interfaces::agent_usage_source::{
    AgentUsageSource, UsageQuery,
};
use crate::*;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
#[cfg(test)]
use tokio_util::sync::CancellationToken;

pub(crate) struct UsageManager {
    source: Arc<dyn AgentUsageSource>,
    codex: Mutex<()>,
    opencode: Mutex<()>,
    codex_budget: Duration,
}
impl UsageManager {
    pub(crate) fn new(source: Arc<dyn AgentUsageSource>) -> Self {
        Self {
            source,
            codex: Mutex::new(()),
            opencode: Mutex::new(()),
            codex_budget: Duration::from_secs(60),
        }
    }
    pub(crate) async fn shutdown(&self) {
        let (_codex, _opencode) = tokio::join!(self.codex.lock(), self.opencode.lock());
    }
}
fn state(status: UsageStatus, code: UsageCode) -> UsageObservation {
    UsageObservation::state(status, code)
}
impl Factory {
    pub async fn agent_usage(&self, agent: &str) -> Result<AgentUsage> {
        anyhow::ensure!(["codex", "opencode"].contains(&agent), "Unsupported agent");
        let mut result = AgentUsage {
            agent: agent.into(),
            status: UsageStatus::Available,
            entries: vec![],
            checked_at: now(),
            code: None,
        };
        let stopping = |mut result: AgentUsage| {
            result.status = UsageStatus::Error;
            result.code = Some(UsageCode::Stopping);
            result
        };
        if self.stop.is_cancelled() {
            return Ok(stopping(result));
        }
        let gate = if agent == "codex" {
            &self.usage.codex
        } else {
            &self.usage.opencode
        };
        let _gate = tokio::select! { biased; _=self.stop.cancelled()=>return Ok(stopping(result)), guard=gate.lock()=>guard };
        if self.stop.is_cancelled() {
            return Ok(stopping(result));
        }
        let deadline = tokio::time::Instant::now() + self.usage.codex_budget;
        let settings = self.settings()?;
        if agent == "opencode" {
            let observation = self
                .usage
                .source
                .read(
                    UsageQuery {
                        agent,
                        executable: &settings.commands.opencode,
                        directory: &self.root,
                        codex_home: None,
                    },
                    self.stop.child_token(),
                )
                .await;
            result.entries.push(UsageEntry {
                id: "opencode:console".into(),
                label: "OpenCode Console".into(),
                email: None,
                account_status: None,
                observation,
                checked_at: now(),
            });
        } else {
            for account in self.agent_accounts()? {
                let observation = match account.status.as_str() {
                    "pending" => state(UsageStatus::Pending, UsageCode::SignInPending),
                    "failed" => state(UsageStatus::Failed, UsageCode::SignInFailed),
                    "ready" => {
                        if self.stop.is_cancelled() {
                            state(UsageStatus::Error, UsageCode::Stopping)
                        } else if tokio::time::Instant::now() >= deadline {
                            state(UsageStatus::Error, UsageCode::TimedOut)
                        } else {
                            match self.lease_codex_account(&account.id) {
                                Err(_) => state(
                                    UsageStatus::Error,
                                    if self.stop.is_cancelled() {
                                        UsageCode::Stopping
                                    } else {
                                        UsageCode::HomeUnavailable
                                    },
                                ),
                                Ok(lease) => {
                                    let cancel = self.stop.child_token();
                                    let read = self.usage.source.read(
                                        UsageQuery {
                                            agent,
                                            executable: &settings.commands.codex,
                                            directory: &self.root,
                                            codex_home: Some(&lease.home),
                                        },
                                        cancel.clone(),
                                    );
                                    tokio::pin!(read);
                                    // Cancellation is cooperative: retain the lease and await
                                    // source cleanup instead of dropping a timed-out process.
                                    tokio::select! {
                                        observation=&mut read=>observation,
                                        _=tokio::time::sleep_until(deadline)=> { cancel.cancel();let _=read.await;state(UsageStatus::Error,UsageCode::TimedOut) }
                                    }
                                }
                            }
                        }
                    }
                    _ => state(UsageStatus::Unavailable, UsageCode::NotConnected),
                };
                result.entries.push(UsageEntry {
                    id: account.id,
                    label: account.label,
                    email: account.email,
                    account_status: Some(account.status),
                    observation,
                    checked_at: now(),
                });
            }
            if result.entries.is_empty() {
                result.status = UsageStatus::NoAccounts;
            }
        }
        result.checked_at = now();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::factory::infrastructure::filesystem::agent_home;
    use std::{future::Future, pin::Pin};
    use tokio::sync::{mpsc, Semaphore};
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
    struct BlockCodex {
        started: mpsc::UnboundedSender<String>,
        release: Semaphore,
    }
    impl AgentUsageSource for BlockCodex {
        fn read<'a>(
            &'a self,
            query: UsageQuery<'a>,
            cancel: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = UsageObservation> + Send + 'a>> {
            Box::pin(async move {
                self.started.send(query.agent.into()).unwrap();
                if query.agent == "codex" {
                    tokio::select! {
                        _=cancel.cancelled()=>return UsageObservation::state(UsageStatus::Error,UsageCode::Stopping),
                        permit=self.release.acquire()=>{permit.unwrap().forget();}
                    }
                }
                UsageObservation::available(vec![])
            })
        }
    }
    fn fixture(
        budget: Duration,
    ) -> (
        tempfile::TempDir,
        Arc<Factory>,
        Arc<BlockCodex>,
        mpsc::UnboundedReceiver<String>,
        String,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mut factory = Factory::open_with_credentials(
            dir.path().join("data"),
            "https://api.github.com",
            Arc::new(NoCredentials),
        )
        .unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        let source = Arc::new(BlockCodex {
            started: tx,
            release: Semaphore::new(0),
        });
        let manager = &mut Arc::get_mut(&mut factory).unwrap().usage;
        manager.source = source.clone();
        manager.codex_budget = budget;
        let id = uuid::Uuid::new_v4().to_string();
        let home = agent_home::create(&factory.root, &id).unwrap();
        std::fs::write(home.join("auth.json"), "fixture-only").unwrap();
        factory
            .store
            .save_agent_account(&AgentAccount {
                id: id.clone(),
                agent: "codex".into(),
                label: "Fixture".into(),
                email: None,
                status: "ready".into(),
                error: None,
            })
            .unwrap();
        (dir, factory, source, rx, id)
    }
    #[tokio::test]
    async fn usage_service_gates_are_independent_and_leases_cover_cleanup() {
        let (_dir, factory, source, mut started, id) = fixture(Duration::from_secs(5));
        let first = {
            let factory = factory.clone();
            tokio::spawn(async move { factory.agent_usage("codex").await.unwrap() })
        };
        assert_eq!(started.recv().await.as_deref(), Some("codex"));
        assert!(factory
            .remove_agent_account(&id)
            .await
            .unwrap_err()
            .to_string()
            .contains("in use"));
        let second = {
            let factory = factory.clone();
            tokio::spawn(async move { factory.agent_usage("codex").await.unwrap() })
        };
        let open = factory.agent_usage("opencode").await.unwrap();
        assert_eq!(open.entries[0].observation.status, UsageStatus::Available);
        assert_eq!(started.recv().await.as_deref(), Some("opencode"));
        assert!(started.try_recv().is_err());
        source.release.add_permits(1);
        first.await.unwrap();
        assert_eq!(started.recv().await.as_deref(), Some("codex"));
        assert!(factory.remove_agent_account(&id).await.is_err());
        source.release.add_permits(1);
        second.await.unwrap();
        factory.remove_agent_account(&id).await.unwrap();
        factory.shutdown().await;
    }
    #[tokio::test]
    async fn usage_service_budget_drains_source_and_marks_remaining_accounts() {
        let (_dir, factory, _source, mut started, _id) = fixture(Duration::from_millis(100));
        let second = uuid::Uuid::new_v4().to_string();
        let home = agent_home::create(&factory.root, &second).unwrap();
        std::fs::write(home.join("auth.json"), "fixture-only").unwrap();
        factory
            .store
            .save_agent_account(&AgentAccount {
                id: second,
                agent: "codex".into(),
                label: "Second".into(),
                email: None,
                status: "ready".into(),
                error: None,
            })
            .unwrap();
        let result = factory.agent_usage("codex").await.unwrap();
        assert_eq!(result.entries.len(), 2);
        assert!(result
            .entries
            .iter()
            .all(|e| e.observation.code == Some(UsageCode::TimedOut)));
        assert_eq!(started.recv().await.as_deref(), Some("codex"));
        assert!(started.try_recv().is_err());
        for account in factory.agent_accounts().unwrap() {
            factory.remove_agent_account(&account.id).await.unwrap();
        }
        factory.shutdown().await;
    }
    #[tokio::test]
    async fn usage_service_shutdown_cancels_and_waits_for_gates() {
        let (_dir, factory, _source, mut started, id) = fixture(Duration::from_secs(5));
        let task = {
            let factory = factory.clone();
            tokio::spawn(async move { factory.agent_usage("codex").await.unwrap() })
        };
        started.recv().await.unwrap();
        factory.shutdown().await;
        assert_eq!(
            task.await.unwrap().entries[0].observation.code,
            Some(UsageCode::Stopping)
        );
        factory.remove_agent_account(&id).await.unwrap();
        assert_eq!(
            factory.agent_usage("opencode").await.unwrap().code,
            Some(UsageCode::Stopping)
        );
        assert!(started.try_recv().is_err());
    }
}
