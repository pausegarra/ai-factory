import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { FactoryEvent, RunDetail, RunView } from "../src/context/factory/domain/types";
import { makeDetail, makeState, type StepSpec } from "./fixtures/run-detail";

const mocks = vi.hoisted(() => ({ listeners: new Set<(event: FactoryEvent) => void>() }));
vi.mock("../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true,
  factoryApi: {
    detail: vi.fn(), logs: vi.fn(), subscribe: vi.fn(), settings: vi.fn(),
    cancel: vi.fn(), retry: vi.fn(), deleteRun: vi.fn(),
  },
}));

import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
import { RunDetailPage } from "../src/context/factory/infrastructure/presentations/runs";

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(factoryApi.subscribe).mockImplementation(async callback => {
    mocks.listeners.add(callback);
    return () => { mocks.listeners.delete(callback); };
  });
  vi.mocked(factoryApi.logs).mockImplementation(async (runId, nodeId, offset = 0) => {
    const text = offset === 0 ? `log:${runId}:${nodeId}` : "";
    return { text, offset, next_offset: offset + text.length, eof: true };
  });
  vi.mocked(factoryApi.cancel).mockResolvedValue(undefined);
  vi.mocked(factoryApi.retry).mockResolvedValue("run-retry");
});
afterEach(() => { cleanup(); mocks.listeners.clear(); });

function timeline(): HTMLElement {
  const section = screen.getByRole("heading", { name: "Workflow steps" }).closest("section");
  if (!section) throw new Error("Workflow steps section not rendered");
  return section;
}
function rows(): HTMLButtonElement[] {
  return within(timeline()).queryAllByRole("button") as HTMLButtonElement[];
}
function names(): (string | null | undefined)[] {
  return rows().map(button => button.querySelector("strong")?.textContent);
}
function numbers(): (string | null | undefined)[] {
  return rows().map(button => button.querySelector(".step-number")?.textContent);
}
function row(name: string): HTMLButtonElement {
  const button = rows().find(button => button.querySelector("strong")?.textContent === name);
  if (!button) throw new Error(`Missing step ${name}`);
  return button;
}
async function selected(name: string): Promise<void> {
  await waitFor(() => expect(row(name).classList.contains("selected")).toBe(true));
}
async function open(detail: RunDetail) {
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "available", detail });
  const onSelect = vi.fn();
  const result = render(<RunDetailPage id={detail.run.id} onBack={() => {}} onSelect={onSelect} />);
  await screen.findByRole("heading", { name: detail.run.title });
  return { ...result, onSelect };
}
async function refresh(detail: RunDetail, kind = "node_changed"): Promise<void> {
  const calls = vi.mocked(factoryApi.detail).mock.calls.length;
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "available", detail });
  await act(async () => {
    mocks.listeners.forEach(callback => callback({
      kind, run_id: kind === "resync" ? undefined : detail.run.id, timestamp: "now",
    }));
  });
  await waitFor(() => expect(vi.mocked(factoryApi.detail).mock.calls.length).toBeGreaterThan(calls));
}

it("renders issue order, consecutive numbers and first ordered selection", async () => {
  const detail = makeDetail();
  await open(detail);
  expect(names()).toEqual(["Check conflicts", "Publish"]);
  expect(numbers()).toEqual(["01", "02"]);
  await selected("Check conflicts");
  expect(await screen.findByText("log:run-order:check_conflicts")).toBeTruthy();
  expect(factoryApi.logs).toHaveBeenCalledWith("run-order", "check_conflicts", 0);
  expect(factoryApi.cancel).not.toHaveBeenCalled();
  expect(factoryApi.retry).not.toHaveBeenCalled();
  expect(factoryApi.deleteRun).not.toHaveBeenCalled();
});

