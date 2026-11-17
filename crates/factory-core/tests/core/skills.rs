use super::{fixture, terminal};
use factory_core::{Factory, RunDetail, TriggerEvent};
use serde_json::{json, Value};
use std::path::Path;

const AGENT: &str = r#"
if [ "$1" = app-server ]; then
  printf 'discovered\n' >> "$(dirname "$0")/skill-discovery.calls"
  read -r line
  printf '%s\n' '{"id":1,"result":{}}'
  read -r line
  read -r line
  printf '{"id":2,"result":{"data":[{"cwd":"%s","skills":[{"name":"factory-selected","path":"%s/.agents/skills/factory-selected/SKILL.md","enabled":true},{"name":"factory-unselected","path":"%s/.agents/skills/factory-unselected/SKILL.md","enabled":true}],"errors":[]}]}}\n' "$PWD" "$PWD" "$PWD"
  exit 0
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

fn install_project_skills(root: &Path) {
    let source = root.join("source");
    for id in ["factory-selected", "factory-unselected"] {
        let directory = source.join(".agents/skills").join(id);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("SKILL.md"),
            format!(
                "---\nname: {id}\ndescription: Fixture skill\n---\nUnique instructions for {id}.\n"
            ),
        )
        .unwrap();
    }
    for args in [
        vec!["add", ".agents"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "fixture skills",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(&source)
            .output()
            .unwrap()
            .status
            .success());
    }
}

async fn run(factory: &std::sync::Arc<Factory>, number: u64) -> RunDetail {
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            number,
            "[AI] skills",
            "alice",
            "",
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

#[tokio::test]
async fn selected_skills_reach_both_agents_without_exposing_unselected_skills() {
    for agent in ["codex", "opencode"] {
        let (root, factory) = fixture(AGENT, 4);
        install_project_skills(root.path());
        let mut settings = factory.settings().unwrap();
        let node = &mut settings.flows[0].nodes[0];
        node.agent = agent.into();
        node.access = "read".into();
        node.skills = Some(vec!["factory-selected".into()]);
        factory.save_settings(settings).unwrap();
        let detail = run(&factory, 211).await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        let prompt = capture(root.path(), &detail.run.id, "stdin.txt");
        assert!(
            prompt.contains("AI Factory skill selection"),
            "{agent}: selection missing"
        );
        assert!(prompt.contains("factory-selected/SKILL.md"));
        assert!(!prompt.contains("factory-unselected/SKILL.md"));
        assert!(
            !prompt.contains("Unique instructions"),
            "skills must remain lazy loaded"
        );
        if agent == "opencode" {
            let config: Value =
                serde_json::from_str(&capture(root.path(), &detail.run.id, "config.json")).unwrap();
            for permissions in [
                &config["permission"],
                &config["agent"]["ai-factory"]["permission"],
            ] {
                assert_eq!(
                    permissions["skill"],
                    json!({"*":"deny","factory-selected":"allow"})
                );
                assert_eq!(permissions["question"], "deny");
                assert_eq!(permissions["external_directory"]["*"], "deny");
            }
        } else {
            let args = capture(root.path(), &detail.run.id, "args.txt");
            assert!(args.contains("skills.config="));
            assert!(args.contains("factory-selected/SKILL.md\",enabled=true"));
            assert!(args.contains("factory-unselected/SKILL.md\",enabled=false"));
            assert!(root.path().join("skill-discovery.calls").exists());
        }
    }
}

#[tokio::test]
async fn opencode_filters_by_directory_id_when_manifest_name_differs() {
    let (root, factory) = fixture(AGENT, 4);
    install_project_skills(root.path());
    let source = root.path().join("source");
    std::fs::write(source.join(".agents/skills/factory-selected/SKILL.md"),
        "---\nname: factory-display-name\ndescription: Different display name\n---\nSkill content.\n").unwrap();
    assert!(std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-am",
            "rename skill metadata"
        ])
        .current_dir(&source)
        .output()
        .unwrap()
        .status
        .success());
    let mut settings = factory.settings().unwrap();
    settings.flows[0].nodes[0].agent = "opencode".into();
    settings.flows[0].nodes[0].skills = Some(vec!["factory-display-name".into()]);
    factory.save_settings(settings).unwrap();
    let detail = run(&factory, 218).await;
    assert_eq!(detail.run.status, "succeeded");
    let config: Value =
        serde_json::from_str(&capture(root.path(), &detail.run.id, "config.json")).unwrap();
    assert_eq!(
        config["permission"]["skill"],
        json!({"*":"deny","factory-selected":"allow"})
    );
    assert!(capture(root.path(), &detail.run.id, "stdin.txt")
        .contains("\"tool_id\": \"factory-selected\""));
}

