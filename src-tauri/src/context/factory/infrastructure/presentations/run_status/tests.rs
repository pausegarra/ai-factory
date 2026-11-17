use super::*;
use factory_core::{FactoryEvent, Metrics};
use std::{collections::VecDeque, future::ready};
use tokio::sync::broadcast;

fn event(kind: &str) -> FactoryEvent {
    FactoryEvent {
        kind: kind.into(),
        run_id: None,
        node_id: None,
        timestamp: "2026-11-17T00:00:00Z".into(),
    }
}

#[test]
fn labels_distinguish_unknown_zero_and_separate_counts() {
    assert_eq!(
        RunStatusLabels::unknown(),
        RunStatusLabels {
            running: "Running: —".into(),
            queued: "Queued: —".into(),
        }
    );
    assert_eq!(
        RunStatusLabels::from_metrics(&Metrics::default()),
        RunStatusLabels {
            running: "Running: 0".into(),
            queued: "Queued: 0".into(),
        }
    );
    let metrics = Metrics {
        total: 99,
        running: 2,
        queued: 3,
        succeeded: 40,
        failed: 20,
        cancelled: 15,
        interrupted: 19,
    };
    assert_eq!(
        RunStatusLabels::from_metrics(&metrics),
        RunStatusLabels {
            running: "Running: 2".into(),
            queued: "Queued: 3".into(),
        }
    );
    assert_eq!(
        RunStatusLabels::from_metrics(&Metrics {
            running: u64::MAX,
            queued: 0,
            ..Metrics::default()
        })
        .running,
        "Running: 18446744073709551615"
    );
}

#[test]
fn refresh_only_events_that_can_change_run_counts_or_require_resync() {
    for kind in [
        "run_created",
        "run_changed",
        "capacity_changed",
        "resync",
        "history_imported",
    ] {
        assert!(requires_refresh(kind), "{kind}");
    }
    for kind in [
        "log",
        "log_changed",
        "node_changed",
        "settings_changed",
        "service_changed",
        "unknown",
    ] {
        assert!(!requires_refresh(kind), "{kind}");
    }
}

fn metrics(running: u64, queued: u64) -> Metrics {
    Metrics {
        running,
        queued,
        ..Metrics::default()
    }
}
fn labels(running: u64, queued: u64) -> RunStatusLabels {
    RunStatusLabels {
        running: format!("Running: {running}"),
        queued: format!("Queued: {queued}"),
    }
}
async fn replay(kinds: &[&str], outcomes: Vec<Result<Metrics, String>>) -> Vec<RunStatusLabels> {
    let (tx, rx) = broadcast::channel(kinds.len() + 1);
    for kind in kinds {
        tx.send(event(kind)).unwrap();
    }
    drop(tx);
    let mut source: VecDeque<_> = outcomes.into();
    let mut written = vec![];
    consume(
        rx,
        || ready(source.pop_front().expect("unexpected snapshot read")),
        |value| {
            written.push(value);
            Ok(())
        },
    )
    .await;
    assert!(source.is_empty(), "a required refresh was skipped");
    written
}

#[tokio::test]
async fn startup_reads_existing_counts_without_event_and_stops_on_closed_channel() {
    assert_eq!(
        replay(&[], vec![Ok(metrics(2, 4))]).await,
        vec![labels(2, 4)]
    );
}

#[tokio::test]
async fn queued_start_completion_cancel_and_admission_replace_both_counts() {
    let written = replay(
        &[
            "run_created",
            "run_changed",
            "run_changed",
            "run_changed",
            "capacity_changed",
            "history_imported",
            "resync",
        ],
        vec![
            Ok(metrics(0, 0)),
            Ok(metrics(0, 1)),
            Ok(metrics(1, 0)),
            Ok(metrics(0, 0)),
            Ok(metrics(0, 0)),
            Ok(metrics(1, 2)),
            Ok(metrics(1, 5)),
            Ok(metrics(2, 3)),
        ],
    )
    .await;
    assert_eq!(
        written,
        vec![
            labels(0, 0),
            labels(0, 1),
            labels(1, 0),
            labels(0, 0),
            labels(0, 0),
            labels(1, 2),
            labels(1, 5),
            labels(2, 3)
        ]
    );
}

