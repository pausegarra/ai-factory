import { expect, it } from "vitest";
import type { Run } from "../src/context/factory/domain/types";
import { countNeedsAttention } from "../src/context/factory/application/services/needs-attention";
import { attentionHistory as history, attentionRun as run } from "./fixtures/attention";

const cases: [string, string[], number][] = [
  ["empty", [], 0], ["success only", ["succeeded"], 0],
  ["neutral only", ["queued", "running", "cancelled"], 0],
  ["failed", ["failed"], 1], ["interrupted", ["interrupted"], 1],
  ["blocked", ["blocked"], 1], ["unknown", ["future_status"], 1],
  ["resolved", ["failed", "succeeded"], 0],
  ["waiting retry", ["failed", "queued"], 1],
  ["active retry", ["failed", "running"], 1],
  ["cancelled retry", ["failed", "cancelled"], 1],
  ["cancelled only", ["cancelled"], 0],
  ["several problems", ["failed", "interrupted", "blocked"], 1],
  ["several resolved", ["failed", "interrupted", "blocked", "succeeded"], 0],
  ["reopened failed", ["failed", "succeeded", "failed"], 1],
  ["reopened interrupted", ["failed", "succeeded", "interrupted"], 1],
  ["reopened blocked", ["failed", "succeeded", "blocked"], 1],
  ["resolved again", ["failed", "succeeded", "failed", "succeeded"], 0],
  ["neutral after resolution", ["blocked", "succeeded", "queued", "running", "cancelled"], 0],
  ["unknown resolved", ["future_status", "succeeded"], 0],
];
it.each(cases)("counts %s", (_name, statuses, expected) => {
  const runs = history(statuses);
  for (const input of [runs, [...runs].reverse(), [...runs.slice(1), ...runs.slice(0, 1)]]) {
    expect(countNeedsAttention(input)).toBe(expected);
  }
});

const identities: [string, string, Run["trigger_id"], string][] = [
  ["issue:owner/repo#26", "issue:other/repo#26", "new_issue", "other/repo"],
  ["issue:owner/repo#26", "approval:owner/repo#26@aaa", "pr_approved", "owner/repo"],
  ["approval:owner/repo#26@aaa", "approval:other/repo#26@bbb", "pr_approved", "other/repo"],
  ["review:owner/repo#26:alice:1", "review:owner/repo#26:bob:1", "pr_review", "owner/repo"],
  ["review:owner/repo#26:alice:1", "review:owner/repo#26:alice:2", "pr_review", "owner/repo"],
];
it.each(identities)("does not resolve %s with %s", (eventA, eventB, trigger, repository) => {
  const failedA = run("a", "failed", { event_id: eventA, title: "Same title",
    trigger_id: eventA.startsWith("approval:") ? "pr_approved" : eventA.startsWith("review:") ? "pr_review" : "new_issue" });
  const failedB = run("b", "failed", { event_id: eventB, trigger_id: trigger, repository, title: "Same title" });
  const successB = run("b-success", "succeeded", { ...failedB, id: "b-success", status: "succeeded",
    created_at: "2026-11-13T09:00:01Z", attempt: 2 });
  expect(countNeedsAttention([failedA, failedB])).toBe(2);
  expect(countNeedsAttention([failedA, failedB, successB])).toBe(1);
});

it.each([
  ["success on another SHA", ["blocked", "blocked", "succeeded"], 0],
  ["failure on another SHA", ["succeeded", "failed"], 1],
  ["blocked on another SHA", ["succeeded", "blocked"], 1],
  ["resolved again on another SHA", ["blocked", "succeeded", "failed", "succeeded"], 0],
  ["queued on another SHA", ["blocked", "queued"], 1],
  ["running on another SHA", ["blocked", "running"], 1],
  ["cancelled on another SHA", ["blocked", "cancelled"], 1],
  ["neutral after success on another SHA", ["blocked", "succeeded", "queued", "running", "cancelled"], 0],
] as [string, string[], number][])("groups approvals: %s", (_name, statuses, expected) => {
  const runs = history(statuses).map((item, index) => ({
    ...item, trigger_id: "pr_approved" as const, attempt: 1,
    event_id: `approval:owner/repo#26@sha-${index}`,
  }));
  expect(countNeedsAttention(runs)).toBe(expected);
  expect(countNeedsAttention([...runs].reverse())).toBe(expected);
});

