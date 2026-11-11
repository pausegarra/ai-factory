export type Trigger = "new_issue" | "pr_approved" | "pr_review";
export type Agent = "codex" | "opencode";
export interface AgentModel {
  id: string;
  name: string;
  efforts: string[];
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
  max_parallel?: number;
  nodes: FlowNode[];
}
export interface TokenProfile {
  id: string;
  label: string;
  account: string;
  token?: string;
  has_token: boolean;
}
export interface Project {
  id: string;
  repository: string;
  source_path: string;
}
export interface Settings {
  owners: string[];
  authors: string[];
  reviewers: string[];
  token_profiles: TokenProfile[];
  projects: Project[];
  commands: { codex: string; opencode: string; git: string };
  poll_interval_seconds: number;
  max_concurrent: number;
  node_timeout_seconds: number;
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
      "Current head approvals or OK comments from authorized authors.",
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
export interface Snapshot {
  runs: Run[];
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

export interface ImportReport {
  runs_imported: number;
  settings_imported: boolean;
  archive_path: string;
  warnings: string[];
}
