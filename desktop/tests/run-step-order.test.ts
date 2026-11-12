import { expect, it } from "vitest";
import type { RunDetail } from "../src/context/factory/domain/types";
import { orderRunSteps } from "../src/context/factory/application/run-step-order";
import { makeDetail, makeState, type StepSpec } from "./fixtures/run-detail";

const ids = (detail: RunDetail) => orderRunSteps(detail).map(node => node.id);

const cases: {
  name: string; steps: StepSpec[]; received?: string[]; expected: string[];
}[] = [
  { name: "issue creation order", steps: [["publish", ["check_conflicts"]], ["check_conflicts", []]], expected: ["check_conflicts", "publish"] },
  { name: "reverse chain", steps: [["C", ["B"]], ["B", ["A"]], ["A", []]], expected: ["A", "B", "C"] },
  { name: "mixed chain", steps: [["B", ["A"]], ["C", ["B"]], ["A", []]], received: ["C", "A", "B"], expected: ["A", "B", "C"] },
  { name: "fork and join", steps: [["publish", ["tests", "check"]], ["tests", ["build"]], ["check", ["build"]], ["build", []]], expected: ["build", "tests", "check", "publish"] },
  { name: "newly ready node competes immediately", steps: [["A", ["B"]], ["B", []], ["C", []]], expected: ["B", "A", "C"] },
  { name: "independent nodes use snapshot not states", steps: [["C", []], ["A", []], ["B", []]], received: ["B", "A", "C"], expected: ["C", "A", "B"] },
  { name: "independent components", steps: [["D", ["C"]], ["B", ["A"]], ["C", []], ["A", []]], expected: ["C", "D", "A", "B"] },
  { name: "repeated dependencies are one relation", steps: [["A", ["B", "B"]], ["B", []]], expected: ["B", "A"] },
];

it.each(cases)("orders $name", ({ steps, received, expected }) => {
  expect(ids(makeDetail(steps, received))).toEqual(expected);
});

function deepFreeze(value: unknown): void {
  if (value !== null && typeof value === "object") {
    Object.values(value).forEach(deepFreeze);
    Object.freeze(value);
  }
}

it("returns original states once without mutating deeply frozen inputs", () => {
  const detail = makeDetail();
  detail.nodes[0].status = "skipped";
  detail.nodes[0].error = "dependency failed";
  detail.nodes[1].status = "failed";
  const before = structuredClone(detail);
  deepFreeze(detail);
  const ordered = orderRunSteps(detail);
  expect(ordered).not.toBe(detail.nodes);
  expect(ordered).toHaveLength(2);
  expect(new Set(ordered).size).toBe(2);
  expect(ordered[0]).toBe(detail.nodes[1]);
  expect(ordered[1]).toBe(detail.nodes[0]);
  expect(detail).toEqual(before);
});

it("ignores names, positions, timestamps, status, agent and current step", () => {
  const detail = makeDetail();
  const expected = ids(detail);
  detail.run.current_step = "publish";
  detail.nodes.forEach((node, index) => {
    node.name = "Same name";
    node.agent = "opencode";
    node.status = index ? "running" : "succeeded";
    node.started_at = index ? "2026-11-12T07:00:00Z" : "2026-11-12T06:00:00Z";
    node.finished_at = index ? null : "2026-11-12T06:01:00Z";
    detail.flow.nodes[index].name = "Same name";
    detail.flow.nodes[index].position = { x: -index * 100, y: index * 999 };
  });
  expect(ids(detail)).toEqual(expected);
  expect(orderRunSteps(detail)).toHaveLength(2);
});

it("handles empty states independently of snapshot definitions", () => {
  const detail = makeDetail();
  detail.nodes = [];
  expect(orderRunSteps(detail)).toEqual([]);
  expect(orderRunSteps(makeDetail([]))).toEqual([]);
});

const invalid: { name: string; edit: (detail: RunDetail) => void }[] = [
  { name: "empty snapshot", edit: d => { d.flow.nodes = []; } },
  { name: "orphan state with equal cardinality", edit: d => { d.nodes[1] = makeState("orphan"); } },
  { name: "extra state", edit: d => { d.nodes.push(makeState("orphan")); } },
  { name: "definition without state", edit: d => { d.nodes.pop(); } },
  { name: "duplicate state IDs", edit: d => { d.nodes[1].id = d.nodes[0].id; } },
  { name: "duplicate definition IDs", edit: d => { d.flow.nodes[1].id = d.flow.nodes[0].id; } },
  { name: "unknown dependency", edit: d => { d.flow.nodes[0].depends_on = ["absent"]; } },
  { name: "self cycle", edit: d => { d.flow.nodes[0].depends_on = ["publish"]; } },
  { name: "two node cycle", edit: d => { d.flow.nodes[1].depends_on = ["publish"]; } },
  { name: "cycle after valid prefix", edit: d => {
    d.flow.nodes[1].depends_on = ["publish"];
    const prefix = makeDetail([["prefix", []]]);
    d.flow.nodes.push(prefix.flow.nodes[0]);
    d.nodes.push(prefix.nodes[0]);
  } },
];

it.each(invalid)("falls back completely for $name", ({ edit }) => {
  const detail = makeDetail();
  edit(detail);
  const before = structuredClone(detail);
  deepFreeze(detail);
  const ordered = orderRunSteps(detail);
  expect(ordered).not.toBe(detail.nodes);
  expect(ordered).toEqual(detail.nodes);
  ordered.forEach((node, index) => expect(node).toBe(detail.nodes[index]));
  expect(orderRunSteps(detail)).toEqual(ordered);
  expect(detail).toEqual(before);
});

it("orders more than 99 nodes without truncating a reverse chain", () => {
  const steps: StepSpec[] = Array.from({ length: 101 }, (_, i) => [
    `step-${i}`, i ? [`step-${i - 1}`] : [],
  ]);
  expect(ids(makeDetail([...steps].reverse()))).toEqual(steps.map(([id]) => id));
});
