import { afterEach, beforeEach, it, expect, vi } from "vitest";
import { render, screen, cleanup, fireEvent, act, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { DeleteRunOutcome, FactoryEvent, Run, RunDetail, Snapshot } from "../src/context/factory/domain/types";
const mocks = vi.hoisted(() => ({ listeners: new Set<(e: FactoryEvent) => void>() }));
vi.mock("../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true,
  factoryApi: {
    settings: vi.fn(), snapshot: vi.fn(), detail: vi.fn(), logs: vi.fn(),
    cancel: vi.fn(), retry: vi.fn(), deleteRun: vi.fn(), agentModels: vi.fn(),
    saveSettings: vi.fn(), poll: vi.fn(),
    subscribe: vi.fn(async (cb: (e: FactoryEvent) => void) => {
      mocks.listeners.add(cb); return () => mocks.listeners.delete(cb);
    }),
  },
}));
import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
import { RunDetailPage } from "../src/context/factory/infrastructure/presentations/runs";
import { App } from "../src/context/factory/infrastructure/presentations/app";
const detail: RunDetail = {
  run: { id: "run-1", event_id: "e", trigger_id: "new_issue", repository: "owner/repo", number: 4,
    title: "[AI] Remove attempt", actor: "alice", url: "", status: "succeeded", current_step: null,
    created_at: "2026-11-11T12:00:00Z", started_at: null, finished_at: null, error: null, attempt: 1, workspace: null },
  event: {}, flow: { id: "new_issue", name: "Issue", enabled: true, nodes: [] },
  nodes: [{ id: "spec", name: "Spec", agent: "codex", status: "succeeded", started_at: null,
    finished_at: null, error: null, result: { summary: "stale result" }, session_id: null }],
};
const other: RunDetail = {
  ...detail,
  run: { ...detail.run, id: "run-2", title: "Other attempt" },
};
const empty: Snapshot = { runs: [], deletions: [], metrics: { total: 0, queued: 0, running: 0,
  succeeded: 0, failed: 0, cancelled: 0, interrupted: 0 }, service: { running: true,
  polling: false, last_poll_at: null, last_error: null, app_data_dir: "/isolated" } };
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "available", detail });
  vi.mocked(factoryApi.logs).mockResolvedValue({ text: "", offset: 0, next_offset: 0, eof: true });
  vi.mocked(factoryApi.deleteRun).mockResolvedValue({ state: "deleted", run_id: "run-1" });
  vi.mocked(factoryApi.agentModels).mockResolvedValue([]);
  vi.mocked(factoryApi.saveSettings).mockImplementation(async value => value);
  vi.mocked(factoryApi.poll).mockResolvedValue(undefined);
  vi.mocked(factoryApi.settings).mockResolvedValue({ token_profiles: [], projects: [], commands: { codex: "", opencode: "", git: "" }, poll_interval_seconds: 15, max_concurrent: 1, start_at_login: false, flows: [detail.flow] });
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(() => { cleanup(); mocks.listeners.clear(); });

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function snapshotWith(...runs: Run[]): Snapshot {
  return {
    ...empty,
    runs,
    metrics: {
      ...empty.metrics,
      total: runs.length,
      succeeded: runs.filter(run => run.status === "succeeded").length,
    },
  };
}

function emit(kind: FactoryEvent["kind"], id?: string) {
  mocks.listeners.forEach(cb => cb({ kind, run_id: id, timestamp: "now" }));
}

async function showApp(runs: Run[] = [detail.run]) {
  vi.mocked(factoryApi.snapshot).mockResolvedValue(snapshotWith(...runs));
  const app = render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  return app;
}

async function openRun(title = detail.run.title) {
  fireEvent.click(screen.getByRole("button", { name: "Runs" }));
  fireEvent.click(screen.getByRole("button", { name: "History" }));
  fireEvent.click(screen.getByRole("button", { name: title }));
  await screen.findByRole("button", { name: "Delete run" });
}

