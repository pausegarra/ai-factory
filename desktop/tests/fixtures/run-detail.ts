import type { FlowNode, NodeRun, RunDetail } from "../../src/context/factory/domain/types";

export type StepSpec = readonly [id: string, dependencies: readonly string[]];

function name(id: string): string {
  return id === "check_conflicts" ? "Check conflicts" : id === "publish" ? "Publish" : id;
}

export function makeState(id: string): NodeRun {
  return {
    id, name: name(id), agent: "codex", status: "pending",
    started_at: null, finished_at: null, error: null,
    result: { node: id }, session_id: `session-${id}`,
  };
}

export function makeDetail(
  steps: readonly StepSpec[] = [["publish", ["check_conflicts"]], ["check_conflicts", []]],
  received?: readonly string[],
): RunDetail {
  const nodes: FlowNode[] = steps.map(([id, dependencies], index) => ({
    id, name: name(id), agent: "codex", model: "", effort: "", access: "read",
    prompt: "Inspect local fixture", result_schema: {}, depends_on: [...dependencies],
    position: { x: index * 280 + 80, y: 180 },
  }));
  return {
    run: {
      id: "run-order", event_id: "event-order", trigger_id: "new_issue",
      repository: "owner/repo", number: 14, title: "[AI] Run step order",
      actor: "author", url: "", status: "queued", current_step: null,
      created_at: "2026-11-12T05:00:00Z", started_at: null, finished_at: null,
      error: null, attempt: 1, workspace: null,
    },
    event: {},
    flow: { id: "new_issue", name: "Issue", enabled: true, nodes },
    nodes: (received ?? steps.map(([id]) => id)).map(makeState),
  };
}