it.each([
  [null, "Check conflicts"], ["absent", "Check conflicts"],
  ["publish", "Publish"], ["check_conflicts", "Check conflicts"],
])("validates current_step %s before selecting %s", async (currentStep, expected) => {
  const detail = makeDetail();
  detail.run.current_step = currentStep;
  await open(detail);
  await selected(expected!);
  const nodeId = expected === "Publish" ? "publish" : "check_conflicts";
  await screen.findByText(`log:run-order:${nodeId}`);
  expect(vi.mocked(factoryApi.logs).mock.calls.every(call => call[1] !== "absent")).toBe(true);
});

it("opens logs, error, session and result of clicked ID, not row index", async () => {
  const detail = makeDetail();
  detail.nodes[0].status = "failed";
  detail.nodes[0].error = "Publish diagnostic";
  await open(detail);
  await selected("Check conflicts");
  fireEvent.click(row("Publish"));
  await selected("Publish");
  expect(await screen.findByText("log:run-order:publish")).toBeTruthy();
  expect(screen.getByText("session-publish")).toBeTruthy();
  expect(screen.getByText("Publish diagnostic")).toBeTruthy();
  fireEvent.click(screen.getByText("Step result"));
  const output = screen.getByText(/"node": "publish"/);
  expect(output.textContent).toBe(JSON.stringify({ node: "publish" }, null, 2));
  expect(factoryApi.logs).toHaveBeenLastCalledWith("run-order", "publish", 0);
});

it("uses DOM tab order and native keyboard activation without remapping indices", async () => {
  const user = userEvent.setup();
  await open(makeDetail());
  const [first, second] = rows();
  first.focus();
  await user.tab();
  expect(document.activeElement).toBe(second);
  await user.keyboard("{Enter}");
  await selected("Publish");
  expect(await screen.findByText("log:run-order:publish")).toBeTruthy();
  await user.tab({ shift: true });
  expect(document.activeElement).toBe(first);
  await user.keyboard(" ");
  await selected("Check conflicts");
  expect(await screen.findByText("log:run-order:check_conflicts")).toBeTruthy();
  expect(first.tabIndex).toBe(0);
  expect(second.tabIndex).toBe(0);
});

it("renders numbers beyond 99 with no truncation", async () => {
  const steps: StepSpec[] = Array.from({ length: 101 }, (_, i) => [`step-${i}`, []]);
  await open(makeDetail(steps, [...steps].reverse().map(([id]) => id)));
  expect(rows()).toHaveLength(101);
  expect(numbers()).toEqual(Array.from({ length: 101 }, (_, i) => String(i + 1).padStart(2, "0")));
  expect(names()[0]).toBe("step-0");
  expect(names()[100]).toBe("step-100");
});

it("keeps order, numbers, selected ID and focused element across events and resync", async () => {
  const detail = makeDetail();
  await open(detail);
  await selected("Check conflicts");
  fireEvent.click(row("Publish"));
  await screen.findByText("log:run-order:publish");
  const focused = row("Publish");
  focused.focus();
  const originalNames = names();
  const originalNumbers = numbers();
  const states = ["queued", "running", "succeeded", "failed", "blocked", "cancelled", "interrupted"];
  for (const [index, status] of states.entries()) {
    const next = structuredClone(detail);
    next.run.status = status;
    next.run.current_step = "check_conflicts";
    next.nodes.reverse();
    next.nodes.forEach(node => { node.status = status === "queued" ? "pending" : status; });
    await refresh(next, index % 2 ? "resync" : "node_changed");
    expect(names()).toEqual(originalNames);
    expect(numbers()).toEqual(originalNumbers);
    await selected("Publish");
    expect(row("Publish")).toBe(focused);
    expect(document.activeElement).toBe(focused);
    expect(screen.getByText("log:run-order:publish")).toBeTruthy();
  }
  const review = structuredClone(detail);
  review.run.current_step = "check_conflicts";
  review.nodes[0].status = "pending";
  review.nodes[1].status = "skipped";
  await refresh(review, "review_changes_requested");
  expect(names()).toEqual(originalNames);
  expect(numbers()).toEqual(originalNumbers);
  await selected("Publish");
  expect(document.activeElement).toBe(focused);
  expect(screen.getByText("session-publish")).toBeTruthy();
});

