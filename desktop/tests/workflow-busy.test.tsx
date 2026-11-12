import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactFlowProps } from "@xyflow/react";
import type { Settings } from "../src/context/factory/domain/types";
const canvas = vi.hoisted(() => ({ props: null as ReactFlowProps | null }));
vi.mock("@xyflow/react", () => ({
  ReactFlow: (props: ReactFlowProps) => { canvas.props = props; return null; },
  Background: () => null,
  Controls: () => null,
  Handle: () => null,
  Position: { Left: "left", Right: "right" },
  MarkerType: { ArrowClosed: "arrowclosed" },
}));
import { FlowsPage } from "../src/context/factory/infrastructure/presentations/flows";
afterEach(cleanup);

it("ignores programmatic canvas move, remove, connect and selection callbacks while saving", async () => {
  const value: Settings = {
    owners: [], authors: [], reviewers: [], token_profiles: [], projects: [],
    commands: { codex: "codex", opencode: "opencode", git: "git" },
    poll_interval_seconds: 15, max_concurrent: 4, start_at_login: false,
    flows: [
      { id: "new_issue", name: "Issue", enabled: true, nodes: ["plan", "build"].map((id, index) => ({
        id, name: id, agent: "codex", model: "", effort: "", access: "read", prompt: "Work",
        result_schema: {}, depends_on: index ? ["plan"] : [], position: { x: 80 + index * 280, y: 180 },
      })) },
      { id: "pr_approved", name: "Approved", enabled: false, nodes: [] },
      { id: "pr_review", name: "Review", enabled: false, nodes: [] },
    ],
  };
  let reject!: (error: Error) => void;
  const save = vi.fn().mockImplementationOnce(() => new Promise<void>((_, fail) => { reject = fail; })).mockResolvedValue(undefined);
  const busy = vi.fn();
  const view = render(<FlowsPage settings={value} onSave={save} onBusyChange={busy} />);
  fireEvent.click(screen.getByRole("checkbox", { name: "Enable workflow globally for new_issue" }));
  fireEvent.click(screen.getByRole("button", { name: "Save workflow" }));
  expect(busy).toHaveBeenLastCalledWith(true);
  const payload = structuredClone(save.mock.calls[0][0]);
  act(() => {
    canvas.props!.onNodesChange?.([{ id: "plan", type: "position", position: { x: 999, y: 888 } }]);
    canvas.props!.onNodesChange?.([{ id: "build", type: "remove" }]);
    canvas.props!.onEdgesChange?.([{ id: "plan::build", type: "select", selected: true }]);
    canvas.props!.onEdgesChange?.([{ id: "plan::build", type: "remove" }]);
    canvas.props!.onConnect?.({ source: "build", target: "plan", sourceHandle: null, targetHandle: null });
  });
  expect(canvas.props!.nodes!.map((node) => node.id)).toEqual(["plan", "build"]);
  expect(canvas.props!.nodes![0].position).toEqual(payload.flows[0].nodes[0].position);
  expect(canvas.props!.edges![0].selected).toBe(false);
  expect(screen.queryByRole("alert")).toBeNull();
  await act(async () => reject(new Error("try again")));
  await waitFor(() => expect(busy).toHaveBeenLastCalledWith(false));
  fireEvent.click(screen.getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  expect(save.mock.calls[1][0]).toEqual(payload);
  view.unmount();
  expect(busy).toHaveBeenLastCalledWith(false);
});
