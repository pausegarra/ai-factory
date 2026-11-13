use super::*;

const ACCOUNT_A: &str = "2bb656dd-79c9-4eec-bd40-1f309d04ce91";
const ACCOUNT_B: &str = "23068e61-63ce-4da1-8df6-ed4353fe2d99";

#[tokio::test]
async fn unassigned_codex_node_blocks_before_cli_launch() {
    let (dir,factory)=fixture("cat >/dev/null\nprintf launched > \"$AI_FACTORY_MEMORY_DIR/should-not-launch\"\nprintf '%s' '{\"status\":\"succeeded\",\"summary\":\"done\",\"artifacts\":[],\"data\":{}}' > \"$AI_FACTORY_RESULT_PATH\"",4);
    let mut settings = serde_json::to_value(factory.settings().unwrap()).unwrap();
    settings["codex_bindings"] = json!({});
    factory
        .save_settings(serde_json::from_value(settings.clone()).unwrap())
        .unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            300,
            "[AI] unassigned",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "blocked", "{:?}", detail.nodes);
    assert_eq!(detail.nodes[0].status, "blocked");
    assert!(detail.nodes[0].result.as_ref().unwrap()["summary"]
        .as_str()
        .unwrap()
        .contains("Codex account"));
    assert!(!dir
        .path()
        .join("data/memory/acme/repo/runs")
        .join(id)
        .join("should-not-launch")
        .exists());
}

#[test]
fn admission_freezes_codex_account_per_github_profile() {
    let (_dir, factory) = fixture("exit 1", 4);
    common::seed_codex_account(&factory, ACCOUNT_A);
    common::seed_codex_account(&factory, ACCOUNT_B);
    let mut settings = serde_json::to_value(factory.settings().unwrap()).unwrap();
    settings["codex_bindings"] = json!({"alice":ACCOUNT_A});
    factory
        .save_settings(serde_json::from_value(settings.clone()).unwrap())
        .unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            301,
            "[AI] frozen account",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    settings["codex_bindings"] = json!({"alice":ACCOUNT_B});
    factory
        .save_settings(serde_json::from_value(settings.clone()).unwrap())
        .unwrap();
    let frozen = serde_json::to_value(factory.run_detail(&id).unwrap().flow).unwrap();
    assert_eq!(frozen["nodes"][0]["codex_account_id"], ACCOUNT_A);
}

#[tokio::test]
async fn unavailable_frozen_codex_account_blocks_without_launching_cli() {
    let (dir, factory) = fixture("touch cli-launched; exit 1", 4);
    let settings = factory.settings().unwrap();
    let account = &settings.codex_bindings["alice"];
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            305,
            "[AI] unavailable account",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    std::fs::remove_file(
        dir.path()
            .join("data/agent-accounts/codex")
            .join(account)
            .join("auth.json"),
    )
    .unwrap();
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "blocked", "{:?}", detail.nodes);
    assert!(detail.nodes[0].result.as_ref().unwrap()["summary"]
        .as_str()
        .unwrap()
        .contains("unavailable"));
    assert!(detail.nodes[0].session_id.is_none());
}

#[tokio::test]
async fn frozen_accounts_isolate_execution_and_skill_discovery_homes() {
    let (dir, factory) = fixture("exit 1", 4);
    common::seed_codex_account(&factory, ACCOUNT_A);
    common::seed_codex_account(&factory, ACCOUNT_B);
    std::fs::write(dir.path().join("fake-agent"),r#"#!/usr/bin/env python3
import json,os,pathlib,sys
home=os.environ['CODEX_HOME']
if sys.argv[1]=='app-server':
    for line in sys.stdin:
        request=json.loads(line)
        if request.get('method')=='initialize': print(json.dumps({'id':1,'result':{}}),flush=True)
        elif request.get('method')=='skills/list':
            pathlib.Path(home,'discovery-used').write_text(home)
            print(json.dumps({'id':2,'result':{'data':[{'skills':[]}]}}),flush=True)
else:
    sys.stdin.read()
    value={'status':'succeeded','summary':'isolated account execution','artifacts':[],
           'data':{'home':home,'auth_overrides':[key for key in ['OPENAI_API_KEY','CODEX_API_KEY','CODEX_ACCESS_TOKEN'] if os.environ.get(key)]}}
    pathlib.Path(os.environ['AI_FACTORY_RESULT_PATH']).write_text(json.dumps(value))
"#).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.flows[0].nodes[0].skills = Some(vec![]);
    settings
        .codex_bindings
        .insert("alice".into(), ACCOUNT_A.into());
    factory.save_settings(settings.clone()).unwrap();
    let first = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            302,
            "[AI] account A",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    settings
        .codex_bindings
        .insert("alice".into(), ACCOUNT_B.into());
    factory.save_settings(settings).unwrap();
    let second = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            303,
            "[AI] account B",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    for (id, account) in [(&first, ACCOUNT_A), (&second, ACCOUNT_B)] {
        let detail = terminal(&factory, id).await;
        assert_eq!(detail.run.status, "succeeded", "{:?}", detail.nodes);
        let data = &detail.nodes[0].result.as_ref().unwrap()["data"];
        assert_eq!(data["auth_overrides"], json!([]));
        let home = std::path::Path::new(data["home"].as_str().unwrap());
        assert_eq!(home.file_name().unwrap(), account);
        assert_eq!(
            std::fs::read_to_string(home.join("discovery-used")).unwrap(),
            home.to_str().unwrap()
        );
    }
    factory.shutdown().await;
}

#[tokio::test]
async fn opencode_remains_default_cli_without_codex_assignment() {
    let (_dir,factory)=fixture("cat >/dev/null\nprintf '%s' '{\"status\":\"succeeded\",\"summary\":\"default OpenCode\",\"artifacts\":[],\"data\":{}}' > \"$AI_FACTORY_RESULT_PATH\"",4);
    let mut settings = factory.settings().unwrap();
    settings.codex_bindings.clear();
    settings.flows[0].nodes[0].agent = "opencode".into();
    factory.save_settings(settings).unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            304,
            "[AI] OpenCode default",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    let flow = factory.run_detail(&id).unwrap().flow;
    assert!(flow.nodes[0].codex_account_id.is_none());
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    assert_eq!(detail.run.status, "succeeded");
}
