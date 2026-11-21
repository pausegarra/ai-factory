use super::release_publication;
use crate::{now, refresh_release_prompt, Factory, Flow, Result, RunDetail};
use anyhow::Context;
use serde_json::json;

pub(super) fn is_publication(detail: &RunDetail) -> bool {
    detail
        .flow
        .nodes
        .iter()
        .any(|node| release_publication::allowed(&detail.event, node))
}

pub(super) fn confirmed(detail: &RunDetail) -> bool {
    detail.nodes.iter().all(|node| node.status == "succeeded")
        && detail.nodes.iter().any(|node| {
            node.id == "publish_app"
                && node.result.as_ref().is_some_and(|result| {
                    result["data"]["distribution_result"]["published"] == true
                })
        })
}

pub(super) fn record_pending(factory: &Factory, detail: &RunDetail) -> Result<()> {
    let event = &detail.event;
    factory.store.observe(
        &format!("release-issue-close:{}", detail.run.id),
        &json!({"repository":event.repository,"number":event.number,"actor":event.actor,
            "event_id":event.event_id,"publication_confirmed":true,"state":"pending"}),
    )
}

// Publication has already completed when closure starts. An explicit retry of
// the same workflow must reconcile closure without running the publisher again.
pub(super) fn reuse_completed(factory: &Factory, current: &RunDetail) -> Result<bool> {
    if !is_publication(current) || current.run.attempt <= 1 {
        return Ok(false);
    }
    let current_flow = effective_release_flow(&current.flow)?;
    let mut candidates: Vec<_> = factory
        .store
        .runs()?
        .into_iter()
        .filter(|run| run.event_id == current.event.event_id && run.attempt < current.run.attempt)
        .collect();
    candidates.sort_by_key(|run| std::cmp::Reverse(run.attempt));
    for run in candidates {
        let Some(proof) = factory
            .store
            .observation(&format!("release-issue-close:{}", run.id))?
        else {
            continue;
        };
        let previous = factory.store.detail(&run.id)?;
        if proof["publication_confirmed"] != true
            || proof["repository"] != current.event.repository
            || proof["number"] != current.event.number
            || proof["event_id"] != current.event.event_id
            || !proof["actor"]
                .as_str()
                .is_some_and(|actor| actor.eq_ignore_ascii_case(&current.event.actor))
            || previous.event.repository != current.event.repository
            || previous.event.number != current.event.number
            || !previous
                .event
                .actor
                .eq_ignore_ascii_case(&current.event.actor)
            || effective_release_flow(&previous.flow)? != current_flow
            || !is_publication(&previous)
            || !confirmed(&previous)
        {
            continue;
        }
        let session = factory.root.join("sessions").join(&current.run.id);
        std::fs::create_dir_all(&session)?;
        for mut node in current.nodes.clone() {
            let previous = previous
                .nodes
                .iter()
                .find(|old| old.id == node.id)
                .context("Previous release step is missing")?;
            let mut result = previous
                .result
                .clone()
                .context("Previous release result is missing")?;
            result["summary"] = json!(format!(
                "Reused successful step from run {}; retrying only release issue closure.",
                run.id
            ));
            // Keep the original evidence in its source run rather than creating
            // cross-run artifact ownership that deletion could invalidate.
            result["artifacts"] = json!([]);
            node.status = "succeeded".into();
            node.started_at = Some(now());
            node.finished_at = Some(now());
            node.error = None;
            node.result = Some(result.clone());
            factory.store.save_node(&current.run.id, &node)?;
            std::fs::write(
                session.join(format!("{}.result.json", node.id)),
                serde_json::to_vec_pretty(&result)?,
            )?;
            factory.emit("node_changed", Some(&current.run.id), Some(&node.id));
        }
        return Ok(true);
    }
    Ok(false)
}

fn effective_release_flow(flow: &Flow) -> Result<serde_json::Value> {
    // The default prompt upgrade does not change the identity of a published
    // workflow. Normalize the comparison while preserving frozen run data.
    let mut effective = flow.clone();
    refresh_release_prompt(&mut effective);
    Ok(serde_json::to_value(effective)?)
}
