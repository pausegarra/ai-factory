use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub struct ExitGate {
    started: AtomicBool,
    ready: AtomicBool,
}

impl ExitGate {
    pub fn request_shutdown(&self) -> bool {
        !self.started.swap(true, Ordering::SeqCst)
    }

    pub fn can_exit(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    pub fn finish_shutdown(&self) {
        self.ready.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_quit_requests_wait_for_child_cleanup() {
        let gate = ExitGate::default();
        assert!(!gate.can_exit());
        assert!(gate.request_shutdown());
        assert!(!gate.can_exit(), "the first quit must wait for cleanup");
        assert!(!gate.request_shutdown(), "only one shutdown worker");
        assert!(!gate.can_exit(), "a second OS quit must still wait");
        gate.finish_shutdown();
        assert!(gate.can_exit());
    }
}