#[tokio::test]
async fn snapshot_failure_preserves_unknown_or_last_valid_and_recovers() {
    assert!(replay(&[], vec![Err("unavailable".into())])
        .await
        .is_empty());
    assert_eq!(
        replay(
            &["run_created", "resync", "run_changed"],
            vec![
                Err("unavailable".into()),
                Ok(metrics(2, 1)),
                Err("unavailable".into()),
                Ok(metrics(0, 3))
            ]
        )
        .await,
        vec![labels(2, 1), labels(0, 3)]
    );
}

#[tokio::test]
async fn log_and_unrelated_events_do_not_read_snapshot() {
    assert_eq!(
        replay(
            &[
                "log",
                "log_changed",
                "node_changed",
                "settings_changed",
                "service_changed"
            ],
            vec![Ok(metrics(1, 2))]
        )
        .await,
        vec![labels(1, 2)]
    );
}

#[tokio::test]
async fn broadcast_lag_forces_full_snapshot_even_when_surviving_event_is_log() {
    let (tx, rx) = broadcast::channel(1);
    tx.send(event("run_changed")).unwrap();
    tx.send(event("log_changed")).unwrap();
    drop(tx);
    let mut source = VecDeque::from([Ok(metrics(0, 1)), Ok(metrics(3, 4))]);
    let mut written = vec![];
    consume(
        rx,
        || ready(source.pop_front().expect("extra snapshot")),
        |value| {
            written.push(value);
            Ok(())
        },
    )
    .await;
    assert!(source.is_empty());
    assert_eq!(written, vec![labels(0, 1), labels(3, 4)]);
}

#[tokio::test]
async fn destination_failure_does_not_end_future_refreshes() {
    let (tx, rx) = broadcast::channel(1);
    tx.send(event("run_changed")).unwrap();
    drop(tx);
    let mut source = VecDeque::from([Ok(metrics(0, 1)), Ok(metrics(1, 0))]);
    let mut attempts = vec![];
    let mut displayed = RunStatusLabels::unknown();
    consume(
        rx,
        || ready(source.pop_front().unwrap()),
        |value| {
            attempts.push(value.clone());
            if attempts.len() == 1 {
                return Err("menu unavailable".into());
            }
            displayed = value;
            Ok(())
        },
    )
    .await;
    assert_eq!(attempts, vec![labels(0, 1), labels(1, 0)]);
    assert_eq!(displayed, labels(1, 0));
}

#[tokio::test]
async fn event_during_initial_read_is_not_lost_and_reads_never_overlap() {
    use std::sync::{Arc, Mutex};
    use tokio::sync::{mpsc, oneshot};
    let (tx, rx) = broadcast::channel(4);
    let current = Arc::new(Mutex::new(metrics(0, 1)));
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let (release_tx, release_rx) = oneshot::channel();
    let mut release = Some(release_rx);
    let mut written = vec![];
    let source = current.clone();
    let consumer = consume(
        rx,
        move || {
            let captured = source.lock().unwrap().clone();
            let gate = release.take();
            let started = started_tx.clone();
            async move {
                started.send(()).unwrap();
                if let Some(gate) = gate {
                    gate.await.unwrap();
                }
                Ok(captured)
            }
        },
        |value| {
            written.push(value);
            Ok(())
        },
    );
    let driver = async move {
        started_rx.recv().await.unwrap();
        assert!(started_rx.try_recv().is_err(), "only one initial read");
        *current.lock().unwrap() = metrics(1, 0);
        tx.send(event("run_changed")).unwrap();
        // Mientras el primer snapshot está suspendido no debe empezar otro.
        tokio::task::yield_now().await;
        assert!(started_rx.try_recv().is_err(), "overlapping snapshot reads");
        release_tx.send(()).unwrap();
        started_rx.recv().await.unwrap();
        // Sin nuevos eventos tampoco se programan lecturas periódicas.
        assert!(started_rx.try_recv().is_err());
        drop(tx);
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(consumer, driver);
    })
    .await
    .unwrap();
    assert_eq!(written, vec![labels(0, 1), labels(1, 0)]);
}
