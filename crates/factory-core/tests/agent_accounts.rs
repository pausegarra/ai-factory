#![cfg(unix)]
mod common;
use factory_core::*;
use serde_json::json;
use std::{
    os::unix::fs::{symlink, PermissionsExt},
    sync::Arc,
    time::Duration,
};

fn fixture(mode: &str) -> (tempfile::TempDir, Arc<Factory>) {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("codex-fixture");
    let script = format!(r#"#!/bin/sh
[ "$CODEX_HOME" != '' ] || exit 61
[ "$CODEX_API_KEY$CODEX_ACCESS_TOKEN$OPENAI_API_KEY" = '' ] || exit 62
case "$*" in *cli_auth_credentials_store*file*) ;; *) exit 63;; esac
printf '%s' "$$" > "$CODEX_HOME/cli.pid"
while IFS= read -r request; do
 case "$request" in
 *'"method":"initialize"'*) echo '{{"id":1,"result":{{}}}}';;
 *'"method":"account/login/start"'*)
  echo '{{"id":2,"result":{{"type":"chatgpt","loginId":"fixture-login","authUrl":"{url}"}}}}'
  {completed}
  ;;
 *'"method":"account/read"'*)
  printf '%s' '{{"auth_mode":"chatgpt","tokens":{{"access_token":"private-fixture-token"}}}}' > "$CODEX_HOME/auth.json"
  echo '{{"id":3,"result":{{"account":{{"type":"chatgpt","email":"fixture@example.com","planType":"plus"}}}}}}';;
 esac
