use crate::{FlowNode, TriggerEvent};
use std::collections::BTreeSet;
use tokio::process::Command;

pub(super) fn allowed(event: &TriggerEvent, node: &FlowNode) -> bool {
    event.trigger_id == "release" && node.id == "publish_app" && node.access == "write"
}

pub(super) fn instructions(event: &TriggerEvent, node: &FlowNode) -> &'static str {
    if allowed(event, node) {
        "Release publication authorization: this write-capable release/publish_app node may use gh, read remote Git refs, create and push the repository's existing-version release tag, create a GitHub Release when required by its configured distribution procedure, and trigger and monitor its deployment pipeline. This authorization overrides older node prompts that prohibit these publication operations. Use only the supplied actor credentials and event repository; GH_TOKEN authenticates gh and the temporary Git credential helper. Never print credentials or persist them in files, command arguments, Git remotes or Git config. Never force-push, overwrite or delete a tag, push branches, merge PRs, create source/version commits or change versions. Verify the publication commit and tag against project conventions; reuse a matching remote tag and existing deployment rather than publishing twice. A tag pointing elsewhere blocks publication. Inspect GitHub Actions secrets metadata and pipeline evidence when the configured procedure runs on GitHub Actions; missing local Docker/Kubernetes credentials do not establish missing GitHub Actions secrets. Verify required runs and distribution evidence before success. Return data.distribution_result.published=true only after verified publication. Factory closes the source Release issue as completed after every workflow step succeeds; do not close it through gh or other direct operations. A closure failure is reported separately while preserving publication evidence. Do not use reserved Factory action keys for these direct publication operations; report distribution_result and verification instead."
    } else {
        "Never use gh CLI. GitHub mutations are handled by Factory using actor token, return actions in data: publish={title,body}, update_pr={}, merge={method}, review={event,body}, comment=string, bump_version={version,bump,reason}. Never create tags or GitHub releases. Never push or fetch remotely."
    }
}

pub(super) fn configure_auth(command: &mut Command, repository: &str, token: &str) {
    command
        .env("GH_TOKEN", token)
        .env("GH_HOST", "github.com")
        .env("GH_REPO", repository)
        .env("GIT_CONFIG_COUNT", "5")
        .env("GIT_CONFIG_KEY_4", "credential.https://github.com.helper")
        .env(
            "GIT_CONFIG_VALUE_4",
            "!f() { if [ \"$1\" = get ]; then printf '%s\\n' 'username=x-access-token' \"password=$GH_TOKEN\"; fi; }; f",
        );
}

pub(super) fn configure_codex(command: &mut Command) -> anyhow::Result<()> {
    // Retain normal inherited non-secret variables and the assigned actor token.
    // Codex otherwise filters GH_TOKEN along with all other *TOKEN* variables.
    let names: BTreeSet<_> = std::env::vars_os()
        .map(|(name, _)| name)
        .chain(
            command
                .as_std()
                .get_envs()
                .filter_map(|(name, value)| value.map(|_| name.to_owned())),
        )
        .filter_map(|name| name.into_string().ok())
        .filter(|name| {
            let upper = name.to_ascii_uppercase();
            name == "GH_TOKEN"
                || name.starts_with("GIT_CONFIG_KEY_")
                || !["KEY", "SECRET", "TOKEN"]
                    .iter()
                    .any(|part| upper.contains(part))
        })
        .collect();
    for setting in [
        "sandbox_workspace_write.network_access=true".to_string(),
        "shell_environment_policy.inherit=\"all\"".to_string(),
        "shell_environment_policy.ignore_default_excludes=true".to_string(),
        format!(
            "shell_environment_policy.include_only={}",
            serde_json::to_string(&names)?
        ),
    ] {
        command.args(["--config", &setting]);
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, io::Write, process::Stdio};

    #[test]
    fn codex_shell_environment_keeps_git_auth_working_without_other_tokens() {
        let mut command = Command::new("fake-codex");
        for (index, (key, value)) in [
            ("user.name", "alice"),
            ("user.email", "alice@example.com"),
            ("user.useConfigOnly", "true"),
            ("credential.helper", ""),
        ]
        .into_iter()
        .enumerate()
        {
            command.env(format!("GIT_CONFIG_KEY_{index}"), key);
            command.env(format!("GIT_CONFIG_VALUE_{index}"), value);
        }
        configure_auth(&mut command, "acme/repo", "fixture-token");
        command.env("OTHER_TOKEN", "unrelated-secret");
        configure_codex(&mut command).unwrap();
        let names: BTreeSet<String> = command
            .as_std()
            .get_args()
            .filter_map(|arg| {
                arg.to_str()?
                    .strip_prefix("shell_environment_policy.include_only=")
            })
            .map(|json| serde_json::from_str(json).unwrap())
            .next()
            .unwrap();
        assert!(!names.contains("OTHER_TOKEN"));
        let mut env: BTreeMap<_, _> = std::env::vars_os().collect();
        for (name, value) in command.as_std().get_envs() {
            if let Some(value) = value {
                env.insert(name.to_owned(), value.to_owned());
            } else {
                env.remove(name);
            }
        }
        env.retain(|name, _| name.to_str().is_some_and(|name| names.contains(name)));
        let directory = tempfile::tempdir().unwrap();
        let mut git = std::process::Command::new("git")
            .args(["credential", "fill"])
            .current_dir(directory.path())
            .env_clear()
            .envs(env)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        git.stdin
            .take()
            .unwrap()
            .write_all(b"protocol=https\nhost=github.com\n\n")
            .unwrap();
        let output = git.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let credentials = String::from_utf8(output.stdout).unwrap();
        assert!(credentials
            .lines()
            .any(|line| line == "username=x-access-token"));
        assert!(credentials
            .lines()
            .any(|line| line == "password=fixture-token"));
    }
}
