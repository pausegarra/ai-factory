import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { FactoryEvent, LogChunk } from "../src/context/factory/domain/types";
import { makeDetail } from "./fixtures/run-detail";
import { envelope, jsonLine, openText } from "./fixtures/run-logs";

const mocks = vi.hoisted(() => ({ listeners: new Set<(event: FactoryEvent) => void>() }));
vi.mock("../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true,
  factoryApi: { detail: vi.fn(), logs: vi.fn(), subscribe: vi.fn(), cancel: vi.fn(), retry: vi.fn(), deleteRun: vi.fn() },
}));

import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
import { RunDetailPage } from "../src/context/factory/infrastructure/presentations/runs";

const { listeners } = mocks;

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(factoryApi.subscribe).mockImplementation(async (callback) => {
    listeners.add(callback);
    return () => { listeners.delete(callback); };
  });
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "available", detail: makeDetail() });
  vi.mocked(factoryApi.logs).mockImplementation(async (_run, _node, offset = 0) =>
    ({ text: "", offset, next_offset: offset, eof: false }));
  vi.mocked(factoryApi.cancel).mockResolvedValue(undefined);
  vi.mocked(factoryApi.retry).mockResolvedValue("run-retry");
  vi.mocked(factoryApi.deleteRun).mockResolvedValue({ state: "deleted", run_id: "run-order" });
});

afterEach(() => { cleanup(); listeners.clear(); });

async function event(kind: string, runId = "run-order", nodeId?: string): Promise<void> {
  await act(async () => {
    listeners.forEach((callback) => callback({
      kind,
      run_id: kind === "resync" ? undefined : runId,
      node_id: nodeId,
      timestamp: "now",
    }));
  });
}

const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

it("finishes provisional JSON on eof with no new text and preserves step result", async () => {
  const detail = makeDetail();
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "available", detail });
  const text = jsonLine(openText("Finished message"));
  const chunks = [
    { text, offset: 0, next_offset: 300, eof: false },
    { text: "", offset: 300, next_offset: 300, eof: false },
    { text: "", offset: 300, next_offset: 300, eof: true },
  ];
  vi.mocked(factoryApi.logs).mockImplementation(async (_run, _node, offset = 0) =>
    chunks.shift() ?? { text: "", offset, next_offset: offset, eof: true });
  render(<RunDetailPage id="run-order" onBack={() => {}} onSelect={() => {}} />);
  await screen.findByText("Receiving output");
  await event("node_changed", "run-order", "check_conflicts");
  await screen.findByText("Finished message");
  expect(screen.queryByText("Receiving output")).toBeNull();
  expect(screen.getByText("session-check_conflicts")).toBeTruthy();
  expect(screen.getByText("Step result")).toBeTruthy();
  expect(factoryApi.logs).toHaveBeenCalledWith("run-order", "check_conflicts", 300);
});

it("keeps current text on IPC failure then resumes from exact offset", async () => {
  const raw = jsonLine(openText("before error")) + "\n";
  let calls = 0;
  vi.mocked(factoryApi.logs).mockImplementation(async (_run, _node, offset = 0) => {
    calls++;
    if (calls === 2) throw new Error("fixture offline");
    if (calls === 1) return { text: raw, offset, next_offset: 800, eof: false };
    return { text: jsonLine(openText("recovered")) + "\n", offset, next_offset: 900, eof: true };
  });
  render(<RunDetailPage id="run-order" onBack={() => {}} onSelect={() => {}} />);
  await screen.findByText("before error");
  expect((await screen.findByRole("alert")).textContent).toContain(
    "Could not read run logs.\nfixture offline",
  );
  await event("resync");
  await screen.findByText("recovered");
  expect(screen.getByText("before error")).toBeTruthy();
  await waitFor(() => expect(screen.queryByText(/Could not read run logs\./)).toBeNull());
  expect(factoryApi.logs).toHaveBeenLastCalledWith("run-order", "check_conflicts", 800);
});