it("uses only historical snapshot on reopen and each attempt's own snapshot", async () => {
  vi.mocked(factoryApi.settings).mockRejectedValue(new Error("Must not consult Settings"));
  const historical = makeDetail();
  const first = await open(historical);
  await selected("Check conflicts");
  expect(names()).toEqual(["Check conflicts", "Publish"]);
  first.unmount();
  const reopened = await open(structuredClone(historical));
  await selected("Check conflicts");
  expect(numbers()).toEqual(["01", "02"]);
  expect(names()).toEqual(["Check conflicts", "Publish"]);
  reopened.unmount();
  const retry = makeDetail([["publish", []], ["check_conflicts", ["publish"]]], ["check_conflicts", "publish"]);
  retry.run.id = "run-second-attempt";
  retry.run.attempt = 2;
  await open(retry);
  await selected("Publish");
  expect(names()).toEqual(["Publish", "Check conflicts"]);
  expect(await screen.findByText("log:run-second-attempt:publish")).toBeTruthy();
  expect(factoryApi.settings).not.toHaveBeenCalled();
});

it.each([
  ["other", "other"], ["removed", "Check conflicts"], [null, "Check conflicts"],
])("reconciles removed selection with current_step %s", async (currentStep, expected) => {
  await open(makeDetail());
  await selected("Check conflicts");
  fireEvent.click(row("Publish"));
  await selected("Publish");
  const replacement = makeDetail([["other", ["check_conflicts"]], ["check_conflicts", []]]);
  replacement.run.current_step = currentStep;
  await refresh(replacement);
  await selected(expected!);
  expect(names()).toEqual(["Check conflicts", "other"]);
  const expectedId = expected === "other" ? "other" : "check_conflicts";
  expect(await screen.findByText(`log:run-order:${expectedId}`)).toBeTruthy();
  expect(vi.mocked(factoryApi.logs).mock.calls.every(call => call[1] !== "removed")).toBe(true);
});

it("shows zero states without inventing selection or requesting logs", async () => {
  const detail = makeDetail();
  detail.nodes = [];
  detail.run.current_step = "publish";
  await open(detail);
  expect(rows()).toHaveLength(0);
  expect(timeline().querySelector(".panel-heading .muted")?.textContent).toBe("0");
  expect(screen.getByRole("heading", { name: "Session" })).toBeTruthy();
  expect(screen.getByText("Waiting for agent output…")).toBeTruthy();
  expect(factoryApi.logs).not.toHaveBeenCalled();
});

it("clears selection and output when states become empty", async () => {
  const detail = makeDetail();
  await open(detail);
  await screen.findByText("log:run-order:check_conflicts");
  const next = structuredClone(detail);
  next.nodes = [];
  await refresh(next, "resync");
  expect(rows()).toHaveLength(0);
  await waitFor(() => expect(screen.queryByText("log:run-order:check_conflicts")).toBeNull());
  expect(screen.getByRole("heading", { name: "Session" })).toBeTruthy();
});

it.each(["cycle", "missing definition", "duplicate states"])("renders complete fallback for %s", async mode => {
  const detail = makeDetail();
  if (mode === "cycle") detail.flow.nodes[1].depends_on = ["publish"];
  if (mode === "missing definition") detail.nodes[1] = makeState("orphan");
  if (mode === "duplicate states") detail.nodes[1] = makeState("publish");
  const expected = detail.nodes.map(node => node.name);
  await open(detail);
  expect(names()).toEqual(expected);
  expect(numbers()).toEqual(["01", "02"]);
  await refresh(structuredClone(detail), "resync");
  expect(names()).toEqual(expected);
  expect(rows()).toHaveLength(2);
});

it("keeps distinct repeated-name states associated with their IDs", async () => {
  const detail = makeDetail();
  detail.nodes.forEach(node => { node.name = "Shared name"; });
  await open(detail);
  expect(names()).toEqual(["Shared name", "Shared name"]);
  expect(await screen.findByText("log:run-order:check_conflicts")).toBeTruthy();
  fireEvent.click(rows()[1]);
  expect(await screen.findByText("log:run-order:publish")).toBeTruthy();
  expect(screen.getByText("session-publish")).toBeTruthy();
});

