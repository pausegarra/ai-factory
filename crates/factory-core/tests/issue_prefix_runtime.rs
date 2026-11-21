#![cfg(unix)]
#[path = "common/activation_http.rs"]
mod activation_http;
mod common;

use activation_http::Server;
use factory_core::{Factory, Settings, TriggerEvent};
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

fn configure(factory: &Factory, root: &Path, prefix: &str) -> Settings {
    let git = root.join("fake-git");
    std::fs::write(
        &git,
        "#!/bin/sh\nprintf '%s' \"$1\" > clone-called\nexit 83\n",
    )
    .unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = common::configured_settings();
    settings.issue_title_prefix = prefix.into();
    settings.commands.git = git.to_string_lossy().into();
    settings.issue_node_comments_enabled = false;
    factory.save_settings(settings).unwrap()
}

async fn run(factory: Arc<Factory>, id: String) -> anyhow::Result<()> {
    factory_core::context::factory::infrastructure::processes::agents::execute(
        &factory,
        &id,
        CancellationToken::new(),
    )
    .await
}

#[tokio::test]
async fn issue_prefix_execution_uses_saved_prefix_instead_of_queued_snapshot() {
    let server = Server::start(|r| async move {
        assert_eq!(r.method, "GET");
        assert_eq!(r.authorization, "Bearer fixture-source-only");
        if r.path == "/user" {
            return (200, json!({"login":"alice"}));
        }
        if r.path == "/repos/acme/repo/issues/97" {
            return (
                200,
                json!({"state":"open","title":"[AI] queued",
                               "user":{"login":"alice"},"number":97,
                               "html_url":"https://github.com/acme/repo/issues/97"}),
            );
        }
        panic!("unexpected {}", r.path);
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    configure(&factory, dir.path(), "[AI]");
    let id = factory
        .enqueue(common::from_source(
            TriggerEvent::issue("acme/repo", 97, "[AI] queued", "alice", ""),
            "alice",
        ))
        .unwrap()
        .unwrap();
    let mut changed = factory.settings().unwrap();
    changed.issue_title_prefix = "bot:".into();
    factory.save_settings(changed).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), run(factory.clone(), id.clone()))
        .await
        .unwrap()
        .unwrap_err();
    assert!(format!("{error:#}").contains("Issue no longer eligible"));
    assert_eq!(
        server
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path.ends_with("/issues/97"))
            .count(),
        1
    );
    assert!(!dir.path().join("clone-called").exists());
    assert!(!dir.path().join("workspaces").join(id).exists());
}

