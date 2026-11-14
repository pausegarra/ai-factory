//! Explicit, read-only migration of the Python application's configuration and history.
use crate::{Flow, FlowNode, NodeRun, Position, Run, RunDetail, Settings, TriggerEvent};
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyRun {
    pub detail: RunDetail,
    pub log_text: String,
    pub raw: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyApprovalComment {
    pub repository: String,
    pub number: u64,
    pub comment_id: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyReviewerRequest {
    pub repository: String,
    pub number: u64,
    pub reviewer: String,
    pub requested: bool,
    pub generation: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyRegisteredPr {
    pub repository: String,
    pub number: u64,
    pub url: String,
    pub head_sha: String,
    pub source_key: String,
    pub source_run_id: String,
    pub source_issue: TriggerEvent,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LegacyImport {
    pub settings: Option<Settings>,
    pub runs: Vec<LegacyRun>,
    pub consumed_events: Vec<String>,
    pub approval_comments: Vec<LegacyApprovalComment>,
    pub reviewer_requests: Vec<LegacyReviewerRequest>,
    pub registered_prs: Vec<LegacyRegisteredPr>,
    pub pr_conflicts: Vec<LegacyPrConflict>,
    pub warnings: Vec<String>,
    pub archive_path: Option<String>,
}

/// Minimal safety evidence, independent of imported run content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyPrConflict {
    pub repository: String,
    pub number: u64,
    pub source_run_id: String,
}

pub fn read_legacy(
    config_path: Option<&Path>,
    database_path: Option<&Path>,
    app_data_dir: &Path,
    current: &Settings,
) -> Result<LegacyImport> {
    read_legacy_excluding(
        config_path,
        database_path,
        app_data_dir,
        current,
        &HashSet::new(),
    )
}

pub(crate) fn read_legacy_excluding(
    config_path: Option<&Path>,
    database_path: Option<&Path>,
    app_data_dir: &Path,
    current: &Settings,
    excluded: &HashSet<String>,
) -> Result<LegacyImport> {
    ensure!(
        config_path.is_some() || database_path.is_some(),
        "Choose a legacy YAML configuration or SQLite database"
    );
    let mut batch = LegacyImport::default();
    if let Some(path) = config_path {
        batch.settings = Some(read_config(path, current, &mut batch.warnings)?);
    }
    if let Some(path) = database_path {
        read_database(
            path,
            app_data_dir,
            batch.settings.as_ref().unwrap_or(current).clone(),
            &mut batch,
            excluded,
        )?;
    }
    let mut sources: HashMap<(String, u64), HashSet<String>> = HashMap::new();
    for pr in &batch.registered_prs {
        sources
            .entry((pr.repository.clone(), pr.number))
            .or_default()
            .insert(pr.source_key.clone());
    }
    let ambiguous: HashSet<_> = sources
        .into_iter()
        .filter(|(_, sources)| sources.len() > 1)
        .map(|(identity, _)| identity)
        .collect();
    batch.pr_conflicts = batch
        .registered_prs
        .iter()
        .filter(|pr| ambiguous.contains(&(pr.repository.clone(), pr.number)))
        .map(|pr| LegacyPrConflict {
            repository: pr.repository.clone(),
            number: pr.number,
            source_run_id: pr.source_run_id.clone(),
        })
        .collect();
    batch
        .registered_prs
        .retain(|pr| !ambiguous.contains(&(pr.repository.clone(), pr.number)));
    for (repository, number) in ambiguous {
        batch.warnings.push(format!(
            "Skipped ambiguous legacy PR provenance for {repository}#{number}"
        ));
    }
    batch.consumed_events.sort();
    batch.consumed_events.dedup();
    Ok(batch)
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
fn optional(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}
fn document(v: &Value, key: &str) -> Value {
    match v.get(key) {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::String(s.clone())),
        Some(v) => v.clone(),
        None => Value::Null,
    }
}
fn count(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}
fn digest(v: &str) -> String {
    format!("{:x}", Sha256::digest(v.as_bytes()))
}
fn run_id(key: &str) -> String {
    format!("legacy-{}", &digest(key)[..32])
}
fn path_from(base: &Path, raw: &str) -> PathBuf {
    let expanded = if let Some(s) = raw.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(s))
            .unwrap_or_else(|| PathBuf::from(raw))
    } else {
        PathBuf::from(raw)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
}
fn strings(v: &Value, key: &str) -> Result<Vec<String>> {
    let Some(value) = v.get(key) else {
        return Ok(vec![]);
    };
    value
        .as_array()
        .context(format!("{key} must be a list"))?
        .iter()
        .map(|x| {
            Ok(x.as_str()
                .context(format!("{key} must contain strings"))?
                .to_lowercase())
        })
        .collect()
}
fn read_config(path: &Path, current: &Settings, warnings: &mut Vec<String>) -> Result<Settings> {
    let path = path.canonicalize().context("Legacy config not found")?;
    let base = path
        .parent()
        .context("Legacy config needs a parent directory")?;
    let data: Value =
        serde_yaml::from_str(&fs::read_to_string(&path)?).context("Invalid legacy YAML")?;
    ensure!(data.is_object(), "Legacy config must be a mapping");
    let mut settings = current.clone();
    let github = &data["github"];
    let owners = strings(github, "owners")?;
    let authors = strings(github, "authors")?;
    let reviewers = strings(github, "reviewers")?;
    for value in owners.iter().chain(&authors).chain(&reviewers) {
        ensure!(crate::valid_login(value), "Invalid GitHub login");
    }
    for profile in &mut settings.token_profiles {
        profile.apply_legacy_scope(&owners, &authors, &reviewers);
    }
    if let Some(n) = data.get("poll_seconds").and_then(Value::as_f64) {
        ensure!(n.is_finite() && n > 0., "Invalid legacy poll_seconds");
        settings.poll_interval_seconds = n.ceil() as u64;
    }
    if let Some(n) = data["concurrency"].get("global").and_then(Value::as_u64) {
        settings.max_concurrent = usize::try_from(n)?;
    }
    for key in ["codex", "git", "opencode"] {
        if let Some(command) = data["commands"].get(key).and_then(Value::as_str) {
            match key {
                "codex" => settings.commands.codex = command.into(),
                "git" => settings.commands.git = command.into(),
                _ => settings.commands.opencode = command.into(),
            }
        }
    }
    if data["commands"].get("gh").is_some() {
        warnings.push("commands.gh ignored: desktop uses GitHub API tokens. Configure a token for each actor account.".into());
    }
    if data["workspace"].get("search_roots").is_some()
        || data["workspace"].get("folder_roots").is_some()
    {
        warnings.push(
            "Legacy workspace search roots require explicit project mappings in desktop settings."
                .into(),
        );
    }
    if data["storage"].get("database").is_some() {
        warnings.push("Legacy storage.database is not used as desktop storage. Select it explicitly to import its history.".into());
    }
    let triggers = data.get("triggers");
    ensure!(
        triggers.is_none() || data.get("workflow").is_none(),
        "Legacy config cannot combine workflow and triggers"
    );
    if let Some(t) = triggers {
        ensure!(t.is_object(), "Legacy triggers must be a mapping");
        for key in t.as_object().unwrap().keys() {
            ensure!(
                ["new_issue", "pr_approved", "pr_review"].contains(&key.as_str()),
                "Unknown legacy trigger {key}"
            );
        }
    }
    for flow in &mut settings.flows {
        flow.profile_activation.clear();
        let declaration = triggers.and_then(|t| t.get(&flow.id));
        let workflow = if triggers.is_some() {
            declaration.and_then(|t| t.get("workflow"))
        } else if flow.id == "new_issue" {
            data.get("workflow")
        } else {
            None
        };
        if let Some(d) = declaration {
            let expected = if flow.id == "pr_review" {
                "pr_review_requested"
            } else {
                &flow.id
            };
            ensure!(
                text(d, "type") == expected,
                "Legacy trigger type mismatch: {}",
                flow.id
            );
            if flow.id == "pr_approved" {
                ensure!(
                    text(d, "source") == "new_issue",
                    "Legacy approval source must be new_issue"
                );
            }
        }
        let Some(workflow) = workflow.filter(|v| !v.is_null()) else {
            flow.enabled = false;
            continue;
        };
        flow.enabled = true;
        flow.nodes = parse_nodes(workflow, base)?;
        flow.max_parallel = usize::try_from(
            workflow
                .get("max_parallel")
                .and_then(Value::as_u64)
                .unwrap_or(3),
        )?;
    }
    settings
        .validate()
        .context("Imported legacy settings are invalid")?;
    Ok(settings)
}
fn parse_nodes(workflow: &Value, base: &Path) -> Result<Vec<FlowNode>> {
    let nodes = workflow["nodes"]
        .as_object()
        .context("Legacy workflow.nodes must be a mapping")?;
    let mut parsed = Vec::new();
    for (index, (id, v)) in nodes.iter().enumerate() {
        ensure!(v.is_object(), "Legacy node {id} must be a mapping");
        ensure!(
            v.get("kind").is_none() || text(v, "kind") == "agent",
            "Unsupported legacy node kind"
        );
        let prompt = match (v.get("instructions"), v.get("instructions_file")) {
            (Some(Value::String(s)), None) => s.clone(),
            (None, Some(Value::String(s))) => fs::read_to_string(path_from(base, s))
                .with_context(|| format!("Cannot read prompt for {id}"))?,
            _ => bail!("Legacy node {id} needs exactly one instructions or instructions_file"),
        };
        let schema = match v.get("result_schema") {
            None | Some(Value::Null) => json!({"type":"object"}),
            Some(Value::String(s)) => {
                serde_json::from_str(&fs::read_to_string(path_from(base, s))?)
                    .with_context(|| format!("Invalid result schema for {id}"))?
            }
            Some(s) if s.is_object() => s.clone(),
            _ => bail!("Invalid result_schema for {id}"),
        };
        let depends_on = match v.get("depends_on") {
            None => vec![],
            Some(v) => v
                .as_array()
                .context("depends_on must be a list")?
                .iter()
                .map(|x| Ok(x.as_str().context("Dependency must be string")?.to_string()))
                .collect::<Result<Vec<_>>>()?,
        };
        ensure!(
            depends_on.iter().collect::<HashSet<_>>().len() == depends_on.len(),
            "Duplicate dependencies for {id}"
        );
        parsed.push(FlowNode {
            id: id.clone(),
            name: id.replace('_', " "),
            agent: text(v, "agent").into(),
            model: text(v, "model").into(),
            effort: text(v, "effort").into(),
            token_overrides: Default::default(),
            skills: None,
            access: text(v, "access").into(),
            prompt,
            result_schema: schema,
            codex_account_id: None,
            depends_on,
            position: Position {
                x: 80. + index as f64 * 280.,
                y: 180.,
            },
        });
    }
    Ok(parsed)
}

fn rows(connection: &Connection, table: &str) -> Result<Vec<Value>> {
    // Callers use static table names; sqlite_master lookup also tolerates pre-DAG databases.
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(vec![]);
    }
    let mut statement = connection.prepare(&format!("SELECT * FROM \"{table}\""))?;
    let columns: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(String::from)
        .collect();
    let mut cursor = statement.query([])?;
    let mut result = Vec::new();
    while let Some(row) = cursor.next()? {
        let mut object = Map::new();
        for (index, name) in columns.iter().enumerate() {
            let value = match row.get_ref(index)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(n) => json!(n),
                ValueRef::Real(n) => json!(n),
                ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into()),
                ValueRef::Blob(b) => json!({"blob_bytes":b.len()}),
            };
            object.insert(name.clone(), value);
        }
        result.push(Value::Object(object));
    }
    Ok(result)
}
fn canonical_event(raw: &str, trigger: &str, repository: &str, number: u64) -> String {
    if trigger == "new_issue" {
        return format!("issue:{}#{number}", repository.to_lowercase());
    }
    if trigger == "pr_approved" && !raw.starts_with("approval:") {
        return format!("approval:{raw}");
    }
    if trigger == "pr_review" {
        return raw
            .strip_prefix("pr_review:")
            .map(|s| format!("review:{s}"))
            .unwrap_or_else(|| raw.into());
    }
    raw.into()
}
fn import_event(task: &Value, workflow: Option<&Value>) -> TriggerEvent {
    let issue = document(task, "issue");
    let trigger = workflow
        .map(|w| text(w, "trigger_name"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| text(&issue, "trigger_name"));
    let trigger = if trigger.is_empty() {
        "new_issue"
    } else {
        trigger
    };
    let context = workflow
        .map(|w| document(w, "trigger_context"))
        .filter(|v| v.is_object())
        .unwrap_or_else(|| issue.clone());
    let pr = context
        .get("pull_request")
        .filter(|v| v.is_object())
        .unwrap_or(&Value::Null)
        .clone();
    let repository = text(task, "repository").to_lowercase();
    let number = count(task, "issue_number");
    let source = context
        .get("source_issue")
        .filter(|v| v.is_object())
        .unwrap_or(&issue);
    let title = if pr.is_object() {
        text(&pr, "title")
    } else {
        text(&issue, "title")
    };
    let actor = optional(&context, "actor")
        .or_else(|| optional(source, "author"))
        .unwrap_or_default();
    let url = optional(&pr, "url")
        .or_else(|| optional(&issue, "url"))
        .unwrap_or_else(|| {
            format!(
                "https://github.com/{repository}/{}/{number}",
                if trigger == "new_issue" {
                    "issues"
                } else {
                    "pull"
                }
            )
        });
    TriggerEvent {
        origin_token_profile_id: None,
        event_id: canonical_event(text(task, "key"), trigger, &repository, number),
        trigger_id: trigger.into(),
        repository,
        number,
        title: title.into(),
        body: if pr.is_object() {
            text(&pr, "body").into()
        } else {
            text(&issue, "body").into()
        },
        actor,
        url,
        head_sha: text(&pr, "head_sha").into(),
        payload: json!({"legacy_context":context,"source_issue":source,"pr_author":text(&pr,"author"),"reviewer":text(&context,"reviewer")}),
    }
}
fn historical_status(state: &str) -> String {
    match state {
        "completed" | "succeeded" | "draft" => "succeeded",
        "failed" => "failed",
        "cancelled" => "cancelled",
        "skipped" => "skipped",
        _ => "interrupted",
    }
    .into()
}

fn snapshot_flow(workflow: &Value, trigger: &str) -> Result<Option<Flow>> {
    let Some(path) = optional(workflow, "definition_path") else {
        return Ok(None);
    };
    let path = Path::new(&path);
    if !path.is_file() {
        return Ok(None);
    }
    ensure!(
        fs::metadata(path)?.len() <= 8 * 1024 * 1024,
        "Legacy flow snapshot is too large"
    );
    let data: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let Some(nodes) = data["nodes"].as_array() else {
        return Ok(None);
    };
    let mut mapping = Map::new();
    for node in nodes {
        let id = text(node, "id");
        ensure!(!id.is_empty(), "Legacy snapshot node missing id");
        ensure!(
            mapping.insert(id.into(), node.clone()).is_none(),
            "Duplicate legacy snapshot node"
        );
    }
    let normalized = json!({"nodes":mapping});
    let flow = Flow {
        id: trigger.into(),
        name: format!("{trigger} (legacy snapshot)"),
        enabled: true,
        profile_activation: Default::default(),
        max_parallel: usize::try_from(data["max_parallel"].as_u64().unwrap_or(3))?,
        nodes: parse_nodes(&normalized, path.parent().unwrap_or(Path::new(".")))?,
    };
    flow.validate()?;
    Ok(Some(flow))
}

fn copy_memory(
    directory: &Path,
    app: &Path,
    run: &str,
    copied: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let mut queue = vec![(directory.to_path_buf(), 0usize)];
    let mut seen = 0;
    while let Some((path, depth)) = queue.pop() {
        if depth > 32 || seen >= 4096 {
            warnings.push(format!(
                "Legacy memory copy limit reached at {}",
                path.display()
            ));
            continue;
        }
        seen += 1;
        let Ok(metadata) = path.symlink_metadata() else {
            warnings.push(format!("Legacy memory unavailable: {}", path.display()));
            continue;
        };
        if metadata.file_type().is_symlink() {
            warnings.push(format!("Legacy memory symlink skipped: {}", path.display()));
            continue;
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(&path)? {
                queue.push((entry?.path(), depth + 1));
            }
        } else if metadata.is_file() {
            if let Some((target, _)) = copy_file(&path, app, run, warnings)? {
                copied.push(target.to_string_lossy().to_string());
            }
        }
    }
    Ok(())
}

fn read_database(
    path: &Path,
    app_data_dir: &Path,
    settings: Settings,
    batch: &mut LegacyImport,
    excluded: &HashSet<String>,
) -> Result<()> {
    let path = path.canonicalize().context("Legacy database not found")?;
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("Cannot open legacy database read-only")?;
    conn.execute_batch("BEGIN")?;
    let tasks = rows(&conn, "tasks")?;
    ensure!(
        !tasks.is_empty()
            || conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='tasks')",
                [],
                |r| r.get::<_, bool>(0)
            )?,
        "Database is not an AI Factory legacy database"
    );
    let workflows = rows(&conn, "workflows")?;
    let nodes = rows(&conn, "workflow_nodes")?;
    let sessions = rows(&conn, "workflow_sessions")?;
    let events = rows(&conn, "events")?;
    let artifacts = rows(&conn, "workflow_artifacts")?;
    let operations = rows(&conn, "workflow_operations")?;
    let workflow_map: HashMap<_, _> = workflows.iter().map(|w| (text(w, "key"), w)).collect();
    for task in &tasks {
        let key = text(task, "key");
        let workflow = workflow_map.get(key).copied();
        let event = import_event(task, workflow);
        batch.consumed_events.push(key.into());
        batch.consumed_events.push(event.event_id.clone());
        let id = run_id(key);
        // Safety provenance is shared operational metadata. Collect it even when
        // deletion excludes this run's history and every file-copy operation.
        collect_pr_provenance(task, workflow, &event, &id, batch);
        // Check before even the first session/artifact copy. SQL filtering alone
        // cannot protect files while a durable deletion is in progress.
        if excluded.contains(&id) {
            continue;
        }
        let related_nodes: Vec<_> = nodes
            .iter()
            .filter(|n| text(n, "workflow_key") == key)
            .cloned()
            .collect();
        let related_sessions: Vec<_> = sessions
            .iter()
            .filter(|n| text(n, "workflow_key") == key)
            .cloned()
            .collect();
        let saved_flow = match workflow
            .map(|w| snapshot_flow(w, &event.trigger_id))
            .transpose()
        {
            Ok(v) => v.flatten(),
            Err(e) => {
                batch
                    .warnings
                    .push(format!("Legacy flow snapshot unavailable for {key}: {e}"));
                None
            }
        };
        let flow = saved_flow
            .or_else(|| {
                settings
                    .flows
                    .iter()
                    .find(|f| f.id == event.trigger_id)
                    .cloned()
                    .map(|mut flow| {
                        flow.profile_activation.clear();
                        flow
                    })
            })
            .unwrap_or(Flow {
                id: event.trigger_id.clone(),
                name: event.trigger_id.clone(),
                enabled: false,
                profile_activation: Default::default(),
                max_parallel: 3,
                nodes: vec![],
            });
        let mut node_runs = Vec::new();
        let mut log = String::new();
        let mut node_logs: HashMap<String, String> = HashMap::new();
        let mut copied = Vec::new();
        for node in &related_nodes {
            let node_id = text(node, "node_id");
            let session = related_sessions
                .iter()
                .find(|s| text(s, "id") == text(node, "session_id"));
            let definition = flow.nodes.iter().find(|n| n.id == node_id);
            let result = session
                .and_then(|s| optional(s, "result_path"))
                .or_else(|| optional(node, "result_path"))
                .and_then(|p| fs::read_to_string(&p).ok())
                .and_then(|s| serde_json::from_str(&s).ok());
            node_runs.push(NodeRun {
                id: node_id.into(),
                name: definition
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| node_id.into()),
                agent: session
                    .map(|s| text(s, "provider"))
                    .filter(|s| !s.is_empty())
                    .unwrap_or("codex")
                    .into(),
                status: historical_status(text(node, "state")),
                started_at: optional(node, "started_at"),
                finished_at: optional(node, "finished_at"),
                error: optional(node, "last_error"),
                result,
                session_id: optional(node, "session_id"),
            });
        }
        for session in &related_sessions {
            for field in [
                "event_path",
                "prompt_path",
                "result_path",
                "output_schema_path",
            ] {
                if let Some(source) = optional(session, field) {
                    if let Some((target, bytes)) =
                        copy_file(Path::new(&source), app_data_dir, &id, &mut batch.warnings)?
                    {
                        copied.push(target.to_string_lossy().to_string());
                        if field == "event_path" {
                            log.push_str(&format!("\n--- {} ---\n", text(session, "id")));
                            log.push_str(&String::from_utf8_lossy(&bytes));
                            let node = text(session, "node_id");
                            if crate::safe_id(node) {
                                node_logs.entry(node.into()).or_default().push_str(&format!(
                                    "\n--- {} ---\n{}",
                                    text(session, "id"),
                                    String::from_utf8_lossy(&bytes)
                                ));
                            }
                        }
                    }
                }
            }
        }
        if !node_logs.is_empty() {
            let sessions = app_data_dir.join("sessions").join(&id);
            fs::create_dir_all(&sessions)?;
            for (node, mut output) in node_logs {
                for token in settings
                    .token_profiles
                    .iter()
                    .filter_map(|p| p.token.as_ref())
                    .filter(|t| !t.is_empty())
                {
                    output = output.replace(token, "[REDACTED]");
                }
                fs::write(sessions.join(format!("{node}.log")), output)?;
            }
        }
        for artifact in artifacts.iter().filter(|a| text(a, "workflow_key") == key) {
            if let Some((target, _)) = copy_file(
                Path::new(text(artifact, "path")),
                app_data_dir,
                &id,
                &mut batch.warnings,
            )? {
                copied.push(target.to_string_lossy().to_string());
            }
        }
        if let Some(workflow) = workflow {
            for field in [
                "spec_path",
                "plan_path",
                "decisions_path",
                "implementation_path",
                "baseline_path",
                "snapshot_path",
                "definition_path",
                "feedback_path",
            ] {
                if let Some(source) = optional(workflow, field) {
                    if let Some((target, _)) =
                        copy_file(Path::new(&source), app_data_dir, &id, &mut batch.warnings)?
                    {
                        copied.push(target.to_string_lossy().to_string());
                    }
                }
            }
            if let Some(memory) = optional(&document(workflow, "documentation"), "memory_dir") {
                copy_memory(
                    Path::new(&memory),
                    app_data_dir,
                    &id,
                    &mut copied,
                    &mut batch.warnings,
                )?;
            }
        }
        let state = historical_status(text(task, "status"));
        let interrupted = state == "interrupted";
        let run=Run{origin_token_profile_id:None,id:id.clone(),event_id:event.event_id.clone(),trigger_id:event.trigger_id.clone(),repository:event.repository.clone(),number:event.number,title:event.title.clone(),actor:event.actor.clone(),url:event.url.clone(),status:state,current_step:related_nodes.iter().find(|n|["running","starting","uncertain"].contains(&text(n,"state"))).map(|n|text(n,"node_id").into()).or_else(||workflow.and_then(|w|optional(w,"phase"))),created_at:optional(task,"detected_at").unwrap_or_else(crate::now),started_at:optional(task,"launched_at"),finished_at:optional(task,"finished_at").or_else(||interrupted.then(crate::now)),error:optional(task,"last_error").or_else(||interrupted.then(||format!("Imported legacy state '{}'; no process was resumed. Retry explicitly after review.",text(task,"status")))),attempt:count(task,"attempts").max(1).min(u32::MAX as u64) as u32,workspace:None};
        let raw = json!({"task":task,"workflow":workflow,"nodes":related_nodes,"sessions":related_sessions,"events":events.iter().filter(|e|text(e,"task_key")==key).collect::<Vec<_>>(),"operations":operations.iter().filter(|o|text(o,"workflow_key")==key).collect::<Vec<_>>(),"artifacts":artifacts.iter().filter(|a|text(a,"workflow_key")==key).collect::<Vec<_>>(),"copied_files":copied,"source_database":path});
        batch.runs.push(LegacyRun {
            detail: RunDetail {
                run,
                event: event.clone(),
                flow,
                nodes: node_runs,
            },
            log_text: log,
            raw,
        });
    }
    let trigger_events = rows(&conn, "trigger_events")?;
    let approval_comments = rows(&conn, "pr_approval_comments")?;
    let reviewer_requests = rows(&conn, "reviewer_requests")?;
    for event in &trigger_events {
        let raw = text(event, "event_id");
        batch.consumed_events.push(raw.into());
        let payload = document(event, "payload");
        let repo = optional(event, "repository")
            .or_else(|| optional(&payload, "repository"))
            .unwrap_or_default();
        let number = count(event, "number").max(count(&payload, "number"));
        batch.consumed_events.push(canonical_event(
            raw,
            text(event, "trigger_name"),
            &repo,
            number,
        ));
        if text(event, "state") == "pending" {
            batch.warnings.push(format!(
                "Pending legacy event {raw} was archived without automatic replay."
            ));
        }
    }
    for c in &approval_comments {
        batch.approval_comments.push(LegacyApprovalComment {
            repository: text(c, "repository").to_lowercase(),
            number: count(c, "pull_number"),
            comment_id: count(c, "comment_id"),
        });
    }
    for r in &reviewer_requests {
        batch.reviewer_requests.push(LegacyReviewerRequest {
            repository: text(r, "repository").to_lowercase(),
            number: count(r, "pull_number"),
            reviewer: text(r, "reviewer").to_lowercase(),
            requested: count(r, "requested") > 0,
            generation: count(r, "generation"),
        });
    }
    let archive = json!({"source_database":path,"tasks":tasks,"workflows":workflows,"workflow_nodes":nodes,"workflow_sessions":sessions,"events":events,"workflow_artifacts":artifacts,"workflow_operations":operations,"trigger_events":trigger_events,"pr_approval_comments":approval_comments,"reviewer_requests":reviewer_requests});
    let contents = serde_json::to_string_pretty(&archive)?;
    let folder = app_data_dir.join("legacy");
    fs::create_dir_all(&folder)?;
    let target = folder.join(format!("archive-{}.json", digest(&contents)));
    if !target.exists() {
        fs::write(&target, contents)?;
    }
    batch.archive_path = Some(target.to_string_lossy().to_string());
    conn.execute_batch("ROLLBACK")?;
    Ok(())
}