it.each(["queued", "succeeded"])("preserves run action for %s without changing ordering", async status => {
  const detail = makeDetail();
  detail.run.status = status;
  const { onSelect } = await open(detail);
  expect(names()).toEqual(["Check conflicts", "Publish"]);
  expect(factoryApi.cancel).not.toHaveBeenCalled();
  expect(factoryApi.retry).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: status === "queued" ? "Cancel" : "Retry" }));
  if (status === "queued") {
    await waitFor(() => expect(factoryApi.cancel).toHaveBeenCalledWith("run-order"));
    expect(onSelect).not.toHaveBeenCalled();
  } else {
    await waitFor(() => expect(onSelect).toHaveBeenCalledWith("run-retry"));
    expect(factoryApi.retry).toHaveBeenCalledWith("run-order");
  }
  expect(names()).toEqual(["Check conflicts", "Publish"]);
});

it("preserves Follow logs control without changing selection", async () => {
  await open(makeDetail());
  await selected("Check conflicts");
  const follow = screen.getByRole("checkbox", { name: "Follow logs" }) as HTMLInputElement;
  expect(follow.checked).toBe(true);
  fireEvent.click(follow);
  expect(follow.checked).toBe(false);
  await refresh(makeDetail(), "resync");
  expect(follow.checked).toBe(false);
  await selected("Check conflicts");
  fireEvent.click(follow);
  expect(follow.checked).toBe(true);
});

it("preserves loading and read error surfaces", async () => {
  vi.mocked(factoryApi.detail).mockImplementationOnce(() => new Promise<RunView>(() => {}));
  const pending = render(<RunDetailPage id="pending" onBack={() => {}} onSelect={() => {}} />);
  expect(pending.container.querySelector(".skeleton")).toBeTruthy();
  await waitFor(() => expect(factoryApi.detail).toHaveBeenCalledWith("pending"));
  expect(factoryApi.logs).not.toHaveBeenCalled();
  pending.unmount();
  vi.mocked(factoryApi.detail).mockRejectedValue(new Error("detail offline"));
  render(<RunDetailPage id="failed-load" onBack={() => {}} onSelect={() => {}} />);
  expect(await screen.findByRole("alert")).toBeTruthy();
  expect(screen.getByRole("alert").textContent).toContain("detail offline");
  expect(screen.queryByRole("heading", { name: "Workflow steps" })).toBeNull();
  expect(factoryApi.logs).not.toHaveBeenCalled();
});

it.each(["missing", "deleting"])("clears ordered detail when next view is %s", async state => {
  const detail = makeDetail();
  await open(detail);
  await screen.findByText("log:run-order:check_conflicts");
  const next: RunView = state === "missing" ? { state: "missing" } : {
    state: "deleting",
    deletion: {
      run_id: detail.run.id, repository: detail.run.repository, number: 14,
      title: detail.run.title, attempt: 1, state: "pending", last_error: null,
    },
  };
  vi.mocked(factoryApi.detail).mockResolvedValue(next);
  await act(async () => {
    mocks.listeners.forEach(callback => callback({ kind: "resync", timestamp: "now" }));
  });
  if (state === "missing") await screen.findByText("This run is no longer available.");
  else await screen.findByRole("heading", { name: "Deletion pending" });
  expect(screen.queryByRole("heading", { name: "Workflow steps" })).toBeNull();
  expect(screen.queryByText("log:run-order:check_conflicts")).toBeNull();
  expect(screen.queryByText(/"node": "check_conflicts"/)).toBeNull();
  const before = vi.mocked(factoryApi.logs).mock.calls.length;
  await act(async () => {
    mocks.listeners.forEach(callback => callback({ kind: "resync", timestamp: "later" }));
  });
  expect(vi.mocked(factoryApi.logs).mock.calls.length).toBe(before);
  expect(factoryApi.retry).not.toHaveBeenCalled();
  expect(factoryApi.deleteRun).not.toHaveBeenCalled();
});