async function startDelete() {
  fireEvent.click(await screen.findByRole("button", { name: "Delete run" }));
  const buttons = screen.getAllByRole("button", { name: "Delete run" });
  fireEvent.click(buttons[buttons.length - 1]);
}
it("confirms once, keeps navigation usable and announces success", async () => {
  let resolve!: (x: { state: "deleted"; run_id: string }) => void;
  vi.mocked(factoryApi.deleteRun).mockImplementation(() => new Promise(r => { resolve = r; }));
  const deleted = vi.fn();
  render(<RunDetailPage id="run-1" onBack={() => {}} onSelect={() => {}} onDeleted={deleted} />);
  fireEvent.click(await screen.findByRole("button", { name: "Delete run" }));
  const buttons = screen.getAllByRole("button", { name: "Delete run" });
  fireEvent.click(buttons[buttons.length - 1]);
  expect(factoryApi.deleteRun).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(screen.getByText("Deleting…")).toBeTruthy();
  vi.mocked(factoryApi.detail).mockImplementationOnce(() => new Promise(() => {}));
  act(() => mocks.listeners.forEach(cb => cb({ kind: "run_deletion_pending", run_id: "run-1", timestamp: "now" })));
  expect(screen.getByText("Deleting…")).toBeTruthy();
  await act(async () => resolve({ state: "deleted", run_id: "run-1" }));
  expect(deleted).toHaveBeenCalledWith("run-1");
});
it.each(["queued", "running", "unknown"])("cannot delete %s run", async status => {
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "available", detail: { ...detail, run: { ...detail.run, status } } });
  render(<RunDetailPage id="run-1" onBack={() => {}} onSelect={() => {}} />);
  expect((await screen.findByRole("button", { name: "Delete run" }) as HTMLButtonElement).disabled).toBe(true);
});
it("shows failed cleanup without logs and retries deletion, never workflow", async () => {
  const deletion = { run_id: "run-1", repository: "owner/repo", number: 4, title: detail.run.title, attempt: 1, state: "failed" as const, last_error: "Cannot remove sessions" };
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "deleting", deletion });
  vi.mocked(factoryApi.deleteRun).mockResolvedValue({ state: "pending", deletion });
  render(<RunDetailPage id="run-1" onBack={() => {}} onSelect={() => {}} />);
  fireEvent.click(await screen.findByRole("button", { name: "Retry deletion" }));
  await waitFor(() => expect(factoryApi.deleteRun).toHaveBeenCalledWith("run-1"));
  expect(factoryApi.retry).not.toHaveBeenCalled(); expect(factoryApi.logs).not.toHaveBeenCalled();
});
it("reconciles a lost delete response using typed missing view", async () => {
  const deleted = vi.fn();
  vi.mocked(factoryApi.deleteRun).mockRejectedValue(new Error("transport lost"));
  render(<RunDetailPage id="run-1" onBack={() => {}} onSelect={() => {}} onDeleted={deleted} />);
  fireEvent.click(await screen.findByRole("button", { name: "Delete run" }));
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "missing" });
  const buttons = screen.getAllByRole("button", { name: "Delete run" });
  fireEvent.click(buttons[buttons.length - 1]);
  await waitFor(() => expect(deleted).toHaveBeenCalledWith("run-1"));
  expect(screen.queryByText("Error: transport lost")).toBeNull();
});
it("keeps stopping rejection inline and never reports successful deletion", async () => {
  const deleted = vi.fn();
  vi.mocked(factoryApi.deleteRun).mockRejectedValue(new Error("This run is still stopping. Try again when cleanup finishes."));
  render(<RunDetailPage id="run-1" onBack={() => {}} onSelect={() => {}} onDeleted={deleted} />);
  fireEvent.click(await screen.findByRole("button", { name: "Delete run" }));
  const buttons = screen.getAllByRole("button", { name: "Delete run" });
  fireEvent.click(buttons[buttons.length - 1]);
  expect(await screen.findByText(/This run is still stopping/)).toBeTruthy();
  expect(deleted).not.toHaveBeenCalled();
});
it("does not leave another page when an old delete request completes", async () => {
  vi.mocked(factoryApi.snapshot).mockResolvedValue({ ...empty, runs: [detail.run], metrics: { ...empty.metrics, total: 1, succeeded: 1 } });
  let resolve!: (x: { state: "deleted"; run_id: string }) => void;
  vi.mocked(factoryApi.deleteRun).mockImplementation(() => new Promise(r => { resolve = r; }));
  render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  fireEvent.click(screen.getByRole("button", { name: "Runs" }));
  fireEvent.click(screen.getByRole("button", { name: "History" }));
  fireEvent.click(screen.getByRole("button", { name: detail.run.title }));
  fireEvent.click(await screen.findByRole("button", { name: "Delete run" }));
  const buttons = screen.getAllByRole("button", { name: "Delete run" });
  fireEvent.click(buttons[buttons.length - 1]);
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  await screen.findByRole("heading", { name: "Settings" });
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  await act(async () => resolve({ state: "deleted", run_id: "run-1" }));
  expect(await screen.findByRole("heading", { name: "Settings" })).toBeTruthy();
});
it.each(["run_deleted", "resync"] as const)("does not claim success after an earlier rejection and later %s", async kind => {
  vi.mocked(factoryApi.snapshot).mockResolvedValue({ ...empty, runs: [detail.run], metrics: { ...empty.metrics, total: 1, succeeded: 1 } });
  vi.mocked(factoryApi.deleteRun).mockRejectedValue(new Error("still stopping"));
  render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  fireEvent.click(screen.getByRole("button", { name: "Runs" }));
  fireEvent.click(screen.getByRole("button", { name: "History" }));
  fireEvent.click(screen.getByRole("button", { name: detail.run.title }));
  fireEvent.click(await screen.findByRole("button", { name: "Delete run" }));
  const buttons = screen.getAllByRole("button", { name: "Delete run" });
  fireEvent.click(buttons[buttons.length - 1]);
  await screen.findByText(/still stopping/);
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "missing" });
  act(() => emit(kind, kind === "resync" ? undefined : "run-1"));
  expect(await screen.findByText("This run is no longer available.")).toBeTruthy();
  expect(screen.queryByText("Run deleted.")).toBeNull();
});
it("refreshes pending deletion snapshot even when lifecycle events were lost", async () => {
  const deletion = { run_id: "run-1", repository: "owner/repo", number: 4, title: detail.run.title, attempt: 1, state: "failed" as const, last_error: "Cleanup failed" };
  vi.mocked(factoryApi.snapshot).mockResolvedValue({ ...empty, runs: [detail.run], metrics: { ...empty.metrics, total: 1, succeeded: 1 } });
  vi.mocked(factoryApi.deleteRun).mockResolvedValue({ state: "pending", deletion });
  render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  fireEvent.click(screen.getByRole("button", { name: "Runs" }));
  fireEvent.click(screen.getByRole("button", { name: "History" }));
  fireEvent.click(screen.getByRole("button", { name: detail.run.title }));
  fireEvent.click(await screen.findByRole("button", { name: "Delete run" }));
  vi.mocked(factoryApi.snapshot).mockResolvedValue({ ...empty, deletions: [deletion] });
  const buttons = screen.getAllByRole("button", { name: "Delete run" });
  fireEvent.click(buttons[buttons.length - 1]);
  await screen.findByRole("heading", { name: "Deletion pending" });
  fireEvent.click(screen.getByRole("button", { name: "Back to runs" }));
  expect(await screen.findByRole("heading", { name: "Pending deletions" })).toBeTruthy();
});
it.each(["run_deleted", "resync"])("closes selected detail after %s despite late load", async kind => {
  vi.mocked(factoryApi.snapshot).mockResolvedValue({ ...empty, runs: [detail.run], metrics: { ...empty.metrics, total: 1, succeeded: 1 } });
  let resolve!: (x: { state: "available"; detail: RunDetail }) => void;
  vi.mocked(factoryApi.detail).mockImplementationOnce(() => new Promise(r => { resolve = r; }));
  render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  fireEvent.click(screen.getByRole("button", { name: "Runs" }));
  fireEvent.click(screen.getByRole("button", { name: "History" }));
  fireEvent.click(screen.getByRole("button", { name: detail.run.title }));
  await waitFor(() => expect(factoryApi.detail).toHaveBeenCalled());
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "missing" });
  act(() => mocks.listeners.forEach(cb => cb({ kind, run_id: kind === "resync" ? undefined : "run-1", timestamp: "now" })));
  await screen.findByText("This run is no longer available.");
  await act(async () => resolve({ state: "available", detail }));
  expect(screen.queryByText("stale result")).toBeNull();
  expect(screen.queryByRole("heading", { name: detail.run.title })).toBeNull();
  expect(await screen.findByRole("heading", { name: "Runs" })).toBeTruthy();
});