done
"#,
      url=if mode=="foreign" {"https://attacker.example/oauth"} else {"https://auth.openai.com/oauth/authorize?state=private-state"},
      completed=match mode { "pending"=>"", "failed"=>"echo '{\"method\":\"account/login/completed\",\"params\":{\"loginId\":\"fixture-login\",\"success\":false,\"error\":\"private-server-secret\"}}'", _=>"echo '{\"method\":\"account/login/completed\",\"params\":{\"loginId\":\"fixture-login\",\"success\":true,\"error\":null}}'" });
    std::fs::write(&executable, script).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let factory = common::open(dir.path().join("data")).unwrap();
    let mut settings = factory.settings().unwrap();
    settings.commands.codex = executable.to_string_lossy().into();
    factory.save_settings(settings).unwrap();
    (dir, factory)
}
async fn terminal(factory: &Factory, id: &str) -> AgentAccount {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let account = factory
                .agent_accounts()
                .unwrap()
                .into_iter()
                .find(|a| a.id == id)
                .unwrap();
            if account.status != "pending" {
                return account;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
fn home(dir: &tempfile::TempDir, id: &str) -> std::path::PathBuf {
    dir.path().join("data/agent-accounts/codex").join(id)
}

#[tokio::test]
async fn oauth_accounts_use_independent_private_homes_and_metadata_without_credentials() {
    let (dir, factory) = fixture("ready");
    let mut events = factory.subscribe();
    let first = factory.start_codex_login("Work".into()).await.unwrap();
    assert_eq!(first.account.status, "pending");
    assert!(first.auth_url.starts_with("https://auth.openai.com/"));
    let account = terminal(&factory, &first.account.id).await;
    assert_eq!(account.email.as_deref(), Some("fixture@example.com"));
    assert_eq!(account.status, "ready");
    let second = factory.start_codex_login("Personal".into()).await.unwrap();
    assert_eq!(terminal(&factory, &second.account.id).await.status, "ready");
    assert_ne!(first.account.id, second.account.id);
    assert_eq!(
        std::fs::metadata(home(&dir, &first.account.id))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(home(&dir, &first.account.id).join("auth.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let db = rusqlite::Connection::open(dir.path().join("data/factory.sqlite3")).unwrap();
    let raw: String = db
        .query_row(
            "SELECT json FROM agent_accounts WHERE id=?1",
            [&first.account.id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!raw.contains("private-fixture-token"));
    assert!(!raw.contains("private-state"));
    assert!(
        std::iter::from_fn(|| events.try_recv().ok()).any(|e| e.kind == "agent_accounts_changed")
    );
    factory.shutdown().await;
}

#[tokio::test]
async fn pending_login_removal_cancels_and_reaps_cli_before_deleting_home() {
    let (dir, factory) = fixture("pending");
    let login = factory.start_codex_login("Pending".into()).await.unwrap();
    assert_eq!(
        factory.codex_login_url(&login.account.id).unwrap(),
        login.auth_url
    );
    assert!(factory.start_codex_login("Other".into()).await.is_err());
    let pid: i32 = std::fs::read_to_string(home(&dir, &login.account.id).join("cli.pid"))
        .unwrap()
        .parse()
        .unwrap();
    factory
        .remove_agent_account(&login.account.id)
        .await
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert!(!home(&dir, &login.account.id).exists());
    assert!(factory.agent_accounts().unwrap().is_empty());
}

#[tokio::test]
async fn login_failures_never_expose_server_errors_or_accept_foreign_urls() {
    let (_dir, factory) = fixture("failed");
    let login = factory.start_codex_login("Failed".into()).await.unwrap();
    let account = terminal(&factory, &login.account.id).await;
    assert_eq!(account.status, "failed");
    assert!(!account.error.unwrap().contains("private-server-secret"));
    let (_dir, foreign) = fixture("foreign");
    assert!(foreign.start_codex_login("Foreign".into()).await.is_err());
    assert_eq!(foreign.agent_accounts().unwrap()[0].status, "failed");
}

#[tokio::test]
async fn shutdown_reaps_pending_login_and_restart_reconciles_pending_metadata() {
    let (dir, factory) = fixture("pending");
    let login = factory.start_codex_login("Pending".into()).await.unwrap();
    let pid: i32 = std::fs::read_to_string(home(&dir, &login.account.id).join("cli.pid"))
        .unwrap()
        .parse()
        .unwrap();
    factory.shutdown().await;
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    drop(factory);
    let db = rusqlite::Connection::open(dir.path().join("data/factory.sqlite3")).unwrap();
    db.execute(
        "UPDATE agent_accounts SET json=json_set(json,'$.status','pending')",
        [],
    )
    .unwrap();
    drop(db);
    let restarted = common::open(dir.path().join("data")).unwrap();
    assert_eq!(restarted.agent_accounts().unwrap()[0].status, "failed");
    assert!(restarted.codex_login_url(&login.account.id).is_err());
}

#[tokio::test]
async fn deletion_rejects_active_frozen_account_and_unsafe_home_paths() {
    let (dir, factory) = fixture("ready");
    let login = factory.start_codex_login("Work".into()).await.unwrap();
    terminal(&factory, &login.account.id).await;
    let db = rusqlite::Connection::open(dir.path().join("data/factory.sqlite3")).unwrap();
    db.execute(
        "INSERT INTO runs VALUES('active','event',?1,'{}',?2,'now')",
        rusqlite::params![
            json!({"status":"queued"}).to_string(),
            json!({"nodes":[{"agent":"codex","codex_account_id":login.account.id}]}).to_string()
        ],
    )
    .unwrap();
    assert!(factory
        .remove_agent_account(&login.account.id)
        .await
        .is_err());
    db.execute("DELETE FROM runs", []).unwrap();
    let path = home(&dir, &login.account.id);
    std::fs::remove_file(path.join("auth.json")).unwrap();
    let outside = dir.path().join("outside-secret");
    std::fs::write(&outside, "keep").unwrap();
    symlink(&outside, path.join("auth.json")).unwrap();
    assert!(factory
        .remove_agent_account(&login.account.id)
        .await
        .is_err());
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "keep");
    assert!(factory.remove_agent_account("../escape").await.is_err());
}

#[tokio::test]
async fn hardlinked_auth_is_not_a_private_managed_credential() {
    let (dir, factory) = fixture("ready");
    let login = factory.start_codex_login("Work".into()).await.unwrap();
    terminal(&factory, &login.account.id).await;
    let auth = home(&dir, &login.account.id).join("auth.json");
    std::fs::remove_file(&auth).unwrap();
    let outside = dir.path().join("outside-credentials");
    std::fs::write(&outside, "outside-only").unwrap();
    std::fs::hard_link(&outside, &auth).unwrap();
    assert!(factory
        .remove_agent_account(&login.account.id)
        .await
        .is_err());
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "outside-only");
}

#[tokio::test]
async fn startup_output_is_bounded_and_never_exposed() {
    let (dir, factory) = fixture("pending");
    std::fs::write(
        dir.path().join("codex-fixture"),
        "#!/bin/sh\nprintf '%070000d\\n' 0\nexec sleep 30\n",
    )
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        factory.start_codex_login("Output".into()),
    )
    .await
    .unwrap();
    let error = result.err().unwrap().to_string();
    assert_eq!(error, "Codex login response too large");
    assert_eq!(factory.agent_accounts().unwrap()[0].status, "failed");
}

#[tokio::test]
async fn failed_cli_start_is_reconciled_without_exposing_stderr() {
    let (dir, factory) = fixture("pending");
    std::fs::write(
        dir.path().join("codex-fixture"),
        "#!/bin/sh\necho private-error-text >&2\nexit 27\n",
    )
    .unwrap();
    let error = factory
        .start_codex_login("Failed".into())
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(!error.contains("private-error-text"));
    let account = &factory.agent_accounts().unwrap()[0];
    assert_eq!(account.status, "failed");
    assert!(factory.codex_login_url(&account.id).is_err());
}

#[tokio::test]
async fn dropping_last_factory_cancels_login_without_retaining_process_or_lock() {
    let (dir, factory) = fixture("pending");
    let login = factory.start_codex_login("Pending".into()).await.unwrap();
    let pid: i32 = std::fs::read_to_string(home(&dir, &login.account.id).join("cli.pid"))
        .unwrap()
        .parse()
        .unwrap();
    drop(factory);
    tokio::time::timeout(Duration::from_secs(3), async {
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let restarted = common::open(dir.path().join("data")).unwrap();
    assert_eq!(restarted.agent_accounts().unwrap()[0].status, "failed");
}

#[test]
fn oauth_clears_ambient_credentials() {
    if std::env::var_os("FACTORY_ACCOUNT_ENV_FIXTURE").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "oauth_clears_ambient_credentials"])
            .env("FACTORY_ACCOUNT_ENV_FIXTURE", "1")
            .env("CODEX_API_KEY", "fixture-ambient-codex-key")
            .env("CODEX_ACCESS_TOKEN", "fixture-ambient-codex-token")
            .env("OPENAI_API_KEY", "fixture-ambient-openai-key")
            .env("CODEX_HOME", "/fixture-global-home-never-used")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "managed login must clear ambient credentials"
        );
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (_dir, factory) = fixture("ready");
        let login = factory.start_codex_login("Isolated".into()).await.unwrap();
        assert_eq!(terminal(&factory, &login.account.id).await.status, "ready");
        factory.shutdown().await;
    });
}

