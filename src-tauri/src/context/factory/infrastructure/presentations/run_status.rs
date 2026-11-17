use factory_core::{Factory, FactoryEvent, Metrics};
use std::{future::Future, sync::Arc};
use tauri::menu::MenuItem;
use tokio::sync::broadcast;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunStatusLabels {
    pub(crate) running: String,
    pub(crate) queued: String,
}
impl RunStatusLabels {
    pub(crate) fn unknown() -> Self {
        Self {
            running: "Running: —".into(),
            queued: "Queued: —".into(),
        }
    }
    fn from_metrics(metrics: &Metrics) -> Self {
        Self {
            running: format!("Running: {}", metrics.running),
            queued: format!("Queued: {}", metrics.queued),
        }
    }
}
fn requires_refresh(kind: &str) -> bool {
    matches!(
        kind,
        "run_created" | "run_changed" | "capacity_changed" | "resync" | "history_imported"
    )
}

async fn consume<Read, ReadFuture, Apply>(
    mut receiver: broadcast::Receiver<FactoryEvent>,
    mut read: Read,
    mut apply: Apply,
) where
    Read: FnMut() -> ReadFuture,
    ReadFuture: Future<Output = Result<Metrics, String>>,
    Apply: FnMut(RunStatusLabels) -> Result<(), String>,
{
    loop {
        // Failed reads leave the last verified labels (or unknown) in place.
        if let Ok(metrics) = read().await {
            let _ = apply(RunStatusLabels::from_metrics(&metrics));
        }
        loop {
            match receiver.recv().await {
                Ok(event) if requires_refresh(&event.kind) => break,
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => break,
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

pub(crate) fn install<R: tauri::Runtime>(
    factory: Arc<Factory>,
    running: MenuItem<R>,
    queued: MenuItem<R>,
) {
    // Subscribe before the initial snapshot and before Factory starts.
    let receiver = factory.subscribe();
    tauri::async_runtime::spawn(async move {
        consume(
            receiver,
            move || {
                let factory = factory.clone();
                crate::blocking(move || factory.snapshot().map(|snapshot| snapshot.metrics))
            },
            move |labels| {
                // Try both rows even if one fails; the next refresh retries both.
                let running_result = running.set_text(&labels.running);
                let queued_result = queued.set_text(&labels.queued);
                running_result
                    .and(queued_result)
                    .map_err(|error| error.to_string())
            },
        )
        .await;
    });
}

#[cfg(test)]
mod tests;
