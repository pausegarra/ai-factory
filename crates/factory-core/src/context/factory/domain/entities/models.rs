use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

mod default_prompts;
mod issue_titles;
mod settings_input;
pub(crate) use issue_titles::{
    is_release_issue_title, issue_title_matches_trigger, normalize_issue_title_prefix,
    normalize_release_issue_title_prefix, DEFAULT_ISSUE_TITLE_PREFIX,
    DEFAULT_RELEASE_ISSUE_TITLE_PREFIX,
};

pub(crate) const TRIGGER_IDS: [&str; 4] = ["new_issue", "pr_approved", "pr_review", "release"];

pub(crate) const VERSION_BUMP_NODE_ID: &str = "version_bump";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "settings_input::SettingsInput")]
pub struct Settings {
    pub notification_connectors: Vec<super::notification_connector::NotificationConnector>,
    pub source_connectors: Vec<SourceConnector>,
    #[serde(default)]
    pub codex_bindings: BTreeMap<String, String>,
    pub projects: Vec<Project>,
    pub commands: Commands,
    pub issue_title_prefix: String,
    pub release_issue_title_prefix: String,
    pub issue_node_comments_enabled: bool,
    pub codex_yolo: bool,
    pub poll_interval_seconds: u64,
    pub max_concurrent: usize,
    pub trigger_max_concurrent: BTreeMap<String, usize>,
    pub start_at_login: bool,
    pub flows: Vec<Flow>,
}
use super::source_connector::SourceConnector;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub repository: String,
    #[serde(default)]
    pub source_path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Commands {
    pub codex: String,
    pub opencode: String,
    pub git: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Flow {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profile_activation: BTreeMap<String, bool>,
    #[serde(default = "default_parallel")]
    pub max_parallel: usize,
    pub nodes: Vec<FlowNode>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationState {
    Enabled,
    DisabledGlobally,
    DisabledForProfile,
    UnavailableSource,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentConfig {
    pub agent: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub effort: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlowNode {
    pub id: String,
    pub name: String,
    pub agent: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub effort: String,
    /// Frozen managed account for this execution, resolved from the GitHub profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub token_overrides: BTreeMap<String, AgentConfig>,
    // None preserves native discovery; Some lists the only skills enabled for this node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    pub access: String,
    pub prompt: String,
    #[serde(default = "empty_schema")]
    pub result_schema: Value,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub position: Position,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}
fn default_parallel() -> usize {
    3
}
fn empty_schema() -> Value {
    json!({"type":"object"})
}
impl Default for Settings {
    fn default() -> Self {
        let mk = |id: &str,
                  name: &str,
                  enabled: bool,
                  steps: Vec<(&str, &str, &str, Vec<&str>)>| {
            Flow {
            id: id.into(),
            name: name.into(),
            enabled,
            profile_activation: BTreeMap::new(),
            max_parallel: 3,
            nodes: steps
                .into_iter()
                .enumerate()
                .map(|(i, (id, prompt, access, deps))| FlowNode {
                    id: id.into(),
                    name: if id == "publish" && name == "Approved PR" { "Merge".into() } else if id == "version_bump" { "Bump version".into() } else { id.replace('_', " ") },
                    agent: "codex".into(),
                    model: String::new(),
                    effort: String::new(),
                    token_overrides: BTreeMap::new(),
                    skills: if id == "version_bump" { Some(vec![]) } else { None },
                    codex_account_id: None,
                    access: access.into(),
                    prompt: prompt.into(),
                    result_schema: if ["plan_review","review_code","review_tests","review_architecture"].contains(&id){json!({"type":"object","required":["approved"],"properties":{"approved":{"type":"boolean"},"publish":false,"update_pr":false,"merge":false,"review":false,"comment":false,"release":false,"bump_version":false}})}else{empty_schema()},
                    depends_on: deps.into_iter().map(String::from).collect(),
                    position: Position {
                        x: 80. + i as f64 * 280.,
                        y: 180.,
                    },
                })
                .collect(),
        }
        };
        Self {
            notification_connectors: vec![],
            source_connectors: vec![],
            codex_bindings: BTreeMap::new(),
            projects: vec![],
            commands: Commands {
                codex: "codex".into(),
                opencode: "opencode".into(),
                git: "git".into(),
            },
            issue_title_prefix: DEFAULT_ISSUE_TITLE_PREFIX.into(),
            release_issue_title_prefix: DEFAULT_RELEASE_ISSUE_TITLE_PREFIX.into(),
            issue_node_comments_enabled: true,
            codex_yolo: false,
            poll_interval_seconds: 15,
            max_concurrent: 4,
            trigger_max_concurrent: BTreeMap::new(),
            start_at_login: false,
            flows: vec![
                mk(
                    "new_issue",
                    "New issue",
                    true,
                    vec![
                        ("spec", default_prompts::SPEC, "write", vec![]),
                        (
                            "architect",
                            default_prompts::ARCHITECT,
                            "write",
                            vec!["spec"],
                        ),
                        (
                            "plan_review",
                            default_prompts::PLAN_REVIEW,
                            "read",
                            vec!["architect"],
                        ),
                        (
                            "builder",
                            default_prompts::BUILDER,
                            "write",
                            vec!["plan_review"],
                        ),
                        (
                            "review_code",
                            default_prompts::REVIEW_CODE,
                            "read",
                            vec!["builder"],
                        ),
                        (
                            "review_tests",
                            default_prompts::REVIEW_TESTS,
                            "write",
                            vec!["builder"],
                        ),
                        (
                            "review_architecture",
                            default_prompts::REVIEW_ARCHITECTURE,
                            "read",
                            vec!["builder"],
                        ),
                        (
                            "publish",
                            default_prompts::PUBLISH_ISSUE,
                            "write",
                            vec!["review_code", "review_tests", "review_architecture"],
                        ),
                    ],
                ),
                mk(
                    "pr_approved",
                    "Approved PR",
                    true,
                    vec![
                        ("publish", default_prompts::APPROVED_PR, "write", vec![]),
                        (
                            "version_bump",
                            default_prompts::VERSION_BUMP,
                            "write",
                            vec!["publish"],
                        ),
                    ],
                ),
                mk(
                    "pr_review",
                    "Review requested",
                    true,
                    vec![("review", default_prompts::REVIEW_PR, "write", vec![])],
                ),
                mk(
                    "release",
                    "Release",
                    false,
                    vec![("publish_app", default_prompts::RELEASE, "write", vec![])],
                ),
            ],
        }
    }
}
impl Settings {
    pub fn validate_concurrency(&self) -> Result<()> {
        ensure!(
            (1..=64).contains(&self.max_concurrent),
            "Concurrency must be between 1 and 64"
        );
        for (id, limit) in &self.trigger_max_concurrent {
            ensure!(
                TRIGGER_IDS.contains(&id.as_str()),
                "Unknown concurrency trigger: {id}"
            );
            ensure!(
                (1..=64).contains(limit),
                "Concurrency for {id} must be between 1 and 64"
            );
        }
        Ok(())
    }

    pub fn concurrency_for(&self, trigger_id: &str) -> Result<usize> {
        self.validate_concurrency()?;
        ensure!(
            TRIGGER_IDS.contains(&trigger_id),
            "Unknown concurrency trigger: {trigger_id}"
        );
        Ok(self
            .trigger_max_concurrent
            .get(trigger_id)
            .copied()
            .unwrap_or(self.max_concurrent))
    }

    pub fn flow_for_source(&self, template: &Flow, source_id: &str) -> Result<Flow> {
        let source = self
            .source_by_id(source_id)
            .ok_or_else(|| anyhow::anyhow!("Source connector unavailable"))?;
        let mut flow = template.for_source(source);
        let account = self.codex_bindings.get(source_id);
        for node in &mut flow.nodes {
            node.codex_account_id = if node.agent == "codex" {
                account.cloned()
            } else {
                None
            };
        }
        Ok(flow)
    }
    pub fn source_by_id(&self, id: &str) -> Option<&SourceConnector> {
        self.source_connectors.iter().find(|s| s.id == id)
    }
    pub fn source_for_event(&self, event: &TriggerEvent) -> Result<&SourceConnector> {
        let source = event
            .origin_token_profile_id
            .as_deref()
            .filter(|id| safe_id(id))
            .and_then(|id| self.source_by_id(id))
            .ok_or_else(|| anyhow::anyhow!("Source connector unavailable"))?;
        ensure!(
            valid_repository(&event.repository)
                && event.number > 0
                && source.account.eq_ignore_ascii_case(&event.actor)
                && source.allows_owner(event.repository.split('/').next().unwrap_or(""))
                && source.allows_actor(&event.trigger_id),
            "Source connector does not authorize this event"
        );
        Ok(source)
    }
    pub fn validate(&self) -> Result<()> {
        normalize_issue_title_prefix(&self.issue_title_prefix)?;
        normalize_release_issue_title_prefix(&self.release_issue_title_prefix)?;
        super::notification_connector::validate_connectors(&self.notification_connectors)?;
        self.validate_concurrency()?;
        ensure!(
            self.poll_interval_seconds > 0,
            "Poll interval must be positive"
        );
        ensure!(
            !self.commands.codex.trim().is_empty()
                && !self.commands.opencode.trim().is_empty()
                && !self.commands.git.trim().is_empty(),
            "CLI commands cannot be empty"
        );
        let mut ids = HashSet::new();
        for source in &self.source_connectors {
            source.validate()?;
            ensure!(
                ids.insert(source.id.clone()),
                "Source connector IDs must be unique"
            );
        }
        for (profile, account) in &self.codex_bindings {
            ensure!(
                safe_id(profile),
                "Invalid source connector ID in Codex binding"
            );
            ensure!(
                uuid::Uuid::parse_str(account).is_ok_and(|id| id.to_string() == *account),
                "Invalid Codex account ID"
            );
        }
        ids.clear();
        let mut repositories = HashSet::new();
        for p in &self.projects {
            ensure!(
                repositories.insert(p.repository.to_lowercase()),
                "Duplicate project repository mapping"
            );
            ensure!(
                safe_id(&p.id) && ids.insert(p.id.clone()),
                "Invalid or duplicate project ID"
            );
            ensure!(valid_repository(&p.repository), "Invalid repository");
        }
        ensure!(
            self.flows.len() == TRIGGER_IDS.len(),
            "Exactly four trigger flows required"
        );
        for id in TRIGGER_IDS {
            ensure!(
                self.flows.iter().filter(|f| f.id == id).count() == 1,
                "Missing or duplicate trigger flow"
            );
        }
        for f in &self.flows {
            f.validate()?;
        }
        Ok(())
    }
    pub fn redacted(&self) -> Self {
        let mut s = self.clone();
        for c in &mut s.notification_connectors {
            if let super::notification_connector::ConnectorProvider::Telegram(config) =
                &mut c.provider
            {
                config.bot_token = None;
            }
            if let super::notification_connector::ConnectorProvider::GoogleChat(config) =
                &mut c.provider
            {
                config.webhook_url = None;
            }
        }
        for t in &mut s.source_connectors {
            t.has_token = t.token.as_ref().is_some_and(|v| !v.is_empty());
            t.token = None;
        }
        s
    }
}
impl Flow {
    pub fn activation_for_source(
        &self,
        source_id: &str,
        sources: &[SourceConnector],
    ) -> ActivationState {
        if !self.enabled {
            return ActivationState::DisabledGlobally;
        }
        if !sources.iter().any(|s| s.id == source_id) {
            return ActivationState::UnavailableSource;
        }
        if self
            .profile_activation
            .get(source_id)
            .copied()
            .unwrap_or(true)
        {
            ActivationState::Enabled
        } else {
            ActivationState::DisabledForProfile
        }
    }
    pub fn for_source(&self, source: &SourceConnector) -> Self {
        let mut flow = self.clone();
        for node in &mut flow.nodes {
            if let Some(config) = node.token_overrides.get(&source.id) {
                node.agent = config.agent.clone();
                node.model = config.model.clone();
                node.effort = config.effort.clone();
            }
            node.token_overrides.clear();
        }
        flow
    }
    pub fn validate(&self) -> Result<()> {
        for profile in self.profile_activation.keys() {
            ensure!(safe_id(profile), "Invalid workflow activation profile ID");
        }
        ensure!(
            (1..=64).contains(&self.max_parallel),
            "Flow parallel limit must be 1..64"
        );
        ensure!(
            !self.enabled || !self.nodes.is_empty(),
            "Enabled flow requires nodes"
        );
        let mut ids = HashSet::new();
        for n in &self.nodes {
            ensure!(
                safe_id(&n.id) && ids.insert(n.id.clone()),
                "Invalid or duplicate node ID"
            );
            validate_agent_config(&n.agent, &n.model, &n.effort)?;
            for (profile, config) in &n.token_overrides {
                ensure!(safe_id(profile), "Invalid token override profile ID");
                validate_agent_config(&config.agent, &config.model, &config.effort)?;
            }
            ensure!(
                ["read", "write"].contains(&n.access.as_str()),
                "Invalid node access"
            );
            ensure!(!n.prompt.trim().is_empty(), "Prompt required");
            ensure!(
                n.position.x.is_finite() && n.position.y.is_finite(),
                "Invalid node position"
            );
            jsonschema::validator_for(&n.result_schema)
                .map_err(|e| anyhow::anyhow!("Invalid result schema: {e}"))?;
        }
        let mut done = HashSet::new();
        loop {
            let before = done.len();
            for n in &self.nodes {
                for dep in &n.depends_on {
                    ensure!(ids.contains(dep), "Unknown node dependency {dep}");
                }
                if n.depends_on.iter().all(|d| done.contains(d)) {
                    done.insert(n.id.clone());
                }
            }
            if done.len() == self.nodes.len() {
                return Ok(());
            }
            if before == done.len() {
                bail!("Flow contains dependency cycle");
            }
        }
    }
}
fn validate_agent_config(agent: &str, model: &str, effort: &str) -> Result<()> {
    ensure!(["codex", "opencode"].contains(&agent), "Unsupported agent");
    ensure!(
        effort.is_empty() || !model.trim().is_empty(),
        "Effort requires model"
    );
    Ok(())
}
pub fn safe_id(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}
pub fn valid_login(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.[]".contains(&b))
}
pub fn valid_repository(s: &str) -> bool {
    let v: Vec<_> = s.split('/').collect();
    v.len() == 2
        && v.iter().all(|s| {
            !s.is_empty()
                && *s != "."
                && *s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TriggerEvent {
    pub event_id: String,
    pub trigger_id: String,
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_token_profile_id: Option<String>,
    pub url: String,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub payload: Value,
}
impl TriggerEvent {
    pub fn release_issue(repo: &str, number: u64, title: &str, actor: &str, body: &str) -> Self {
        let mut event = Self::issue(repo, number, title, actor, body);
        event.trigger_id = "release".into();
        event.event_id = format!("release:{}#{number}", event.repository);
        event
    }

    pub fn issue(repo: &str, number: u64, title: &str, actor: &str, body: &str) -> Self {
        Self {
            event_id: format!("issue:{repo}#{number}"),
            trigger_id: "new_issue".into(),
            repository: repo.to_lowercase(),
            number,
            title: title.into(),
            body: body.into(),
            actor: actor.into(),
            origin_token_profile_id: None,
            url: format!("https://github.com/{repo}/issues/{number}"),
            head_sha: String::new(),
            payload: json!({}),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub event_id: String,
    pub trigger_id: String,
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_token_profile_id: Option<String>,
    pub url: String,
    pub status: String,
    pub current_step: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    pub attempt: u32,
    pub workspace: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeRun {
    pub id: String,
    pub name: String,
    pub agent: String,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    pub result: Option<Value>,
    pub session_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunDetail {
    pub run: Run,
    pub event: TriggerEvent,
    pub flow: Flow,
    pub nodes: Vec<NodeRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_work: Option<IssueWorkSummary>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssueWorkSummary {
    pub repositories: Vec<IssueRepositorySummary>,
    pub parent_closed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssueRepositorySummary {
    pub repository: String,
    pub sub_issues: Vec<u64>,
    pub pull_request: Option<u64>,
    pub status: IssueRepositoryStatus,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueRepositoryStatus {
    Selected,
    Pending,
    Published,
    NoChanges,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Metrics {
    pub total: u64,
    pub queued: u64,
    pub running: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub interrupted: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub running: bool,
    pub polling: bool,
    pub last_poll_at: Option<String>,
    pub last_error: Option<String>,
    pub app_data_dir: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapacityUsage {
    pub active: usize,
    pub limit: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionCapacity {
    pub global: CapacityUsage,
    pub by_trigger: BTreeMap<String, CapacityUsage>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub runs: Vec<Run>,
    #[serde(default)]
    pub deletions: Vec<crate::RunDeletion>,
    pub metrics: Metrics,
    pub capacity: ExecutionCapacity,
    pub service: ServiceStatus,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactoryEvent {
    pub kind: String,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub timestamp: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogChunk {
    pub text: String,
    pub offset: u64,
    pub next_offset: u64,
    pub eof: bool,
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportReport {
    pub runs_imported: usize,
    pub settings_imported: bool,
    pub archive_path: Option<String>,
    pub warnings: Vec<String>,
}
