use super::agents::{clean_env, resolve_command, ProcessTree};
use crate::{context::factory::infrastructure::filesystem::skill_catalog, InstalledSkill, Result};
use anyhow::{bail, ensure, Context};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;

pub(crate) struct SkillPolicy {
    selected: Option<Vec<InstalledSkill>>,
    codex_rules: Vec<(PathBuf, bool)>,
}

impl SkillPolicy {
    pub async fn prepare(
        agent: &str,
        executable: &str,
        selection: Option<&[String]>,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        let Some(selection) = selection else {
            return Ok(Self {
                selected: None,
                codex_rules: vec![],
            });
        };
        let catalog = if selection.is_empty() {
            vec![]
        } else {
            skill_catalog::list_for_workspace(workspace)
        };
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for id in selection {
            ensure!(
                !id.is_empty() && !id.chars().any(|c| c.is_control() || "*?[]\\".contains(c)),
                "Invalid selected skill ID: {id}"
            );
            if !seen.insert(id) {
                continue;
            }
            let skill = catalog.iter().find(|skill| &skill.id == id)
                .with_context(|| format!("Selected skill is not installed: {id}. Update this node's skill selection."))?;
            ensure!(
                !skill
                    .opencode_id
                    .chars()
                    .any(|c| c.is_control() || "*?[]\\".contains(c)),
                "Invalid selected skill tool ID: {id}"
            );
            ensure!(
                skill.manifest_path.is_file(),
                "Selected skill is not installed: {id}"
            );
            let mut resolved = skill.clone();
            resolved.manifest_path = skill
                .manifest_path
                .canonicalize()
                .with_context(|| format!("Cannot read selected skill: {id}"))?;
            selected.push(resolved);
        }
        let mut codex_rules = Vec::new();
        if agent == "codex" {
            for path in discover_codex(executable, workspace, cancel).await? {
                let enabled = selected
                    .iter()
                    .any(|skill| same_path(&path, &skill.manifest_path));
                codex_rules.push((path, enabled));
            }
            // Explicit paths also cover selected files exposed through the policy below
            // but not automatically advertised by this Codex installation.
            for skill in &selected {
                if !codex_rules
                    .iter()
                    .any(|(path, _)| same_path(path, &skill.manifest_path))
                {
                    codex_rules.push((skill.manifest_path.clone(), true));
                }
            }
        }
        Ok(Self {
            selected: Some(selected),
            codex_rules,
        })
    }

    pub fn prompt(&self) -> Result<String> {
        let Some(selected) = &self.selected else {
            return Ok(String::new());
        };
        if selected.is_empty() {
            return Ok("AI Factory skill selection (workflow owner's instruction):\nNo skills are enabled for this node. Do not invoke, discover or read installed skills. Complete the assigned role using the task and repository guidance directly.".into());
        }
        let entries: Vec<_> = selected
            .iter()
            .map(|skill| {
                json!({
                    "id":skill.id,"tool_id":skill.opencode_id,"name":skill.name,"description":skill.description,
                    "path":skill.manifest_path
                })
            })
            .collect();
        Ok(format!(
            "AI Factory skill selection (workflow owner's instruction):\nOnly the skills listed below are enabled for this node. Use them when relevant to the assigned role; selection does not assign extra workflow phases. Load each skill at most once while its instructions remain in context. Use its exact tool_id with the skill tool, or read its exact SKILL.md path if that tool_id is not advertised by this CLI. Resolve relative references and scripts from that manifest's directory. Do not discover, invoke or read other skills, including dependencies recommended by an enabled skill unless they also appear here. A reference to an unselected skill does not authorize it.\n{}",
            serde_json::to_string_pretty(&entries)?
        ))
    }