fn collect_pr_provenance(
    task: &Value,
    workflow: Option<&Value>,
    event: &TriggerEvent,
    id: &str,
    batch: &mut LegacyImport,
) {
    if event.trigger_id != "new_issue" {
        return;
    }
    let key = text(task, "key");
    if let Some(workflow) = workflow {
        if let Some(prs) = document(workflow, "pull_requests").as_array() {
            for pr in prs {
                let repo = text(pr, "repository").to_lowercase();
                let url = text(pr, "url");
                let number = pr
                    .get("number")
                    .and_then(Value::as_u64)
                    .or_else(|| url.rsplit('/').next().and_then(|s| s.parse().ok()))
                    .unwrap_or(0);
                if number > 0
                    && repo == event.repository
                    && url.eq_ignore_ascii_case(&format!("https://github.com/{repo}/pull/{number}"))
                    && optional(pr, "source_key").is_none_or(|s| s == key)
                {
                    batch.registered_prs.push(LegacyRegisteredPr {
                        repository: repo,
                        number,
                        url: url.into(),
                        head_sha: text(pr, "head_sha").into(),
                        source_key: key.into(),
                        source_run_id: id.into(),
                        source_issue: event.clone(),
                    });
                } else {
                    batch
                        .warnings
                        .push(format!("Skipped invalid legacy PR provenance for {key}"));
                }
            }
        }
    }
}