it("keeps block expansion on append and resync but resets after a step change", async () => {
  const user = userEvent.setup();
  const raw = [jsonLine(openText("a".repeat(1201))), jsonLine(openText("b".repeat(1201)))].join("\n") + "\n";
  let appended = false;
  vi.mocked(factoryApi.logs).mockImplementation(async (_run, node, offset = 0) => {
    const text = offset === 0 ? (node === "check_conflicts" ? raw : "publish output\n") :
      appended && offset === 500 ? "tail\n" : "";
    return { text, offset, next_offset: offset === 0 ? 500 : offset + text.length, eof: true };
  });
  render(<RunDetailPage id="run-order" onBack={() => {}} onSelect={() => {}} />);
  const first = (await screen.findAllByRole("button", { name: /Show more/ }))[0];
  await user.click(first);
  first.focus();
  appended = true;
  await event("log", "run-order", "check_conflicts");
  await screen.findByText("tail");
  expect(globalThis.document.activeElement).toBe(first);
  expect(screen.getAllByRole("button", { name: /Show less/ })).toHaveLength(1);
  await event("resync");
  expect(globalThis.document.activeElement).toBe(first);
  await user.click(screen.getByRole("button", { name: "Raw" }));
  await user.click(screen.getByRole("button", { name: "Readable" }));
  expect(screen.getAllByRole("button", { name: /Show less/ })).toHaveLength(1);
  const timeline = screen.getByRole("heading", { name: "Workflow steps" }).closest("section")!;
  await user.click(screen.getByRole("checkbox", { name: "Follow logs" }));
  await user.click(within(timeline).getByRole("button", { name: /Publish/ }));
  await screen.findByText("publish output");
  await user.click(within(timeline).getByRole("button", { name: /Check conflicts/ }));
  expect(await screen.findAllByRole("button", { name: /Show more/ })).toHaveLength(2);
  expect(screen.queryByRole("button", { name: /Show less/ })).toBeNull();
  expect((screen.getByRole("checkbox", { name: "Follow logs" }) as HTMLInputElement).checked).toBe(false);
});

it("resets preferences on direct run change and ignores the old run response", async () => {
  const user = userEvent.setup();
  const old = deferred<LogChunk>();
  vi.mocked(factoryApi.detail).mockImplementation(async (id) => {
    const detail = makeDetail();
    detail.run.id = id;
    detail.run.title = `Title ${id}`;
    return { state: "available", detail };
  });
  vi.mocked(factoryApi.logs).mockImplementation(async (runId, _node, offset = 0) => runId === "run-order"
    ? old.promise
    : { text: "new run output\n", offset, next_offset: 120, eof: true });
  const { rerender, unmount } = render(
    <RunDetailPage id="run-order" onBack={() => {}} onSelect={() => {}} />,
  );
  await screen.findByRole("button", { name: "Raw" });
  await waitFor(() => expect(factoryApi.logs).toHaveBeenCalledWith("run-order", "check_conflicts", 0));
  await user.click(screen.getByRole("button", { name: "Raw" }));
  await user.click(screen.getByRole("checkbox", { name: "Follow logs" }));
  rerender(<RunDetailPage id="run-next" onBack={() => {}} onSelect={() => {}} />);
  await screen.findByText("new run output");
  await act(async () => old.resolve({ text: "old run output\n", offset: 0, next_offset: 150, eof: true }));
  expect(screen.queryByText("old run output")).toBeNull();
  expect(screen.queryByRole("heading", { name: "Title run-order" })).toBeNull();
  expect(screen.getByRole("button", { name: "Readable" }).getAttribute("aria-pressed")).toBe("true");
  expect((screen.getByRole("checkbox", { name: "Follow logs" }) as HTMLInputElement).checked).toBe(true);
  await user.click(screen.getByRole("button", { name: "Raw" }));
  unmount();
  render(<RunDetailPage id="run-next" onBack={() => {}} onSelect={() => {}} />);
  await screen.findByText("new run output");
  expect(screen.getByRole("button", { name: "Readable" }).getAttribute("aria-pressed")).toBe("true");
});