#[tokio::test]
async fn empty_skill_selection_disables_skills_for_both_agents() {
    for agent in ["codex", "opencode"] {
        let (root, factory) = fixture(AGENT, 4);
        let mut settings = factory.settings().unwrap();
        settings.flows[0].nodes[0].agent = agent.into();
        settings.flows[0].nodes[0].skills = Some(vec![]);
        factory.save_settings(settings).unwrap();
        let detail = run(&factory, 212).await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        let prompt = capture(root.path(), &detail.run.id, "stdin.txt");
        assert!(prompt.contains("No skills are enabled for this node"));
        if agent == "opencode" {
            let config: Value =
                serde_json::from_str(&capture(root.path(), &detail.run.id, "config.json")).unwrap();
            assert_eq!(config["permission"]["skill"], json!({"*":"deny"}));
        } else {
            let args = capture(root.path(), &detail.run.id, "args.txt");
            assert!(args.contains("skills.include_instructions=false"));
            assert!(!args.contains("enabled=true"));
            assert!(args.contains("enabled=false"));
        }
    }
}

#[tokio::test]
async fn select_all_retains_native_discovery_without_an_extra_catalog_process() {
    for agent in ["codex", "opencode"] {
        let (root, factory) = fixture(AGENT, 4);
        let mut settings = factory.settings().unwrap();
        settings.flows[0].nodes[0].agent = agent.into();
        factory.save_settings(settings).unwrap();
        let detail = run(&factory, 213).await;
        assert_eq!(detail.run.status, "succeeded");
        assert!(!root.path().join("skill-discovery.calls").exists());
        let args = capture(root.path(), &detail.run.id, "args.txt");
        assert!(!args.contains("skills.config="));
        if agent == "opencode" {
            let config: Value =
                serde_json::from_str(&capture(root.path(), &detail.run.id, "config.json")).unwrap();
            assert!(config["permission"].get("skill").is_none());
        }
    }
}

#[tokio::test]
async fn unavailable_selection_fails_before_agent_execution() {
    let (root, factory) = fixture(AGENT, 4);
    let mut settings = factory.settings().unwrap();
    settings.flows[0].nodes[0].skills = Some(vec!["factory-skill-no-longer-installed".into()]);
    factory.save_settings(settings).unwrap();
    let detail = run(&factory, 214).await;
    assert_eq!(detail.run.status, "failed");
    assert!(detail.nodes[0]
        .error
        .as_deref()
        .unwrap()
        .contains("Selected skill is not installed"));
    assert!(!root.path().join("skill-execution.calls").exists());
}

