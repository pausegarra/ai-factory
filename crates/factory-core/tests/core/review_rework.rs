use super::{fixture, terminal};
use factory_core::*;
use serde_json::json;
use std::time::Duration;

fn configure(factory: &Factory, agent: &str, parallel: usize, reviewers: &[&str]) {
    let mut settings = factory.settings().unwrap();
    let template = settings.flows[0].nodes[0].clone();
    let make = |id: &str, access: &str, dependencies: &[&str], review: bool| {
        let mut node = template.clone();
        node.id = id.into();
        node.name = id.into();
        node.agent = agent.into();
        node.access = access.into();
        node.depends_on = dependencies.iter().map(|s| (*s).into()).collect();
        if review {
            node.result_schema = json!({"type":"object","required":["approved"],"properties":{"approved":{"type":"boolean"}}});
        }
        node
    };
    let mut nodes = vec![
        make("spec", "write", &[], false),
        make("builder", "write", &["spec"], false),
    ];
    for reviewer in reviewers {
        nodes.push(make(reviewer, "read", &["builder"], true));
    }
    // This consumer must also wait for sibling reviewers it does not depend on.
    nodes.push(make("finish", "write", &[reviewers[0]], false));
    settings.flows[0].nodes = nodes;
    settings.flows[0].max_parallel = parallel;
    factory.save_settings(settings).unwrap();
}

fn enqueue(factory: &Factory) -> String {
    factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            91,
            "[AI] review feedback",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap()
}

const SCRIPT: &str = r#"set -eu
node="$AI_FACTORY_NODE_ID"
memory="$AI_FACTORY_MEMORY_DIR"
cat > "$memory/$node.prompt"
count=0
[ ! -f "$memory/$node.count" ] || count=$(cat "$memory/$node.count")
count=$((count + 1))
printf '%s' "$count" > "$memory/$node.count"
printf '%s:%s\n' "$node" "$count" >> "$memory/order"
data='{}'
status=succeeded
summary="$node round $count"
case "$node" in
  builder)
    if [ "$count" -gt 1 ]; then
      [ -f working.txt ] || exit 41
      grep -q 'spec round 1' "$memory/builder.prompt" || exit 42
      grep -q 'Review feedback' "$memory/builder.prompt" || exit 43
      grep -q 'builder round' "$memory/builder.prompt" || exit 44
    fi
    printf '%s' "$count" > working.txt
    ;;
  review_a|review_b|review_c)
    [ -f working.txt ] || exit 45
    data='{"approved":true}'
    if [ "$node" = review_b ] && [ "$count" -le 2 ]; then
      status=blocked
      data='{"approved":false}'
      summary="fix requested $count"
    fi
    ;;
esac
printf '{"status":"%s","summary":"%s","artifacts":[],"data":%s}' "$status" "$summary" "$data" > "$AI_FACTORY_RESULT_PATH"
"#;

#[tokio::test]
async fn rejected_reviews_return_feedback_then_recheck_until_approved() {
    for agent in ["codex", "opencode"] {
        let (dir, factory) = fixture(SCRIPT, 4);
        configure(&factory, agent, 3, &["review_b"]);
        let id = enqueue(&factory);
        factory.start();
        let detail = terminal(&factory, &id).await;
        factory.shutdown().await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
        let order = std::fs::read_to_string(memory.join("order")).unwrap();
        assert_eq!(order, "spec:1\nbuilder:1\nreview_b:1\nbuilder:2\nreview_b:2\nbuilder:3\nreview_b:3\nfinish:1\n");
        let prompt = std::fs::read_to_string(memory.join("builder.prompt")).unwrap();
        assert!(prompt.contains("fix requested 2"), "{prompt}");
        assert!(prompt.contains("builder round 2"), "{prompt}");
        assert!(detail.nodes.iter().all(|n| n.status == "succeeded"));
        assert!(detail.run.workspace.is_none());
    }
}

