import { capacityFixture } from "./capacity";
import type { Run, Settings, Snapshot } from "../../src/context/factory/domain/types";

export const attentionSettings: Settings = {
  token_profiles: [], projects: [],
  commands: { codex: "codex", opencode: "opencode", git: "git" },
  poll_interval_seconds: 15, max_concurrent: 4, start_at_login: false,
  flows: [
    { id: "new_issue", name: "Issue", enabled: true, nodes: [] },
    { id: "pr_approved", name: "Approval", enabled: true, nodes: [] },
    { id: "pr_review", name: "Review", enabled: true, nodes: [] },
  ],
};

export function attentionRun(id: string, status: string, overrides: Partial<Run> = {}): Run {
  return {
    id, event_id: "issue:owner/repo#26", trigger_id: "new_issue",
    repository: "owner/repo", number: 26, title: `[AI] ${id}`, actor: "alice",
    url: "", status, current_step: null, created_at: "2026-11-13T09:00:00Z",
    started_at: null, finished_at: null, error: null, attempt: 1, workspace: null,
    ...overrides,
  };
}

export function attentionHistory(statuses: readonly string[], eventId = "issue:owner/repo#26"): Run[] {
  return statuses.map((status, index) => attentionRun(`attempt-${index + 1}`, status, {
    event_id: eventId, attempt: index + 1,
    created_at: new Date(Date.UTC(2026, 10, 13, 9, 0, index)).toISOString(),
  }));
}

export function attentionSnapshot(runs: Run[], deletions: Snapshot["deletions"] = []): Snapshot {
  const metrics: Snapshot["metrics"] = {
    total: runs.length, queued: 0, running: 0, succeeded: 0,
    failed: 0, cancelled: 0, interrupted: 0,
  };
  for (const run of runs) {
    switch (run.status) {
      case "queued": metrics.queued++; break;
      case "running": metrics.running++; break;
      case "succeeded": metrics.succeeded++; break;
      case "cancelled": metrics.cancelled++; break;
      case "interrupted": metrics.interrupted++; break;
      default: metrics.failed++;
    }
  }
  const active = { new_issue: 0, pr_review: 0, pr_approved: 0 };
  for (const run of runs) if (run.status === "running") active[run.trigger_id]++;
  return { runs, deletions, metrics, capacity: capacityFixture(active), service: {
    running: true, polling: false, last_poll_at: null,
    last_error: null, app_data_dir: "/fixture-only",
  } };
}
