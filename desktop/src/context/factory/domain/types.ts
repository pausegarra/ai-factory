export type Trigger = "new_issue" | "pr_approved" | "pr_review";
export type Agent = "codex" | "opencode";
export interface AgentAccount {
  id: string;
  agent: "codex";
  label: string;
  email: string | null;
  status: "pending" | "ready" | "failed";
  error: string | null;
}
export interface CodexLogin {
  account: AgentAccount;
  auth_url: string;
}
export interface AgentModel {
  id: string;
  name: string;
  efforts: string[];
}
export interface InstalledSkill {
  id: string;
  name: string;
  description: string;
}
export interface AgentConfig {
  agent: Agent;
  model: string;
  effort: string;
}
export interface FlowNode extends AgentConfig {
  id: string;
  name: string;
  token_overrides?: Record<string, AgentConfig>;
  /** Undefined means all installed skills; a list stores an explicit selection. */
  skills?: string[];
  access: "read" | "write";
  prompt: string;
  result_schema: unknown;
  depends_on: string[];
  position: { x: number; y: number };
}
export interface Flow {
  id: Trigger;
  name: string;
  enabled: boolean;
  profile_activation?: Record<string, boolean>;
  max_parallel?: number;
  nodes: FlowNode[];
}
export interface TokenProfile {
  id: string;
  label: string;
  account: string;
  token?: string;
  has_token: boolean;
  owners: string[];
  authors: string[];
  reviewers: string[];
}
export interface Project {
  id: string;
  repository: string;
  source_path: string;
}
export interface Settings {
  approval_workflow_version?: number;
  notification_connectors?: NotificationConnector[];
  token_profiles: TokenProfile[];
  codex_bindings?: Record<string, string>;
  projects: Project[];
  commands: { codex: string; opencode: string; git: string };
  codex_yolo?: boolean;
  poll_interval_seconds: number;
  max_concurrent: number;
  trigger_max_concurrent?: Partial<Record<Trigger, number>>;
  start_at_login: boolean;
  flows: Flow[];
}
export interface FactoryEvent {
  kind: string;
  run_id?: string;
  node_id?: string;
  timestamp: string;
}
export const TRIGGERS: Record<
  Trigger,
  { label: string; short: string; description: string }
> = {
  new_issue: {
    label: "New task",
    short: "[AI] issue",
    description:
      "Open issues with [AI] in the title, created by allowed authors.",
  },
  pr_approved: {
    label: "PR approved",
    short: "PR approved",
    description:
      "Current head approvals or OK comments merge the PR, then bump its version based on the diff.",
  },
  pr_review: {
    label: "Review requested",
    short: "PR review",
    description: "Review requests sent to your configured reviewers.",
  },
};
export interface Run {
  id: string;
  event_id: string;
  trigger_id: Trigger;
  repository: string;
  number: number;
  title: string;
  actor: string;
  url: string;
  status: string;
  current_step: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  error: string | null;
  attempt: number;
  workspace: string | null;
}
export interface NodeRun {
  id: string;
  name: string;
  agent: Agent;
  status: string;
  started_at: string | null;
  finished_at: string | null;
  error: string | null;
  result: unknown;
  session_id: string | null;
}
export interface RunDetail {
  run: Run;
  event: unknown;
  flow: Flow;
  nodes: NodeRun[];
}
export interface CapacityUsage { active: number; limit: number }
export interface ExecutionCapacity {
  global: CapacityUsage;
  by_trigger: Record<Trigger, CapacityUsage>;
}
export interface Snapshot {
  capacity: ExecutionCapacity;
  runs: Run[];
  deletions: RunDeletion[];
  metrics: {
    total: number;
    queued: number;
    running: number;
    succeeded: number;
    failed: number;
    cancelled: number;
    interrupted: number;
  };
  service: {
    running: boolean;
    polling: boolean;
    last_poll_at: string | null;
    last_error: string | null;
    app_data_dir: string;
  };
}
export interface LogChunk {
  text: string;
  offset: number;
  next_offset: number;
  eof: boolean;
}

export interface RunDeletion {
  run_id: string;
  repository: string;
  number: number;
  title: string;
  attempt: number;
  state: "pending" | "failed";
  last_error: string | null;
}
export type RunView =
  | { state: "available"; detail: RunDetail }
  | { state: "deleting"; deletion: RunDeletion }
  | { state: "missing" };
export type DeleteRunOutcome =
  | { state: "deleted"; run_id: string }
  | { state: "pending"; deletion: RunDeletion };

export interface ImportReport {
  runs_imported: number;
  settings_imported: boolean;
  archive_path: string;
  warnings: string[];
}

export type UsageStatus = "available" | "no_accounts" | "pending" | "failed" | "unauthenticated" | "unavailable" | "error" | "preview";
export type UsageMetricKind = "remaining" | "used" | "reported" | "limit";
export type UsageMetricDerivation = "codex_used_percent_complement";
export type UsageCode = "not_connected" | "sign_in_pending" | "sign_in_failed" | "unsupported" | "no_quota_data" | "cli_missing" | "timed_out" | "protocol" | "query_failed" | "home_unavailable" | "stopping" | "desktop_required";
export interface UsageMetric {
  id: string;
  label: string;
  kind: UsageMetricKind;
  derivation: UsageMetricDerivation | null;
  value: number;
  unit: string;
  period: string | null;
  resets_at: string | null;
  limit_reached: boolean | null;
}
export interface UsageObservation {
  status: UsageStatus;
  metrics: UsageMetric[];
  code: UsageCode | null;
}
export interface UsageEntry {
  id: string;
  label: string;
  email: string | null;
  account_status: string | null;
  observation: UsageObservation;
  checked_at: string;
}
export interface AgentUsage {
  agent: Agent;
  status: UsageStatus;
  entries: UsageEntry[];
  checked_at: string;
  code: UsageCode | null;
}
export type NotificationEvent = "run_started" | "run_succeeded" | "run_failed" | "run_blocked";
export interface NotificationConnector {
  id: string;
  label: string;
  enabled: boolean;
  events: NotificationEvent[];
  type: string;
  config: Record<string, unknown>;
}
export type TelegramNotificationConnector = NotificationConnector & {
  type: "telegram";
  config: { chat_id: string; has_token: boolean; bot_token?: string };
};