    pub fn configure_codex(&self, command: &mut Command) -> Result<()> {
        if self.selected.is_none() {
            return Ok(());
        }
        command.arg("--strict-config");
        let rules = self
            .codex_rules
            .iter()
            .map(|(path, enabled)| {
                Ok(format!(
                    "{{path={},enabled={enabled}}}",
                    serde_json::to_string(path)?
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        command
            .arg("--config")
            .arg(format!("skills.config=[{}]", rules.join(",")));
        // The explicit catalog in the node prompt replaces the automatic instructions.
        command
            .arg("--config")
            .arg("skills.include_instructions=false");
        Ok(())
    }

    pub fn configure_opencode(&self, permissions: &mut Value, allow_resources: bool) {
        let Some(selected) = &self.selected else {
            return;
        };
        let mut skill_rules = serde_json::Map::new();
        skill_rules.insert("*".into(), json!("deny"));
        for skill in selected {
            skill_rules.insert(skill.opencode_id.clone(), json!("allow"));
            if allow_resources {
                if let Some(directory) = skill.manifest_path.parent() {
                    let pattern = format!("{}/**", directory.display());
                    // Skills and their supporting references are read-only inputs.
                    permissions["external_directory"][&pattern] = json!("allow");
                    set_path_permission(permissions, "read", &pattern, "allow");
                    set_path_permission(permissions, "edit", &pattern, "deny");
                }
            }
        }
        permissions["skill"] = Value::Object(skill_rules);
    }
}

fn set_path_permission(permissions: &mut Value, action: &str, path: &str, effect: &str) {
    if !permissions[action].is_object() {
        let default = permissions[action]
            .as_str()
            .or_else(|| permissions["*"].as_str())
            .unwrap_or("deny")
            .to_string();
        permissions[action] = json!({"*":default});
    }
    permissions[action][path] = json!(effect);
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || match (left.canonicalize(), right.canonicalize()) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
}

async fn discover_codex(
    executable: &str,
    workspace: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<PathBuf>> {
    let mut command = Command::new(resolve_command(executable));
    command
        .args(["app-server", "--stdio"])
        .current_dir(workspace)
        .env("PWD", workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    clean_env(&mut command);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    let mut child = command
        .spawn()
        .context("Cannot enumerate Codex skills before applying selection")?;
    let mut tree = ProcessTree::attach(&mut child)?;
    let mut input = child
        .stdin
        .take()
        .context("Codex skill discovery input unavailable")?;
    let output = child
        .stdout
        .take()
        .context("Codex skill discovery output unavailable")?;
    let mut lines = BufReader::new(output.take(16 * 1024 * 1024)).lines();
    let discovery = tokio::time::timeout(Duration::from_secs(20), async {
        input.write_all(b"{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"ai_factory\",\"version\":\"0.2.0\"}}}\n").await?;
        response(&mut lines, 1).await?;
        input.write_all(b"{\"method\":\"initialized\"}\n").await?;
        let request =
            json!({"id":2,"method":"skills/list","params":{"cwds":[workspace],"forceReload":true}});
        input.write_all(format!("{request}\n").as_bytes()).await?;
        let page = response(&mut lines, 2).await?;
        let entries = page["data"]
            .as_array()
            .context("Invalid Codex skill catalog")?;
        ensure!(
            !entries.is_empty(),
            "Codex returned no skill catalog for this workspace"
        );
        let mut paths = Vec::new();
        for entry in entries {
            for skill in entry["skills"]
                .as_array()
                .context("Invalid Codex workspace skill catalog")?
            {
                let path =
                    PathBuf::from(skill["path"].as_str().context("Codex skill path missing")?);
                ensure!(path.is_absolute(), "Codex skill path must be absolute");
                paths.push(path);
            }
        }
        Ok::<_, anyhow::Error>(paths)
    });
    let result = tokio::select! {
        _ = cancel.cancelled() => Err(anyhow::anyhow!("Run cancelled during Codex skill discovery")),
        result = discovery => result.context("Codex skill discovery timed out").and_then(|result| result),
    };
    tree.terminate();
    let _ = child.kill().await;
    result
}

async fn response<R: tokio::io::AsyncBufRead + Unpin>(
    lines: &mut tokio::io::Lines<R>,
    id: u64,
) -> Result<Value> {
    for _ in 0..512 {
        let line = lines
            .next_line()
            .await?
            .context("Codex stopped during skill discovery")?;
        let message: Value =
            serde_json::from_str(&line).context("Invalid Codex skill discovery protocol")?;
        if message["id"] != id {
            continue;
        }
        if message.get("error").is_some() {
            bail!("Codex does not support skill filtering discovery; update the configured CLI");
        }
        return message
            .get("result")
            .cloned()
            .context("Codex skill discovery response missing result");
    }
    bail!("Codex skill discovery response limit reached")
}