it.each(["local", "external"] as const)("clears %s feedback on every page exit", async source => {
  for (const destination of ["Overview", "Workflows", "Settings"]) {
    const app = await showApp();
    await openRun();
    vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
    vi.mocked(factoryApi.detail).mockResolvedValue({ state: "missing" });
    if (source === "local") await startDelete();
    else act(() => emit("run_deleted", "run-1"));
    const text = source === "local" ? "Run deleted." : "This run is no longer available.";
    await screen.findByText(text);
    fireEvent.click(screen.getByRole("button", { name: destination }));
    expect(screen.queryByText(text)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Runs" }));
    expect(screen.queryByText(text)).toBeNull();
    app.unmount();
    vi.mocked(factoryApi.detail).mockResolvedValue({ state: "available", detail });
  }
});

it("ignores a delete callback from an earlier visit to the same run", async () => {
  const deletion = deferred<DeleteRunOutcome>();
  vi.mocked(factoryApi.deleteRun).mockReturnValue(deletion.promise);
  await showApp();
  await openRun();
  await startDelete();
  fireEvent.click(screen.getByRole("button", { name: "All runs" }));
  await openRun();
  await act(async () => deletion.resolve({ state: "deleted", run_id: "run-1" }));
  expect(screen.getByRole("heading", { name: detail.run.title })).toBeTruthy();
  expect(screen.queryByText("Run deleted.")).toBeNull();
});

it.each(["local", "external"] as const)("dismisses %s feedback and keeps useful focus", async source => {
  const user = userEvent.setup();
  await showApp();
  await openRun();
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "missing" });
  const nav = screen.getByRole("button", { name: "Runs" });
  if (source === "local") await startDelete();
  nav.focus();
  if (source === "external") act(() => emit("run_deleted", "run-1"));
  const message = source === "local" ? "Run deleted." : "This run is no longer available.";
  const text = await screen.findByText(message);
  const status = text.closest('[role="status"]') as HTMLElement;
  expect(status).toBeTruthy();
  expect(status.classList.contains(source === "local" ? "notice-success" : "notice-info")).toBe(true);
  if (source === "external") expect(document.activeElement).toBe(nav);
  const close = within(status).getByRole("button", { name: "Dismiss notice" });
  close.focus();
  await user.keyboard("{Enter}");
  expect(screen.queryByText(message)).toBeNull();
  expect(screen.queryByRole("button", { name: "Dismiss notice" })).toBeNull();
  expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Runs" }));
  await user.tab();
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Running 0" }));
  await act(async () => { emit("run_deleted", "run-1"); emit("resync"); });
  expect(screen.queryByText(message)).toBeNull();
  expect(screen.getByText("0 runs")).toBeTruthy();
});

