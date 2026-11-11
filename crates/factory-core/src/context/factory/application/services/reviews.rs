use crate::{Flow, NodeRun};
use serde_json::{json, Value};
use std::collections::HashSet;

pub(crate) struct ReviewRework {
    pub authors: HashSet<String>,
    pub affected: HashSet<String>,
    pub feedback: Vec<Value>,
}

#[derive(Default)]
pub(crate) struct ReviewDecision {
    pub held: HashSet<String>,
    pub skipped: HashSet<String>,
    pub rework: Vec<ReviewRework>,
}

struct Batch {
    authors: HashSet<String>,
    reviewers: HashSet<String>,
}

fn descendants(flow: &Flow, roots: &HashSet<String>) -> HashSet<String> {
    let mut affected = roots.clone();
    loop {
        let previous = affected.len();
        for node in &flow.nodes {
            if node.depends_on.iter().any(|id| affected.contains(id)) {
                affected.insert(node.id.clone());
            }
        }
        if previous == affected.len() {
            return affected;
        }
    }
}

/// Approval reviews form a barrier, even for consumers depending on only one
/// reviewer. Reviewers sharing authors belong to a batch when neither depends
/// on the other; dependent reviews remain separate stages of the DAG.
pub(crate) fn decide(
    flow: &Flow,
    states: &[NodeRun],
    known_reviewers: &mut HashSet<String>,
) -> ReviewDecision {
    for node in &flow.nodes {
        let result = states
            .iter()
            .find(|n| n.id == node.id)
            .and_then(|n| n.result.as_ref());
        if node.result_schema["properties"]["approved"]["type"] == "boolean"
            || result.is_some_and(|v| v["data"]["approved"].is_boolean())
        {
            known_reviewers.insert(node.id.clone());
        }
    }
    // Legacy/custom flows may omit the approval schema. Once one review is
    // identified, wait for all read-only siblings sharing its work, including
    // those still running. Remember identities when resets clear their results.
    loop {
        let previous = known_reviewers.len();
        for node in &flow.nodes {
            if node.access == "read"
                && flow.nodes.iter().any(|review| {
                    known_reviewers.contains(&review.id)
                        && node
                            .depends_on
                            .iter()
                            .any(|id| review.depends_on.contains(id))
                })
            {
                known_reviewers.insert(node.id.clone());
            }
        }
        if previous == known_reviewers.len() {
            break;
        }
    }
    let mut batches: Vec<Batch> = vec![];
    for node in &flow.nodes {
        if !known_reviewers.contains(&node.id) || node.depends_on.is_empty() {
            continue;
        }
        let mut batch = Batch {
            authors: node.depends_on.iter().cloned().collect(),
            reviewers: HashSet::from([node.id.clone()]),
        };
        // Merge connected groups, including bridges between existing groups.
        let mut index = 0;
        while index < batches.len() {
            if !batch.authors.is_disjoint(&batches[index].authors)
                && descendants(flow, &batch.reviewers).is_disjoint(&batches[index].reviewers)
                && descendants(flow, &batches[index].reviewers).is_disjoint(&batch.reviewers)
            {
                let other = batches.remove(index);
                batch.authors.extend(other.authors);
                batch.reviewers.extend(other.reviewers);
                index = 0;
            } else {
                index += 1;
            }
        }
        batches.push(batch);
    }

    let mut decision = ReviewDecision::default();
    for batch in batches {
        if !batch.authors.iter().all(|id| {
            states
                .iter()
                .any(|n| &n.id == id && n.status == "succeeded")
        }) {
            continue;
        }
        let reviews: Vec<_> = states
            .iter()
            .filter(|n| batch.reviewers.contains(&n.id))
            .collect();
        let downstream: HashSet<_> = descendants(flow, &batch.reviewers)
            .difference(&batch.reviewers)
            .cloned()
            .collect();
        if reviews
            .iter()
            .any(|n| ["pending", "running"].contains(&n.status.as_str()))
        {
            decision.held.extend(downstream);
        } else if reviews.iter().any(|n| {
            n.status != "succeeded"
                && !(n.status == "blocked"
                    && n.result
                        .as_ref()
                        .is_some_and(|v| v["data"]["approved"] == false))
        }) {
            // Waiting for read-only siblings does not turn operational blockers
            // into content rejection. Only an explicit negative approval retries.
            decision.skipped.extend(downstream);
        } else if reviews.iter().any(|n| n.status == "blocked") {
            decision.held.extend(downstream);
            decision.rework.push(ReviewRework {
                affected: descendants(flow, &batch.authors),
                authors: batch.authors,
                feedback: reviews
                    .iter()
                    .map(|n| json!({"id":n.id,"result":n.result}))
                    .collect(),
            });
        }
    }
    decision
}
