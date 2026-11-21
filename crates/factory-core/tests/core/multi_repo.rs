use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

fn child(repo: &str, number: u64) -> serde_json::Value {
    json!({
        "repository_url":format!("https://api.github.com/repos/{repo}"),
        "html_url":format!("https://github.com/{repo}/issues/{number}"),
        "number":number,"state":"open","title":format!("Work item {number}"),"body":"Implement filter"
    })
}

fn multi_repo_git(dir: &FixtureDir, f: &Factory) {
    let wrapper = dir.path().join("multi-git");
    fs::write(
        &wrapper,
        format!(
            r#"#!/bin/sh
if [ "$1" = clone ]; then
  for arg do old="$last"; last="$arg"; done
  printf '%s\n' "$old" >> '{root}/clones'
  case "$old" in
    https://github.com/acme/api.git|https://github.com/acme/front.git)
      name="${{old##*/}}"
      source='{root}/source'
      if [ -d '{root}/'"$name" ]; then source='{root}/'"$name"; fi
      exec git clone --no-hardlinks -- "$source" "$last";;
    *) exit 61;;
  esac
fi
if [ "$1" = push ]; then
  name="${{4##*/}}"
  if [ "$name" = front.git ] && [ -f '{root}/fail-front-push' ]; then
    rm '{root}/fail-front-push'
    exit 66
  fi
  exec git push "$2" -- '{root}/'"$name" "$5"
fi
if [ "$1" = fetch ]; then
  case "$4" in https://github.com/acme/*.git)
    name="${{4##*/}}"
    exec git fetch "$2" -- '{root}/'"$name" "$5";;
  esac
fi
exec git "$@"
"#,
            root = dir.path().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = f.settings().unwrap();
    settings.commands.git = wrapper.to_string_lossy().into_owned();
    settings.issue_node_comments_enabled = false;
    settings.poll_interval_seconds = 3600;
    f.save_settings(settings).unwrap();
}

fn fixture_git(dir: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[tokio::test]
async fn retries_only_unpublished_repo_and_closes_parent_after_all_merges() {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    let (dir, f) = fixture(
        r#"set -eu
cat >/dev/null
for repo in api front; do
  checkout="repos/acme/$repo"
  printf '%s\n' "$repo" > "$checkout/filter"
  git -C "$checkout" add -- filter
  git -C "$checkout" commit -m 'Add filter'
done
printf '%s' '{"status":"succeeded","summary":"ready","artifacts":[],"data":{"publish":{"title":"Add filter","body":"Implement filter"}}}' > "$AI_FACTORY_RESULT_PATH""#,
        1,
    );
    *dir.sub_issues.lock().unwrap() = json!([child("acme/api", 18), child("acme/front", 31)]);
    *dir.issue.lock().unwrap() = json!({
        "state":"open", "title":"[AI] Add status filter", "user":{"login":"alice"},
        "number":42, "html_url":"https://github.com/acme/repo/issues/42"
    });
    let mut settings = f.settings().unwrap();
    settings.flows[0].nodes[0].id = "publish".into();
    f.save_settings(settings).unwrap();
    multi_repo_git(&dir, &f);
    for repo in ["api.git", "front.git"] {
        fixture_git(dir.path(), &["clone", "--bare", "source", repo]);
    }
    fs::write(dir.path().join("fail-front-push"), "").unwrap();
    let prs = Arc::new(Mutex::new(BTreeMap::<String, serde_json::Value>::new()));
    let mock_prs = prs.clone();
    let root = dir.path().to_path_buf();
    *dir.route.lock().unwrap() = Some(Box::new(move |method, path, body| {
        if path.starts_with("/search/issues?") {
            return Some((
                200,
                json!({"items":[],"total_count":0,"incomplete_results":false}),
            ));
        }
        let segments: Vec<_> = path.split('/').collect();
        let (Some(repo @ ("api" | "front")), Some(resource)) =
            (segments.get(3).copied(), segments.get(4).copied())
        else {
            return None;
        };
        let mut prs = mock_prs.lock().unwrap();
        if resource.starts_with("pulls?") && method == "GET" {
            return Some((200, json!(prs.get(repo).into_iter().collect::<Vec<_>>())));
        }
        if resource == "git" && path.contains("/ref/heads/ai-factory/") {
            return Some((404, json!({})));
        }
        if resource == "pulls" && method == "POST" {
            let branch = body["head"].as_str().unwrap();
            let sha = fixture_git(
                &root.join(format!("{repo}.git")),
                &["rev-parse", &format!("refs/heads/{branch}")],
            );
            let pr = json!({"number":7,"html_url":format!("https://github.com/acme/{repo}/pull/7"),"body":body["body"],"state":"open","merged":false,
                "head":{"ref":branch,"sha":sha,"repo":{"full_name":format!("acme/{repo}")}},
                "base":{"ref":body["base"],"repo":{"full_name":format!("acme/{repo}")}},
                "user":{"login":"alice"}});
            assert!(
                prs.insert(repo.into(), pr.clone()).is_none(),
                "duplicate PR"
            );
            return Some((201, pr));
        }
        if resource == "pulls" && segments.get(5) == Some(&"7") {
            return Some((200, prs[repo].clone()));
        }
        None
    }));
    let id = enqueue_parent(&f);
    f.start();
    let first = terminal(&f, &id).await;
    assert_eq!(first.run.status, "failed", "{:?}", first.run.error);
    assert_eq!(prs.lock().unwrap().len(), 1);
    assert!(first.run.workspace.is_none());
    dir.issue.lock().unwrap()["state"] = json!("closed");
    let closed_attempt = f.retry(&id).unwrap();
    let blocked = terminal(&f, &closed_attempt).await;
    assert_eq!(blocked.run.status, "failed");
    assert!(blocked
        .run
        .error
        .unwrap()
        .contains("Parent issue is no longer eligible"));
    assert_eq!(prs.lock().unwrap().len(), 1);
    dir.issue.lock().unwrap()["state"] = json!("open");
    let retry = f.retry(&closed_attempt).unwrap();
    let next = terminal(&f, &retry).await;
    assert_eq!(next.run.status, "succeeded", "{:?}", next.run.error);
    assert_eq!(prs.lock().unwrap().len(), 2);
    assert_eq!(
        fs::read_to_string(dir.path().join("clones"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    for repository in ["api", "front"] {
        let prs = prs.lock().unwrap();
        let body = prs[repository]["body"].as_str().unwrap();
        assert!(body.contains("acme/repo#42"), "{body}");
        assert!(
            !body.to_lowercase().contains("closes acme/repo#42"),
            "{body}"
        );
    }
    prs.lock().unwrap().get_mut("api").unwrap()["merged"] = json!(true);
    f.poll_now().await.unwrap();
    assert_eq!(dir.issue.lock().unwrap()["state"], "open");
    prs.lock().unwrap().get_mut("front").unwrap()["merged"] = json!(true);
    f.poll_now().await.unwrap();
    assert_eq!(dir.issue.lock().unwrap()["state"], "open");
    *dir.sub_issues.lock().unwrap() = json!([
        {"state": "closed"},
        {"state": "closed"},
        child("acme/front", 57)
    ]);
    f.poll_now().await.unwrap();
    assert_eq!(dir.issue.lock().unwrap()["state"], "open");
    *dir.sub_issues.lock().unwrap() = json!([{"state": "closed"}, {"state": "closed"}]);
    f.poll_now().await.unwrap();
    assert_eq!(dir.issue.lock().unwrap()["state"], "closed");
    f.poll_now().await.unwrap();
    {
        let requests = dir.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|(method, _, _)| method == "PATCH")
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|(method, path, _)| method == "POST" && path.ends_with("/pulls"))
                .count(),
            2
        );
    }
    f.shutdown().await;
}

fn enqueue_parent(f: &Factory) -> String {
    f.enqueue(common::from_source(
        TriggerEvent::issue(
            "acme/repo",
            42,
            "[AI] Add status filter",
            "alice",
            "Add filter",
        ),
        "alice",
    ))
    .unwrap()
    .unwrap()
}

#[tokio::test]
async fn subissues_define_shared_workspace_and_skip_parent_clone() {
    let (dir, f) = fixture(
        r#"set -eu
prompt=$(cat)
case " $* " in *" --skip-git-repo-check "*) ;; *) exit 63;; esac
for repo in api front; do
  test -d "repos/acme/$repo/.git"
done
test ! -e "repos/acme/repo"
test ! -e ".git"
case "$prompt" in *"Work item 18"*"Work item 19"*"Work item 31"*) ;; *) exit 62;; esac
printf '%s' '{"status":"succeeded","summary":"scoped","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH""#,
        1,
    );
    *dir.sub_issues.lock().unwrap() = json!([
        child("acme/front", 31),
        child("acme/api", 18),
        child("acme/api", 19)
    ]);
    multi_repo_git(&dir, &f);
    let id = enqueue_parent(&f);
    f.start();
    let detail = terminal(&f, &id).await;
    assert_eq!(detail.run.status, "succeeded", "{:?}", detail.run.error);
    assert_eq!(
        fs::read_to_string(dir.path().join("clones")).unwrap(),
        "https://github.com/acme/api.git\nhttps://github.com/acme/front.git\n"
    );
    assert!(detail.run.workspace.is_none());
    f.shutdown().await;
}