it.each(["run_deleted", "run_deletion_pending", "run_deletion_failed", "unmount"])(
  "suppresses late logs after %s",
  async (kind) => {
    const pending = deferred<LogChunk>();
    vi.mocked(factoryApi.logs).mockReturnValue(pending.promise);
    const { unmount } = render(<RunDetailPage id="run-order" onBack={() => {}} onSelect={() => {}} />);
    await waitFor(() => expect(factoryApi.logs).toHaveBeenCalled());
    if (kind === "unmount") unmount();
    else {
      vi.mocked(factoryApi.detail).mockResolvedValue(kind === "run_deleted" ? { state: "missing" } : {
        state: "deleting",
        deletion: {
          run_id: "run-order",
          repository: "owner/repo",
          number: 14,
          title: "Cleanup",
          attempt: 1,
          state: "pending",
          last_error: null,
        },
      });
      await event(kind);
      await waitFor(() => expect(screen.queryByRole("heading", { name: "Workflow steps" })).toBeNull());
    }
    const calls = vi.mocked(factoryApi.logs).mock.calls.length;
    await act(async () => pending.resolve({ text: "obsolete\n", offset: 0, next_offset: 200, eof: false }));
    expect(screen.queryByText("obsolete")).toBeNull();
    expect(vi.mocked(factoryApi.logs).mock.calls).toHaveLength(calls);
  },
);

it("subscribes before read and retains cumulative text across in-flight invalidation", async () => {
  const subscribed = deferred<() => void>();
  const first = deferred<LogChunk>();
  const order: string[] = [];
  let notify!: (event: FactoryEvent) => void;
  let concurrent = 0;
  let maximum = 0;
  let calls = 0;
  let subscriptions = 0;
  vi.mocked(factoryApi.subscribe).mockImplementation(async (callback) => {
    if (++subscriptions === 1) {
      listeners.add(callback);
      return () => { listeners.delete(callback); };
    }
    notify = callback;
    order.push("subscribe");
    return subscribed.promise;
  });
  vi.mocked(factoryApi.logs).mockImplementation(async (_run, _node, offset = 0) => {
    order.push("load");
    concurrent++;
    maximum = Math.max(maximum, concurrent);
    try {
      if (++calls === 1) return await first.promise;
      return { text: offset === 10 ? "second\n" : "", offset, next_offset: offset === 10 ? 20 : offset, eof: true };
    } finally {
      concurrent--;
    }
  });
  render(<RunDetailPage id="run-order" onBack={() => {}} onSelect={() => {}} />);
  await waitFor(() => expect(order).toEqual(["subscribe"]));
  expect(factoryApi.logs).not.toHaveBeenCalled();
  await act(async () => subscribed.resolve(() => {}));
  await waitFor(() => expect(factoryApi.logs).toHaveBeenCalledTimes(1));
  await act(async () => {
    notify({ kind: "log", run_id: "run-order", node_id: "check_conflicts", timestamp: "now" });
    notify({ kind: "resync", timestamp: "now" });
    first.resolve({ text: "first\n", offset: 0, next_offset: 10, eof: true });
  });
  await screen.findByText("second");
  expect(screen.getAllByText("first")).toHaveLength(1);
  expect(screen.getAllByText("second")).toHaveLength(1);
  expect(order[0]).toBe("subscribe");
  expect(maximum).toBe(1);
});

it.each(["succeeded", "failed", "blocked", "cancelled", "interrupted"])(
  "shows imported logs and reported envelope without changing %s",
  async (status) => {
    const user = userEvent.setup();
    const detail = makeDetail();
    detail.run.status = status;
    detail.nodes.forEach((node) => { node.status = status; });
    detail.nodes.find((node) => node.id === "check_conflicts")!.error = "Persisted node error";
    vi.mocked(factoryApi.detail).mockResolvedValue({ state: "available", detail });
    const raw = "legacy plain output\n" + jsonLine(openText(jsonLine({
      ...envelope,
      data: { publish: { title: "never execute" } },
    })));
    vi.mocked(factoryApi.logs).mockImplementation(async (_run, _node, offset = 0) =>
      ({ text: offset === 0 ? raw : "", offset, next_offset: 400, eof: true }));
    render(<RunDetailPage id="run-order" onBack={() => {}} onSelect={() => {}} />);
    await screen.findByText("legacy plain output");
    expect(screen.getByText("Reported status: succeeded")).toBeTruthy();
    expect(screen.getByText("Persisted node error")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Result details" }));
    await user.click(screen.getByText("Step result"));
    expect(screen.getByText(/"node": "check_conflicts"/)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Raw" }));
    expect(screen.getByTestId("raw-log-output").textContent).toBe(raw);
    expect(detail.run.status).toBe(status);
    expect(factoryApi.cancel).not.toHaveBeenCalled();
    expect(factoryApi.retry).not.toHaveBeenCalled();
    expect(factoryApi.deleteRun).not.toHaveBeenCalled();
  },
);