#[tokio::test]
async fn issue_prefix_execution_reads_settings_after_detail_await() {
    let reached = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let server = Server::start({
        let reached = reached.clone();
        let resume = resume.clone();
        move |r| {
            let reached = reached.clone();
            let resume = resume.clone();
            async move {
                assert_eq!(r.method, "GET");
                assert_eq!(r.authorization, "Bearer fixture-source-only");
                if r.path == "/user" {
                    return (200, json!({"login":"alice"}));
                }
                if r.path == "/repos/acme/repo/issues/97" {
                    reached.notify_one();
                    resume.notified().await;
                    return (
                        200,
                        json!({"state":"open","title":"[AI] queued",
                                       "user":{"login":"alice"},"number":97,
                                       "html_url":"https://github.com/acme/repo/issues/97"}),
                    );
                }
                panic!("unexpected {}", r.path);
            }
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    configure(&factory, dir.path(), "[AI]");
    let id = factory
        .enqueue(common::from_source(
            TriggerEvent::issue("acme/repo", 97, "[AI] queued", "alice", ""),
            "alice",
        ))
        .unwrap()
        .unwrap();
    let executing = tokio::spawn(run(factory.clone(), id.clone()));
    tokio::time::timeout(Duration::from_secs(5), reached.notified())
        .await
        .unwrap();
    let mut changed = factory.settings().unwrap();
    changed.issue_title_prefix = "bot:".into();
    factory.save_settings(changed).unwrap();
    resume.notify_one();
    let error = tokio::time::timeout(Duration::from_secs(5), executing)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(format!("{error:#}").contains("Issue no longer eligible"));
    assert_eq!(
        server
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path.ends_with("/issues/97"))
            .count(),
        1
    );
    assert!(!dir.path().join("clone-called").exists());
    assert!(!dir.path().join("workspaces").join(id).exists());
}

#[tokio::test]
async fn issue_prefix_execution_checks_custom_title_case_trim_and_release_independently() {
    for (trigger, prefix, release_prefix, title, accepted) in [
        ("new_issue", "bot:", "[RELEASE]", " \tbot:fresh\n", true),
        ("new_issue", "BOT:", "[RELEASE]", "bot:fresh", false),
        ("new_issue", "bot:", "[RELEASE]", "note bot:fresh", false),
        ("new_issue", "bot:", "[RELEASE]", "[AI] old", false),
        ("release", "task:", "ship:", " \tship:fresh\n", true),
        ("release", "task:", "Ship:", "ship:fresh", false),
        ("release", "task:", "ship:", "note ship:fresh", false),
        ("release", "task:", "[RELEASE]", "[RELEASE]2", true),
        ("release", "task:", "[RELEASE]", " [RELEASE]2", true),
        ("release", "task:", "[RELEASE]", "[Release]2", false),
    ] {
        let server = Server::start(move |r| async move {
            assert_eq!(r.method, "GET");
            assert_eq!(r.authorization, "Bearer fixture-source-only");
            if r.path == "/user" {
                return (200, json!({"login":"alice"}));
            }
            if r.path == "/repos/acme/repo/issues/97" {
                return (
                    200,
                    json!({"state":"open","title":title,
                                   "user":{"login":"alice"},"number":97,
                                   "html_url":"https://github.com/acme/repo/issues/97"}),
                );
            }
            if trigger == "new_issue"
                && r.path.starts_with("/repos/acme/repo/issues/97/sub_issues?")
            {
                return (200, json!([]));
            }
            panic!("unexpected {}", r.path);
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
        let mut settings = configure(&factory, dir.path(), prefix);
        settings.release_issue_title_prefix = release_prefix.into();
        settings
            .flows
            .iter_mut()
            .find(|f| f.id == trigger)
            .unwrap()
            .enabled = true;
        factory.save_settings(settings).unwrap();
        let event = if trigger == "release" {
            TriggerEvent::release_issue("acme/repo", 97, title, "alice", "")
        } else {
            TriggerEvent::issue("acme/repo", 97, title, "alice", "")
        };
        let id = factory
            .enqueue(common::from_source(event, "alice"))
            .unwrap()
            .unwrap();
        let error = tokio::time::timeout(Duration::from_secs(5), run(factory, id.clone()))
            .await
            .unwrap()
            .unwrap_err();
        let marker = dir.path().join("clone-called");
        if accepted {
            assert!(format!("{error:#}").contains("Git command failed"));
            assert_eq!(std::fs::read_to_string(marker).unwrap(), "clone");
        } else {
            assert!(format!("{error:#}").contains("Issue no longer eligible"));
            assert!(!marker.exists());
        }
        assert!(!dir.path().join("workspaces").join(id).exists());
    }
}

#[tokio::test]
async fn release_prefix_execution_uses_current_settings_after_detail_await() {
    let reached = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let server = Server::start({
        let reached = reached.clone();
        let resume = resume.clone();
        move |r| {
            let reached = reached.clone();
            let resume = resume.clone();
            async move {
                assert_eq!(r.method, "GET");
                if r.path == "/user" { return (200, json!({"login":"alice"})); }
                if r.path.ends_with("/issues/97") {
                    reached.notify_one();
                    resume.notified().await;
                    return (200, json!({"number":97,"state":"open","title":"ship:queued",
                        "user":{"login":"alice"},"html_url":"https://github.com/acme/repo/issues/97"}));
                }
                panic!("unexpected {}", r.path);
            }
        }
    }).await;
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut s = configure(&factory, dir.path(), "task:");
    s.release_issue_title_prefix = "ship:".into();
    s.flows
        .iter_mut()
        .find(|f| f.id == "release")
        .unwrap()
        .enabled = true;
    factory.save_settings(s).unwrap();
    let id = factory
        .enqueue(common::from_source(
            TriggerEvent::release_issue("acme/repo", 97, "ship:queued", "alice", ""),
            "alice",
        ))
        .unwrap()
        .unwrap();
    let executing = tokio::spawn(run(factory.clone(), id.clone()));
    tokio::time::timeout(Duration::from_secs(5), reached.notified())
        .await
        .unwrap();
    let mut changed = factory.settings().unwrap();
    changed.release_issue_title_prefix = "deploy:".into();
    factory.save_settings(changed).unwrap();
    resume.notify_one();
    let error = tokio::time::timeout(Duration::from_secs(5), executing)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(format!("{error:#}").contains("Issue no longer eligible"));
    assert!(!dir.path().join("clone-called").exists());
    assert!(!dir.path().join("workspaces").join(id).exists());
}

#[tokio::test]
async fn release_prefix_execution_does_not_use_queued_snapshot() {
    let server = Server::start(|r| async move {
        if r.path == "/user" {
            return (200, json!({"login":"alice"}));
        }
        if r.path.ends_with("/issues/97") {
            return (
                200,
                json!({"state":"open","title":"ship:queued","user":{"login":"alice"}}),
            );
        }
        panic!("unexpected {}", r.path);
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open_with_api(dir.path().into(), &server.base).unwrap();
    let mut s = configure(&factory, dir.path(), "task:");
    s.release_issue_title_prefix = "ship:".into();
    s.flows
        .iter_mut()
        .find(|f| f.id == "release")
        .unwrap()
        .enabled = true;
    factory.save_settings(s).unwrap();
    let id = factory
        .enqueue(common::from_source(
            TriggerEvent::release_issue("acme/repo", 97, "ship:queued", "alice", ""),
            "alice",
        ))
        .unwrap()
        .unwrap();
    let mut changed = factory.settings().unwrap();
    changed.release_issue_title_prefix = "deploy:".into();
    factory.save_settings(changed).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), run(factory, id.clone()))
        .await
        .unwrap()
        .unwrap_err();
    assert!(format!("{error:#}").contains("Issue no longer eligible"));
    assert!(!dir.path().join("clone-called").exists());
    assert!(!dir.path().join("workspaces").join(id).exists());
}