#[tokio::test]
async fn all_reviews_finish_before_rework_and_approved_siblings_run_again() {
    for parallel in [1, 3] {
        let script = SCRIPT.replace(
            "data='{}'",
            r#"if [ "$node" = review_c ]; then
  touch "$memory/slow-started-$count"
  while [ ! -f "$memory/release-$count" ]; do sleep 0.02; done
fi
data='{}'"#,
        );
        let (dir, factory) = fixture(&script, 4);
        configure(
            &factory,
            "codex",
            parallel,
            &["review_a", "review_b", "review_c"],
        );
        let id = enqueue(&factory);
        let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
        factory.start();
        for round in 1..=3 {
            for _ in 0..200 {
                if memory.join(format!("slow-started-{round}")).exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(
                memory.join(format!("slow-started-{round}")).exists(),
                "{:?}",
                factory.run_detail(&id).unwrap()
            );
            let detail = factory.run_detail(&id).unwrap();
            assert_eq!(detail.run.status, "running");
            assert_eq!(
                std::fs::read_to_string(memory.join("builder.count")).unwrap(),
                round.to_string()
            );
            assert!(!memory.join("finish.count").exists());
            std::fs::write(memory.join(format!("release-{round}")), "").unwrap();
        }
        let detail = terminal(&factory, &id).await;
        factory.shutdown().await;
        assert_eq!(detail.run.status, "succeeded", "{:?}", detail.nodes);
        for node in ["builder", "review_a", "review_b", "review_c"] {
            assert_eq!(
                std::fs::read_to_string(memory.join(format!("{node}.count"))).unwrap(),
                "3"
            );
        }
        let prompt = std::fs::read_to_string(memory.join("builder.prompt")).unwrap();
        for feedback in ["review_a round 2", "fix requested 2", "review_c round 2"] {
            assert!(prompt.contains(feedback), "missing {feedback}: {prompt}");
        }
    }
}

#[tokio::test]
async fn review_execution_failure_does_not_trigger_content_rework() {
    let script = SCRIPT.replace("status=blocked", "status=failed");
    let (dir, factory) = fixture(&script, 4);
    configure(&factory, "codex", 3, &["review_b"]);
    let id = enqueue(&factory);
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "failed");
    let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
    assert_eq!(
        std::fs::read_to_string(memory.join("builder.count")).unwrap(),
        "1"
    );
}

#[tokio::test]
async fn correction_cannot_reuse_previous_result_when_agent_returns_no_output() {
    let script = SCRIPT.replace(
        "printf '%s' \"$count\" > working.txt",
        "if [ \"$count\" -gt 1 ]; then exit 0; fi\nprintf '%s' \"$count\" > working.txt",
    );
    let (_dir, factory) = fixture(&script, 4);
    configure(&factory, "codex", 3, &["review_b"]);
    let id = enqueue(&factory);
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "failed", "{:?}", detail.nodes);
    let builder = detail.nodes.iter().find(|n| n.id == "builder").unwrap();
    assert!(
        builder
            .error
            .as_ref()
            .is_some_and(|e| e.contains("no valid structured final result")),
        "{builder:?}"
    );
}

#[tokio::test]
async fn cancellation_while_waiting_for_reviews_never_starts_correction() {
    let script = SCRIPT.replace(
        "data='{}'",
        "if [ \"$node\" = review_c ]; then echo WAITING; sleep 30; fi\ndata='{}'",
    );
    let (dir, factory) = fixture(&script, 4);
    configure(&factory, "codex", 3, &["review_b", "review_c"]);
    let id = enqueue(&factory);
    factory.start();
    for _ in 0..200 {
        let detail = factory.run_detail(&id).unwrap();
        if detail
            .nodes
            .iter()
            .any(|n| n.id == "review_b" && n.status == "blocked")
            && factory
                .logs(&id, "review_c", 0)
                .unwrap()
                .text
                .contains("WAITING")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    factory.cancel(&id).unwrap();
    factory.shutdown().await;
    let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
    assert_eq!(
        std::fs::read_to_string(memory.join("builder.count")).unwrap(),
        "1"
    );
    let detail = factory.run_detail(&id).unwrap();
    assert_eq!(detail.run.status, "cancelled");
    assert!(detail
        .nodes
        .iter()
        .all(|n| !["pending", "running"].contains(&n.status.as_str())));
}

#[tokio::test]
async fn failed_reviewer_prevents_rework_and_downstream_even_when_sibling_approved() {
    let script = SCRIPT.replace(
        "data='{}'",
        "if [ \"$node\" = review_c ]; then exit 19; fi\ndata='{}'",
    );
    let (dir, factory) = fixture(&script, 4);
    configure(&factory, "codex", 3, &["review_a", "review_b", "review_c"]);
    let id = enqueue(&factory);
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "failed", "{:?}", detail.nodes);
    let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
    assert_eq!(
        std::fs::read_to_string(memory.join("builder.count")).unwrap(),
        "1"
    );
    assert!(!memory.join("finish.count").exists());
}