#[tokio::test]
async fn removing_account_clears_only_its_bindings_and_preserves_github_credentials() {
    let (dir, factory) = fixture("ready");
    let first = factory.start_codex_login("Work".into()).await.unwrap();
    terminal(&factory, &first.account.id).await;
    let second = factory.start_codex_login("Other".into()).await.unwrap();
    terminal(&factory, &second.account.id).await;
    let mut settings = factory.settings().unwrap();
    for (id, account, codex) in [
        ("first", "octocat", &first.account.id),
        ("second", "hubot", &second.account.id),
    ] {
        settings.token_profiles.push(TokenProfile {
            id: id.into(),
            label: id.into(),
            account: account.into(),
            token: Some(format!("github-fixture-{id}")),
            ..Default::default()
        });
        settings.codex_bindings.insert(id.into(), codex.clone());
    }
    factory.save_settings(settings).unwrap();
    factory
        .remove_agent_account(&first.account.id)
        .await
        .unwrap();
    let settings = factory.settings().unwrap();
    assert_eq!(settings.codex_bindings.len(), 1);
    assert_eq!(settings.codex_bindings["second"], second.account.id);
    assert!(settings
        .token_profiles
        .iter()
        .all(|profile| profile.has_token && profile.token.is_none()));
    assert_eq!(factory.agent_accounts().unwrap().len(), 1);
    assert!(!home(&dir, &first.account.id).exists());
    assert!(home(&dir, &second.account.id).join("auth.json").is_file());
}