fn copy_file(
    source: &Path,
    app: &Path,
    run: &str,
    warnings: &mut Vec<String>,
) -> Result<Option<(PathBuf, Vec<u8>)>> {
    if source.as_os_str().is_empty() {
        return Ok(None);
    }
    let metadata = match source.symlink_metadata() {
        Ok(v) => v,
        Err(_) => {
            warnings.push(format!("Legacy file unavailable: {}", source.display()));
            return Ok(None);
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 32 * 1024 * 1024
    {
        warnings.push(format!(
            "Legacy file skipped (symlink, non-file or over 32 MiB): {}",
            source.display()
        ));
        return Ok(None);
    }
    let bytes = match fs::read(source) {
        Ok(v) => v,
        Err(_) => {
            warnings.push(format!("Legacy file unreadable: {}", source.display()));
            return Ok(None);
        }
    };
    let folder = app.join("legacy").join(run);
    fs::create_dir_all(&folder)?;
    let name = source
        .file_name()
        .context("Legacy file has no name")?
        .to_string_lossy();
    let target = folder.join(format!(
        "{}-{}-{name}",
        &digest(&source.to_string_lossy())[..16],
        &format!("{:x}", Sha256::digest(&bytes))[..16]
    ));
    if target.exists() {
        ensure!(
            fs::read(&target)? == bytes,
            "Previously imported legacy file changed; existing archive retained"
        );
    } else {
        fs::write(&target, &bytes)?;
    }
    Ok(Some((target, bytes)))
}