#[tokio::test]
async fn a_failed_step_after_the_publisher_prevents_remote_publication() {
    let (dir, f) = fixture(
        r#"set -eu
cat >/dev/null
if [ "$AI_FACTORY_NODE_ID" = publish ]; then
  printf '%s' '{"status":"succeeded","summary":"ready","artifacts":[],"data":{"publish":{"title":"Add filter","body":"Implement filter"}}}' > "$AI_FACTORY_RESULT_PATH"
else
  printf '%s' '{"status":"failed","summary":"integration check failed","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH"
fi"#,
        1,
    );
    *dir.sub_issues.lock().unwrap() = json!([child("acme/api", 18)]);
    multi_repo_git(&dir, &f);
    let mut settings = f.settings().unwrap();
    let flow = &mut settings.flows[0];
    flow.nodes[0].id = "publish".into();
    let mut check = flow.nodes[0].clone();
    check.id = "integration".into();
    check.depends_on = vec!["publish".into()];
    flow.nodes.push(check);
    f.save_settings(settings).unwrap();
    let id = enqueue_parent(&f);
    f.start();
    let detail = terminal(&f, &id).await;
    assert_eq!(detail.run.status, "failed");
    assert_eq!(detail.nodes[0].status, "succeeded");
    assert_eq!(detail.nodes[1].status, "failed");
    assert!(dir
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|(_, path, _)| !path.contains("/pulls")));
    f.shutdown().await;
}