it("keeps approvals for different PR numbers independent", () => {
  const [blocked, success] = history(["blocked", "succeeded"]).map((item, index) => ({
    ...item, trigger_id: "pr_approved" as const, number: 26 + index,
    event_id: `approval:owner/repo#${26 + index}@sha`,
  }));
  expect(countNeedsAttention([blocked, success])).toBe(1);
});

it("resolves approvals when repository casing changes", () => {
  const runs = history(["blocked", "succeeded"]).map((item, index) => ({
    ...item, trigger_id: "pr_approved" as const,
    repository: index ? "Owner/Repo" : "owner/repo",
    event_id: `approval:${index ? "Owner/Repo" : "owner/repo"}#26@sha-${index}`,
  }));
  expect(countNeedsAttention(runs)).toBe(0);
});

const orders: { name: string; runs: Run[]; expected: number }[] = [
  { name: "new retry has lower attempt", runs: [
    run("z", "failed", { attempt: 9 }),
    run("a", "succeeded", { attempt: 2, created_at: "2026-11-13T09:00:01Z" }),
  ], expected: 0 },
  { name: "repeated attempt", runs: [
    run("z", "failed", { attempt: 2 }),
    run("a", "succeeded", { attempt: 2, created_at: "2026-11-13T09:00:01Z" }),
  ], expected: 0 },
  { name: "timestamp tie uses numeric attempt", runs: [
    run("z", "failed", { attempt: 2 }), run("a", "succeeded", { attempt: 10 }),
  ], expected: 0 },
  { name: "full tie uses lexical ID", runs: [
    run("run-10", "failed"), run("run-2", "succeeded"),
  ], expected: 0 },
  { name: "lexical ID is case sensitive", runs: [
    run("Z", "failed"), run("a", "succeeded"),
  ], expected: 0 },
  { name: "offsets compare instants", runs: [
    run("z", "failed", { created_at: "2026-11-13T10:00:00+01:00" }),
    run("a", "succeeded", { created_at: "2026-11-13T09:00:01Z" }),
  ], expected: 0 },
  { name: "equal instant and fraction representation uses attempt", runs: [
    run("z", "failed", { attempt: 2, created_at: "2026-11-13T10:00:00.1+01:00" }),
    run("a", "succeeded", { attempt: 3, created_at: "2026-11-13T09:00:00.100000000Z" }),
  ], expected: 0 },
  { name: "nanoseconds precede attempt and ID", runs: [
    run("z", "failed", { attempt: 9, created_at: "2026-11-13T09:00:00.000000001Z" }),
    run("a", "succeeded", { attempt: 2, created_at: "2026-11-13T09:00:00.000000002Z" }),
  ], expected: 0 },
  { name: "late old failure is still old", runs: [
    run("z", "failed", { finished_at: "2026-11-13T12:00:00Z" }),
    run("a", "succeeded", { created_at: "2026-11-13T09:00:01Z", finished_at: "2026-11-13T10:00:00Z" }),
  ], expected: 0 },
];
it.each(orders)("orders $name", ({ runs, expected }) => {
  expect(countNeedsAttention(runs)).toBe(expected);
  expect(countNeedsAttention([...runs].reverse())).toBe(expected);
});

it("preserves frozen inputs, statuses, timestamps, errors and ordering", () => {
  const runs = history(["failed", "succeeded"]);
  runs[0].error = "historical failure";
  const before = structuredClone(runs);
  runs.forEach(Object.freeze);
  Object.freeze(runs);
  expect(countNeedsAttention(runs)).toBe(0);
  expect(countNeedsAttention(runs)).toBe(0);
  expect(runs).toEqual(before);
});