#[tokio::test]
async fn pending_or_missing_accounts_cannot_be_bound_to_github_profiles() {
    let (_dir, factory) = fixture("pending");
    let login = factory.start_codex_login("Pending".into()).await.unwrap();
    let mut settings = factory.settings().unwrap();
    settings.token_profiles.push(TokenProfile {
        id: "work".into(),
        account: "octocat".into(),
        ..Default::default()
    });
    settings
        .codex_bindings
        .insert("work".into(), login.account.id.clone());
    assert!(factory.save_settings(settings.clone()).is_err());
    settings
        .codex_bindings
        .insert("work".into(), uuid::Uuid::new_v4().to_string());
    assert!(factory.save_settings(settings).is_err());
    assert!(factory.settings().unwrap().codex_bindings.is_empty());
    factory
        .remove_agent_account(&login.account.id)
        .await
        .unwrap();
}

#[tokio::test]
async fn authenticated_chatgpt_accounts_can_omit_email() {
    let (dir, factory) = fixture("ready");
    let executable = dir.path().join("codex-fixture");
    let script = std::fs::read_to_string(&executable)
        .unwrap()
        .replace("\"fixture@example.com\"", "null");
    std::fs::write(executable, script).unwrap();
    let login = factory
        .start_codex_login("Without email".into())
        .await
        .unwrap();
    let account = terminal(&factory, &login.account.id).await;
    assert_eq!(account.status, "ready");
    assert_eq!(account.email, None);
}

#[tokio::test]
async fn pending_login_keeps_url_when_initial_request_is_dropped() {
    let (_dir, factory) = fixture("pending");
    let mut request = Box::pin(factory.start_codex_login("Pending".into()));
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(request.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(request);
    let id = factory.agent_accounts().unwrap()[0].id.clone();
    tokio::time::timeout(Duration::from_secs(3), async {
        while factory.codex_login_url(&id).is_err() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    factory.remove_agent_account(&id).await.unwrap();
}

#[tokio::test]
async fn removing_account_without_managed_home_clears_metadata_and_bindings() {
    for remove_parents in [false, true] {
        let (dir, factory) = fixture("ready");
        let login = factory
            .start_codex_login("Lost credentials".into())
            .await
            .unwrap();
        terminal(&factory, &login.account.id).await;
        let mut settings = factory.settings().unwrap();
        settings.token_profiles.push(TokenProfile {
            id: "work".into(),
            account: "octocat".into(),
            ..Default::default()
        });
        settings
            .codex_bindings
            .insert("work".into(), login.account.id.clone());
        factory.save_settings(settings).unwrap();
        std::fs::remove_dir_all(home(&dir, &login.account.id)).unwrap();
        if remove_parents {
            std::fs::remove_dir_all(dir.path().join("data/agent-accounts")).unwrap();
        }
        factory
            .remove_agent_account(&login.account.id)
            .await
            .unwrap();
        assert!(factory.agent_accounts().unwrap().is_empty());
        assert!(factory.settings().unwrap().codex_bindings.is_empty());
    }
}

#[tokio::test]
async fn missing_home_does_not_allow_symlinked_ancestors_or_dangling_leaf_links() {
    for parent_link in [false, true] {
        let (dir, factory) = fixture("ready");
        let login = factory.start_codex_login("Unsafe".into()).await.unwrap();
        terminal(&factory, &login.account.id).await;
        let path = home(&dir, &login.account.id);
        std::fs::remove_dir_all(&path).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep"), "keep").unwrap();
        if parent_link {
            std::fs::remove_dir(path.parent().unwrap()).unwrap();
            symlink(&outside, path.parent().unwrap()).unwrap();
        } else {
            symlink(outside.join("missing"), &path).unwrap();
        }
        assert!(factory
            .remove_agent_account(&login.account.id)
            .await
            .is_err());
        assert_eq!(factory.agent_accounts().unwrap().len(), 1);
        assert_eq!(
            std::fs::read_to_string(outside.join("keep")).unwrap(),
            "keep"
        );
    }
}
