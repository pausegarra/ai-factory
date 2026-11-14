use crate::UsageObservation;
use std::{future::Future, path::Path, pin::Pin};
use tokio_util::sync::CancellationToken;
pub struct UsageQuery<'a> {
    pub agent: &'a str,
    pub executable: &'a str,
    pub directory: &'a Path,
    pub codex_home: Option<&'a Path>,
}
pub trait AgentUsageSource: Send + Sync {
    fn read<'a>(
        &'a self,
        query: UsageQuery<'a>,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = UsageObservation> + Send + 'a>>;
}