it("does not resurrect dismissed feedback when a late delete response arrives", async () => {
  const deletion = deferred<DeleteRunOutcome>();
  vi.mocked(factoryApi.deleteRun).mockReturnValue(deletion.promise);
  await showApp();
  await openRun();
  await startDelete();
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "missing" });
  act(() => emit("run_deleted", "run-1"));
  const status = (await screen.findByText("Run deleted.")).closest('[role="status"]') as HTMLElement;
  fireEvent.click(within(status).getByRole("button", { name: "Dismiss notice" }));
  await act(async () => deletion.resolve({ state: "deleted", run_id: "run-1" }));
  await act(async () => emit("resync"));
  expect(screen.queryByText("Run deleted.")).toBeNull();
  expect(screen.queryByText("This run is no longer available.")).toBeNull();
});

it("dismisses external feedback by pointer without changing runs", async () => {
  await showApp();
  await openRun();
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "missing" });
  act(() => emit("run_deleted", "run-1"));
  const status = (await screen.findByText("This run is no longer available.")).closest('[role="status"]') as HTMLElement;
  fireEvent.click(within(status).getByRole("button", { name: "Dismiss notice" }));
  expect(screen.queryByText("This run is no longer available.")).toBeNull();
  expect(screen.getByText("0 runs")).toBeTruthy();
  expect(factoryApi.deleteRun).not.toHaveBeenCalled();
});

it("does not steal focus when successful feedback appears", async () => {
  const deletion = deferred<DeleteRunOutcome>();
  vi.mocked(factoryApi.deleteRun).mockReturnValue(deletion.promise);
  await showApp();
  await openRun();
  await startDelete();
  const nav = screen.getByRole("button", { name: "Runs" });
  nav.focus();
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  await act(async () => deletion.resolve({ state: "deleted", run_id: "run-1" }));
  await screen.findByText("Run deleted.");
  expect(document.activeElement).toBe(nav);
});

it.each([
  ["response", "event", "resync"], ["response", "resync", "event"],
  ["event", "response", "resync"], ["event", "resync", "response"],
  ["resync", "response", "event"], ["resync", "event", "response"],
] as const)("resolves deletion once in order %s / %s / %s", async (...order) => {
  const deletion = deferred<DeleteRunOutcome>();
  vi.mocked(factoryApi.deleteRun).mockReturnValue(deletion.promise);
  await showApp();
  await openRun();
  await startDelete();
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "missing" });
  for (const source of order) {
    await act(async () => {
      if (source === "response") deletion.resolve({ state: "deleted", run_id: "run-1" });
      else emit(source === "event" ? "run_deleted" : "resync", source === "event" ? "run-1" : undefined);
    });
  }
  await screen.findByText("Run deleted.");
  expect(screen.getAllByText("Run deleted.")).toHaveLength(1);
  expect(screen.queryByText("This run is no longer available.")).toBeNull();
  expect(screen.getByRole("heading", { name: "Runs" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "All runs" }));
  expect(screen.getByText("0 runs")).toBeTruthy();
  expect(screen.queryByRole("button", { name: detail.run.title })).toBeNull();
});

