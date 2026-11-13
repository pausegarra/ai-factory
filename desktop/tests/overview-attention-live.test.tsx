import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { FactoryEvent, Snapshot } from "../src/context/factory/domain/types";
import { attentionHistory as history, attentionSettings as settings,
  attentionSnapshot as snapshot } from "./fixtures/attention";
const mocks = vi.hoisted(() => ({ listeners: new Set<(event: FactoryEvent) => void>() }));
vi.mock("../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true,
  factoryApi: {
    settings: vi.fn(), snapshot: vi.fn(), detail: vi.fn(), logs: vi.fn(),
    agentModels: vi.fn(async () => []), poll: vi.fn(),
    subscribe: vi.fn(async (cb: (event: FactoryEvent) => void) => {
      mocks.listeners.add(cb);
      return () => { mocks.listeners.delete(cb); };
    }),
  },
}));
import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
import { App } from "../src/context/factory/infrastructure/presentations/app";
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(factoryApi.settings).mockResolvedValue(settings);
  vi.mocked(factoryApi.snapshot).mockResolvedValue(snapshot([]));
  vi.mocked(factoryApi.agentModels).mockResolvedValue([]);
  vi.mocked(factoryApi.subscribe).mockImplementation(async cb => {
    mocks.listeners.add(cb); return () => { mocks.listeners.delete(cb); };
  });
  globalThis.ResizeObserver = class {
    observe() {} unobserve() {} disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(() => { cleanup(); mocks.listeners.clear(); });
function attention() {
  return screen.getByText("Need attention").closest("section")?.querySelector("strong")?.textContent;
}
function emit(kind: string, runId?: string, nodeId?: string) {
  mocks.listeners.forEach(cb => cb({ kind, run_id: runId, node_id: nodeId, timestamp: "fixture" }));
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(yes => { resolve = yes; });
  return { promise, resolve };
}
async function start(value: Snapshot) {
  vi.mocked(factoryApi.snapshot).mockResolvedValue(value);
  const view = render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  return view;
}
async function apply(value: Snapshot, kind = "run_changed", id?: string, nodeId?: string) {
  const calls = vi.mocked(factoryApi.snapshot).mock.calls.length;
  vi.mocked(factoryApi.snapshot).mockResolvedValue(value);
  await act(async () => { emit(kind, id, nodeId); });
  await waitFor(() => expect(factoryApi.snapshot).toHaveBeenCalledTimes(calls + 1));
}

it("keeps attention until the retry run succeeds and updates without polling", async () => {
  const [failed, retry] = history(["failed", "queued"]);
  await start(snapshot([failed]));
  expect(attention()).toBe("01");
  await apply(snapshot([failed, retry]), "run_created", retry.id);
  expect(attention()).toBe("01");
  await apply(snapshot([failed, { ...retry, status: "running" }]), "run_changed", retry.id);
  expect(attention()).toBe("01");
  // A node completion only refreshes data; it cannot resolve a still-running run.
  await apply(snapshot([failed, { ...retry, status: "running" }]), "node_changed", retry.id, "builder");
  expect(attention()).toBe("01");
  await apply(snapshot([failed, { ...retry, status: "succeeded" }]), "run_changed", retry.id);
  await waitFor(() => expect(attention()).toBe("00"));
  expect(factoryApi.poll).not.toHaveBeenCalled();
  expect(factoryApi.detail).not.toHaveBeenCalled();
  expect(factoryApi.logs).not.toHaveBeenCalled();
});

it("loads resolved history initially, after navigation and after remount", async () => {
  const value = snapshot(history(["failed", "succeeded"]));
  const view = await start(value);
  expect(attention()).toBe("00");
  fireEvent.click(screen.getByRole("button", { name: "Runs" }));
  fireEvent.click(screen.getByRole("button", { name: "Overview" }));
  expect(attention()).toBe("00");
  view.unmount();
  expect(mocks.listeners.size).toBe(0);
  await start(value);
  expect(attention()).toBe("00");
});

it.each(["run_changed", "resync"])("discards a stale pending snapshot after %s during load", async kind => {
  const pending = snapshot(history(["failed", "running"]));
  const resolved = snapshot(history(["failed", "succeeded"]));
  await start(resolved);
  expect(attention()).toBe("00");
  const old = deferred<Snapshot>();
  const fresh = deferred<Snapshot>();
  vi.mocked(factoryApi.snapshot).mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
  act(() => { emit("resync"); });
  await waitFor(() => expect(factoryApi.snapshot).toHaveBeenCalledTimes(2));
  act(() => { emit(kind); });
  // Resolving an invalidated read must not commit its pending value.
  await act(async () => { old.resolve(pending); });
  await waitFor(() => expect(factoryApi.snapshot).toHaveBeenCalledTimes(3));
  expect(attention()).toBe("00");
  await act(async () => { fresh.resolve(resolved); });
  await waitFor(() => expect(attention()).toBe("00"));
  expect(factoryApi.snapshot).toHaveBeenCalledTimes(3);
  expect(factoryApi.poll).not.toHaveBeenCalled();
});

it("discards an invalidated initial snapshot before showing the overview", async () => {
  const old = deferred<Snapshot>();
  const fresh = deferred<Snapshot>();
  vi.mocked(factoryApi.snapshot).mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
  render(<App />);
  await waitFor(() => expect(factoryApi.snapshot).toHaveBeenCalledTimes(1));
  expect(mocks.listeners.size).toBe(1);
  act(() => { emit("resync"); });
  await act(async () => { old.resolve(snapshot(history(["failed"]))); });
  await waitFor(() => expect(factoryApi.snapshot).toHaveBeenCalledTimes(2));
  expect(screen.queryByRole("heading", { name: "Factory overview." })).toBeNull();
  await act(async () => { fresh.resolve(snapshot(history(["failed", "succeeded"]))); });
  await screen.findByRole("heading", { name: "Factory overview." });
  expect(attention()).toBe("00");
  expect(factoryApi.poll).not.toHaveBeenCalled();
});

it("does not reopen on a late update from an older failed run", async () => {
  const [old, success] = history(["failed", "succeeded"]);
  await start(snapshot([old, success]));
  expect(attention()).toBe("00");
  await apply(snapshot([{ ...old, finished_at: "2026-11-13T12:00:00Z" }, success]), "run_changed", old.id);
  expect(attention()).toBe("00");
});

it("keeps the previous nonzero counter on transport failure, then recovers by resync", async () => {
  await start(snapshot(history(["failed"])));
  expect(attention()).toBe("01");
  vi.mocked(factoryApi.snapshot).mockRejectedValueOnce(new Error("fixture snapshot denied"));
  await act(async () => { emit("resync"); });
  await screen.findByText("Error: fixture snapshot denied");
  expect(attention()).toBe("01");
  await apply(snapshot(history(["failed", "succeeded"])), "resync");
  await waitFor(() => expect(attention()).toBe("00"));
  expect(screen.queryByText("Error: fixture snapshot denied")).toBeNull();
});

it.each(["pending", "failed"] as const)("uses visible history when deletion is %s", async state => {
  const [failed, success] = history(["failed", "succeeded"]);
  const deletion = (id: string): Snapshot["deletions"][number] => ({
    run_id: id, repository: failed.repository, number: failed.number,
    title: "Hidden attempt", attempt: 2, state, last_error: state === "failed" ? "cleanup failed" : null,
  });
  await start(snapshot([failed]));
  expect(attention()).toBe("01");
  await apply(snapshot([], [deletion(failed.id)]), "run_deletion_pending", failed.id);
  expect(attention()).toBe("00");
  await apply(snapshot([failed, success]), "resync");
  expect(attention()).toBe("00");
  await apply(snapshot([failed], [deletion(success.id)]), "run_deletion_pending", success.id);
  expect(attention()).toBe("01");
  await apply(snapshot([]), "run_deleted", failed.id);
  expect(attention()).toBe("00");
});
