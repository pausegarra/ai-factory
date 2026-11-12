import { afterEach, beforeEach, it, expect, vi } from "vitest";
import { render, screen, cleanup, fireEvent, act, waitFor } from "@testing-library/react";
import type { FactoryEvent, RunDetail, Snapshot } from "../src/context/factory/domain/types";
const mocks = vi.hoisted(() => ({ listeners: new Set<(e: FactoryEvent) => void>() }));
vi.mock("../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true,
  factoryApi: {
    settings: vi.fn(), snapshot: vi.fn(), detail: vi.fn(), logs: vi.fn(),
    cancel: vi.fn(), retry: vi.fn(), deleteRun: vi.fn(),
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
const empty: Snapshot = { runs: [], deletions: [], metrics: { total: 0, queued: 0, running: 0,
  succeeded: 0, failed: 0, cancelled: 0, interrupted: 0 }, service: { running: true,
  polling: false, last_poll_at: null, last_error: null, app_data_dir: "/isolated" } };
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "available", detail });
  vi.mocked(factoryApi.logs).mockResolvedValue({ text: "", offset: 0, next_offset: 0, eof: true });
  vi.mocked(factoryApi.settings).mockResolvedValue({ owners: [], authors: [], reviewers: [], token_profiles: [], projects: [], commands: { codex: "", opencode: "", git: "" }, poll_interval_seconds: 15, max_concurrent: 1, node_timeout_seconds: 1, start_at_login: false, flows: [detail.flow] });
});
afterEach(() => { cleanup(); mocks.listeners.clear(); });
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
  expect(screen.getByText("Deleting...")).toBeTruthy();
  vi.mocked(factoryApi.detail).mockImplementationOnce(() => new Promise(() => {}));
  act(() => mocks.listeners.forEach(cb => cb({ kind: "run_deletion_pending", run_id: "run-1", timestamp: "now" })));
  expect(screen.getByText("Deleting...")).toBeTruthy();
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
it("does not claim success for an external deletion after an earlier rejected request", async () => {
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
  act(() => mocks.listeners.forEach(cb => cb({ kind: "run_deleted", run_id: "run-1", timestamp: "now" })));
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