#[tokio::test]
async fn output_correction_does_not_repeat_work_or_skill_discovery() {
    let success = r#"printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH""#;
    let repair = r#"
attempt=$(wc -l < "$(dirname "$0")/skill-execution.calls" | tr -d ' ')
base="${AI_FACTORY_RESULT_PATH%.result.json}"
cp "$base.args.txt" "$base.attempt-$attempt.args.txt"
cp "$base.config.json" "$base.attempt-$attempt.config.json"
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
        settings.flows[0].nodes[0].skills = Some(vec![]);
        factory.save_settings(settings).unwrap();
        let detail = run(&factory, 215).await;
        assert_eq!(
            detail.run.status, "succeeded",
            "{agent}: {:?}",
            detail.nodes
        );
        for attempt in [1] {
            if agent == "codex" {
                let args = capture(
                    root.path(),
                    &detail.run.id,
                    &format!("attempt-{attempt}.args.txt"),
                );
                assert!(args.contains("skills.include_instructions=false"));
                assert!(!args.contains("enabled=true"));
            } else {
                let config: Value = serde_json::from_str(&capture(
                    root.path(),
                    &detail.run.id,
                    &format!("attempt-{attempt}.config.json"),
                ))
                .unwrap();
                assert_eq!(config["permission"]["skill"], json!({"*":"deny"}));
            }
        }
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
        if agent == "codex" {
            let args = std::fs::read_to_string(root.path().join("correction.args.txt")).unwrap();
            assert!(args.contains("features.shell_tool=false"));
            assert!(args.contains("skills.include_instructions=false"));
            assert!(!args.contains("skills.config="));
            assert_eq!(
                std::fs::read_to_string(root.path().join("skill-discovery.calls"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
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

#[tokio::test]
async fn codex_discovery_failure_never_falls_back_to_unrestricted_execution() {
    let script = r#"
if [ "$1" = app-server ]; then
  read -r line
  printf '%s\n' '{"id":1,"result":{}}'
  read -r line
  read -r line
  printf '%s\n' '{"id":2,"error":{"code":-32601,"message":"unknown method"}}'
  exit 0
fi
touch "$(dirname "$0")/unexpected-execution"
"#;
    let (root, factory) = fixture(script, 4);
    let mut settings = factory.settings().unwrap();
    settings.flows[0].nodes[0].skills = Some(vec![]);
    factory.save_settings(settings).unwrap();
    let detail = run(&factory, 216).await;
    assert_eq!(detail.run.status, "failed");
    assert!(detail.nodes[0]
        .error
        .as_deref()
        .unwrap()
        .contains("update the configured CLI"));
    assert!(!root.path().join("unexpected-execution").exists());
}

#[tokio::test]
async fn cancelling_codex_skill_discovery_stops_before_execution() {
    let (root, factory) = fixture(
        r#"
if [ "$1" = app-server ]; then
  touch "$(dirname "$0")/skill-discovery-started"
  sleep 30
  exit 0
fi
touch "$(dirname "$0")/unexpected-execution"
"#,
        4,
    );
    let mut settings = factory.settings().unwrap();
    settings.flows[0].nodes[0].skills = Some(vec![]);
    factory.save_settings(settings).unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            217,
            "[AI] cancel skills",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    let started = root.path().join("skill-discovery-started");
    for _ in 0..200 {
        if started.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(started.exists());
    factory.cancel(&id).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), factory.shutdown())
        .await
        .unwrap();
    assert_eq!(factory.run_detail(&id).unwrap().run.status, "cancelled");
    assert!(!root.path().join("unexpected-execution").exists());
}

async fn run_release(factory: &std::sync::Arc<Factory>) -> RunDetail {
    let id = factory
        .enqueue(TriggerEvent::release_issue(
            "acme/repo",
            811,
            "[Release] fixture",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    let detail = terminal(factory, &id).await;
    factory.shutdown().await;
    detail
}
fn configure_release(
    root: &super::FixtureDir,
    factory: &Factory,
    agent: &str,
    skills: Option<Vec<String>>,
) {
    *root.issue.lock().unwrap() = json!({"number":811,"html_url":"https://github.com/acme/repo/issues/811","state":"open","title":"[Release] fixture","user":{"login":"alice"}});
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
    node.skills = skills;
    factory.save_settings(settings).unwrap();
}

#[tokio::test]
async fn release_selected_skill_reaches_both_agents_in_one_editable_step() {
    for agent in ["codex", "opencode"] {
        let (root, factory) = fixture(AGENT, 4);
        install_project_skills(root.path());
        configure_release(
            &root,
            &factory,
            agent,
            Some(vec!["factory-selected".into()]),
        );
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
        assert!(prompt.contains("factory-selected/SKILL.md"));
        assert!(!prompt.contains("factory-unselected/SKILL.md"));
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
        configure_release(&root, &factory, "codex", None);
        let id = factory
            .enqueue(TriggerEvent::release_issue(
                "acme/repo",
                811,
                "[Release] fixture",
                "alice",
                "",
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
async fn release_missing_selected_skill_fails_without_running_agent() {
    let (root, factory) = fixture(AGENT, 4);
    configure_release(
        &root,
        &factory,
        "codex",
        Some(vec!["factory-skill-no-longer-installed".into()]),
    );
    let detail = run_release(&factory).await;
    assert_eq!(detail.run.status, "failed");
    assert!(detail.nodes[0]
        .error
        .as_deref()
        .unwrap()
        .contains("Selected skill is not installed"));
    assert!(!root.path().join("skill-execution.calls").exists());
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
        configure_release(&root, &factory, "codex", None);
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
        configure_release(&root, &factory, agent, None);
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
            configure_release(&root, &factory, agent, None);
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
                TriggerEvent::release_issue("acme/repo", 811, "[Release] fixture", "alice", "")
            } else {
                *root.issue.lock().unwrap() =
                    json!({"state":"open","title":"[AI] work","user":{"login":"alice"}});
                TriggerEvent::issue("acme/repo", 811, "[AI] work", "alice", "")
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
    configure_release(&root, &factory, "codex", None);
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
    configure_release(&root, &factory, "codex", None);
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
    configure_release(&root, &factory, "codex", None);
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
        configure_release(&root, &factory, "codex", None);
        let id = factory
            .enqueue(TriggerEvent::release_issue(
                "acme/repo",
                811,
                "[Release] fixture",
                "alice",
                "",
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
        configure_release(&root, &factory, "codex", None);
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
    for failure in ["permission", "confirmation"] {
        let (root, factory) = fixture(AGENT, 4);
        configure_release(&root, &factory, "codex", None);
        if failure == "permission" {
            root.issue_close_status
                .store(403, std::sync::atomic::Ordering::SeqCst);
        } else {
            root.issue_confirmation_failures
                .store(1, std::sync::atomic::Ordering::SeqCst);
        }
        let id = factory
            .enqueue(TriggerEvent::release_issue(
                "acme/repo",
                811,
                "[Release] fixture",
                "alice",
                "",
            ))
            .unwrap()
            .unwrap();
        factory.start();
        let first = terminal(&factory, &id).await;
        assert_eq!(
            first.run.status, "failed",
            "{failure}: {:?}",
            first.run.error
        );
        assert_eq!(first.nodes[0].status, "succeeded");
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
