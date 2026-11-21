use super::{fixture, terminal};
use crate::common;
use factory_core::{Factory, RunDetail, TriggerEvent};
use serde_json::{json, Value};
use std::path::Path;

const AGENT: &str = r#"
if [ "$1" = app-server ]; then
  printf 'discovered\n' >> "$(dirname "$0")/skill-discovery.calls"
  exit 87
fi
if [ "${AI_FACTORY_RESPONSE_MODE:-initial}" = repair ]; then
  printf 'corrected\n' >> "$(dirname "$0")/skill-correction.calls"
else
  printf 'executed\n' >> "$(dirname "$0")/skill-execution.calls"
fi
cat > "${AI_FACTORY_RESULT_PATH%.result.json}.stdin.txt"
printf '%s\n' "$@" > "${AI_FACTORY_RESULT_PATH%.result.json}.args.txt"
printf '%s' "$OPENCODE_CONFIG_CONTENT" > "${AI_FACTORY_RESULT_PATH%.result.json}.config.json"
printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH"
if [ "$AI_FACTORY_NODE_ID" = publish_app ]; then
  printf '%s' '{"status":"succeeded","summary":"published","artifacts":[],"data":{"distribution_result":{"published":true}}}' > "$AI_FACTORY_RESULT_PATH"
fi
"#;

async fn run(factory: &std::sync::Arc<Factory>, number: u64) -> RunDetail {
    let id = factory
        .enqueue(common::from_source(
            TriggerEvent::issue("acme/repo", number, "[AI] skills", "alice", ""),
            "alice",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    let detail = terminal(factory, &id).await;
    factory.shutdown().await;
    detail
}

fn capture(root: &Path, run: &str, extension: &str) -> String {
    std::fs::read_to_string(
        root.join("data/sessions")
            .join(run)
            .join(format!("spec.{extension}")),
    )
    .unwrap()
}

fn json_member<'a>(object: &'a str, field: &str) -> &'a str {
    let mut rest = object.trim_start().strip_prefix('{').unwrap().trim_start();
    loop {
        let mut key = serde_json::Deserializer::from_str(rest).into_iter::<String>();
        let name = key
            .next()
            .expect("object member")
            .expect("JSON member name");
        let input = rest[key.byte_offset()..]
            .trim_start()
            .strip_prefix(':')
            .expect("JSON member separator")
            .trim_start();
        let mut value = serde_json::Deserializer::from_str(input).into_iter::<Value>();
        value
            .next()
            .expect("object value")
            .expect("JSON member value");
        let end = value.byte_offset();
        if name == field {
            return &input[..end];
        }
        rest = input[end..]
            .trim_start()
            .strip_prefix(',')
            .unwrap_or_else(|| panic!("missing JSON member {field}"))
            .trim_start();
    }
}

