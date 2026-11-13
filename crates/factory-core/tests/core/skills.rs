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
printf 'executed\n' >> "$(dirname "$0")/skill-execution.calls"
cat > "${AI_FACTORY_RESULT_PATH%.result.json}.stdin.txt"
printf '%s\n' "$@" > "${AI_FACTORY_RESULT_PATH%.result.json}.args.txt"
printf '%s' "$OPENCODE_CONFIG_CONTENT" > "${AI_FACTORY_RESULT_PATH%.result.json}.config.json"
printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH"
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
async fn invalid_output_does_not_restart_agent_or_repeat_skill_discovery() {
    let success = r#"printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH""#;
    let repair = r#"
attempt=$(wc -l < "$(dirname "$0")/skill-execution.calls" | tr -d ' ')
base="${AI_FACTORY_RESULT_PATH%.result.json}"
cp "$base.args.txt" "$base.attempt-$attempt.args.txt"
cp "$base.config.json" "$base.attempt-$attempt.config.json"
if [ "$attempt" -eq 1 ]; then
  printf '%s' '{"status":"invalid","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH"
else
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
        assert_eq!(detail.run.status, "failed", "{agent}: {:?}", detail.nodes);
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
        if agent == "codex" {
            assert_eq!(
                std::fs::read_to_string(root.path().join("skill-discovery.calls"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
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
