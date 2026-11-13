import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  Settings,
  Snapshot,
  RunView,
  DeleteRunOutcome,
  LogChunk,
  FactoryEvent,
  ImportReport,
  Agent,
  AgentModel,
  AgentAccount,
  CodexLogin,
  InstalledSkill,
} from "../../domain/types";
export const native = isTauri();
const previewSettings: Settings = {
  token_profiles: [],
  projects: [],
  commands: { codex: "codex", opencode: "opencode", git: "git" },
  poll_interval_seconds: 15,
  max_concurrent: 4,
  start_at_login: false,
  flows: [
    { id: "new_issue", name: "New task", enabled: true, nodes: [] },
    { id: "pr_approved", name: "PR approved", enabled: true, nodes: [] },
    { id: "pr_review", name: "Review requested", enabled: true, nodes: [] },
  ],
};
const previewSnapshot: Snapshot = {
  runs: [],
  deletions: [],
  metrics: {
    total: 0,
    queued: 0,
    running: 0,
    succeeded: 0,
    failed: 0,
    cancelled: 0,
    interrupted: 0,
  },
  service: {
    running: false,
    polling: false,
    last_poll_at: null,
    last_error: null,
    app_data_dir: "Available in the desktop app",
  },
};
function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  if (!native)
    return Promise.reject(
      new Error("Open AI Factory desktop to save settings and run tasks."),
    );
  return invoke<T>(name, args);
}
export const factoryApi = {
  agentAccounts: () => native ? command<AgentAccount[]>("get_agent_accounts") : Promise.resolve([]),
  startCodexLogin: (label: string) => command<CodexLogin>("start_codex_login", { label }),
  openCodexLogin: (accountId: string) => command<void>("open_codex_login", { accountId }),
  removeAgentAccount: (accountId: string) => command<void>("remove_agent_account", { accountId }),
  agentModels: (agent: Agent, tokenProfileId?: string) => command<AgentModel[]>("get_agent_models", { agent, ...(tokenProfileId === undefined ? {} : { tokenProfileId }) }),
  installedSkills: () =>
    native
      ? invoke<InstalledSkill[]>("get_installed_skills")
      : Promise.reject(
          new Error("Open AI Factory desktop to view installed skills."),
        ),
  settings: () =>
    native
      ? invoke<Settings>("get_settings")
      : Promise.resolve(structuredClone(previewSettings)),
  snapshot: () =>
    native
      ? invoke<Snapshot>("get_snapshot")
      : Promise.resolve(structuredClone(previewSnapshot)),
  saveSettings: (settings: Settings) =>
    command<Settings>("save_settings", { settings }),
  detail: (runId: string) => command<RunView>("get_run_detail", { runId }),
  deleteRun: (runId: string) => command<DeleteRunOutcome>("delete_run", { runId }),
  logs: (runId: string, nodeId: string, offset = 0) =>
    command<LogChunk>("get_logs", { runId, nodeId, offset }),
  cancel: (runId: string) => command<void>("cancel_run", { runId }),
  retry: (runId: string) => command<string>("retry_run", { runId }),
  poll: () => command<void>("poll_now"),
  importLegacy: (configPath: string | null, databasePath: string | null) =>
    command<ImportReport>("import_legacy", { configPath, databasePath }),
  subscribe: async (
    callback: (event: FactoryEvent) => void,
  ): Promise<() => void> =>
    native
      ? listen<FactoryEvent>("factory:event", (e) => callback(e.payload))
      : () => {},
};
