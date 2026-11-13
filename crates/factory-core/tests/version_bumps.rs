mod common;

use factory_core::{context::factory::infrastructure::rest_clients::github::Github, TriggerEvent};
use std::path::Path;

#[tokio::test]
async fn version_preparation_rejects_non_approval_before_network() {
    let dir = tempfile::tempdir().unwrap();
    let factory = common::open(dir.path().into()).unwrap();
    let api = Github::with_base("fixture-token", "http://127.0.0.1:1").unwrap();
    let mut event = TriggerEvent::issue("acme/repo", 5, "PR", "alice", "OK");
    event.trigger_id = "pr_review".into();
    let error = api
        .prepare_version_workspace(&factory, &event, "run", Path::new("/nonexistent"))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Version bump requires approved PR flow"),
        "{error:#}"
    );
}

#[cfg(unix)]
mod local {
    use super::*;
    use factory_core::*;
    use serde_json::{json, Value};
    use std::{
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::{Arc, Mutex},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn git(path: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    fn maybe_git(path: &Path, args: &[&str]) -> Option<String> {
        let output = std::process::Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap();
        output
            .status
            .success()
            .then(|| String::from_utf8(output.stdout).unwrap().trim().into())
    }

    struct Fixture {
        dir: tempfile::TempDir,
        factory: Arc<Factory>,
        api: Github,
        event: TriggerEvent,
        run: String,
        workspace: PathBuf,
        remote: PathBuf,
        merge_sha: String,
        writes: Arc<Mutex<Vec<Value>>>,
        server: tokio::task::JoinHandle<()>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source");
            std::fs::create_dir(&source).unwrap();
            git(&source, &["init", "-b", "main"]);
            git(&source, &["config", "user.name", "Alice"]);
            git(&source, &["config", "user.email", "alice@example.test"]);
            std::fs::write(source.join("VERSION"), "0.1.0\n").unwrap();
            git(&source, &["add", "."]);
            git(&source, &["commit", "-m", "initial"]);
            git(&source, &["checkout", "-b", "feature"]);
            std::fs::write(source.join("fix.txt"), "fixed\n").unwrap();
            git(&source, &["add", "."]);
            git(&source, &["commit", "-m", "fix bug"]);
            let head_sha = git(&source, &["rev-parse", "HEAD"]);
            git(&source, &["checkout", "main"]);
            git(&source, &["merge", "--squash", "feature"]);
            git(&source, &["commit", "-m", "squashed fix"]);
            let merge_sha = git(&source, &["rev-parse", "HEAD"]);
            let remote = dir.path().join("remote.git");
            let workspace = dir.path().join("workspace");
            git(
                dir.path(),
                &[
                    "clone",
                    "--bare",
                    source.to_str().unwrap(),
                    remote.to_str().unwrap(),
                ],
            );
            git(&remote, &["update-ref", "refs/pull/5/head", &head_sha]);
            git(
                dir.path(),
                &[
                    "clone",
                    remote.to_str().unwrap(),
                    workspace.to_str().unwrap(),
                ],
            );
            git(&workspace, &["config", "user.name", "Alice"]);
            git(&workspace, &["config", "user.email", "alice@example.test"]);
            let writes = Arc::new(Mutex::new(Vec::new()));
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let flags = dir.path().to_path_buf();
            let repo = remote.clone();
            let api_head = head_sha.clone();
            let api_merge = merge_sha.clone();
            let mutations = writes.clone();
            let server = tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let flags = flags.clone();
                    let repo = repo.clone();
                    let head = api_head.clone();
                    let merge = api_merge.clone();
                    let mutations = mutations.clone();
                    tokio::spawn(async move {
                        let mut bytes = Vec::new();
                        let (body_start, length) = loop {
                            let mut chunk = [0; 4096];
                            let n = socket.read(&mut chunk).await.unwrap();
                            if n == 0 {
                                return;
                            }
                            bytes.extend_from_slice(&chunk[..n]);
                            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                                let header = String::from_utf8_lossy(&bytes[..end]);
                                let length = header
                                    .lines()
                                    .find_map(|line| {
                                        line.to_lowercase()
                                            .strip_prefix("content-length:")
                                            .and_then(|s| s.trim().parse::<usize>().ok())
                                    })
                                    .unwrap_or(0);
                                break (end + 4, length);
                            }
                        };
                        while bytes.len() < body_start + length {
                            let mut chunk = [0; 4096];
                            let n = socket.read(&mut chunk).await.unwrap();
                            if n == 0 {
                                return;
                            }
                            bytes.extend_from_slice(&chunk[..n]);
                        }
                        let header = String::from_utf8_lossy(&bytes[..body_start]);
                        assert!(header
                            .to_ascii_lowercase()
                            .contains("authorization: bearer fixture-token"));
                        let mut parts = header.lines().next().unwrap().split_whitespace();
                        let method = parts.next().unwrap();
                        let path = parts.next().unwrap();
                        let body: Value =
                            serde_json::from_slice(&bytes[body_start..]).unwrap_or(Value::Null);
                        if method != "GET" {
                            mutations
                                .lock()
                                .unwrap()
                                .push(json!({"method":method,"path":path,"body":body}));
                        }
                        let (status, value) = match (method, path) {
                            ("GET", "/user") => (200, json!({"login":"alice","id":1})),
                            ("GET", "/repos/acme/repo") => (
                                200,
                                json!({"full_name":"acme/repo","default_branch":"main"}),
                            ),
                            ("GET", "/repos/acme/repo/pulls/5") => (
                                200,
                                json!({"number":5,"state":"closed","html_url":"https://github.com/acme/repo/pull/5","merged":!flags.join("unmerged").exists(),
                                "head":{"ref":"feature","sha":if flags.join("changed-head").exists(){"b".repeat(40)}else{head},"repo":{"full_name":"acme/repo"}},
                                "base":{"ref":"main","repo":{"full_name":"acme/repo"}}, "merge_commit_sha":merge}),
                            ),
                            ("GET", "/repos/acme/repo/git/ref/heads/main") => (
                                200,
                                json!({"ref":"refs/heads/main","object":{"type":"commit","sha":git(&repo,&["rev-parse","refs/heads/main"])}}),
                            ),
                            ("GET", "/repos/acme/repo/git/ref/heads/feature") => {
                                if let Some(sha) = maybe_git(
                                    &repo,
                                    &["show-ref", "--verify", "--hash", "refs/heads/feature"],
                                ) {
                                    (
                                        200,
                                        json!({"ref":"refs/heads/feature","object":{"type":"commit","sha":sha}}),
                                    )
                                } else {
                                    (404, json!({}))
                                }
                            }
                            ("DELETE", "/repos/acme/repo/git/refs/heads/feature") => {
                                git(&repo, &["update-ref", "-d", "refs/heads/feature"]);
                                (204, Value::Null)
                            }
                            ("GET", path) if path.starts_with("/search/issues?") => (
                                200,
                                json!({"total_count":0,"incomplete_results":false,"items":[]}),
                            ),
                            ("GET", path) if path.starts_with("/repos/acme/repo/compare/") => {
                                let (base, head) = path
                                    .strip_prefix("/repos/acme/repo/compare/")
                                    .unwrap()
                                    .split_once("...")
                                    .unwrap();
                                if let Some(ancestor) =
                                    maybe_git(&repo, &["merge-base", base, head])
                                {
                                    let status = if base == head {
                                        "identical"
                                    } else if ancestor == base {
                                        "ahead"
                                    } else {
                                        "diverged"
                                    };
                                    (
                                        200,
                                        json!({"base_commit":{"sha":base},"merge_base_commit":{"sha":ancestor},"status":status}),
                                    )
                                } else {
                                    (404, json!({}))
                                }
                            }
                            _ => panic!("Unexpected fixture request {method} {path}"),
                        };
                        let data = value.to_string();
                        let response = format!("HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",data.len());
                        let _ = socket.write_all(response.as_bytes()).await;
                    });
                }
            });
            let actual = std::process::Command::new("which")
                .arg("git")
                .output()
                .unwrap();
            let actual = String::from_utf8(actual.stdout).unwrap().trim().to_string();
            let wrapper = dir.path().join("git-wrapper");
            std::fs::write(
                &wrapper,
                format!(
                    r##"#!/bin/bash
args=()
for arg do
  if [ "$arg" = 'https://github.com/acme/repo.git' ]; then arg='{remote}'; fi
  args+=("$arg")
done
if [ "$1" = fetch ] || [ "$1" = push ]; then
  [ "$GIT_CONFIG_COUNT" = 4 ] || exit 81
  case "$GIT_CONFIG_VALUE_0" in 'AUTHORIZATION: basic '*) ;; *) exit 82;; esac
