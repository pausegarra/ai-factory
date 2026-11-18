use crate::{BackupErrorCode, BackupFailure};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
type Result<T> = std::result::Result<T, BackupFailure>;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationKind {
    ReadOnly,
    Mutating,
    ExternalEffect,
}
#[derive(Default)]
struct GateState {
    exclusive: bool,
    stopping: bool,
    recovery: bool,
    in_flight: usize,
    epoch: u64,
}
#[derive(Default, Clone)]
pub(crate) struct MaintenanceGate {
    state: Arc<Mutex<GateState>>,
    notify: Arc<Notify>,
}
pub(crate) struct OperationPermit {
    gate: MaintenanceGate,
}
pub(crate) struct RestorePermit {
    gate: MaintenanceGate,
}
impl MaintenanceGate {
    pub fn try_enter(&self, kind: OperationKind) -> Result<OperationPermit> {
        self.enter(kind, None)
    }
    pub fn try_enter_at_epoch(&self, kind: OperationKind, epoch: u64) -> Result<OperationPermit> {
        self.enter(kind, Some(epoch))
    }
    fn enter(&self, kind: OperationKind, epoch: Option<u64>) -> Result<OperationPermit> {
        let mut s = self.state.lock().unwrap();
        if s.exclusive
            || (s.stopping && kind != OperationKind::ReadOnly)
            || epoch.is_some_and(|e| e != s.epoch)
        {
            return Err(BackupFailure::new(BackupErrorCode::OperationBusy));
        }
        if s.recovery && kind != OperationKind::ReadOnly {
            return Err(BackupFailure::new(BackupErrorCode::RecoveryRequired));
        }
        s.in_flight += 1;
        Ok(OperationPermit { gate: self.clone() })
    }
    pub fn try_restore(&self) -> Result<RestorePermit> {
        self.exclusive(false)
    }
    pub fn try_recover(&self) -> Result<RestorePermit> {
        self.exclusive(true)
    }
    fn exclusive(&self, recover: bool) -> Result<RestorePermit> {
        let mut s = self.state.lock().unwrap();
        if s.stopping || s.exclusive || s.in_flight > 0 {
            return Err(BackupFailure::new(BackupErrorCode::OperationBusy));
        }
        if s.recovery && !recover {
            return Err(BackupFailure::new(BackupErrorCode::RecoveryRequired));
        }
        s.exclusive = true;
        Ok(RestorePermit { gate: self.clone() })
    }
    pub fn require_recovery(&self) {
        self.state.lock().unwrap().recovery = true;
    }
    pub fn recovery_required(&self) -> bool {
        self.state.lock().unwrap().recovery
    }
    pub fn epoch(&self) -> u64 {
        self.state.lock().unwrap().epoch
    }
    pub fn request_shutdown(&self) {
        self.state.lock().unwrap().stopping = true;
    }
    pub async fn wait_for_restore_completion(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.state.lock().unwrap().exclusive {
                return;
            }
            notified.await;
        }
    }
}
impl RestorePermit {
    pub fn commit_epoch(&mut self) -> u64 {
        let mut s = self.gate.state.lock().unwrap();
        s.epoch += 1;
        s.epoch
    }
    pub fn recovered(&self) {
        self.gate.state.lock().unwrap().recovery = false;
    }
}
impl Drop for OperationPermit {
    fn drop(&mut self) {
        self.gate.state.lock().unwrap().in_flight -= 1;
    }
}
impl Drop for RestorePermit {
    fn drop(&mut self) {
        self.gate.state.lock().unwrap().exclusive = false;
        self.gate.notify.notify_waiters();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    #[test]
    fn backup_restore_never_races_an_admitted_operation() {
        let gate = MaintenanceGate::default();
        let running = gate.try_enter(OperationKind::ExternalEffect).unwrap();
        assert!(gate.try_restore().is_err());
        drop(running);
        let mut restore = gate.try_restore().unwrap();
        assert!(gate.try_enter(OperationKind::ReadOnly).is_err());
        assert!(gate.try_enter(OperationKind::Mutating).is_err());
        let old = gate.epoch();
        restore.commit_epoch();
        drop(restore);
        assert!(gate
            .try_enter_at_epoch(OperationKind::ExternalEffect, old)
            .is_err());
        assert!(gate.try_enter(OperationKind::ReadOnly).is_ok());
        gate.require_recovery();
        assert!(gate.try_enter(OperationKind::Mutating).is_err());
        assert!(gate.try_enter(OperationKind::ReadOnly).is_ok());
        assert!(gate.try_recover().is_ok());
        gate.request_shutdown();
        assert!(gate.try_enter(OperationKind::Mutating).is_err());
        assert!(gate.try_enter(OperationKind::ReadOnly).is_ok());
    }
    #[test]
    fn backup_restore_and_concurrent_admission_have_one_winner() {
        for _ in 0..32 {
            let gate = MaintenanceGate::default();
            let barrier = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                let shared = scope.spawn(|| {
                    barrier.wait();
                    gate.try_enter(OperationKind::ExternalEffect)
                });
                let exclusive = scope.spawn(|| {
                    barrier.wait();
                    gate.try_restore()
                });
                let first = shared.join().unwrap();
                let second = exclusive.join().unwrap();
                assert_ne!(first.is_ok(), second.is_ok());
            });
        }
    }
    #[tokio::test]
    async fn backup_shutdown_waits_for_restore_permit_without_cancelling_it() {
        let gate = MaintenanceGate::default();
        let restore = gate.try_restore().unwrap();
        gate.request_shutdown();
        let waiting = gate.wait_for_restore_completion();
        tokio::pin!(waiting);
        let pending = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(waiting.as_mut().poll(cx).is_pending())
        })
        .await;
        assert!(pending);
        drop(restore);
        waiting.await;
    }
    #[test]
    fn backup_factory_entrypoints_and_workers_respect_exclusion_and_file_origin() {
        let dir = tempfile::tempdir().unwrap();
        let factory = crate::Factory::open(dir.path().into()).unwrap();
        let event = crate::TriggerEvent::issue("acme/repo", 89, "[AI] fixture", "alice", "");
        let id = factory.enqueue(event).unwrap().unwrap();
        let worker = factory.register_queued_worker(&id).unwrap();
        assert!(worker.is_some());
        assert!(factory.maintenance.try_restore().is_err());
        factory.cancel(&id).unwrap();
        assert!(factory.maintenance.try_restore().is_err());
        factory.active.lock().unwrap().remove(&id);
        let exclusive = factory.maintenance.try_restore().unwrap();
        assert!(factory.snapshot().is_err());
        assert!(factory.save_settings(crate::Settings::default()).is_err());
        drop(exclusive);
        factory
            .store
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE run_file_origins SET origin='imported' WHERE run_id=?1",
                [&id],
            )
            .unwrap();
        factory
            .store
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE runs SET json=json_set(json,'$.status','queued') WHERE id=?1",
                [&id],
            )
            .unwrap();
        assert!(factory.register_queued_worker(&id).unwrap().is_none());
        assert!(!factory.store.claim_queued(&id).unwrap());
        assert!(factory
            .store
            .set_workspace(&id, Some("/fixture/local"))
            .is_err());
        assert!(factory.store.track_pr("acme/repo", 90, &id).is_err());
    }
    #[test]
    fn backup_stopping_preserves_reads_and_lets_admitted_workers_finish() {
        let dir = tempfile::tempdir().unwrap();
        let f = crate::Factory::open(dir.path().into()).unwrap();
        let id = f
            .enqueue(crate::TriggerEvent::issue(
                "acme/repo",
                89,
                "[AI] fixture",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        f.register_queued_worker(&id).unwrap().unwrap();
        f.maintenance.request_shutdown();
        assert!(f.set_run_status(&id, "failed", None).is_err());
        f.set_worker_run_status(&id, "interrupted", None).unwrap();
        assert_eq!(f.run_detail(&id).unwrap().run.status, "interrupted");
        assert!(f.maintenance.try_restore().is_err());
    }
}
