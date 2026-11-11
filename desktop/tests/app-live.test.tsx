import { beforeAll, afterEach, it, expect, vi } from "vitest";
import {
  render,
  screen,
  cleanup,
  act,
  fireEvent,
  waitFor,
} from "@testing-library/react";
import type {
  FactoryEvent,
  Settings,
  Snapshot,
  RunDetail,
  Run,
} from "../src/context/factory/domain/types";
const mocked = vi.hoisted(() => ({
  listeners: new Set<(e: FactoryEvent) => void>(),
}));
vi.mock("../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true,
  factoryApi: {
    settings: vi.fn(),
    agentModels: vi.fn(async () => []),
    snapshot: vi.fn(),
    detail: vi.fn(),
    logs: vi.fn(),
    subscribe: vi.fn(async (cb: (e: FactoryEvent) => void) => {
      mocked.listeners.add(cb);
      return () => mocked.listeners.delete(cb);
    }),
  },
}));
import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
import { App } from "../src/context/factory/infrastructure/presentations/app";
import { RunDetailPage } from "../src/context/factory/infrastructure/presentations/runs";
import { OverviewPage } from "../src/context/factory/infrastructure/presentations/overview";
beforeAll(() => {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(() => {
  cleanup();
  mocked.listeners.clear();
});
const settings: Settings = {
  owners: [],
  authors: [],
  reviewers: [],
  token_profiles: [],
  projects: [],
  commands: { codex: "codex", opencode: "opencode", git: "git" },
  poll_interval_seconds: 15,
  max_concurrent: 4,
  node_timeout_seconds: 3600,
  start_at_login: false,
  flows: [
    { id: "new_issue", name: "Issue", enabled: true, nodes: [] },
    { id: "pr_approved", name: "Approved", enabled: true, nodes: [] },
    { id: "pr_review", name: "Review", enabled: true, nodes: [] },
  ],
};
const run: Run = {
  id: "run1",
  event_id: "event1",
  trigger_id: "new_issue",
  repository: "owner/repo",
  number: 12,
  title: "[AI] Add live runs",
  actor: "author",
  url: "https://github.com/owner/repo/issues/12",
  status: "queued",
  current_step: null,
  created_at: "2026-11-11T12:00:00Z",
  started_at: null,
  finished_at: null,
  error: null,
  attempt: 1,
  workspace: null,
};
const snapshot: Snapshot = {
  runs: [],
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
    last_error: null,
    app_data_dir: "/data",
  },
};
it("considers setup complete with GitHub credentials and owners without a local project", () => {
  render(
    <OverviewPage
      snapshot={snapshot}
      settings={{
        ...settings,
        owners: ["owner"],
        token_profiles: [{ id: "personal", label: "Personal", account: "author", has_token: true }],
      }}
      onPage={() => {}}
      onRun={() => {}}
    />,
  );
  expect(screen.queryByRole("heading", { name: "Set up your first production line" })).toBeNull();
});
it.each(["token", "owner"])("keeps setup guidance when %s is missing without requesting a local project", (missing) => {
  render(
    <OverviewPage
      snapshot={snapshot}
      settings={{
        ...settings,
        owners: missing === "owner" ? [] : ["owner"],
        token_profiles: [{ id: "personal", label: "Personal", account: "author", has_token: missing !== "token" }],
      }}
      onPage={() => {}}
      onRun={() => {}}
    />,
  );
  expect(screen.getByRole("heading", { name: "Set up your first production line" })).toBeTruthy();
  expect(screen.queryByText("Local project")).toBeNull();
  expect(screen.queryByText(/add a project/i)).toBeNull();
});
it("shows a newly enqueued run from an event without refresh or polling", async () => {
  vi.mocked(factoryApi.settings).mockResolvedValue(settings);
  vi.mocked(factoryApi.snapshot).mockResolvedValue(snapshot);
  render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  fireEvent.click(screen.getByRole("button", { name: "Runs" }));
  expect(screen.queryByText(run.title)).toBeNull();
  vi.mocked(factoryApi.snapshot).mockResolvedValue({
    ...snapshot,
    runs: [run],
    metrics: { ...snapshot.metrics, total: 1, queued: 1 },
  });
  act(() =>
    mocked.listeners.forEach((cb) =>
      cb({ kind: "run_created", run_id: run.id, timestamp: "now" }),
    ),
  );
  expect(await screen.findByText(run.title)).toBeTruthy();
  expect(vi.mocked(factoryApi.snapshot).mock.calls.length).toBe(2);
});
it("resync refreshes active details and logs even without a run identifier", async () => {
  const detail: RunDetail = {
    run: { ...run, status: "running" },
    event: {},
    flow: settings.flows[0],
    nodes: [
      {
        id: "plan",
        name: "Plan",
        agent: "codex",
        status: "running",
        started_at: null,
        finished_at: null,
        error: null,
        result: null,
        session_id: null,
      },
    ],
  };
  vi.mocked(factoryApi.detail).mockResolvedValue(detail);
  vi.mocked(factoryApi.logs).mockResolvedValue({
    text: "",
    offset: 0,
    next_offset: 0,
    eof: false,
  });
  render(<RunDetailPage id="run1" onBack={() => {}} onSelect={() => {}} />);
  await screen.findByText("Waiting for agent output…");
  const before = vi.mocked(factoryApi.detail).mock.calls.length;
  vi.mocked(factoryApi.logs).mockResolvedValue({
    text: "Recovered session output",
    offset: 0,
    next_offset: 24,
    eof: true,
  });
  act(() =>
    mocked.listeners.forEach((cb) => cb({ kind: "resync", timestamp: "now" })),
  );
  await waitFor(() =>
    expect(vi.mocked(factoryApi.detail).mock.calls.length).toBeGreaterThan(
      before,
    ),
  );
  expect(await screen.findByText("Recovered session output")).toBeTruthy();
});

it("keeps workflow edits when navigation is cancelled", async () => {
  vi.mocked(factoryApi.settings).mockResolvedValue(settings);
  vi.mocked(factoryApi.snapshot).mockResolvedValue(snapshot);
  render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  fireEvent.click(screen.getByRole("button", { name: "Workflows" }));
  fireEvent.click(screen.getByRole("button", { name: "Add step" }));
  fireEvent.change(screen.getByLabelText("Name"), {
    target: { value: "Unsaved step" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByRole("alertdialog")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe(
    "Unsaved step",
  );
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
  expect(screen.queryByLabelText("Name")).toBeNull();
  expect(screen.getByRole("heading", { name: "Settings" })).toBeTruthy();
});
