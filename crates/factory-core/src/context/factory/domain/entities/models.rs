use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub owners: Vec<String>,
    pub authors: Vec<String>,
    pub reviewers: Vec<String>,
    pub token_profiles: Vec<TokenProfile>,
    pub projects: Vec<Project>,
    pub commands: Commands,
    pub poll_interval_seconds: u64,
    pub max_concurrent: usize,
    pub node_timeout_seconds: u64,
    pub start_at_login: bool,
    pub flows: Vec<Flow>,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct TokenProfile {
    pub id: String,
    pub label: String,
    pub account: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default)]
    pub has_token: bool,
}
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
    #[serde(default = "default_parallel")]
    pub max_parallel: usize,
    pub nodes: Vec<FlowNode>,
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub token_overrides: BTreeMap<String, AgentConfig>,
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
        let mk = |id: &str, name: &str, steps: Vec<(&str, &str, &str, Vec<&str>)>| {
            Flow {
            id: id.into(),
            name: name.into(),
            enabled: true,
            max_parallel: 3,
            nodes: steps
                .into_iter()
                .enumerate()
                .map(|(i, (id, prompt, access, deps))| FlowNode {
                    id: id.into(),
                    name: id.replace('_', " "),
                    agent: "codex".into(),
                    model: String::new(),
                    effort: String::new(),
                    token_overrides: BTreeMap::new(),
                    access: access.into(),
                    prompt: prompt.into(),
                    result_schema: if ["plan_review","review_code","review_tests","review_architecture"].contains(&id){json!({"type":"object","required":["approved"],"properties":{"approved":{"type":"boolean"}}})}else{empty_schema()},
                    depends_on: deps.into_iter().map(String::from).collect(),
                    position: Position {
                        x: 80. + i as f64 * 280.,
                        y: 180.,
                    },
                })
                .collect(),
        }
        };
        Self{owners:vec![],authors:vec![],reviewers:vec![],token_profiles:vec![],projects:vec![],commands:Commands{codex:"codex".into(),opencode:"opencode".into(),git:"git".into()},poll_interval_seconds:15,max_concurrent:4,node_timeout_seconds:3600,start_at_login:false,flows:vec![
   mk("new_issue","New issue",vec![("spec","Write a specification for the issue.","write",vec![]),("architect","Write an implementation plan from the specification.","write",vec!["spec"]),("plan_review","Review specification and plan. Return data.approved=true only when approved; status=blocked when changes are required.","read",vec!["architect"]),("builder","Implement the approved plan. Run real verification and record evidence.","write",vec!["plan_review"]),("review_code","Review code and tests. Return data.approved=true only when approved; status=blocked when changes are required.","read",vec!["builder"]),("review_tests","Review test coverage and verification evidence. Return data.approved=true only when approved; status=blocked when changes are required.","read",vec!["builder"]),("review_architecture","Review the implementation against the approved architecture. Return data.approved=true only when approved; status=blocked when changes are required.","read",vec!["builder"]),("publish","Check approvals and commit changes locally. Return data.publish={title,body,base} to publish a PR through the API; body must include Closes #<issue number>. Do not use gh.","write",vec!["review_code","review_tests","review_architecture"]) ]),
   mk("pr_approved","Approved PR",vec![("publish","Check the approved PR. Return data.merge={method:'squash'} to merge through the API. Do not use gh.","write",vec![])]),
   mk("pr_review","Review requested",vec![("review","Review PR changes and run relevant tests. Return data.review={event:'APPROVE'|'REQUEST_CHANGES'|'COMMENT',body:'report'} to submit a review through the API. Do not use gh.","read",vec![])]) ]}
    }
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=64).contains(&self.max_concurrent),
            "Concurrency must be between 1 and 64"
        );
        ensure!(
            self.poll_interval_seconds > 0 && self.node_timeout_seconds > 0,
            "Intervals must be positive"
        );
        ensure!(
            !self.commands.codex.trim().is_empty()
                && !self.commands.opencode.trim().is_empty()
                && !self.commands.git.trim().is_empty(),
            "CLI commands cannot be empty"
        );
        let mut ids = HashSet::new();
        let mut accounts = HashSet::new();
        for t in &self.token_profiles {
            ensure!(
                safe_id(&t.id) && ids.insert(t.id.clone()),
                "Invalid or duplicate token profile ID"
            );
            ensure!(
                valid_login(&t.account) && accounts.insert(t.account.to_lowercase()),
                "Invalid or duplicate token account"
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
        for v in self
            .owners
            .iter()
            .chain(&self.authors)
            .chain(&self.reviewers)
        {
            ensure!(valid_login(v), "Invalid GitHub login");
        }
        ensure!(
            self.flows.len() == 3,
            "Exactly three trigger flows required"
        );
        for id in ["new_issue", "pr_approved", "pr_review"] {
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
        for t in &mut s.token_profiles {
            t.has_token = t.token.as_ref().is_some_and(|v| !v.is_empty());
            t.token = None;
        }
        s
    }
    pub fn token_for(&self, actor: &str) -> Result<&str> {
        self.token_profiles
            .iter()
            .find(|p| p.account.eq_ignore_ascii_case(actor))
            .and_then(|p| p.token.as_deref())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("No API token configured for actor {actor}"))
    }
}
impl Flow {
    /// Freeze effective agent settings for each node using the event actor's profile.
    pub fn for_actor(&self, actor: &str, profiles: &[TokenProfile]) -> Self {
        let profile = profiles
            .iter()
            .find(|p| p.account.eq_ignore_ascii_case(actor));
        let mut flow = self.clone();
        for node in &mut flow.nodes {
            if let Some(config) = profile.and_then(|p| node.token_overrides.get(&p.id)) {
                node.agent = config.agent.clone();
                node.model = config.model.clone();
                node.effort = config.effort.clone();
            }
            node.token_overrides.clear();
        }
        flow
    }

    pub fn validate(&self) -> Result<()> {
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
    pub url: String,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub payload: Value,
}
impl TriggerEvent {
    pub fn issue(repo: &str, number: u64, title: &str, actor: &str, body: &str) -> Self {
        Self {
            event_id: format!("issue:{repo}#{number}"),
            trigger_id: "new_issue".into(),
            repository: repo.to_lowercase(),
            number,
            title: title.into(),
            body: body.into(),
            actor: actor.into(),
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
pub struct Snapshot {
    pub runs: Vec<Run>,
    pub metrics: Metrics,
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
