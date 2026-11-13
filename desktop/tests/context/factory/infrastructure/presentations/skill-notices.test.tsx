import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { FlowNode, InstalledSkill } from "../../../../../src/context/factory/domain/types";
import { deferred, noticeSettings } from "../../../../fixtures/notices";
import { makeDetail } from "../../../../fixtures/run-detail";

type CanvasNode = { id: string; data: { step: FlowNode } };
vi.mock("../../../../../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true,
  factoryApi: { agentModels: vi.fn(async () => []), installedSkills: vi.fn(async () => []) },
}));
vi.mock("@xyflow/react", async (original) => ({
  ...(await original<typeof import("@xyflow/react")>()),
  Background: () => null,
  Controls: () => null,
  Handle: () => null,
  useReactFlow: () => ({ fitView: vi.fn() }),
  ReactFlow: ({ nodes, onNodeClick, children }: {
    nodes: CanvasNode[];
    onNodeClick: (_event: null, node: CanvasNode) => void;
    children: React.ReactNode;
  }) => <div>
    {nodes.map((node) => <button key={node.id} onClick={() => onNodeClick(null, node)}>{node.data.step.name}</button>)}
    {children}
  </div>,
}));

import { FlowsPage } from "../../../../../src/context/factory/infrastructure/presentations/flows";

afterEach(cleanup);

function workflowSettings() {
  const value = noticeSettings();
  value.flows[0] = { ...value.flows[0], enabled: true, nodes: makeDetail().flow.nodes };
  return value;
}

function openSkillSelector() {
  fireEvent.click(screen.getByRole("button", { name: "Check conflicts" }));
  fireEvent.click(screen.getByRole("button", { name: /Select skills/ }));
}

it("keeps the newer skill result after closing an older opening", async () => {
  const first = deferred<InstalledSkill[]>();
  const second = deferred<InstalledSkill[]>();
  const load = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
  render(<FlowsPage settings={workflowSettings()} onSave={vi.fn()} onLoadSkills={load} />);
  openSkillSelector();
  expect(load).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole("button", { name: "Close skill selector" }));
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /Select skills/ }));
  expect(load).toHaveBeenCalledTimes(2);
  await act(async () => second.resolve([{ id: "current", name: "Current", description: "Current catalog" }]));
  expect(await screen.findByText("current", { exact: true })).toBeTruthy();
  await act(async () => first.reject(new Error("Old opening failed")));
  expect(screen.queryByText(/Old opening failed/)).toBeNull();
  expect(screen.getByText("current", { exact: true })).toBeTruthy();
});

it("keeps installed-skill errors and retry feedback inside the dialog", async () => {
  const load = vi.fn()
    .mockRejectedValueOnce(new Error("fixture skill read denied"))
    .mockResolvedValueOnce([{ id: "retry-skill", name: "Retry skill", description: "Loaded after retry" }]);
  render(<FlowsPage settings={workflowSettings()} onSave={vi.fn()} onLoadSkills={load} />);
  openSkillSelector();
  const dialog = await screen.findByRole("dialog");
  expect((await screen.findByRole("alert")).textContent).toContain("Could not load installed skills.");
  expect(screen.getByRole("alert").textContent).toContain("fixture skill read denied");
  expect(dialog.querySelector(".skill-dialog-error")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByText("retry-skill", { exact: true })).toBeTruthy();
  expect(screen.queryByRole("alert")).toBeNull();
  expect(load).toHaveBeenCalledTimes(2);
});

it("invalidates a pending catalog when its command context changes", async () => {
  const pending = deferred<InstalledSkill[]>();
  const load = vi.fn(() => pending.promise);
  const original = workflowSettings();
  const view = render(<FlowsPage settings={original} onSave={vi.fn()} onLoadSkills={load} />);
  openSkillSelector();
  const updated = structuredClone(original);
  updated.commands.codex = "codex --profile next";
  view.rerender(<FlowsPage settings={updated} onSave={vi.fn()} onLoadSkills={load} />);
  expect(await screen.findByRole("button", { name: "Check conflicts" })).toBeTruthy();
  expect(screen.queryByRole("dialog")).toBeNull();
  await act(async () => pending.reject(new Error("old profile failed")));
  expect(screen.queryByRole("alert")).toBeNull();
});

it("closes an open catalog when its node configuration changes externally", async () => {
  const pending = deferred<InstalledSkill[]>();
  const load = vi.fn(() => pending.promise);
  const original = workflowSettings();
  const view = render(<FlowsPage settings={original} onSave={vi.fn()} onLoadSkills={load} />);
  openSkillSelector();
  const updated = structuredClone(original);
  updated.flows[0].nodes[1].model = "new-model";
  view.rerender(<FlowsPage settings={updated} onSave={vi.fn()} onLoadSkills={load} />);
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await act(async () => pending.reject(new Error("old node context failed")));
  expect(screen.queryByRole("alert")).toBeNull();
});

it("does not let an earlier retry replace a newly opened skills catalog", async () => {
  const failed = deferred<InstalledSkill[]>();
  const retrying = deferred<InstalledSkill[]>();
  const reopened = deferred<InstalledSkill[]>();
  const load = vi.fn()
    .mockReturnValueOnce(failed.promise)
    .mockReturnValueOnce(retrying.promise)
    .mockReturnValueOnce(reopened.promise);
  render(<FlowsPage settings={workflowSettings()} onSave={vi.fn()} onLoadSkills={load} />);
  openSkillSelector();
  await act(async () => failed.reject(new Error("initial read failed")));
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  fireEvent.click(screen.getByRole("button", { name: "Close skill selector" }));
  fireEvent.click(screen.getByRole("button", { name: /Select skills/ }));
  expect(load).toHaveBeenCalledTimes(3);
  await act(async () => reopened.resolve([{ id: "new-opening", name: "New opening", description: "Current result" }]));
  expect(await screen.findByText("new-opening", { exact: true })).toBeTruthy();
  await act(async () => retrying.reject(new Error("older retry failed")));
  expect(screen.queryByText(/older retry failed/)).toBeNull();
  expect(screen.getByText("new-opening", { exact: true })).toBeTruthy();
});
