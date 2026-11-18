import type { BackupSelection, BackupInspection, PreparedBackupImport, BackupOutcome, BackupRecoveryStatus } from "../../application/dto/backup";
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
  AgentUsage,
  CodexLogin,
  InstalledSkill,
} from "../../domain/types";
export const native = isTauri();
const previewSettings: Settings = {
  notification_connectors: [],
  token_profiles: [],
  projects: [],
  commands: { codex: "codex", opencode: "opencode", git: "git" },
  issue_node_comments_enabled: true,
  codex_yolo: false,
  poll_interval_seconds: 15,
  max_concurrent: 4,
  trigger_max_concurrent: {},
  start_at_login: false,
  flows: [
    { id: "new_issue", name: "New task", enabled: true, nodes: [] },
    { id: "pr_approved", name: "PR approved", enabled: true, nodes: [] },
    { id: "pr_review", name: "Review requested", enabled: true, nodes: [] },
    { id: "release", name: "Release", enabled: false, nodes: [] },
  ],
};
const previewSnapshot: Snapshot = {
  capacity: {
    global: { active: 0, limit: 4 },
    by_trigger: {
      new_issue: { active: 0, limit: 4 },
      pr_review: { active: 0, limit: 4 },
      pr_approved: { active: 0, limit: 4 },
      release: { active: 0, limit: 4 },
    },
  },
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
function backupCommand<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  if (!native) return Promise.reject({ code: "desktop_required", category: null, message: "Open AI Factory desktop to manage backups" });
  return invoke<T>(name, args);
}
export const factoryApi = {
  exportBackup: (path: string, selection: BackupSelection, password: string | null) => backupCommand<BackupOutcome>("export_backup", { path, selection, password: selection.components.includes("credentials") ? password : null }),
  inspectBackup: (path: string) => backupCommand<BackupInspection>("inspect_backup", { path }),
  prepareBackupImport: (path: string, selection: BackupSelection, password: string | null) => backupCommand<PreparedBackupImport>("prepare_backup_import", { path, selection, password: selection.components.includes("credentials") ? password : null }),
  applyBackupImport: (confirmationId: string, confirmed: boolean, password: string | null) => backupCommand<BackupOutcome>("apply_backup_import", { confirmationId, confirmed, password }),
  discardBackupImport: (confirmationId: string) => backupCommand<void>("discard_backup_import", { confirmationId }),
  recoverBackupImport: () => backupCommand<void>("recover_backup_import"),
  backupRecoveryStatus: () => backupCommand<BackupRecoveryStatus>("get_backup_recovery_status"),
  agentUsage: (agent: Agent): Promise<AgentUsage> => native
    ? command<AgentUsage>("get_agent_usage", { agent })
    : Promise.resolve({ agent, status: "preview", entries: [], checked_at: new Date().toISOString(), code: "desktop_required" }),
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
