use crate::{CleanupTarget, Result};

pub trait RunCleanup: Send + Sync {
    fn remove(&self, repository: &str, run_id: &str, target: &CleanupTarget) -> Result<()>;
}