it("does not close a new visit using an old snapshot of the same ID", async () => {
  const oldSnapshot = deferred<Snapshot>();
  await showApp();
  await openRun();
  vi.mocked(factoryApi.snapshot).mockReturnValueOnce(oldSnapshot.promise);
  act(() => emit("resync"));
  await waitFor(() => expect(factoryApi.snapshot).toHaveBeenCalledTimes(2));
  fireEvent.click(screen.getByRole("button", { name: "All runs" }));
  await openRun();
  await act(async () => oldSnapshot.resolve(empty));
  expect(screen.getByRole("heading", { name: detail.run.title })).toBeTruthy();
  expect(screen.queryByText("This run is no longer available.")).toBeNull();
});

it("refreshes data without feedback when deletion completes after leaving detail", async () => {
  const deletion = deferred<DeleteRunOutcome>();
  vi.mocked(factoryApi.deleteRun).mockReturnValue(deletion.promise);
  await showApp();
  await openRun();
  await startDelete();
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  await act(async () => deletion.resolve({ state: "deleted", run_id: "run-1" }));
  expect(screen.getByRole("heading", { name: "Settings" })).toBeTruthy();
  expect(screen.queryByText("Run deleted.")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Runs" }));
  await screen.findByText("0 runs");
  expect(screen.queryByText("Run deleted.")).toBeNull();
});

it("keeps a different selected run when an old delete finishes", async () => {
  const deletion = deferred<DeleteRunOutcome>();
  vi.mocked(factoryApi.deleteRun).mockReturnValue(deletion.promise);
  vi.mocked(factoryApi.detail).mockImplementation(async id => ({
    state: "available",
    detail: id === "run-2" ? other : detail,
  }));
  await showApp([detail.run, other.run]);
  await openRun();
  await startDelete();
  fireEvent.click(screen.getByRole("button", { name: "All runs" }));
  await openRun(other.run.title);
  vi.mocked(factoryApi.snapshot).mockResolvedValue(snapshotWith(other.run));
  await act(async () => {
    deletion.resolve({ state: "deleted", run_id: "run-1" });
    emit("run_deleted", "run-1");
  });
  expect(screen.getByRole("heading", { name: other.run.title })).toBeTruthy();
  expect(screen.queryByText("Run deleted.")).toBeNull();
  expect(screen.queryByText("This run is no longer available.")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "All runs" }));
  fireEvent.click(screen.getByRole("button", { name: "All runs" }));
  await screen.findByText("1 run");
});

it.each([false, true])("starts clean across views with existing runs=%s", async populated => {
  await showApp(populated ? [detail.run] : []);
  for (const page of ["Overview", "Runs", "Workflows", "Settings", "Runs", "Overview"]) {
    fireEvent.click(screen.getByRole("button", { name: page }));
    expect(screen.queryByText("Run deleted.")).toBeNull();
    expect(screen.queryByText("This run is no longer available.")).toBeNull();
  }
  if (populated) {
    fireEvent.click(screen.getByRole("button", { name: "Runs" }));
    fireEvent.click(screen.getByRole("button", { name: "History" }));
    fireEvent.click(screen.getByRole("button", { name: detail.run.title }));
    await screen.findByRole("button", { name: "Delete run" });
    expect(screen.queryByText("Run deleted.")).toBeNull();
  }
  await act(async () => { emit("resync"); emit("run_deleted", "unselected-run"); });
  expect(screen.queryByText("Run deleted.")).toBeNull();
  expect(factoryApi.deleteRun).not.toHaveBeenCalled();
});

it("does not restore feedback across App instances", async () => {
  const deletion = deferred<DeleteRunOutcome>();
  vi.mocked(factoryApi.deleteRun).mockReturnValue(deletion.promise);
  const first = await showApp();
  await openRun();
  await startDelete();
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  act(() => emit("run_deleted", "run-1"));
  await screen.findByText("Run deleted.");
  first.unmount();
  await showApp([]);
  await act(async () => deletion.resolve({ state: "deleted", run_id: "run-1" }));
  fireEvent.click(screen.getByRole("button", { name: "Runs" }));
  expect(screen.queryByText("Run deleted.")).toBeNull();
  expect(screen.queryByText("This run is no longer available.")).toBeNull();
  expect(screen.queryByRole("button", { name: "Dismiss notice" })).toBeNull();
});