fn opencode_permission_bytes(config: &str, memory: &Path) -> Value {
    let agent = json_member(json_member(config, "agent"), "ai-factory");
    let memory_rule = serde_json::to_string(&format!("{}/**", memory.display())).unwrap();
    let normalize = |raw: &str| {
        assert!(raw.starts_with('{') && raw.ends_with('}'));
        let external = json_member(raw, "external_directory");
        assert_eq!(
            external.matches(memory_rule.as_str()).count(),
            1,
            "{external}"
        );
        assert_eq!(raw.matches(memory_rule.as_str()).count(), 1, "{raw}");
        raw.replacen(memory_rule.as_str(), r#""__FACTORY_RUN_MEMORY__/**""#, 1)
    };
    json!({
        "global": normalize(json_member(config, "permission")),
        "agent": normalize(json_member(agent, "permission")),
    })
}

pub(super) fn assert_opencode_permissions(access: &str, config: &str, memory: &Path) {
    let baseline: Value = serde_json::from_str(include_str!(
        "fixtures/opencode_no_selection_permissions.json"
    ))
    .unwrap();
    let actual = opencode_permission_bytes(config, memory);
    for layer in ["global", "agent"] {
        assert_eq!(
            actual[layer].as_str().unwrap(),
            baseline[access][layer].as_str().unwrap(),
            "permission bytes changed for {access}/{layer}"
        );
    }
}

#[test]
fn opencode_permission_comparison_detects_rule_reordering() {
    let reference: Value = serde_json::from_str(include_str!(
        "fixtures/opencode_no_selection_permissions.json"
    ))
    .unwrap();
    let memory = Path::new("__FACTORY_RUN_MEMORY__");
    let reorder = |object: &str, left: &str, right: &str| {
        let member = |key| {
            format!(
                "{}:{}",
                serde_json::to_string(key).unwrap(),
                json_member(object, key)
            )
        };
        let left = member(left);
        let right = member(right);
        let mut replacements = object
            .match_indices(&left)
            .take(1)
            .map(|(index, _)| (index, &left, &right))
            .chain(
                object
                    .match_indices(&right)
                    .take(1)
                    .map(|(index, _)| (index, &right, &left)),
            )
            .collect::<Vec<_>>();
        assert_eq!(replacements.len(), 2);
        replacements.sort_unstable_by_key(|(index, _, _)| std::cmp::Reverse(*index));
        let mut changed = object.to_owned();
        for (at, from, to) in replacements {
            changed.replace_range(at..at + from.len(), to);
        }
        changed
    };
    let parse = |wire: &str| serde_json::from_str::<Value>(wire).unwrap();
    for access in ["read", "write"] {
        let global = reference[access]["global"].as_str().unwrap();
        let agent = reference[access]["agent"].as_str().unwrap();
        let config = |global: &str, agent: &str| {
            format!(
                r#"{{"permission":{global},"agent":{{"ai-factory":{{"permission":{agent}}}}}}}"#
            )
        };
        let original = config(global, agent);
        for layer in ["global", "agent"] {
            let raw = reference[access][layer].as_str().unwrap();
            let external = json_member(raw, "external_directory");
            let reversed_directory = reorder(external, "*", "__FACTORY_RUN_MEMORY__/**");
            for reordered in [
                reorder(raw, "question", "external_directory"),
                raw.replacen(external, &reversed_directory, 1),
            ] {
                let mutated = if layer == "global" {
                    config(&reordered, agent)
                } else {
                    config(global, &reordered)
                };
                assert_eq!(parse(&original), parse(&mutated));
                let actual = opencode_permission_bytes(&mutated, memory);
                assert_ne!(actual[layer], reference[access][layer], "{access}/{layer}");
                let other = if layer == "global" { "agent" } else { "global" };
                assert_eq!(actual[other], reference[access][other], "{access}/{other}");
            }
        }
    }
}

#[tokio::test]
async fn legacy_snapshot_codex_removed_id() {
    assert_legacy_snapshot_runs(
        "codex",
        "read",
        json!(["factory-skill-no-longer-installed"]),
    )
    .await;
}

#[tokio::test]
async fn legacy_snapshot_codex_empty_list() {
    assert_legacy_snapshot_runs("codex", "write", json!([])).await;
}

#[tokio::test]
async fn legacy_snapshot_opencode_removed_id() {
    assert_legacy_snapshot_runs(
        "opencode",
        "read",
        json!(["factory-skill-no-longer-installed"]),
    )
    .await;
}

#[tokio::test]
async fn legacy_snapshot_opencode_empty_list() {
    assert_legacy_snapshot_runs("opencode", "write", json!([])).await;
}

async fn assert_legacy_snapshot_runs(agent: &str, access: &str, legacy: Value) {
    let (root, factory) = fixture(
        &AGENT.replace(r#""data":{}"#, r#""data":{"verified":true}"#),
        4,
    );
    let mut settings = factory.settings().unwrap();
    let node = &mut settings.flows[0].nodes[0];
    node.agent = agent.into();
    node.access = access.into();
    node.prompt = "NATIVE_SKILLS_PROMPT_SENTINEL".into();
    node.result_schema = json!({"type":"object","properties":{"verified":{"type":"boolean"}},"required":["verified"]});
    factory.save_settings(settings).unwrap();
    let id = factory
        .enqueue(common::from_source(
            TriggerEvent::issue("acme/repo", 213, "[AI] legacy snapshot", "alice", ""),
            "alice",
        ))
        .unwrap()
        .unwrap();

    let db = rusqlite::Connection::open(root.path().join("data/factory.sqlite3")).unwrap();
    let raw: String = db
        .query_row("SELECT flow FROM runs WHERE id=?1", [&id], |row| row.get(0))
        .unwrap();
    let mut wire: Value = serde_json::from_str(&raw).unwrap();
    let removed_id = legacy.get(0).and_then(Value::as_str).map(str::to_owned);
    wire["nodes"][0]["skills"] = legacy;
    let legacy_snapshot = wire.to_string();
    assert_eq!(
        db.execute(
            "UPDATE runs SET flow=?1 WHERE id=?2",
            rusqlite::params![&legacy_snapshot, &id],
        )
        .unwrap(),
        1,
    );
    drop(db);

    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    let launches = |marker| {
        std::fs::read_to_string(root.path().join(marker))
            .unwrap_or_default()
            .lines()
            .count()
    };
    eprintln!(
        "{agent}/{access} error={:?}; work={}, catalog={}, corrections={}",
        detail.nodes[0].error,
        launches("skill-execution.calls"),
        launches("skill-discovery.calls"),
        launches("skill-correction.calls")
    );

    if let Some(removed_id) = removed_id
        .as_deref()
        .filter(|_| detail.run.status == "failed")
    {
        let expected = format!(
            "Selected skill is not installed: {removed_id}. Update this node's skill selection."
        );
        assert!(
            detail.nodes[0]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains(&expected),
            "{agent}/{access}: {:?}",
            detail.nodes
        );
        for marker in ["skill-execution.calls", "skill-discovery.calls"] {
            assert!(
                !root.path().join(marker).exists(),
                "unexpected launch: {marker}"
            );
        }
    }
    assert_eq!(
        detail.run.status, "succeeded",
        "{agent}/{access}: {:?}",
        detail.nodes
    );
    assert_eq!(
        detail.nodes[0].result.as_ref().unwrap()["data"]["verified"],
        true
    );
    let prompt = capture(root.path(), &id, "stdin.txt");
    let args = capture(root.path(), &id, "args.txt");
    let config_text = capture(root.path(), &id, "config.json");
    if agent == "codex" {
        assert!(
            !args
                .lines()
                .any(|arg| arg == "--strict-config" || arg.starts_with("skills.")),
            "Codex work launch still filters legacy skills: {args}"
        );
    } else {
        let emitted: Value = serde_json::from_str(&config_text).unwrap();
        let global = &emitted["permission"];
        let local = &emitted["agent"]["ai-factory"]["permission"];
        assert!(
            global.get("skill").is_none() && local.get("skill").is_none(),
            "OpenCode work launch still filters legacy skills: {:?} / {:?}",
            global.get("skill"),
            local.get("skill")
        );
    }
    assert!(prompt.contains("NATIVE_SKILLS_PROMPT_SENTINEL"));
    for removed in [
        "AI Factory skill selection",
        "No skills are enabled for this node",
        "factory-skill-no-longer-installed",
    ] {
        assert!(
            !prompt.contains(removed),
            "legacy selection leaked: {removed}"
        );
    }

    assert!(prompt.contains("Single-work-execution output contract"));
    assert_eq!(launches("skill-execution.calls"), 1);
    assert_eq!(launches("skill-discovery.calls"), 0);
    assert_eq!(launches("skill-correction.calls"), 0);
    assert!(serde_json::to_value(&detail.flow).unwrap()["nodes"][0]
        .get("skills")
        .is_none());
    let db = rusqlite::Connection::open(root.path().join("data/factory.sqlite3")).unwrap();
    let stored: String = db
        .query_row("SELECT flow FROM runs WHERE id=?1", [&id], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, legacy_snapshot);

    if agent == "codex" {
        assert!(args.lines().any(|line| line == "--output-schema"));
        assert!(args.lines().any(|line| line == "approval_policy=\"never\""));
        let sandbox = if access == "read" {
            "read-only"
        } else {
            "workspace-write"
        };
        assert!(args.lines().any(|line| line == sandbox));
    } else {
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            [
                "run",
                "--format",
                "json",
                "--standalone",
                "--auto",
                "--agent",
                "ai-factory"
            ],
        );
        let config: Value = serde_json::from_str(&config_text).unwrap();
        let memory = root
            .path()
            .canonicalize()
            .unwrap()
            .join("data/memory/acme/repo/runs")
            .join(&id);
        assert_opencode_permissions(access, &config_text, &memory);
        assert_eq!(config["agent"]["ai-factory"]["mode"], "primary");
        assert!(config["agent"]["ai-factory"]["prompt"]
            .as_str()
            .unwrap()
            .contains("final response formatting"));
        for key in ["plugin", "plugins", "skills"] {
            assert!(config.get(key).is_none(), "{key}");
        }
    }
}

#[tokio::test]
async fn output_correction_does_not_repeat_work_or_skill_discovery() {
    let success = r#"printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH""#;
    let repair = r#"
base="${AI_FACTORY_RESULT_PATH%.result.json}"
if [ "${AI_FACTORY_RESPONSE_MODE:-initial}" != repair ]; then
  printf '%s' '{"status":"invalid","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH"
else
  cp "$base.args.txt" "$(dirname "$0")/correction.args.txt"
  cp "$base.config.json" "$(dirname "$0")/correction.config.json"
  printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH"
fi
"#;
    for agent in ["codex", "opencode"] {
        let (root, factory) = fixture(&AGENT.replace(success, repair), 4);
        let mut settings = factory.settings().unwrap();
        settings.flows[0].nodes[0].agent = agent.into();
        factory.save_settings(settings).unwrap();
        let detail = run(&factory, 215).await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("skill-execution.calls"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("skill-correction.calls"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert!(!root.path().join("skill-discovery.calls").exists());
        if agent == "codex" {
            let args = std::fs::read_to_string(root.path().join("correction.args.txt")).unwrap();
            assert!(args.contains("features.shell_tool=false"));
            assert!(args.contains("skills.include_instructions=false"));
            assert!(!args.contains("skills.config="));
        } else {
            let config: Value = serde_json::from_slice(
                &std::fs::read(root.path().join("correction.config.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(config["permission"]["*"], "deny");
            assert_eq!(
                config["agent"]["ai-factory-response-repair"]["tools"]["*"],
                false
            );
            assert!(!config.to_string().contains("allow"));
        }
    }
}

async fn run_release(factory: &std::sync::Arc<Factory>) -> RunDetail {
    let id = factory
        .enqueue(common::from_source(
            TriggerEvent::release_issue("acme/repo", 811, "[RELEASE] fixture", "alice", ""),
            "alice",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    let detail = terminal(factory, &id).await;
    factory.shutdown().await;
    detail
}
fn configure_release(root: &super::FixtureDir, factory: &Factory, agent: &str) {
    *root.issue.lock().unwrap() = json!({"number":811,"html_url":"https://github.com/acme/repo/issues/811","state":"open","title":"[RELEASE] fixture","user":{"login":"alice"}});
    let mut settings = factory.settings().unwrap();
    settings.issue_node_comments_enabled = false;
    for flow in &mut settings.flows {
        flow.enabled = flow.id == "release";
    }
    let node = &mut settings
        .flows
        .iter_mut()
        .find(|f| f.id == "release")
        .unwrap()
        .nodes[0];
    node.agent = agent.into();
    factory.save_settings(settings).unwrap();
}

#[tokio::test]
async fn release_uses_native_discovery_in_one_editable_step() {
    for agent in ["codex", "opencode"] {
        let (root, factory) = fixture(AGENT, 4);
        configure_release(&root, &factory, agent);
        let detail = run_release(&factory).await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.run.error
        );
        assert_eq!(detail.nodes.len(), 1);
        assert_eq!(detail.nodes[0].id, "publish_app");
        let prompt = std::fs::read_to_string(
            root.path()
                .join("data/sessions")
                .join(&detail.run.id)
                .join("publish_app.stdin.txt"),
        )
        .unwrap();
        assert!(!prompt.contains("AI Factory skill selection"));
        assert!(!root.path().join("skill-discovery.calls").exists());
        assert!(prompt.contains("Adapt or omit pipeline phases"));
        assert!(prompt.contains("return blocked"));
        assert!(prompt.contains("Release publication authorization"));
        assert_eq!(root.issue.lock().unwrap()["state"], "closed");
        let db = rusqlite::Connection::open(root.path().join("data/factory.sqlite3")).unwrap();
        let receipt: String = db
            .query_row(
                "SELECT json FROM observations WHERE key=?1",
                [format!("release-issue-close:{}", detail.run.id)],
                |row| row.get(0),
            )
            .unwrap();
        let receipt: Value = serde_json::from_str(&receipt).unwrap();
        assert_eq!(receipt["state_reason"], "completed");
        assert_eq!(receipt["number"], 811);
        assert_eq!(
            root.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, path, body)| method == "PATCH"
                    && path == "/repos/acme/repo/issues/811"
                    && body == &json!({"state":"closed","state_reason":"completed"}))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn release_revalidates_fresh_issue_before_any_agent_execution() {
    for change in ["closed", "title", "author", "pr"] {
        let (root, factory) = fixture(AGENT, 4);
        configure_release(&root, &factory, "codex");
        let id = factory
            .enqueue(common::from_source(
                TriggerEvent::release_issue("acme/repo", 811, "[RELEASE] fixture", "alice", ""),
                "alice",
            ))
            .unwrap()
            .unwrap();
        {
            let mut issue = root.issue.lock().unwrap();
            match change {
                "closed" => issue["state"] = json!("closed"),
                "title" => issue["title"] = json!("[AI] changed"),
                "author" => issue["user"]["login"] = json!("bob"),
                _ => issue["pull_request"] = json!({}),
            }
        }
        factory.start();
        let detail = terminal(&factory, &id).await;
        factory.shutdown().await;
        assert_eq!(detail.run.status, "failed", "{change}");
        assert!(detail
            .run
            .error
            .unwrap()
            .contains("Issue no longer eligible"));
        assert!(!root.path().join("skill-execution.calls").exists());
    }
}

#[tokio::test]
async fn release_missing_prerequisites_block_and_reserved_release_action_fails() {
    for (status, data, want) in [
        (
            "blocked",
            json!({"verification":{"reason":"fixture prerequisite absent"}}),
            "blocked",
        ),
        ("succeeded", json!({"release":{}}), "failed"),
    ] {
        let script = format!(
            "cat >/dev/null\nprintf '%s' '{}' > \"$AI_FACTORY_RESULT_PATH\"",
            json!({"status":status,"summary":"Missing distribution credentials","artifacts":[],"data":data})
        );
        let (root, factory) = fixture(&script, 4);
        configure_release(&root, &factory, "codex");
        let detail = run_release(&factory).await;
        assert_eq!(detail.run.status, want, "{:?}", detail.nodes);
        if status == "blocked" {
            assert_eq!(
                detail.nodes[0].result.as_ref().unwrap()["summary"],
                "Missing distribution credentials"
            );
        } else {
            assert!(detail.nodes[0]
                .error
                .as_deref()
                .unwrap()
                .contains("Release actions are no longer supported"));
        }
        assert!(root
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|(method, _, _)| method == "GET"));
    }
}

const PUBLICATION_PROBE: &str = r#"
base="${AI_FACTORY_RESULT_PATH%.result.json}"
actor=false
if [ "${GH_TOKEN:-}" = dummy-token ] && [ "${GH_REPO:-}" = acme/repo ] && [ "${GH_HOST:-}" = github.com ]; then actor=true; fi
git_auth=false
if [ "$actor" = true ]; then
  credentials=$(printf 'protocol=https\nhost=github.com\npath=acme/repo\n\n' | git credential fill)
  case "$credentials" in *"username=x-access-token"*"password=$GH_TOKEN") git_auth=true ;; esac
  printf 'publication-token:%s\n' "$GH_TOKEN"
fi
printf '{"actor":%s,"git_auth":%s}' "$actor" "$git_auth" > "$base.auth.json"
"#;

#[tokio::test]
async fn release_publication_allows_tag_pipeline_with_actor_auth_for_both_agents() {
    for agent in ["codex", "opencode"] {
        let script = format!("{AGENT}\n{PUBLICATION_PROBE}");
        let (root, factory) = fixture(&script, 4);
        configure_release(&root, &factory, agent);
        let detail = run_release(&factory).await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        let session = root.path().join("data/sessions").join(&detail.run.id);
        let prompt = std::fs::read_to_string(session.join("publish_app.stdin.txt")).unwrap();
        assert!(
            prompt.contains("Release publication authorization"),
            "{agent}: missing permission"
        );
        assert!(!prompt.contains("Never use gh CLI."));
        assert!(!prompt.contains("Never create tags or GitHub Releases"));
        assert!(prompt.contains("GitHub Actions secrets"));
        assert!(prompt.contains("Never force-push"));
        let auth: Value =
            serde_json::from_slice(&std::fs::read(session.join("publish_app.auth.json")).unwrap())
                .unwrap();
        assert_eq!(auth, json!({"actor":true,"git_auth":true}), "{agent}");
        let args = std::fs::read_to_string(session.join("publish_app.args.txt")).unwrap();
        assert!(!args.contains("dummy-token"));
        assert!(!prompt.contains("dummy-token"));
        if agent == "codex" {
            assert!(args.contains("sandbox_workspace_write.network_access=true"));
            assert!(args.contains("shell_environment_policy.ignore_default_excludes=true"));
            assert!(args.contains("shell_environment_policy.include_only="));
        } else {
            let config = std::fs::read_to_string(session.join("publish_app.config.json")).unwrap();
            assert!(config.contains("Release publication authorization"));
            assert!(!config.contains("Never use gh CLI."));
            assert!(!config.contains("dummy-token"));
        }
        let log = factory.logs(&detail.run.id, "publish_app", 0).unwrap().text;
        assert!(log.contains("publication-token:[REDACTED]"), "{log}");
        assert!(!log.contains("dummy-token"));
    }
}

#[tokio::test]
async fn release_publication_permissions_do_not_reach_readonly_or_other_roles() {
    for (trigger, node_id, access) in [
        ("release", "publish_app", "read"),
        ("release", "inspect", "write"),
        ("new_issue", "publish_app", "write"),
    ] {
        for agent in ["codex", "opencode"] {
            let script = format!("{AGENT}\n{PUBLICATION_PROBE}");
            let (root, factory) = fixture(&script, 4);
            configure_release(&root, &factory, agent);
            let mut settings = factory.settings().unwrap();
            for flow in &mut settings.flows {
                flow.enabled = flow.id == trigger;
            }
            let flow = settings.flows.iter_mut().find(|f| f.id == trigger).unwrap();
            flow.nodes[0].id = node_id.into();
            flow.nodes[0].access = access.into();
            flow.nodes[0].agent = agent.into();
            factory.save_settings(settings).unwrap();
            let event = if trigger == "release" {
                common::from_source(
                    TriggerEvent::release_issue("acme/repo", 811, "[RELEASE] fixture", "alice", ""),
                    "alice",
                )
            } else {
                *root.issue.lock().unwrap() =
                    json!({"state":"open","title":"[AI] work","user":{"login":"alice"}});
                common::from_source(
                    TriggerEvent::issue("acme/repo", 811, "[AI] work", "alice", ""),
                    "alice",
                )
            };
            let id = factory.enqueue(event).unwrap().unwrap();
            factory.start();
            let detail = terminal(&factory, &id).await;
            factory.shutdown().await;
            assert_eq!(
                detail.run.status, "succeeded",
                "{trigger}/{node_id}/{access}/{agent}: {:?}",
                detail.nodes
            );
            let session = root.path().join("data/sessions").join(id);
            let prompt =
                std::fs::read_to_string(session.join(format!("{node_id}.stdin.txt"))).unwrap();
            assert!(prompt.contains("Never use gh CLI."));
            assert!(!prompt.contains("Release publication authorization"));
            let auth: Value = serde_json::from_slice(
                &std::fs::read(session.join(format!("{node_id}.auth.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(auth, json!({"actor":false,"git_auth":false}));
            let args =
                std::fs::read_to_string(session.join(format!("{node_id}.args.txt"))).unwrap();
            assert!(!args.contains("sandbox_workspace_write.network_access=true"));
        }
    }
}

#[tokio::test]
async fn release_issue_closure_failure_preserves_publication_and_reports_error() {
    let (root, factory) = fixture(AGENT, 4);
    configure_release(&root, &factory, "codex");
    root.issue_close_status
        .store(403, std::sync::atomic::Ordering::SeqCst);
    let detail = run_release(&factory).await;
    assert_eq!(detail.run.status, "failed");
    assert!(detail
        .run
        .error
        .as_deref()
        .unwrap()
        .contains("Publication succeeded, but closing release issue failed"));
    assert_eq!(detail.nodes[0].status, "succeeded");
    assert!(detail.nodes[0].result.is_some());
    assert_eq!(root.issue.lock().unwrap()["state"], "open");
}

#[tokio::test]
async fn release_issue_closure_reconciles_uncertain_response_without_second_write() {
    let (root, factory) = fixture(AGENT, 4);
    configure_release(&root, &factory, "codex");
    root.issue_close_status
        .store(500, std::sync::atomic::Ordering::SeqCst);
    let detail = run_release(&factory).await;
    assert_eq!(detail.run.status, "succeeded", "{:?}", detail.run.error);
    assert_eq!(root.issue.lock().unwrap()["state"], "closed");
    assert_eq!(
        root.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, _, _)| method == "PATCH")
            .count(),
        1
    );
}

#[tokio::test]
async fn release_issue_closure_waits_for_all_workflow_steps() {
    let script = AGENT.replace("cat > \"${AI_FACTORY_RESULT_PATH%.result.json}.stdin.txt\"", "cat > \"${AI_FACTORY_RESULT_PATH%.result.json}.stdin.txt\"\nif [ \"$AI_FACTORY_NODE_ID\" = verify ]; then\n printf '%s' '{\"status\":\"blocked\",\"summary\":\"verification blocked\",\"artifacts\":[],\"data\":{}}' > \"$AI_FACTORY_RESULT_PATH\"\n exit 0\nfi");
    let (root, factory) = fixture(&script, 4);
    configure_release(&root, &factory, "codex");
    let mut settings = factory.settings().unwrap();
    let flow = settings
        .flows
        .iter_mut()
        .find(|f| f.id == "release")
        .unwrap();
    let mut verify = flow.nodes[0].clone();
    verify.id = "verify".into();
    verify.depends_on = vec!["publish_app".into()];
    flow.nodes.push(verify);
    factory.save_settings(settings).unwrap();
    let detail = run_release(&factory).await;
    assert_eq!(detail.run.status, "blocked");
    assert_eq!(
        detail
            .nodes
            .iter()
            .find(|n| n.id == "publish_app")
            .unwrap()
            .status,
        "succeeded"
    );
    assert_eq!(root.issue.lock().unwrap()["state"], "open");
    assert!(root
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|(method, _, _)| method == "GET"));
}

#[tokio::test]
async fn release_issue_closure_rechecks_identity_and_accepts_already_closed() {
    for change in ["author", "title", "pr", "url", "closed", "cancel"] {
        let script = format!("{AGENT}\ntouch \"$(dirname \"$0\")/publication-ready\"\nwhile [ ! -f \"$(dirname \"$0\")/publication-finish\" ]; do sleep 0.01; done\n");
        let (root, factory) = fixture(&script, 4);
        configure_release(&root, &factory, "codex");
        let mut settings = factory.settings().unwrap();
        settings.source_connectors[0].issue_title_prefix = "bot:".into();
        factory.save_settings(settings).unwrap();
        let id = factory
            .enqueue(common::from_source(
                TriggerEvent::release_issue("acme/repo", 811, "[RELEASE] fixture", "alice", ""),
                "alice",
            ))
            .unwrap()
            .unwrap();
        factory.start();
        for _ in 0..200 {
            if root.path().join("publication-ready").exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(root.path().join("publication-ready").exists());
        {
            let mut issue = root.issue.lock().unwrap();
            match change {
                "author" => issue["user"]["login"] = json!("bob"),
                "title" => issue["title"] = json!("[AI] changed"),
                "pr" => issue["pull_request"] = json!({}),
                "url" => issue["html_url"] = json!("https://github.com/other/repo/issues/811"),
                "closed" => issue["state"] = json!("closed"),
                _ => (),
            }
        }
        if change == "cancel" {
            factory.cancel(&id).unwrap();
        }
        std::fs::write(root.path().join("publication-finish"), "done").unwrap();
        let detail = terminal(&factory, &id).await;
        factory.shutdown().await;
        let expected = if change == "closed" {
            "succeeded"
        } else if change == "cancel" {
            "cancelled"
        } else {
            "failed"
        };
        assert_eq!(
            detail.run.status, expected,
            "{change}: {:?}",
            detail.run.error
        );
        assert!(
            root.requests
                .lock()
                .unwrap()
                .iter()
                .all(|(method, _, _)| method == "GET"),
            "{change}"
        );
    }
}

#[tokio::test]
async fn release_issue_closure_requires_explicit_publication_confirmation() {
    for (status, data, expected) in [
        ("succeeded", json!({}), "blocked"),
        (
            "succeeded",
            json!({"distribution_result":{"published":false}}),
            "blocked",
        ),
        (
            "blocked",
            json!({"distribution_result":{"published":true}}),
            "blocked",
        ),
        (
            "failed",
            json!({"distribution_result":{"published":true}}),
            "failed",
        ),
    ] {
        let script = format!(
            "cat >/dev/null\nprintf '%s' '{}' > \"$AI_FACTORY_RESULT_PATH\"",
            json!({"status":status,"summary":"inspection only","artifacts":[],"data":data})
        );
        let (root, factory) = fixture(&script, 4);
        configure_release(&root, &factory, "codex");
        let detail = run_release(&factory).await;
        assert_eq!(detail.run.status, expected);
        if status == "succeeded" {
            assert!(detail
                .run
                .error
                .as_deref()
                .unwrap()
                .contains("publication was not confirmed"));
        }
        assert_eq!(root.issue.lock().unwrap()["state"], "open");
        assert!(root
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|(method, _, _)| method == "GET"));
    }
}

#[tokio::test]
async fn release_issue_closure_retry_reuses_publication_after_close_or_confirmation_error() {
    let old_prompt =
        include_str!("../../src/context/factory/domain/entities/models/release_prompt_legacy.txt");
    for failure in ["permission", "confirmation"] {
        let (root, factory) = fixture(AGENT, 4);
        configure_release(&root, &factory, "codex");
        let mut settings = factory.settings().unwrap();
        settings.source_connectors[0].issue_title_prefix = "bot:".into();
        factory.save_settings(settings).unwrap();
        if failure == "permission" {
            root.issue_close_status
                .store(403, std::sync::atomic::Ordering::SeqCst);
        } else {
            root.issue_confirmation_failures
                .store(1, std::sync::atomic::Ordering::SeqCst);
        }
        let id = factory
            .enqueue(common::from_source(
                TriggerEvent::release_issue("acme/repo", 811, "[RELEASE] fixture", "alice", ""),
                "alice",
            ))
            .unwrap()
            .unwrap();
        let mut old_flow = factory.run_detail(&id).unwrap().flow;
        old_flow.nodes[0].prompt = old_prompt.into();
        let db = rusqlite::Connection::open(root.path().join("data/factory.sqlite3")).unwrap();
        db.execute(
            "UPDATE runs SET flow=?1 WHERE id=?2",
            rusqlite::params![serde_json::to_string(&old_flow).unwrap(), id],
        )
        .unwrap();
        drop(db);
        factory.start();
        let first = terminal(&factory, &id).await;
        assert_eq!(
            first.run.status, "failed",
            "{failure}: {:?}",
            first.run.error
        );
        assert_eq!(first.nodes[0].status, "succeeded");
        assert_eq!(first.flow.nodes[0].prompt, old_prompt);
        root.issue_close_status
            .store(200, std::sync::atomic::Ordering::SeqCst);
        let mut retry = None;
        for _ in 0..100 {
            if let Ok(id) = factory.retry(&id) {
                retry = Some(id);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let retry = retry.expect("Retry admission after worker cleanup");
        let detail = terminal(&factory, &retry).await;
        factory.shutdown().await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{failure}: {:?}",
            detail.run.error
        );
        assert_eq!(root.issue.lock().unwrap()["state"], "closed");
        assert_eq!(
            std::fs::read_to_string(root.path().join("skill-execution.calls"))
                .unwrap()
                .lines()
                .count(),
            1,
            "{failure}: publication repeated"
        );
        let work_prompt = std::fs::read_to_string(
            root.path()
                .join("data/sessions")
                .join(&id)
                .join("publish_app.stdin.txt"),
        )
        .unwrap();
        assert!(
            work_prompt.contains("a suitable installed publishing skill available to the agent")
        );
        let expected_writes = if failure == "permission" { 2 } else { 1 };
        assert_eq!(
            root.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, _, _)| method == "PATCH")
                .count(),
            expected_writes,
            "{failure}"
        );
    }
}

#[tokio::test]
async fn release_prefix_closure_uses_current_prefix_and_preserves_publication() {
    for change in ["none", "prefix", "title", "both", "new_issue"] {
        let script = format!("{AGENT}\ntouch \"$(dirname \"$0\")/publication-ready\"\nwhile [ ! -f \"$(dirname \"$0\")/publication-finish\" ]; do sleep 0.01; done\n");
        let (root, factory) = fixture(&script, 4);
        configure_release(&root, &factory, "codex");
        root.issue.lock().unwrap()["title"] = json!(" \tship:fixture ");
        let mut s = factory.settings().unwrap();
        s.source_connectors[0].issue_title_prefix = "task:".into();
        s.source_connectors[0].release_issue_title_prefix = " ship: ".into();
        factory.save_settings(s).unwrap();
        let id = factory
            .enqueue(common::from_source(
                TriggerEvent::release_issue("acme/repo", 811, "ship:queued", "alice", ""),
                "alice",
            ))
            .unwrap()
            .unwrap();
        factory.start();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !root.path().join("publication-ready").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        if matches!(change, "prefix" | "both" | "new_issue") {
            let mut changed = factory.settings().unwrap();
            if change == "new_issue" {
                changed.source_connectors[0].issue_title_prefix = "other-task:".into();
            } else {
                changed.source_connectors[0].release_issue_title_prefix = "deploy:".into();
            }
            factory.save_settings(changed).unwrap();
        }
        if matches!(change, "title" | "both") {
            root.issue.lock().unwrap()["title"] = json!("deploy:fresh");
        }
        std::fs::write(root.path().join("publication-finish"), "done").unwrap();
        let result = terminal(&factory, &id).await;
        factory.shutdown().await;
        let accepted = matches!(change, "none" | "both" | "new_issue");
        assert_eq!(
            result.run.status,
            if accepted { "succeeded" } else { "failed" },
            "{change}: {:?}",
            result.run.error
        );
        assert_eq!(result.nodes[0].status, "succeeded");
        let writes = root
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, _, _)| method == "PATCH")
            .count();
        assert_eq!(writes, usize::from(accepted), "{change}");
        assert_eq!(
            root.issue.lock().unwrap()["state"],
            if accepted { "closed" } else { "open" }
        );
        let db = rusqlite::Connection::open(root.path().join("data/factory.sqlite3")).unwrap();
        let proof: String = db
            .query_row(
                "SELECT json FROM observations WHERE key=?1",
                [format!("release-issue-close:{}", id)],
                |r| r.get(0),
            )
            .unwrap();
        let proof: Value = serde_json::from_str(&proof).unwrap();
        assert_eq!(proof["publication_confirmed"], true, "{change}");
        assert_eq!(
            proof["state"],
            if accepted { "closed" } else { "pending" },
            "{change}"
        );
        if !accepted {
            assert!(result
                .run
                .error
                .as_deref()
                .unwrap()
                .contains("Publication succeeded, but closing release issue failed"));
        }
    }
}