#[tokio::test]
async fn all_authors_receive_feedback_without_rerunning_independent_work() {
    let (dir, factory) = fixture(SCRIPT, 4);
    configure(&factory, "codex", 3, &["review_b"]);
    let mut settings = factory.settings().unwrap();
    let mut designer = settings.flows[0].nodes[1].clone();
    designer.id = "designer".into();
    settings.flows[0].nodes.insert(1, designer);
    settings.flows[0]
        .nodes
        .iter_mut()
        .find(|n| n.id == "review_b")
        .unwrap()
        .depends_on
        .push("designer".into());
    let mut independent = settings.flows[0].nodes[0].clone();
    independent.id = "independent".into();
    settings.flows[0].nodes.push(independent);
    factory.save_settings(settings).unwrap();
    let id = enqueue(&factory);
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "succeeded", "{:?}", detail.nodes);
    let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
    for node in ["builder", "designer"] {
        assert_eq!(
            std::fs::read_to_string(memory.join(format!("{node}.count"))).unwrap(),
            "3"
        );
        let prompt = std::fs::read_to_string(memory.join(format!("{node}.prompt"))).unwrap();
        assert!(prompt.contains("fix requested 2"), "{prompt}");
        assert!(prompt.contains(&format!("{node} round 2")), "{prompt}");
    }
    for node in ["spec", "independent", "finish"] {
        assert_eq!(
            std::fs::read_to_string(memory.join(format!("{node}.count"))).unwrap(),
            "1"
        );
    }
}

#[tokio::test]
async fn plain_blocked_node_still_stops_without_review_feedback() {
    let script = SCRIPT.replace("status=succeeded", "status=blocked");
    let (dir, factory) = fixture(&script, 4);
    configure(&factory, "codex", 3, &["review_b"]);
    let id = enqueue(&factory);
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "blocked");
    let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
    assert_eq!(
        std::fs::read_to_string(memory.join("spec.count")).unwrap(),
        "1"
    );
    assert!(!memory.join("builder.count").exists());
}

#[tokio::test]
async fn approval_result_without_custom_schema_also_returns_work_to_author() {
    let (dir, factory) = fixture(SCRIPT, 4);
    configure(&factory, "codex", 3, &["review_b"]);
    let mut settings = factory.settings().unwrap();
    settings.flows[0]
        .nodes
        .iter_mut()
        .find(|n| n.id == "review_b")
        .unwrap()
        .result_schema = json!({"type":"object"});
    factory.save_settings(settings).unwrap();
    let id = enqueue(&factory);
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "succeeded", "{:?}", detail.nodes);
    let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
    assert_eq!(
        std::fs::read_to_string(memory.join("builder.count")).unwrap(),
        "3"
    );
}