fi
if [ "$1" = push ]; then
  printf '%s\n' "$*" >> '{root}/pushes'
  if [ -f '{root}/advance-before-push' ]; then
    '{actual}' --git-dir='{remote}' update-ref refs/heads/main "$(cat '{root}/advance-before-push')"
    rm '{root}/advance-before-push'
  fi
  '{actual}' "${{args[@]}}" || exit $?
  if [ -f '{root}/lose-push-response' ]; then exit 83; fi
  exit 0
fi
exec '{actual}' "${{args[@]}}"
"##,
                    remote = remote.display(),
                    root = dir.path().display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
            let factory = common::open_with_api(dir.path().join("factory"), &base).unwrap();
            let mut settings = factory.settings().unwrap();
            settings.commands.git = wrapper.to_str().unwrap().into();
            settings.commands.opencode = dir.path().join("agent").to_str().unwrap().into();
            settings.poll_interval_seconds = 3600;
            let mut merge = settings.flows[1].nodes[0].clone();
            merge.id = "merge".into();
            merge.name = "Merge PR".into();
            merge.agent = "opencode".into();
            merge.depends_on.clear();
            merge.prompt = "Merge PR".into();
            let mut bump = settings.flows[1].nodes[1].clone();
            bump.agent = "opencode".into();
            bump.depends_on = vec!["merge".into()];
            settings.flows[1].nodes = vec![merge, bump];
            settings.token_profiles = vec![TokenProfile {
                id: "alice".into(),
                account: "alice".into(),
                label: "Alice".into(),
                owners: vec!["acme".into()],
                authors: vec!["alice".into()],
                token: Some("fixture-token".into()),
                ..Default::default()
            }];
            factory.save_settings(settings).unwrap();
            let mut event = TriggerEvent::issue("acme/repo", 5, "PR", "alice", "release");
            event.trigger_id = "pr_approved".into();
            event.head_sha = head_sha;
            event.url = "https://github.com/acme/repo/pull/5".into();
            event.event_id = format!("approval:acme/repo#5@{}", event.head_sha);
            event.payload = json!({"approval":{"command":"ok","actor":"alice","head_sha":event.head_sha,"comment_id":7}});
            let run = factory.enqueue(event.clone()).unwrap().unwrap();
            let db =
                rusqlite::Connection::open(dir.path().join("factory/factory.sqlite3")).unwrap();
            db.execute("INSERT INTO observations VALUES(?1,?2)",rusqlite::params![format!("pr-merge:{run}"),json!({"repository":event.repository,"number":event.number,"head_sha":event.head_sha}).to_string()]).unwrap();
            let api = Github::with_base("fixture-token", &base).unwrap();
            Self {
                dir,
                factory,
                api,
                event,
                run,
                workspace,
                remote,
                merge_sha,
                writes,
                server,
            }
        }

        fn observation(&self, key: &str) -> Option<Value> {
            let db = rusqlite::Connection::open(self.dir.path().join("factory/factory.sqlite3"))
                .unwrap();
            use rusqlite::OptionalExtension;
            db.query_row("SELECT json FROM observations WHERE key=?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .unwrap()
            .map(|s| serde_json::from_str(&s).unwrap())
        }

        async fn prepare(&self) -> Value {
            self.api
                .prepare_version_workspace(&self.factory, &self.event, &self.run, &self.workspace)
                .await
                .unwrap()
        }

        fn version_commit(&self) -> String {
            std::fs::write(self.workspace.join("VERSION"), "0.1.1\n").unwrap();
            git(
                &self.workspace,
                &["commit", "-am", "chore: bump version to 0.1.1"],
            );
            git(&self.workspace, &["rev-parse", "HEAD"])
        }

        fn action() -> Value {
            json!({"version":"0.1.1","bump":"patch","reason":"Merged PR fixes retry bug"})
        }
    }

    #[tokio::test]
    async fn version_requires_merged_matching_head_before_workspace_changes() {
        for flag in ["unmerged", "changed-head"] {
            let f = Fixture::new().await;
            std::fs::write(f.dir.path().join(flag), "").unwrap();
            let error = f
                .api
                .prepare_version_workspace(&f.factory, &f.event, &f.run, &f.workspace)
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains(if flag == "unmerged" {
                    "confirmed merged PR"
                } else {
                    "PR head changed"
                }),
                "{error:#}"
            );
            assert!(f
                .observation(&format!("version-workspace:{}", f.run))
                .is_none());
            assert!(!f.dir.path().join("pushes").exists());
        }
    }

    #[tokio::test]
    async fn publishes_only_version_commit_once_despite_lost_push_response() {
        let f = Fixture::new().await;
        let context = f.prepare().await;
        assert_eq!(context["completed"], false);
        assert_eq!(context["base_sha"], f.merge_sha);
        let version = f.version_commit();
        std::fs::write(f.dir.path().join("lose-push-response"), "").unwrap();
        f.api
            .publish_version_bump(
                &f.factory,
                &f.event,
                &f.run,
                &f.workspace,
                &Fixture::action(),
                &|| Ok(()),
            )
            .await
            .unwrap();
        assert_eq!(git(&f.remote, &["rev-parse", "refs/heads/main"]), version);
        assert_eq!(git(&f.remote, &["show", "main:VERSION"]), "0.1.1");
        assert!(git(&f.remote, &["tag", "--list"]).is_empty());
        assert!(f.writes.lock().unwrap().is_empty());
        assert_eq!(
            f.observation(&format!("pr-version:{}", f.run)).unwrap()["commit_sha"],
            version
        );
        let pushes = std::fs::read_to_string(f.dir.path().join("pushes")).unwrap();
        assert!(!pushes.contains("refs/tags"));
        assert!(pushes.contains(&format!(
            "--force-with-lease=refs/heads/main:{}",
            f.merge_sha
        )));
        assert_eq!(pushes.lines().count(), 1);
        assert_eq!(f.prepare().await["completed"], true);
        f.api
            .publish_version_bump(
                &f.factory,
                &f.event,
                &f.run,
                &f.workspace,
                &Fixture::action(),
                &|| Ok(()),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("pushes")).unwrap(),
            pushes
        );
    }

    #[tokio::test]
    async fn retry_recovers_pushed_commit_after_interruption_and_later_default_commits() {
        let f = Fixture::new().await;
        f.prepare().await;
        let version = f.version_commit();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let interrupted = || {
            if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                anyhow::bail!("fixture interrupted after push");
            }
            Ok(())
        };
        assert!(f
            .api
            .publish_version_bump(
                &f.factory,
                &f.event,
                &f.run,
                &f.workspace,
                &Fixture::action(),
                &interrupted
            )
            .await
            .is_err());
        assert!(f.observation(&format!("pr-version:{}", f.run)).is_none());
        assert_eq!(git(&f.remote, &["rev-parse", "refs/heads/main"]), version);
        git(
            &f.workspace,
            &["commit", "--allow-empty", "-m", "later merged PR"],
        );
        let later = git(&f.workspace, &["rev-parse", "HEAD"]);
        git(
            &f.workspace,
            &[
                "push",
                f.remote.to_str().unwrap(),
                &format!("{later}:refs/heads/main"),
            ],
        );
        f.factory.cancel(&f.run).unwrap();
        let retry = f.factory.retry(&f.run).unwrap();
        let retry_workspace = f.dir.path().join("retry-workspace");
        git(
            f.dir.path(),
            &[
                "clone",
                f.remote.to_str().unwrap(),
                retry_workspace.to_str().unwrap(),
            ],
        );
        let db = rusqlite::Connection::open(f.dir.path().join("factory/factory.sqlite3")).unwrap();
        db.execute("INSERT INTO observations VALUES(?1,?2)", rusqlite::params![format!("pr-merge:{retry}"), json!({"repository":f.event.repository,"number":f.event.number,"head_sha":f.event.head_sha}).to_string()]).unwrap();
        let completed = f
            .api
            .prepare_version_workspace(&f.factory, &f.event, &retry, &retry_workspace)
            .await
            .unwrap();
        assert_eq!(completed["completed"], true);
        assert_eq!(completed["receipt"]["commit_sha"], version);
        assert_eq!(
            f.observation(&format!("pr-version:{retry}")).unwrap()["commit_sha"],
            version
        );
        assert_eq!(git(&f.remote, &["rev-parse", "refs/heads/main"]), later);
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("pushes"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert!(git(&f.remote, &["tag", "--list"]).is_empty());
        assert!(f.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn already_included_bump_records_receipt_without_push() {
        let f = Fixture::new().await;
        f.prepare().await;
        // HEAD unchanged represents a bump already included in the approved diff.
        let action = json!({"version":"0.1.0","bump":"patch","reason":"Required bump already included in approved PR"});
        f.api
            .publish_version_bump(&f.factory, &f.event, &f.run, &f.workspace, &action, &|| {
                Ok(())
            })
            .await
            .unwrap();
        assert!(!f.dir.path().join("pushes").exists());
        assert_eq!(
            f.observation(&format!("pr-version:{}", f.run)).unwrap()["commit_sha"],
            f.merge_sha
        );
        assert!(git(&f.remote, &["tag", "--list"]).is_empty());
    }

    #[tokio::test]
    async fn preparation_uses_current_default_when_another_pr_has_merged() {
        let f = Fixture::new().await;
        git(
            &f.workspace,
            &[
                "commit",
                "--allow-empty",
                "-m",
                "another PR merged before bump",
            ],
        );
        let later = git(&f.workspace, &["rev-parse", "HEAD"]);
        git(
            &f.workspace,
            &[
                "push",
                f.remote.to_str().unwrap(),
                &format!("{later}:refs/heads/main"),
            ],
        );
        let context = f.prepare().await;
        assert_eq!(context["base_sha"], later);
        assert_eq!(context["merge_sha"], f.merge_sha);
        assert_eq!(git(&f.workspace, &["rev-parse", "HEAD"]), later);
        let version = f.version_commit();
        f.api
            .publish_version_bump(
                &f.factory,
                &f.event,
                &f.run,
                &f.workspace,
                &Fixture::action(),
                &|| Ok(()),
            )
            .await
            .unwrap();
        assert_eq!(git(&f.remote, &["rev-parse", "main"]), version);
        assert_eq!(git(&f.remote, &["rev-parse", "main^"]), later);
    }

    #[tokio::test]
    async fn version_push_never_follows_local_tags_even_if_git_config_enables_it() {
        let f = Fixture::new().await;
        f.prepare().await;
        f.version_commit();
        git(
            &f.workspace,
            &[
                "tag",
                "--annotate",
                "v0.1.0",
                "--message",
                "old local tag",
                &f.merge_sha,
            ],
        );
        git(&f.workspace, &["config", "push.followTags", "true"]);
        f.api
            .publish_version_bump(
                &f.factory,
                &f.event,
                &f.run,
                &f.workspace,
                &Fixture::action(),
                &|| Ok(()),
            )
            .await
            .unwrap();
        assert!(git(&f.remote, &["tag", "--list"]).is_empty());
    }

    #[tokio::test]
    async fn rejected_lease_can_retry_from_new_default_without_overwriting_other_bump() {
        let f = Fixture::new().await;
        f.prepare().await;
        f.version_commit();
        let other = f.dir.path().join("other-workspace");
        git(
            f.dir.path(),
            &["clone", f.remote.to_str().unwrap(), other.to_str().unwrap()],
        );
        git(&other, &["config", "user.name", "Alice"]);
        git(&other, &["config", "user.email", "alice@example.test"]);
        std::fs::write(other.join("VERSION"), "0.2.0\n").unwrap();
        git(&other, &["commit", "-am", "another PR and its minor bump"]);
        let later = git(&other, &["rev-parse", "HEAD"]);
        // Make object available before moving the default ref inside the push wrapper.
        git(
            &other,
            &["push", f.remote.to_str().unwrap(), "HEAD:refs/heads/other"],
        );
        std::fs::write(f.dir.path().join("advance-before-push"), &later).unwrap();
        let error = f
            .api
            .publish_version_bump(
                &f.factory,
                &f.event,
                &f.run,
                &f.workspace,
                &Fixture::action(),
                &|| Ok(()),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("push"), "{error:#}");
        assert_eq!(git(&f.remote, &["rev-parse", "main"]), later);
        assert!(f.observation(&format!("pr-version:{}", f.run)).is_none());
        let context = f.prepare().await;
        assert_eq!(context["base_sha"], later);
        assert_eq!(git(&f.workspace, &["show", "HEAD:VERSION"]), "0.2.0");
        std::fs::write(f.workspace.join("VERSION"), "0.2.1\n").unwrap();
        git(&f.workspace, &["commit", "-am", "recomputed patch bump"]);
        let action = json!({"version":"0.2.1","bump":"patch","reason":"PR fixes retry bug; current version advanced to 0.2.0"});
        f.api
            .publish_version_bump(&f.factory, &f.event, &f.run, &f.workspace, &action, &|| {
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(git(&f.remote, &["rev-parse", "main^"]), later);
        assert_eq!(git(&f.remote, &["show", "main:VERSION"]), "0.2.1");
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("pushes"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        assert!(git(&f.remote, &["tag", "--list"]).is_empty());
    }

    #[tokio::test]
    async fn preparation_rejects_default_that_does_not_contain_approved_merge() {
        let f = Fixture::new().await;
        let old = git(&f.remote, &["rev-parse", "main^"]);
        git(&f.remote, &["update-ref", "refs/heads/main", &old]);
        let error = f
            .api
            .prepare_version_workspace(&f.factory, &f.event, &f.run, &f.workspace)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not contain approved PR merge"),
            "{error:#}"
        );
        assert!(!f.dir.path().join("pushes").exists());
    }

    #[tokio::test]
    async fn version_rejects_changed_default_branch_before_any_push() {
        let f = Fixture::new().await;
        f.prepare().await;
        let version = f.version_commit();
        git(
            &f.workspace,
            &[
                "push",
                f.remote.to_str().unwrap(),
                &format!("{version}:refs/heads/main"),
            ],
        );
        let error = f
            .api
            .publish_version_bump(
                &f.factory,
                &f.event,
                &f.run,
                &f.workspace,
                &Fixture::action(),
                &|| Ok(()),
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("Default branch moved"),
            "{error:#}"
        );
        assert!(f.writes.lock().unwrap().is_empty());
        assert!(!f.dir.path().join("pushes").exists());
    }

    #[tokio::test]
    async fn caller_cannot_change_stored_approval() {
        let f = Fixture::new().await;
        let mut forged = f.event.clone();
        forged.payload["approval"]["command"] = json!("review");
        let error = f
            .api
            .prepare_version_workspace(&f.factory, &forged, &f.run, &f.workspace)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Stored run lacks matching approval"),
            "{error:#}"
        );
        assert!(f
            .observation(&format!("version-workspace:{}", f.run))
            .is_none());
    }

    #[tokio::test]
    async fn runtime_merges_then_bumps_without_tag_or_release() {
        let f = Fixture::new().await;
        let agent = f.dir.path().join("agent");
        std::fs::write(&agent, r#"#!/bin/sh
set -eu
cat > "$AI_FACTORY_MEMORY_DIR/$AI_FACTORY_NODE_ID.prompt"
if [ "$AI_FACTORY_NODE_ID" = merge ]; then
  printf '%s' '{"status":"succeeded","summary":"Merged approved PR","artifacts":[],"data":{"merge":{"method":"squash"}}}' > "$AI_FACTORY_RESULT_PATH"
  exit 0
fi
[ "$AI_FACTORY_NODE_ID" = version_bump ]
printf '%s' "$AI_FACTORY_VERSION_CONTEXT" > "$AI_FACTORY_MEMORY_DIR/version-context.json"
case "$(git branch --show-current)" in ai-factory/version/*) ;; *) exit 91;; esac
[ "$(cat VERSION)" = 0.1.0 ]
printf '0.1.1\n' > VERSION
git -c user.name=Alice -c user.email=alice@example.test commit -am 'chore: bump version to 0.1.1'
printf '%s' '{"status":"succeeded","summary":"Prepared version bump","artifacts":[],"data":{"bump_version":{"version":"0.1.1","bump":"patch","reason":"Merged PR fixes retry bug"}}}' > "$AI_FACTORY_RESULT_PATH"
"#).unwrap();
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let db = rusqlite::Connection::open(f.dir.path().join("factory/factory.sqlite3")).unwrap();
        db.execute(
            "DELETE FROM observations WHERE key=?1",
            [format!("pr-merge:{}", f.run)],
        )
        .unwrap();
        f.factory.start();
        let detail = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                let detail = f.factory.run_detail(&f.run).unwrap();
                if !["queued", "running"].contains(&detail.run.status.as_str()) {
                    break detail;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        f.factory.shutdown().await;
        assert_eq!(detail.run.status, "succeeded", "{:?}", detail.run.error);
        assert!(
            detail.nodes.iter().all(|node| node.status == "succeeded"),
            "{:?}",
            detail.nodes
        );
        let receipt = f.observation(&format!("pr-version:{}", f.run)).unwrap();
        assert_eq!(receipt["version"], "0.1.1");
        let target = git(&f.remote, &["rev-parse", "refs/heads/main"]);
        assert_ne!(target, f.merge_sha);
        assert_eq!(receipt["commit_sha"], target);
        assert_eq!(git(&f.remote, &["show", "main:VERSION"]), "0.1.1");
        assert!(git(&f.remote, &["tag", "--list"]).is_empty());
        assert!(maybe_git(&f.remote, &["show-ref", "--verify", "refs/heads/feature"]).is_none());
        assert_eq!(
            *f.writes.lock().unwrap(),
            vec![
                json!({"method":"DELETE","path":"/repos/acme/repo/git/refs/heads/feature","body":null})
            ]
        );
    }
}