it.each([
  ["local", "list"], ["local", "overview"],
  ["external", "list"], ["external", "overview"],
] as const)("clears %s feedback before opening detail from %s", async (source, entry) => {
  vi.mocked(factoryApi.detail).mockImplementation(async id => ({
    state: "available",
    detail: id === "run-2" ? other : detail,
  }));
  await showApp([detail.run, other.run]);
  await openRun();
  vi.mocked(factoryApi.snapshot).mockResolvedValue(snapshotWith(other.run));
  if (source === "local") await startDelete();
  else act(() => emit("run_deleted", "run-1"));
  await screen.findByText(source === "local" ? "Run deleted." : "This run is no longer available.");
  fireEvent.click(screen.getByRole("button", { name: "All runs" }));
  await waitFor(() => expect(screen.getByText("1 run")).toBeTruthy());
  if (entry === "overview") {
    fireEvent.click(screen.getByRole("button", { name: "Overview" }));
    fireEvent.click(screen.getByRole("button", { name: other.run.title }));
  } else {
    fireEvent.click(screen.getByRole("button", { name: "History" }));
    fireEvent.click(screen.getByRole("button", { name: other.run.title }));
  }
  await screen.findByRole("heading", { name: other.run.title });
  expect(screen.queryByText("Run deleted.")).toBeNull();
  expect(screen.queryByText("This run is no longer available.")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "All runs" }));
  expect(screen.queryByText("Run deleted.")).toBeNull();
  expect(screen.queryByText("This run is no longer available.")).toBeNull();
});

it("keeps current feedback through list filters and refresh, but never recreates dismissed feedback", async () => {
  await showApp();
  await openRun();
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  await startDelete();
  const status = (await screen.findByText("Run deleted.")).closest('[role="status"]') as HTMLElement;
  for (const dismissed of [false, true]) {
    if (dismissed) fireEvent.click(within(status).getByRole("button", { name: "Dismiss notice" }));
    for (const tab of ["History", "All runs", "Running 0"]) {
      fireEvent.click(screen.getByRole("button", { name: tab }));
    }
    fireEvent.change(screen.getByRole("textbox", { name: "Search runs" }), { target: { value: dismissed ? "after" : "before" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Filter trigger" }), { target: { value: dismissed ? "all" : "new_issue" } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Sync now" })); });
    await act(async () => emit("resync"));
    expect(screen.queryAllByText("Run deleted.")).toHaveLength(dismissed ? 0 : 1);
  }
  expect(factoryApi.poll).toHaveBeenCalledTimes(2);
});

it("does not announce success when the delete dialog is cancelled", async () => {
  await showApp();
  await openRun();
  fireEvent.click(screen.getByRole("button", { name: "Delete run" }));
  fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
  expect(factoryApi.deleteRun).not.toHaveBeenCalled();
  expect(screen.queryByText("Run deleted.")).toBeNull();
  expect(screen.getByRole("heading", { name: detail.run.title })).toBeTruthy();
});

it("only announces success after pending cleanup is retried and completed", async () => {
  const deletion = deferred<DeleteRunOutcome>();
  const pending = {
    run_id: "run-1", repository: "owner/repo", number: 4,
    title: detail.run.title, attempt: 1, state: "failed" as const, last_error: "Cleanup failed",
  };
  vi.mocked(factoryApi.deleteRun).mockReturnValueOnce(deletion.promise);
  await showApp();
  await openRun();
  await startDelete();
  expect(screen.getByText("Deleting…")).toBeTruthy();
  expect(screen.queryByText("Run deleted.")).toBeNull();
  vi.mocked(factoryApi.snapshot).mockResolvedValue({ ...empty, deletions: [pending] });
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "deleting", deletion: pending });
  await act(async () => deletion.resolve({ state: "pending", deletion: pending }));
  await screen.findByRole("heading", { name: "Deletion pending" });
  expect(screen.getByText(/Cleanup failed/)).toBeTruthy();
  expect(screen.queryByText("Run deleted.")).toBeNull();
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "missing" });
  vi.mocked(factoryApi.deleteRun).mockResolvedValue({ state: "deleted", run_id: "run-1" });
  fireEvent.click(screen.getByRole("button", { name: "Retry deletion" }));
  await screen.findByText("Run deleted.");
  expect(factoryApi.retry).not.toHaveBeenCalled();
  expect(factoryApi.deleteRun).toHaveBeenCalledTimes(2);
});

