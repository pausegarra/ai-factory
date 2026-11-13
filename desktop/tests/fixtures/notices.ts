import { capacityFixture } from "./capacity";
import type { Settings, Snapshot } from "../../src/context/factory/domain/types";

export function noticeSettings(): Settings {
  return {
    token_profiles: [],
    projects: [],
    commands: { codex: "codex", opencode: "opencode", git: "git" },
    poll_interval_seconds: 15,
    max_concurrent: 4,
    start_at_login: false,
    flows: [
      { id: "new_issue", name: "Issue", enabled: false, nodes: [] },
      { id: "pr_approved", name: "Approved", enabled: false, nodes: [] },
      { id: "pr_review", name: "Review", enabled: false, nodes: [] },
    ],
  };
}

export function noticeSnapshot(lastError: string | null = null): Snapshot {
  return {
    capacity: capacityFixture(),
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
      running: true,
      polling: false,
      last_poll_at: null,
      last_error: lastError,
      app_data_dir: "/fixture/data",
    },
  };
}

export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