#[tokio::test]
async fn single_repository_retry_reuses_the_resolved_scope() {
    let (dir, f) = fixture(
        r#"cat >/dev/null
printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH""#,
        1,
    );
    let id = enqueue_parent(&f);
    f.start();
    assert_eq!(terminal(&f, &id).await.run.status, "succeeded");
    *dir.sub_issues.lock().unwrap() = json!([child("acme/api", 18)]);
    let retry = f.retry(&id).unwrap();
    let detail = terminal(&f, &retry).await;
    assert_eq!(detail.run.status, "succeeded", "{:?}", detail.run.error);
    assert!(detail.issue_work.is_none());
    let clones = fs::read_to_string(dir.path().join("clone-args.log")).unwrap();
    assert_eq!(
        clones.matches("https://github.com/acme/repo.git").count(),
        2
    );
    f.shutdown().await;
}

#[tokio::test]
async fn retry_can_discover_work_after_a_subissue_reopens() {
    let (dir, f) = fixture(
        r#"cat >/dev/null
printf '%s' '{"status":"succeeded","summary":"done","artifacts":[],"data":{}}' > "$AI_FACTORY_RESULT_PATH""#,
        1,
    );
    let mut closed_child = child("acme/api", 18);
    closed_child["state"] = json!("closed");
    *dir.sub_issues.lock().unwrap() = json!([closed_child]);
    multi_repo_git(&dir, &f);
    let id = enqueue_parent(&f);
    f.start();
    assert_eq!(terminal(&f, &id).await.run.status, "blocked");
    assert!(!dir.path().join("clones").exists());
    *dir.sub_issues.lock().unwrap() = json!([child("acme/api", 18)]);
    let retry = f.retry(&id).unwrap();
    assert_eq!(terminal(&f, &retry).await.run.status, "succeeded");
    assert_eq!(
        fs::read_to_string(dir.path().join("clones")).unwrap(),
        "https://github.com/acme/api.git\n"
    );
    f.shutdown().await;
}

#[tokio::test]
async fn inaccessible_or_malformed_subissues_never_fall_back_to_parent() {
    for reply in [json!([child("outside/api", 18)]), json!({})] {
        let (dir, f) = fixture("exit 62", 1);
        *dir.sub_issues.lock().unwrap() = reply;
        multi_repo_git(&dir, &f);
        let id = enqueue_parent(&f);
        f.start();
        let detail = terminal(&f, &id).await;
        assert_eq!(detail.run.status, "blocked", "{:?}", detail.run.error);
        assert!(!dir.path().join("clones").exists());
        f.shutdown().await;
    }
}