it("announces a reconciled local deletion at App level", async () => {
  vi.mocked(factoryApi.deleteRun).mockRejectedValue(new Error("transport lost"));
  await showApp();
  await openRun();
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "missing" });
  vi.mocked(factoryApi.snapshot).mockResolvedValue(empty);
  await startDelete();
  await screen.findByText("Run deleted.");
  expect(screen.queryByText("This run is no longer available.")).toBeNull();
  expect(screen.queryByText("Error: transport lost")).toBeNull();
});

it("does not clear a real sync error when dismissing deletion feedback", async () => {
  await showApp();
  await openRun();
  vi.mocked(factoryApi.snapshot).mockResolvedValue({
    ...empty,
    service: { ...empty.service, last_error: "Fixture sync denied" },
  });
  await startDelete();
  const notice = (await screen.findByText("Run deleted.")).closest('[role="status"]') as HTMLElement;
  await screen.findByText(/Factory reported a problem\./);
  fireEvent.click(within(notice).getByRole("button", { name: "Dismiss notice" }));
  expect(screen.getByRole("alert").textContent).toContain("Fixture sync denied");
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByRole("alert").textContent).toContain("Fixture sync denied");
});

it("keeps a snapshot load failure visible without deletion success", async () => {
  await showApp();
  vi.mocked(factoryApi.snapshot).mockRejectedValue(new Error("Fixture load denied"));
  await act(async () => emit("resync"));
  await screen.findByText(/Could not load Factory data\./);
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByRole("alert").textContent).toContain("Fixture load denied");
  expect(screen.queryByText("Run deleted.")).toBeNull();
});

it("keeps deletion help in the action group and updates it with run state", async () => {
  const states: Run["status"][] = ["queued", "succeeded", "running"];
  vi.mocked(factoryApi.detail).mockImplementation(async () => ({
    state: "available",
    detail: { ...detail, run: { ...detail.run, status: states.shift() ?? "running" } },
  }));
  render(<RunDetailPage id="run-1" onBack={() => {}} onSelect={() => {}} />);
  const button = await screen.findByRole("button", { name: "Delete run" });
  const reasonId = button.getAttribute("aria-describedby");
  expect((button as HTMLButtonElement).disabled).toBe(true);
  expect(reasonId).toBeTruthy();
  const reason = document.getElementById(reasonId!);
  expect(reason?.textContent).toBe("Cancel this run before deleting it.");
  expect(button.closest(".run-action-group")?.contains(reason)).toBe(true);
  expect(reason?.getAttribute("role")).toBeNull();
  expect(reason?.querySelector("button")).toBeNull();

  await act(async () => emit("resync", "run-1"));
  await waitFor(() => expect(button.getAttribute("aria-describedby")).toBeNull());
  expect(document.getElementById(reasonId!)).toBeNull();
  expect((button as HTMLButtonElement).disabled).toBe(false);

  await act(async () => emit("resync", "run-1"));
  await waitFor(() => expect(button.getAttribute("aria-describedby")).toBe(reasonId));
  expect(document.getElementById(reasonId!)?.textContent).toBe(
    "Cancel this run before deleting it.",
  );
});

it("closes only the cancel error and preserves historical run and step errors", async () => {
  vi.mocked(factoryApi.detail).mockResolvedValue({
    state: "available",
    detail: {
      ...detail,
      run: { ...detail.run, status: "running", error: "Historical run failure" },
      nodes: [{ ...detail.nodes[0], error: "Historical step failure" }],
    },
  });
  vi.mocked(factoryApi.cancel).mockRejectedValue(new Error("fixture cancel denied"));
  render(<RunDetailPage id="run-1" onBack={() => {}} onSelect={() => {}} />);
  fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
  const message = await screen.findByText(/Could not cancel run\./);
  const alert = message.closest('[role="alert"]') as HTMLElement;
  expect(alert.textContent).toContain("Could not cancel run.");
  expect(alert.textContent).toContain("fixture cancel denied");
  fireEvent.click(within(alert).getByRole("button", { name: "Dismiss notice" }));
  expect(screen.queryByText(/Could not cancel run\./)).toBeNull();
  expect(screen.getByText("Historical run failure")).toBeTruthy();
  expect(screen.getByText("Historical step failure")).toBeTruthy();
});

it("retries a failed detail load without retrying the workflow", async () => {
  vi.mocked(factoryApi.detail)
    .mockRejectedValueOnce(new Error("fixture load denied"))
    .mockResolvedValueOnce({ state: "available", detail });
  render(<RunDetailPage id="run-1" onBack={() => {}} onSelect={() => {}} />);
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Could not load run details.");
  expect(alert.textContent).toContain("fixture load denied");
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await screen.findByRole("button", { name: "Delete run" });
  expect(factoryApi.retry).not.toHaveBeenCalled();
});