#[tokio::test]
async fn correction_history_retains_review_results_and_archived_artifacts() {
    use sha2::{Digest, Sha256};
    let hash = format!("{:x}", Sha256::digest(b"retained evidence"));
    let script = SCRIPT
        .replace("data='{}'", "artifacts='[]'\ndata='{}'")
        .replace(
            "printf '%s' \"$count\" > working.txt",
            &format!(
                r#"printf '%s' "$count" > working.txt
    printf '%s' 'retained evidence' > report.md
    artifacts='[{{"kind":"plan","path":"report.md","sha256":"{hash}"}}]'"#
            ),
        )
        .replace("\"artifacts\":[]", "\"artifacts\":%s")
        .replace(
            "\"$summary\" \"$data\"",
            "\"$summary\" \"$artifacts\" \"$data\"",
        );
    let (dir, factory) = fixture(&script, 4);
    configure(&factory, "codex", 3, &["review_b"]);
    let id = enqueue(&factory);
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "succeeded", "{:?}", detail.nodes);
    let db = rusqlite::Connection::open(dir.path().join("data/factory.sqlite3")).unwrap();
    let mut query = db
        .prepare("SELECT json FROM observations WHERE key LIKE ?1")
        .unwrap();
    let history = query
        .query_map([format!("review-history:{id}:%")], |r| {
            r.get::<_, String>(0)
        })
        .unwrap()
        .map(|row| serde_json::from_str::<serde_json::Value>(&row.unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(history.len(), 2);
    for round in history {
        assert_eq!(round["feedback"][0]["result"]["data"]["approved"], false);
        let builder = round["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == "builder")
            .unwrap();
        let artifact = &builder["result"]["artifacts"][0];
        assert_eq!(artifact["sha256"], hash);
        assert_eq!(
            std::fs::read_to_string(artifact["path"].as_str().unwrap()).unwrap(),
            "retained evidence"
        );
    }
    assert!(!dir.path().join("data/workspaces").join(&id).exists());
}

#[tokio::test]
async fn pending_reviewer_without_schema_holds_consumers_in_every_round() {
    let script = SCRIPT.replace(
        "data='{}'",
        r#"if [ "$node" = review_c ]; then
  touch "$memory/slow-started-$count"
  while [ ! -f "$memory/release-$count" ]; do sleep 0.02; done
fi
data='{}'"#,
    );
    let (dir, factory) = fixture(&script, 4);
    configure(&factory, "codex", 3, &["review_a", "review_b", "review_c"]);
    let mut settings = factory.settings().unwrap();
    for node in settings.flows[0]
        .nodes
        .iter_mut()
        .filter(|n| n.id.starts_with("review_"))
    {
        node.result_schema = json!({"type":"object"});
    }
    factory.save_settings(settings).unwrap();
    let id = enqueue(&factory);
    let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
    factory.start();
    for round in 1..=3 {
        for _ in 0..200 {
            let detail = factory.run_detail(&id).unwrap();
            if memory.join(format!("slow-started-{round}")).exists()
                && detail
                    .nodes
                    .iter()
                    .any(|n| n.id == "review_a" && n.status == "succeeded")
                && detail.nodes.iter().any(|n| {
                    n.id == "review_b" && ["succeeded", "blocked"].contains(&n.status.as_str())
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Allow scheduling after the fast reviewers have persisted their result.
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(
            !memory.join("finish.count").exists(),
            "consumer ran before all reviews finished in round {round}"
        );
        assert_eq!(
            std::fs::read_to_string(memory.join("builder.count")).unwrap(),
            round.to_string()
        );
        std::fs::write(memory.join(format!("release-{round}")), "").unwrap();
    }
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "succeeded", "{:?}", detail.nodes);
}

#[tokio::test]
async fn chained_reviews_of_shared_author_return_work_without_skipping_next_stage() {
    let (dir, factory) = fixture(SCRIPT, 4);
    configure(&factory, "codex", 3, &["review_b", "review_c"]);
    let mut settings = factory.settings().unwrap();
    settings.flows[0]
        .nodes
        .iter_mut()
        .find(|n| n.id == "review_c")
        .unwrap()
        .depends_on
        .push("review_b".into());
    settings.flows[0]
        .nodes
        .iter_mut()
        .find(|n| n.id == "finish")
        .unwrap()
        .depends_on = vec!["review_c".into()];
    factory.save_settings(settings).unwrap();
    let id = enqueue(&factory);
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "succeeded", "{:?}", detail.nodes);
    let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
    assert_eq!(
        std::fs::read_to_string(memory.join("review_c.count")).unwrap(),
        "1"
    );
    assert_eq!(
        std::fs::read_to_string(memory.join("builder.count")).unwrap(),
        "3"
    );
}

#[tokio::test]
async fn readonly_sibling_blocked_without_rejection_does_not_loop_author() {
    let script = SCRIPT.replace("data='{}'", r#"if [ "$node" = inspect ]; then
  printf '%s' '{"status":"blocked","summary":"Missing credentials","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH"
  exit 0
fi
data='{}'"#);
    let (dir, factory) = fixture(&script, 4);
    configure(&factory, "codex", 3, &["review_a"]);
    let mut settings = factory.settings().unwrap();
    let mut inspect = settings.flows[0]
        .nodes
        .iter()
        .find(|n| n.id == "review_a")
        .unwrap()
        .clone();
    inspect.id = "inspect".into();
    inspect.result_schema = json!({"type":"object"});
    settings.flows[0].nodes.push(inspect);
    factory.save_settings(settings).unwrap();
    let id = enqueue(&factory);
    let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
    factory.start();
    // Stop observing as soon as incorrect rework starts; avoid an infinite fixture.
    for _ in 0..200 {
        let status = factory.run_detail(&id).unwrap().run.status;
        let count = std::fs::read_to_string(memory.join("builder.count")).unwrap_or_default();
        if !["queued", "running"].contains(&status.as_str()) || (!count.is_empty() && count != "1")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let detail = factory.run_detail(&id).unwrap();
    factory.shutdown().await;
    assert_eq!(detail.run.status, "blocked", "{:?}", detail.nodes);
    assert_eq!(
        std::fs::read_to_string(memory.join("builder.count")).unwrap(),
        "1"
    );
    assert!(!memory.join("finish.count").exists());
}