it("shows and retries a failed detail reload while deletion is pending", async () => {
  const deletion = {
    run_id: "run-1", repository: "owner/repo", number: 4,
    title: detail.run.title, attempt: 1, state: "failed" as const,
    last_error: "Cleanup failed",
  };
  vi.mocked(factoryApi.detail).mockResolvedValueOnce({ state: "deleting", deletion });
  render(<RunDetailPage id="run-1" onBack={() => {}} onSelect={() => {}} />);
  await screen.findByRole("heading", { name: "Deletion pending" });

  vi.mocked(factoryApi.detail).mockRejectedValueOnce(new Error("fixture reload denied"));
  await act(async () => emit("resync"));
  await waitFor(() => expect(factoryApi.detail).toHaveBeenCalledTimes(2));
  const message = await screen.findByText(/Could not load run details\./);
  const alert = message.closest('[role="alert"]') as HTMLElement;
  expect(alert.textContent).toContain("fixture reload denied");

  vi.mocked(factoryApi.detail).mockResolvedValueOnce({ state: "available", detail });
  fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
  await screen.findByRole("button", { name: "Delete run" });
  expect(screen.queryByText(/Could not load run details\./)).toBeNull();
  expect(factoryApi.detail).toHaveBeenCalledTimes(3);
  expect(factoryApi.deleteRun).not.toHaveBeenCalled();
  expect(factoryApi.retry).not.toHaveBeenCalled();
});

it("moves focus to Back to runs when pending deletion removes the focused detail heading", async () => {
  const request = deferred<DeleteRunOutcome>();
  const deletion = {
    run_id: "run-1", repository: "owner/repo", number: 4,
    title: detail.run.title, attempt: 1, state: "failed" as const,
    last_error: "Cleanup failed",
  };
  vi.mocked(factoryApi.deleteRun).mockReturnValue(request.promise);
  render(<RunDetailPage id="run-1" onBack={() => {}} onSelect={() => {}} />);
  fireEvent.click(await screen.findByRole("button", { name: "Delete run" }));
  const dialog = await screen.findByRole("alertdialog");
  fireEvent.click(within(dialog).getByRole("button", { name: "Delete run" }));
  const heading = screen.getByRole("heading", { name: detail.run.title });
  expect(document.activeElement).toBe(heading);

  vi.mocked(factoryApi.detail).mockResolvedValueOnce({ state: "deleting", deletion });
  await act(async () => emit("run_deletion_pending", "run-1"));
  await screen.findByRole("heading", { name: "Deletion pending" });
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Back to runs" }));
});

it("keeps outside focus when a pending deletion removes its detail", async () => {
  const request = deferred<DeleteRunOutcome>();
  const deletion = {
    run_id: "run-1", repository: "owner/repo", number: 4,
    title: detail.run.title, attempt: 1, state: "failed" as const,
    last_error: "Cleanup failed",
  };
  vi.mocked(factoryApi.deleteRun).mockReturnValue(request.promise);
  render(<><button type="button">Outside navigation</button><RunDetailPage id="run-1" onBack={() => {}} onSelect={() => {}} /></>);
  fireEvent.click(await screen.findByRole("button", { name: "Delete run" }));
  const dialog = await screen.findByRole("alertdialog");
  fireEvent.click(within(dialog).getByRole("button", { name: "Delete run" }));
  const outside = screen.getByRole("button", { name: "Outside navigation" });
  outside.focus();

  vi.mocked(factoryApi.detail).mockResolvedValueOnce({ state: "deleting", deletion });
  await act(async () => emit("run_deletion_pending", "run-1"));
  await screen.findByRole("heading", { name: "Deletion pending" });
  expect(document.activeElement).toBe(outside);
});

it("does not navigate when a retry resolves after leaving its run", async () => {
  const pending = deferred<string>();
  const select = vi.fn();
  vi.mocked(factoryApi.detail).mockImplementation(async (id) => ({
    state: "available",
    detail: { ...detail, run: { ...detail.run, id, status: "failed" } },
  }));
  vi.mocked(factoryApi.retry).mockReturnValue(pending.promise);
  const view = render(
    <RunDetailPage id="run-1" onBack={() => {}} onSelect={select} />,
  );
  fireEvent.click(await screen.findByRole("button", { name: "Retry" }));
  view.rerender(<RunDetailPage id="run-2" onBack={() => {}} onSelect={select} />);
  await screen.findByRole("heading", { name: detail.run.title });
  await act(async () => pending.resolve("run-3"));
  expect(select).not.toHaveBeenCalled();
});
